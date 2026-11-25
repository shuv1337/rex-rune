//! Per-host consent and the deliberately small remote surface: view-only actions plus
//! explicitly targeted, idempotent tab muting.
use super::{
    actions::{self, Remote},
    navigation::Visit,
    tabs::TabTarget,
    workspace::{HostId, Workspace},
};
use rex_rune::connection::{
    HostCommand, ViewFetchOutcome,
    perform::{MAX_WAIT, Outcome, Pending, sanitize_text},
};
use rune::Tx;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

pub struct Navigation {
    host: HostId,
    pending: Pending,
    id: u64,
    deadline: Instant,
}

pub struct PendingMute {
    host: HostId,
    pending: Pending,
    fetch_id: u64,
    session: String,
    deadline: Instant,
}

enum MuteTarget {
    Ready(TabTarget),
    FetchSession(String),
}

const TYPING: Duration = Duration::from_millis(1500);

fn sanitize_label(label: &str) -> String {
    let cleaned = sanitize_text(label, 60);
    if cleaned.trim().is_empty() {
        "tab".into()
    } else {
        cleaned
    }
}

impl Workspace {
    pub fn toggle_remote_control(&mut self, host: usize) {
        self.settings.hosts[host].remote_control = !self.settings.hosts[host].remote_control;
        if !self.settings.hosts[host].remote_control {
            self.poll_remote_mutes(Instant::now());
        }
        self.save();
    }

    fn note_remote_off(&mut self, host: usize, pending: &Pending) {
        if self.remote_off_notices.insert(self.hosts[host].id) {
            self.inform(format!(
                "{} on {} asked for {}. Remote control is off (Settings → Remote Hosts).",
                pending.request.requester, self.settings.hosts[host].label, pending.request.action
            ));
        }
    }

    fn resolve_remote_mute(&self, host: usize, pending: &Pending) -> Result<MuteTarget, Outcome> {
        let request = &pending.request;
        let Some(window_id) = request.args["window_id"].as_str() else {
            return Err(Outcome::error(
                "bad_request",
                "Remote client.tab.mute requires window_id and muted; nothing was done.",
            ));
        };
        if !request.args["muted"].is_boolean() {
            return Err(Outcome::error(
                "bad_request",
                "Remote client.tab.mute requires window_id and muted; nothing was done.",
            ));
        }
        let model = &self.hosts[host].model;
        if !model.connected || pending.received.elapsed() >= MAX_WAIT {
            return Err(Outcome::error(
                "internal",
                "Rex Rune did not act in time or the host disconnected; nothing was done.",
            ));
        }
        let arg_session = request.args["session_id"].as_str();
        let env_session = request.session_id.as_deref();
        let env_block = request.block_id.as_deref();
        if let Some(session_id) = arg_session {
            if env_session.is_some_and(|id| id != session_id) {
                return Err(Outcome::error(
                    "refused",
                    "The requested tab does not belong to the request's session; nothing was done.",
                ));
            }
            if !model.sessions.iter().any(|s| s.session_id == session_id) {
                return Err(Outcome::error(
                    "not_found",
                    "That session does not exist on this host.",
                ));
            }
            if !model.views.contains_key(session_id) {
                return Ok(MuteTarget::FetchSession(session_id.to_owned()));
            }
            let Some(target) = self.tab_target(host, session_id, window_id) else {
                return Err(Outcome::error(
                    "not_found",
                    "That tab does not exist in the requested session.",
                ));
            };
            if env_block.is_some_and(|block| !self.tab_has_block(&target, block)) {
                return Err(Outcome::error(
                    "refused",
                    "The requested pane does not belong to that tab; nothing was done.",
                ));
            }
            return Ok(MuteTarget::Ready(target));
        }
        if let Some(target) = self.find_tab(host, window_id, None) {
            if env_session.is_some_and(|id| id != target.session) {
                return Err(Outcome::error(
                    "refused",
                    "The requested tab does not belong to the request's session; nothing was done.",
                ));
            }
            if env_block.is_some_and(|block| !self.tab_has_block(&target, block)) {
                return Err(Outcome::error(
                    "refused",
                    "The requested pane does not belong to that tab; nothing was done.",
                ));
            }
            return Ok(MuteTarget::Ready(target));
        }
        if let Some(session_id) = env_session
            && model.sessions.iter().any(|s| s.session_id == session_id)
            && !model.views.contains_key(session_id)
        {
            return Ok(MuteTarget::FetchSession(session_id.to_owned()));
        }
        Err(Outcome::error(
            "not_found",
            "That tab does not exist on this host.",
        ))
    }

    fn apply_remote_mute(&mut self, host: usize, pending: Pending, target: TabTarget) {
        let muted = pending.request.args["muted"]
            .as_bool()
            .expect("validated muted bool");
        let changed = self.set_tab_muted(&target, muted);
        let notice = changed.then(|| {
            let verb = if muted { "muted" } else { "unmuted" };
            format!(
                "{}: {} {verb} \"{}\"",
                self.settings.hosts[host].label,
                pending.request.requester,
                sanitize_label(&target.name)
            )
        });
        self.remote_reply_with_notice(host, pending, Outcome::Performed, notice);
    }

    fn remote_policy(
        &self,
        host: usize,
        pending: &Pending,
        field_focused: bool,
    ) -> Result<(), Outcome> {
        let request = &pending.request;
        let Some(spec) = actions::find(&request.action) else {
            return Err(Outcome::error(
                "not_supported",
                "This remote action is not supported by Rex Rune; nothing was done.",
            ));
        };
        let Remote::View { navigation } = spec.remote() else {
            let Remote::Never(message) = spec.remote() else {
                unreachable!("Remote::TabState is handled before remote_policy")
            };
            return Err(Outcome::error(
                "not_supported",
                format!(
                    "{} is not available remotely. {message} Nothing was done.",
                    request.action
                ),
            ));
        };
        if !self.settings.hosts[host].remote_control {
            return Err(Outcome::error(
                "refused",
                "Remote control is off for this host in Rex Rune. The person at that computer can turn it on in Settings → Remote Hosts. Nothing was done.",
            ));
        }
        if !spec.accepts(&request.args) {
            return Err(Outcome::error(
                "bad_request",
                format!(
                    "Invalid arguments for {}; nothing was done.",
                    request.action
                ),
            ));
        }
        let selecting = request.action == actions::SELECT_SESSION;
        let model = &self.hosts[host].model;
        let refused = if self.panel.is_some() {
            Some("A dialog or popup is open")
        } else if field_focused {
            Some("A text field has focus")
        } else if self.bindings.sequence.depth > 0 {
            Some("A key sequence is pending")
        } else if !selecting && self.active != host {
            Some("This host is not on screen")
        } else if request.session_id.as_ref().is_some_and(|id| {
            if selecting {
                request.args["session_id"].as_str() != Some(id.as_str())
            } else {
                model.selected.as_ref() != Some(id)
            }
        }) {
            Some("The requested session is not on screen")
        } else if request
            .block_id
            .as_deref()
            .is_some_and(|id| model.focused() != Some(id))
        {
            Some("The requested pane is not focused")
        } else if navigation && self.last_keypress.is_some_and(|at| at.elapsed() < TYPING) {
            Some("The person is typing; try again after a pause")
        } else if self.navigation.pending.is_some() || self.remote_navigation.is_some() {
            Some("Another navigation is still in progress")
        } else {
            None
        };
        if let Some(reason) = refused {
            return Err(Outcome::error(
                "refused",
                format!("{reason}; nothing was done."),
            ));
        }
        if !self.hosts[host].model.connected || pending.received.elapsed() >= MAX_WAIT {
            return Err(Outcome::error(
                "internal",
                "Rex Rune did not act in time or the host disconnected; nothing was done.",
            ));
        }
        if selecting {
            self.session_visit(host, &request.args)?;
            return Ok(());
        }
        if request.action != "client.sidebar.toggle" && self.model().window().is_none() {
            return Err(Outcome::error(
                "internal",
                "No tab is on screen; nothing was done.",
            ));
        }
        if request.action == "client.tab.goto"
            && actions::tab_index(
                &request.args,
                self.model().view().map_or(0, |v| v.windows.len()),
            )
            .is_none()
        {
            return Err(Outcome::error(
                "bad_request",
                "That tab index is out of range; nothing was done.",
            ));
        }
        if request.action == "pane.scroll" && self.active_terminal().is_none() {
            return Err(Outcome::error(
                "internal",
                "No terminal is on screen; nothing was done.",
            ));
        }
        Ok(())
    }

    pub(super) fn remote_perform(&mut self, host: usize, pending: Pending, tx: &mut Tx) {
        // A sync wake may outlive its socket. Do not execute or answer it on the replacement.
        if pending.epoch != self.hosts[host].model.connection_epoch {
            return;
        }
        if let Some(spec) =
            actions::find(&pending.request.action).filter(|s| s.remote() == Remote::TabState)
        {
            if !self.settings.hosts[host].remote_control {
                self.note_remote_off(host, &pending);
                self.remote_reply(
                    host,
                    pending,
                    Outcome::error(
                        "refused",
                        "Remote control is off for this host in Rex Rune. The person at that computer can turn it on in Settings → Remote Hosts. Nothing was done.",
                    ),
                );
                return;
            }
            if !spec.accepts(&pending.request.args) {
                let message = format!(
                    "Invalid arguments for {}; nothing was done.",
                    pending.request.action
                );
                self.remote_reply(host, pending, Outcome::error("bad_request", message));
                return;
            }
            match self.resolve_remote_mute(host, &pending) {
                Ok(MuteTarget::Ready(target)) => self.apply_remote_mute(host, pending, target),
                Ok(MuteTarget::FetchSession(session)) => {
                    self.next_fetch_id += 1;
                    let fetch_id = self.next_fetch_id;
                    let deadline = pending.received + MAX_WAIT;
                    let epoch = pending.epoch;
                    self.send(
                        host,
                        HostCommand::FetchView {
                            id: fetch_id,
                            epoch,
                            deadline,
                            session: session.clone(),
                        },
                    );
                    self.remote_mutes.push(PendingMute {
                        host: self.hosts[host].id,
                        pending,
                        fetch_id,
                        session,
                        deadline,
                    });
                    tx.wake_after(deadline.saturating_duration_since(Instant::now()));
                }
                Err(outcome) => self.remote_reply(host, pending, outcome),
            }
            return;
        }
        // Rune supplies Tx focus for input handlers, but not AppHandle::update.
        // Remote requests arrive through the latter, so inspect mounted search editors too.
        let field_focused = tx
            .focused()
            .is_some_and(|key| key != super::input::TERMINAL_FOCUS)
            || self.model().window().is_some_and(|window| {
                rex_rune::layout::placements(window, None)
                    .0
                    .iter()
                    .any(|p| {
                        self.terminals
                            .get(&self.term_key(&p.block))
                            .and_then(|t| t.search.as_ref())
                            .is_some_and(|edit| edit.focused)
                    })
            });
        let outcome = match self.remote_policy(host, &pending, field_focused) {
            Err(outcome) => {
                if !self.settings.hosts[host].remote_control
                    && matches!(
                        actions::find(&pending.request.action).map(|s| s.remote()),
                        Some(Remote::View { .. })
                    )
                {
                    self.note_remote_off(host, &pending);
                }
                outcome
            }
            Ok(()) => {
                if pending.request.action == actions::SELECT_SESSION {
                    let visit = self
                        .session_visit(host, &pending.request.args)
                        .expect("validated above");
                    if let Some(id) = self.navigate_remote(visit, tx) {
                        self.remote_navigation = Some(Navigation {
                            host: self.hosts[host].id,
                            pending,
                            id,
                            deadline: Instant::now() + MAX_WAIT,
                        });
                        return;
                    }
                    self.remote_reply(
                        host,
                        pending,
                        Outcome::error("internal", "Navigation could not start."),
                    );
                    return;
                }
                if actions::run(self, &pending.request.action, &pending.request.args, tx) {
                    Outcome::Performed
                } else {
                    Outcome::error("internal", "The action could not run; nothing was done.")
                }
            }
        };
        self.remote_reply(host, pending, outcome);
    }

    pub(super) fn poll_remote_mutes(&mut self, now: Instant) -> bool {
        let mut completions = HashMap::new();
        for host in &mut self.hosts {
            if !host.model.view_fetches.is_empty() {
                for (id, outcome) in std::mem::take(&mut host.model.view_fetches) {
                    completions.insert((host.id, id), outcome);
                }
            }
        }
        if self.remote_mutes.is_empty() {
            return false;
        }
        let mutes = std::mem::take(&mut self.remote_mutes);
        let mut changed = false;
        for mute in mutes {
            let Some(host) = self.host_index(mute.host) else {
                changed = true;
                continue;
            };
            let model = &self.hosts[host].model;
            if !model.connected || model.connection_epoch != mute.pending.epoch {
                changed = true;
                continue;
            }
            if !self.settings.hosts[host].remote_control {
                changed = true;
                self.remote_reply(
                    host,
                    mute.pending,
                    Outcome::error(
                        "refused",
                        "Remote control is off for this host in Rex Rune. The person at that computer can turn it on in Settings → Remote Hosts. Nothing was done.",
                    ),
                );
                continue;
            }
            let view_loaded = model.views.contains_key(&mute.session);
            let fetch_done = completions.get(&(mute.host, mute.fetch_id)).copied();
            if view_loaded || fetch_done.is_some() {
                changed = true;
                if view_loaded {
                    match self.resolve_remote_mute(host, &mute.pending) {
                        Ok(MuteTarget::Ready(target)) => {
                            self.apply_remote_mute(host, mute.pending, target);
                        }
                        Ok(MuteTarget::FetchSession(_)) => {
                            self.remote_reply(
                                host,
                                mute.pending,
                                Outcome::error(
                                    "internal",
                                    "Could not load the session view; nothing was done.",
                                ),
                            );
                        }
                        Err(outcome) => self.remote_reply(host, mute.pending, outcome),
                    }
                } else if fetch_done == Some(ViewFetchOutcome::NotFound)
                    || !self.hosts[host]
                        .model
                        .sessions
                        .iter()
                        .any(|s| s.session_id == mute.session)
                {
                    self.remote_reply(
                        host,
                        mute.pending,
                        Outcome::error("not_found", "That session does not exist on this host."),
                    );
                } else {
                    self.remote_reply(
                        host,
                        mute.pending,
                        Outcome::error(
                            "internal",
                            "Could not load the session view; nothing was done.",
                        ),
                    );
                }
            } else if now >= mute.deadline {
                changed = true;
                self.remote_reply(
                    host,
                    mute.pending,
                    Outcome::error(
                        "internal",
                        "Rex Rune did not load the session view in time; nothing was done.",
                    ),
                );
            } else {
                self.remote_mutes.push(mute);
            }
        }
        changed
    }

    pub(super) fn next_remote_mute_deadline(&self, now: Instant) -> Option<Duration> {
        self.remote_mutes
            .iter()
            .map(|m| m.deadline.saturating_duration_since(now))
            .min()
    }

    pub(super) fn session_visit(
        &self,
        host: usize,
        args: &serde_json::Value,
    ) -> Result<Visit, Outcome> {
        let model = &self.hosts[host].model;
        let session = args["session_id"].as_str().unwrap_or_default();
        let window = args["window_id"].as_str();
        if !model.sessions.iter().any(|s| s.session_id == session) {
            return Err(Outcome::error(
                "not_found",
                "That session does not exist on this host.",
            ));
        }
        // For an uncached session the server checks window membership during focus;
        // attachment fills the cache so a failed acknowledgement can name not_found.
        if let (Some(window), Some(view)) = (window, model.views.get(session))
            && !view
                .windows
                .iter()
                .any(|w| w.window_id.as_deref() == Some(window))
        {
            return Err(Outcome::error(
                "not_found",
                "That tab does not exist in the requested session.",
            ));
        }
        Ok(Visit {
            host: self.hosts[host].id,
            session: session.into(),
            window: window.map(str::to_owned),
        })
    }

    /// Success means the worker acknowledged navigation and the target is showing.
    /// A disconnect abandons the call: its id must never reach a replacement socket.
    pub(super) fn poll_remote_navigation(&mut self, now: Instant) -> bool {
        let Some(navigation) = self.remote_navigation.take() else {
            return false;
        };
        let Some(host) = self.host_index(navigation.host) else {
            return true;
        };
        let model = &self.hosts[host].model;
        if !model.connected || model.connection_epoch != navigation.pending.epoch {
            return true;
        }
        let result = model
            .navigation_result
            .filter(|(id, _)| *id == navigation.id);
        let outcome = if let Some((_, success)) = result {
            let args = &navigation.pending.request.args;
            let showing = self.active == host
                && model.selected.as_deref() == args["session_id"].as_str()
                && args["window_id"].as_str().is_none_or(|id| {
                    model.window().and_then(|w| w.window_id.as_deref()) == Some(id)
                });
            if success && showing {
                Some(Outcome::Performed)
            } else if let Err(error) = self.session_visit(host, args) {
                Some(error)
            } else {
                Some(Outcome::error(
                    "internal",
                    "Navigation failed or the view changed before it completed.",
                ))
            }
        } else if now >= navigation.deadline {
            Some(Outcome::error(
                "internal",
                "Rex Rune did not finish navigation in five seconds.",
            ))
        } else if self
            .navigation
            .pending
            .as_ref()
            .is_none_or(|p| p.id != navigation.id)
        {
            Some(Outcome::error("internal", "Navigation was interrupted."))
        } else {
            None
        };
        if let Some(outcome) = outcome {
            self.remote_reply(host, navigation.pending, outcome);
            true
        } else {
            self.remote_navigation = Some(navigation);
            false
        }
    }

    pub(super) fn remote_reply(&mut self, host: usize, pending: Pending, outcome: Outcome) {
        let notice = (outcome == Outcome::Performed).then(|| {
            format!(
                "{}: {} performed {}",
                self.settings.hosts[host].label, pending.request.requester, pending.request.action
            )
        });
        self.remote_reply_with_notice(host, pending, outcome, notice);
    }

    fn remote_reply_with_notice(
        &mut self,
        host: usize,
        pending: Pending,
        outcome: Outcome,
        notice: Option<String>,
    ) {
        log::info!(
            "Remote control on {}: {} requested {}: {:?}",
            self.settings.hosts[host].label,
            pending.request.requester,
            pending.request.action,
            outcome
        );
        if let Some(text) = notice {
            self.inform(text);
        }
        self.send(
            host,
            HostCommand::PerformReply {
                epoch: pending.epoch,
                call_id: pending.request.call_id,
                outcome,
            },
        );
    }
}
