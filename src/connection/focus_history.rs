//! Server-side focus history (`focus_history.reverse`, Rex Server API 19+).
use super::{
    Client, ControlState, Shared, attach_within, list_within, refresh_background_within,
    status_sweep,
};
use anyhow::Result;
use rexrs::api::focus_history::Reverse;
use std::time::{Duration, Instant};

const CALL_TIMEOUT: Duration = Duration::from_secs(8);

/// Minimum Rex Server API revision that supports `focus_history.reverse`.
pub const FOCUS_HISTORY_MIN_API_REVISION: u32 = 19;
pub const FOCUS_HISTORY_UNSUPPORTED_NOTICE: &str =
    "This Rex server doesn't support focus history. Update Rex on the host.";
pub const FOCUS_HISTORY_EMPTY_NOTICE: &str = "No earlier focus target.";
pub const FOCUS_HISTORY_DISCONNECTED_NOTICE: &str =
    "Focus history not sent while disconnected. Try again after reconnecting.";
pub const FOCUS_HISTORY_EXPIRED_NOTICE: &str =
    "Focus history request timed out; action was not retried.";
pub const FOCUS_HISTORY_SYNC_TIMEOUT_NOTICE: &str =
    "Moved focus, but refreshing the target view timed out.";
pub const FOCUS_HISTORY_REQUIRES_API: &str = "Requires Rex Server API 19 or later";
pub const FOCUS_HISTORY_DISCONNECTED: &str = "Connect to the host to use focus history";

/// Filter scope for `focus_history.reverse`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FocusHistoryScope {
    /// Last focused pane anywhere on this host (`{}`).
    #[default]
    Any,
    /// Last focused pane in the current tab (`{"window_id": current_window}`).
    Pane,
    /// Last focused other tab in this session (`{"session_id": current_session, "not_window_id": current_window}`).
    Tab,
    /// Last focused other tab in any session on this host (`{"not_window_id": current_window}`).
    TabAnySession,
    /// Last focused other session on this host (`{"not_session_id": current_session}`).
    Session,
}

impl FocusHistoryScope {
    pub const ALL_ARGS: [&'static str; 5] = ["any", "pane", "tab", "tab_any_session", "session"];

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "any" => Some(Self::Any),
            "pane" => Some(Self::Pane),
            "tab" => Some(Self::Tab),
            "tab_any_session" => Some(Self::TabAnySession),
            "session" => Some(Self::Session),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Pane => "pane",
            Self::Tab => "tab",
            Self::TabAnySession => "tab_any_session",
            Self::Session => "session",
        }
    }

    /// Build the typed `focus_history.reverse` payload from the live foreground session/window.
    pub fn build_request(
        self,
        session_id: Option<&str>,
        window_id: Option<&str>,
        cursor: Option<u64>,
    ) -> std::result::Result<Reverse, &'static str> {
        let mut req = match self {
            Self::Any => Reverse::new(),
            Self::Pane => {
                let window = window_id.ok_or("No active tab to search for an earlier pane.")?;
                Reverse::new().window_id(window)
            }
            Self::Tab => {
                let session = session_id.ok_or("No active session to step back from.")?;
                let window = window_id.ok_or("No active tab to step back from.")?;
                Reverse::new().session_id(session).not_window_id(window)
            }
            Self::TabAnySession => {
                let window = window_id.ok_or("No active tab to step back from.")?;
                Reverse::new().not_window_id(window)
            }
            Self::Session => {
                let session = session_id.ok_or("No active session to step back from.")?;
                Reverse::new().not_session_id(session)
            }
        };
        if let Some(cursor) = cursor {
            req = req.cursor(cursor);
        }
        Ok(req)
    }
}

/// Correlated completion of a `HostCommand::ReverseFocusHistory` request on one connection epoch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusHistoryResult {
    pub epoch: u64,
    pub id: u64,
    pub reversed: bool,
    pub cursor: Option<u64>,
    pub session_id: Option<String>,
    pub window_id: Option<String>,
    pub block_id: Option<String>,
}

impl FocusHistoryResult {
    pub fn not_reversed(epoch: u64, id: u64) -> Self {
        Self {
            epoch,
            id,
            reversed: false,
            cursor: None,
            session_id: None,
            window_id: None,
            block_id: None,
        }
    }
}

fn is_transport_error(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<rexrs::Error>(),
        Some(rexrs::Error::Transport(_) | rexrs::Error::Closed { .. })
    )
}

/// Reject an expired reverse request without sending `focus_history.reverse`.
/// Silently ignored when the connection epoch has already moved on.
pub(super) fn expire(shared: &Shared, epoch: u64, id: u64) {
    shared.update(|m| {
        if m.connection_epoch != epoch {
            return;
        }
        m.notice = Some(FOCUS_HISTORY_EXPIRED_NOTICE.into());
        m.focus_history_result = Some(FocusHistoryResult::not_reversed(epoch, id));
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn reverse(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    sweep: &mut status_sweep::Reader,
    epoch: u64,
    id: u64,
    scope: FocusHistoryScope,
    cursor: Option<u64>,
    deadline: Instant,
) -> Result<()> {
    let (live_epoch, current_session, current_window) = {
        let m = shared.model.lock().expect("host model");
        (
            m.connection_epoch,
            m.selected.clone(),
            m.window().and_then(|w| w.window_id.clone()),
        )
    };
    if epoch != live_epoch {
        return Ok(());
    }
    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
        expire(shared, epoch, id);
        return Ok(());
    };
    if control.welcome().api_revision < FOCUS_HISTORY_MIN_API_REVISION {
        shared.update(|m| {
            m.notice = Some(FOCUS_HISTORY_UNSUPPORTED_NOTICE.into());
            m.focus_history_result = Some(FocusHistoryResult::not_reversed(epoch, id));
        });
        return Ok(());
    }
    let req = match scope.build_request(
        current_session.as_deref(),
        current_window.as_deref(),
        cursor,
    ) {
        Ok(req) => req,
        Err(reason) => {
            shared.update(|m| {
                m.notice = Some(reason.into());
                m.focus_history_result = Some(FocusHistoryResult::not_reversed(epoch, id));
            });
            return Ok(());
        }
    };

    let response = match tokio::time::timeout(remaining.min(CALL_TIMEOUT), control.call(req)).await
    {
        Ok(Ok(response)) => response,
        Ok(Err(rexrs::Error::Api(
            rexrs::ApiError::MethodNotFound { .. }
            | rexrs::ApiError::RevisionTooLow { .. }
            | rexrs::ApiError::NotSupported { .. },
        ))) => {
            shared.update(|m| {
                m.notice = Some(FOCUS_HISTORY_UNSUPPORTED_NOTICE.into());
                m.focus_history_result = Some(FocusHistoryResult::not_reversed(epoch, id));
            });
            return Ok(());
        }
        Ok(Err(rexrs::Error::Api(error))) => {
            let message = crate::download::display_text(&error.to_string());
            shared.update(|m| {
                m.notice = Some(format!("Could not step focus history: {message}"));
                m.focus_history_result = Some(FocusHistoryResult::not_reversed(epoch, id));
            });
            return Ok(());
        }
        Ok(Err(error)) => return Err(error.into()),
        Err(_) => {
            expire(shared, epoch, id);
            return Ok(());
        }
    };

    if !response.reversed {
        shared.update(|m| {
            m.notice = Some(FOCUS_HISTORY_EMPTY_NOTICE.into());
            m.focus_history_result = Some(FocusHistoryResult::not_reversed(epoch, id));
        });
        return Ok(());
    }

    // Publish the confirmed reverse and cursor immediately so the UI captures the
    // server cursor even if follow-up target sync later hits the shared deadline.
    shared.update(|m| {
        m.notice = None;
        m.focus_history_result = Some(FocusHistoryResult {
            epoch,
            id,
            reversed: true,
            cursor: response.cursor,
            session_id: response.session_id.clone(),
            window_id: response.window_id.clone(),
            block_id: response.block_id.clone(),
        });
    });

    let target_session = response.session_id.or(current_session);
    let mut sync_timed_out = false;
    if let Some(target) = target_session.as_deref() {
        let (listed, has_view) = {
            let m = shared.model.lock().expect("host model");
            (
                m.sessions.iter().any(|s| s.session_id == target),
                m.views.contains_key(target),
            )
        };
        let mut sync_error: Option<anyhow::Error> = None;
        if !listed {
            match deadline.checked_duration_since(Instant::now()) {
                Some(rem) => {
                    match list_within(control, shared, state, sweep, rem.min(CALL_TIMEOUT)).await {
                        Ok(()) => {}
                        Err(err) if err.is::<tokio::time::error::Elapsed>() => {
                            sync_timed_out = true;
                        }
                        Err(err) => sync_error = Some(err),
                    }
                }
                None => sync_timed_out = true,
            }
        }
        if !sync_timed_out && sync_error.is_none() {
            match deadline.checked_duration_since(Instant::now()) {
                Some(rem) => {
                    let budget = rem.min(CALL_TIMEOUT);
                    let res = if !state.attached.contains(target) || !has_view {
                        attach_within(control, shared, state, sweep, target, budget).await
                    } else {
                        // Use session.attach rather than session.view so refreshing the
                        // reversed target's view never re-writes server-side focus history.
                        refresh_background_within(control, shared, state, sweep, target, budget)
                            .await
                    };
                    match res {
                        Ok(()) => {}
                        Err(err) if err.is::<tokio::time::error::Elapsed>() => {
                            sync_timed_out = true;
                        }
                        Err(err) => sync_error = Some(err),
                    }
                }
                None => sync_timed_out = true,
            }
        }
        if let Some(error) = sync_error {
            if is_transport_error(&error) {
                return Err(error);
            }
            let message = crate::download::display_text(&format!("{error:#}"));
            shared.update(|m| {
                m.notice = Some(format!("Could not show previous focus target: {message}"));
            });
            return Ok(());
        }
        shared.update(|m| {
            if sync_timed_out {
                m.notice = Some(FOCUS_HISTORY_SYNC_TIMEOUT_NOTICE.into());
            } else {
                m.background_views.remove(target);
                m.selected = Some(target.to_string());
            }
        });
    }
    Ok(())
}
