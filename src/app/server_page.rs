//! Cached server facts, a page-local uptime clock, and an explicitly confirmed stop.
use super::{
    settings_kit::{self as kit, button, card, footnote, page},
    theme::*,
    widgets::Pill,
    workspace::Workspace,
};
use rex_rune::connection::server_info::Answer;
use rune::{El, col, text};

pub const STOP: &str = "host.stop_server";
const MISSING: &str = "Not reported";

#[derive(Clone, Debug)]
pub struct StopTarget {
    host: super::workspace::HostId,
    epoch: u64,
    endpoint: String,
}

pub(super) fn uptime(
    model: &rex_rune::connection::HostModel,
    now: std::time::SystemTime,
) -> Option<String> {
    if !model.connected {
        return None;
    }
    let start = model
        .server_info
        .process
        .reported()?
        .started_at
        .as_deref()?;
    let seconds = rex_rune::connection::activity::time::reported_elapsed(start, now)?.as_secs();
    Some(match seconds {
        0..60 => "now".into(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86400),
    })
}

impl Workspace {
    pub fn can_stop_server(&self) -> bool {
        self.hosts
            .get(self.active)
            .is_some_and(|h| h.model.connected)
            && self.settings.hosts.get(self.active).is_some_and(|h| {
                h.endpoint.starts_with("unix://") || h.endpoint.starts_with("ssh://")
            })
    }

    pub fn stop_server(&mut self) {
        if !self.can_stop_server() {
            return;
        }
        let host = &self.hosts[self.active];
        let saved = &self.settings.hosts[self.active];
        let target = StopTarget {
            host: host.id,
            epoch: host.model.connection_epoch,
            endpoint: saved.endpoint.clone(),
        };
        let count = host
            .model
            .server_info
            .process
            .reported()
            .and_then(|p| p.session_count);
        let sessions = count.map_or_else(
            || "The session count was not reported.".into(),
            |n| format!("Last reported session count: {n}."),
        );
        let description = format!(
            "Stop the server on {}? Every program running in its sessions ends. {sessions} Other clients are disconnected. Rex Rune will not start it again.",
            saved.label
        );
        self.confirm(
            "Stop server?",
            "server.stop",
            serde_json::json!({}),
            &description,
        );
        self.panel.as_mut().unwrap().stop_server = Some(target);
    }

    pub(super) fn confirm_stop_server(&mut self, target: StopTarget) {
        let current = self.can_stop_server()
            && self.hosts[self.active].id == target.host
            && self.hosts[self.active].model.connection_epoch == target.epoch
            && self.settings.hosts[self.active].endpoint == target.endpoint;
        if !current {
            self.inform("Stop not sent: the host or connection changed. Review Stop server again.");
            return;
        }
        self.send(
            self.active,
            rex_rune::connection::HostCommand::StopServer {
                epoch: target.epoch,
            },
        );
    }

    pub(super) fn refresh_server_at(
        &mut self,
        now: std::time::Instant,
    ) -> (bool, Option<std::time::Duration>) {
        let visible = self.settings_page() == super::panel::SettingsSection::Server
            && self
                .panel
                .as_ref()
                .is_some_and(|p| matches!(p.kind, super::panel::PanelKind::Settings));
        let Some(panel) = self.panel.as_mut() else {
            return (false, None);
        };
        if !visible {
            panel.server_refresh = None;
            return (false, None);
        }
        let deadline = panel
            .server_refresh
            .get_or_insert(now + std::time::Duration::from_secs(60));
        let changed = now >= *deadline;
        if changed {
            *deadline = now + std::time::Duration::from_secs(60);
        }
        (changed, Some(deadline.saturating_duration_since(now)))
    }
}

fn controls(s: &Workspace) -> El {
    let duration = uptime(&s.hosts[s.active].model, std::time::SystemTime::now());
    let mut rows = Vec::new();
    if let Some(duration) = duration {
        rows.push(fact("Running for", duration, None));
    }
    rows.push(kit::setting(
        "Stop server…",
        if s.can_stop_server() {
            "Ends every program in this server’s sessions."
        } else if !s.hosts[s.active].model.connected {
            "Available while this host is connected over its Unix socket or SSH."
        } else {
            "A server can be stopped only from its own machine or over SSH."
        },
        if s.can_stop_server() {
            button("server-stop", "Stop server…", Pill::Danger, |s, _| {
                s.stop_server()
            })
            .help(s.shortcut_help("Stop server…", STOP))
        } else {
            rune::el()
        },
    ));
    let mut contents = vec![
        card(rows),
        footnote(
            "Running time uses the last reported start time and this client’s clock, not live monitoring.",
        ),
    ];
    if let Some(result) = &s.hosts[s.active].model.server_stop_result {
        contents.push(footnote(format!("Last stop request: {result}")).key("server-stop-result"));
    }
    kit::section("Server controls", contents)
}

/// Preserve literal absolute Unix paths; strip credentials, query and fragment from URLs.
/// Never link reported addresses. Unknown listener syntax is not echoed unfiltered.
pub(super) fn address(raw: &str) -> Option<String> {
    if raw.chars().any(char::is_control) {
        return None;
    }
    // Transport treats everything after unix:// as a literal path, including ? and #.
    if let Some(path) = raw.strip_prefix("unix://") {
        return path.starts_with('/').then(|| raw.to_owned());
    }
    if let Ok(socket) = raw.parse::<std::net::SocketAddr>() {
        return Some(socket.to_string());
    }
    let mut url = url::Url::parse(raw).ok()?;
    match url.scheme() {
        "https" | "http" | "wss" | "ws" | "ssh" if url.host_str().is_some() => {
            url.set_password(None).ok()?;
            url.set_username("").ok()?;
        }
        _ => return None,
    }
    url.set_query(None);
    url.set_fragment(None);
    Some(url.to_string())
}

fn absent<T>(answer: &Answer<T>, connected: bool) -> &'static str {
    if connected && !answer.settled() {
        "Waiting for report"
    } else {
        MISSING
    }
}

/// Stacked values leave room for long addresses at 640 pt. Search indexes the actual label,
/// not the host's private values. The whole value remains selectable and in its tooltip.
fn fact(label: &str, value: impl Into<String>, control: Option<El>) -> El {
    let value = value.into();
    let words = col()
        .grow(1.)
        .shrink()
        .min_w(0.)
        .gap(2.)
        .child(text(label.to_owned()).font(ui(13.)).color(rgb(ink())))
        .child(
            kit::value(value.clone())
                .w_full()
                .truncate()
                .help_when_truncated(value),
        );
    let mut row = super::settings_nav::row(label, "", kit::row_frame(true)).child(words);
    if let Some(control) = control {
        row = row.child(control);
    }
    row
}

fn address_row(key: &str, label: &str, address: Option<String>, fallback: &str) -> El {
    let control = address.as_ref().map(|address| {
        let address = address.clone();
        button(key, "Copy", Pill::Neutral, move |s, tx| {
            tx.copy(address.clone());
            s.inform("Copied address to clipboard.");
        })
        .help(format!("Copy {}", label.to_lowercase()))
    });
    fact(label, address.unwrap_or_else(|| fallback.into()), control)
}

/// Its own row, so it can be found by name and says why it is not offered: pairing shows
/// only an address this server reports now, never one last heard from a host that has
/// since dropped.
fn pair_row(available: bool, reported: bool) -> El {
    fact(
        "Pair device",
        if available {
            "A QR code of the Tailscale address, for Rex on another device."
        } else if reported {
            "Available while this host is connected."
        } else {
            "Needs a Tailscale HTTPS address reported by this server."
        },
        available.then(|| {
            button("server-pair", "Pair", Pill::Neutral, |s, _| s.pair_device())
                .help("Show the address as a QR code")
        }),
    )
}

pub(super) fn tailscale_address(model: &rex_rune::connection::HostModel) -> Option<String> {
    model
        .server_info
        .process
        .reported()?
        .tailscale
        .as_ref()?
        .https
        .as_ref()
        .filter(|h| h.state == "available")
        .and_then(|h| h.url.as_deref())
        .and_then(address)
        .filter(|url| url.starts_with("https://"))
}

fn last_state(state: &str, connected: bool) -> String {
    if connected || state == MISSING {
        state.into()
    } else {
        format!("{state} (last reported)")
    }
}

pub fn section(s: &Workspace) -> El {
    let Some((host, saved)) = s.hosts.get(s.active).zip(s.settings.hosts.get(s.active)) else {
        return page([kit::section(
            "",
            [
                footnote("No active host. Add a host to see its reported server details.")
                    .key("server-empty"),
            ],
        )]);
    };
    let model = &host.model;
    let info = &model.server_info;
    let current = kit::section(
        "Active host",
        [card([
            fact("Configured host", saved.label.clone(), None),
            fact("Connection", super::about::state(model), None),
            address_row(
                "server-copy-endpoint",
                "Configured address",
                address(&saved.endpoint),
                "Address unavailable",
            ),
        ])],
    );
    let build = info.build.reported();
    let process = info.process.reported();
    let b_missing = absent(&info.build, model.connected);
    let p_missing = absent(&info.process, model.connected);
    let build_text = |value: Option<&str>| value.unwrap_or(b_missing).to_owned();
    let api = if !model.connected {
        "Not connected".into()
    } else if model.protocol.api_revision == 0 {
        MISSING.into()
    } else {
        model.protocol.api_revision.to_string()
    };
    let build = kit::section(
        "Server build",
        [card([
            fact(
                "Server version",
                build_text(build.and_then(|b| b.rex_version.as_deref())),
                None,
            ),
            fact("Negotiated API revision", api, None),
            fact(
                "Supported API revisions",
                info.api_revisions
                    .map_or_else(|| MISSING.into(), |(min, max)| format!("{min}–{max}")),
                None,
            ),
            fact(
                "Platform",
                build_text(build.and_then(|b| b.platform.as_deref())),
                None,
            ),
            fact(
                "Commit",
                build_text(build.and_then(|b| b.commit.as_deref())),
                None,
            ),
            fact(
                "Working tree",
                build
                    .and_then(|b| b.dirty)
                    .map_or(b_missing, |dirty| if dirty { "Modified" } else { "Clean" }),
                None,
            ),
            fact(
                "Built at",
                build_text(build.and_then(|b| b.built_at.as_deref())),
                None,
            ),
        ])],
    );
    let process_rows = kit::section(
        "Server process",
        [card([
            fact(
                "Process ID",
                process
                    .and_then(|p| p.pid)
                    .map_or_else(|| p_missing.into(), |pid| pid.to_string()),
                None,
            ),
            fact(
                "Started at",
                process
                    .and_then(|p| p.started_at.as_deref())
                    .unwrap_or(p_missing),
                None,
            ),
            fact(
                "Session count",
                process
                    .and_then(|p| p.session_count)
                    .map_or_else(|| p_missing.into(), |n| n.to_string()),
                None,
            ),
        ])],
    );
    let listeners = process.map(|p| &p.listeners);
    let mut listener_rows = vec![fact(
        "Listeners",
        listeners
            .filter(|l| !l.is_empty())
            .map_or_else(|| p_missing.into(), |l| format!("{} reported", l.len())),
        None,
    )];
    for (index, listener) in listeners.into_iter().flatten().enumerate() {
        listener_rows.push(fact(
            &format!("Listener {}", index + 1),
            address(&listener.endpoint).unwrap_or_else(|| "Endpoint format not shown".into()),
            None,
        ));
    }
    let listeners = kit::section(
        "Server listeners",
        [
            card(listener_rows),
            footnote(
                "Last reported endpoints, not a reachability check. Bound addresses and admission details are not shown.",
            ),
        ],
    );
    let tailscale = process.and_then(|p| p.tailscale.as_ref());
    let state = tailscale.map_or(p_missing, |t| match t.state.as_str() {
        "disabled" => "Disabled",
        "starting" => "Starting",
        "needs_login" => "Needs sign-in",
        "needs_machine_approval" => "Needs machine approval",
        "running" => "Running",
        _ => "Unknown (starting)",
    });
    let https = tailscale.and_then(|t| t.https.as_ref());
    let https_state = https.map_or(p_missing, |h| {
        if h.state == "available" {
            "Available"
        } else {
            "Pending"
        }
    });
    let remote = tailscale_address(model);
    let tailscale = kit::section(
        "Server built-in Tailscale",
        [
            card([
                fact(
                    "Built-in Tailscale",
                    last_state(state, model.connected),
                    None,
                ),
                fact(
                    "Tailscale HTTPS",
                    last_state(https_state, model.connected),
                    None,
                ),
                address_row(
                    "server-copy-tailnet",
                    "Tailscale address",
                    remote.clone(),
                    p_missing,
                ),
                pair_row(model.connected && remote.is_some(), remote.is_some()),
            ]),
            footnote(
                "Last reported by this Rex server, not this client's local Tailscale. Missing data does not mean disabled.",
            ),
        ],
    );
    page([
        current,
        footnote(if model.connected {
            "Last reported server facts, not live monitoring. Counts and states may have changed. URLs omit credentials, queries and fragments; Unix paths and timestamps are as reported."
        } else {
            "Disconnected: last reported server facts only. They do not mean the server is running now. URLs omit credentials, queries and fragments; Unix paths and timestamps are as reported."
        }).key("server-freshness"),
        controls(s),
        build,
        process_rows,
        listeners,
        tailscale,
    ])
}
