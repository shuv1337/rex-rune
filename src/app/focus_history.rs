//! Client walk-cursor and toggle state machine for server-side focus history
//! (`focus_history.reverse`, Rex Server API 19+).
//!
//! Consecutive triggers of `client.focus_history.reverse` with the same scope on the same
//! host and connection epoch forward the server's returned `cursor` to keep stepping deeper
//! through history without revisiting already-visited blocks. Any other user interaction
//! (typing, IME, clicking, running another action, changing scope, opening a panel/dialog,
//! entering copy mode, opening find, switching/removing hosts, or disconnect/epoch change)
//! immediately drops the walk cursor so the next trigger sends `cursor: None` and toggles
//! cleanly between the current focus and the previous target.
use super::workspace::{HostId, Workspace};
use rex_rune::connection::{
    FOCUS_HISTORY_DISCONNECTED, FOCUS_HISTORY_MIN_API_REVISION, FOCUS_HISTORY_REQUIRES_API,
    FocusHistoryScope, HostCommand,
};
use rune::Tx;
use std::time::{Duration, Instant};

/// Safety bound for an in-flight `focus_history.reverse` request (slightly above the 8s
/// worker timeout so normal responses always settle before expiry).
const PENDING_TIMEOUT: Duration = Duration::from_secs(10);

/// Active consecutive focus-history walk on one host connection epoch and scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Walk {
    pub host: HostId,
    pub epoch: u64,
    pub scope: FocusHistoryScope,
    pub cursor: Option<u64>,
    pub pending_id: Option<u64>,
    pub pending_deadline: Option<Instant>,
}

#[derive(Default)]
pub struct State {
    pub walk: Option<Walk>,
    serial: u64,
}

impl State {
    fn next_id(&mut self) -> u64 {
        self.serial = self.serial.wrapping_add(1).max(1);
        self.serial
    }

    pub fn clear(&mut self) {
        self.walk = None;
    }

    pub fn is_on_host(&self, host: HostId) -> bool {
        self.walk.as_ref().is_some_and(|w| w.host == host)
    }
}

impl Workspace {
    /// Reason `client.focus_history.reverse` cannot run on the active host right now, if any.
    pub fn focus_history_blocked(&self) -> Option<&'static str> {
        let model = self.model();
        if !model.connected {
            Some(FOCUS_HISTORY_DISCONNECTED)
        } else if !model
            .protocol
            .api_revision_at_least(FOCUS_HISTORY_MIN_API_REVISION)
        {
            Some(FOCUS_HISTORY_REQUIRES_API)
        } else {
            None
        }
    }

    /// Drop any active walk cursor so the next trigger starts fresh with `cursor: None`.
    pub fn clear_focus_history_walk(&mut self) {
        self.focus_history.clear();
    }

    /// Step back through server-side focus history on the active host.
    pub(super) fn reverse_focus_history(&mut self, scope: FocusHistoryScope, tx: &mut Tx) {
        if let Some(reason) = self.focus_history_blocked() {
            self.clear_focus_history_walk();
            self.inform(reason);
            return;
        }
        self.exit_copy_mode();
        self.cancel_layer_drag();
        self.navigation.cancel();

        let host_id = self.hosts[self.active].id;
        let epoch = self.model().connection_epoch;
        let now = Instant::now();

        if let Some(walk) = &self.focus_history.walk {
            if walk.host != host_id || walk.epoch != epoch || walk.scope != scope {
                self.focus_history.clear();
            } else if walk.pending_id.is_some() {
                let expired = walk
                    .pending_deadline
                    .is_some_and(|deadline| now >= deadline);
                if !expired {
                    // Ignore rapid duplicate triggers while a reverse is already in flight so
                    // we never send a second `cursor: None` that would bounce focus back.
                    return;
                }
                self.focus_history.clear();
            }
        }

        let cursor = self.focus_history.walk.as_ref().and_then(|w| w.cursor);
        let id = self.focus_history.next_id();
        let deadline = now + PENDING_TIMEOUT;
        self.focus_history.walk = Some(Walk {
            host: host_id,
            epoch,
            scope,
            cursor,
            pending_id: Some(id),
            pending_deadline: Some(deadline),
        });
        if self.hosts[self.active]
            .connection
            .commands
            .send(HostCommand::ReverseFocusHistory {
                epoch,
                id,
                scope,
                cursor,
                deadline,
            })
            .is_err()
        {
            self.focus_history.clear();
            self.notify("Host worker stopped. Restart Rex to reconnect.");
            return;
        }
        tx.wake_after(PENDING_TIMEOUT);
    }

    /// Reconcile the walk state after host `index` updates its `HostModel`.
    pub(super) fn reconcile_focus_history(&mut self, index: usize, tx: &mut Tx) {
        let host_id = self.hosts[index].id;
        let Some(walk) = &self.focus_history.walk else {
            return;
        };
        if walk.host != host_id {
            return;
        }
        let walk_epoch = walk.epoch;
        let pending_id = walk.pending_id;
        let (connected, live_epoch, result) = {
            let model = &self.hosts[index].model;
            (
                model.connected,
                model.connection_epoch,
                model.focus_history_result.clone(),
            )
        };
        if !connected || live_epoch != walk_epoch {
            self.focus_history.clear();
            return;
        }
        let Some(result) = result else {
            return;
        };
        if result.epoch != walk_epoch || pending_id != Some(result.id) {
            return;
        }
        if result.reversed {
            if let Some(walk) = &mut self.focus_history.walk {
                walk.pending_id = None;
                walk.pending_deadline = None;
                walk.cursor = result.cursor;
            }
            if self.session_inspector_out_of_context() {
                self.close_panel(tx);
            }
            self.refresh_session_inspector_watches();
            self.observe_navigation();
        } else {
            self.focus_history.clear();
        }
    }

    /// Expire an overdue in-flight reverse request so the walk never wedges indefinitely.
    pub fn poll_focus_history(&mut self, now: Instant, tx: &mut Tx) -> bool {
        let Some(walk) = &self.focus_history.walk else {
            return false;
        };
        if walk.pending_id.is_none() {
            return false;
        }
        let Some(deadline) = walk.pending_deadline else {
            return false;
        };
        if now >= deadline {
            self.focus_history.clear();
            return true;
        }
        tx.wake_after(deadline - now);
        false
    }
}
