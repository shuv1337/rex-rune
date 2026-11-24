//! Read-only synchronization of Rex's shared host list into the workspace and the
//! Settings → Remote Hosts **Shared host list** card.
use super::{
    settings_kit::{
        ChipTone, card, footnote, row_frame, section as kit_section, setting, state_chip,
    },
    theme::*,
    workspace::{HostId, Workspace},
};
use rex_rune::{
    connection::{HostCommand, hosts::SharedHostState},
    hosts::{self, SharedHost, SharedHostDiagnostic, UnconnectableHost},
    settings::Host,
};
use rune::{El, Tx, col, text};
use rune_kit::Switch;
use std::collections::{HashMap, HashSet};

pub const MANAGED_REASON: &str = "Managed by rex hosts (read-only)";

#[derive(Default)]
pub struct SharedHostsState {
    /// Override for which host endpoint acts as the shared-host source (used by tests);
    /// `None` uses `rex_rune::transport::local_endpoint()`.
    pub source_override: Option<String>,
    /// Last `HostModel::shared_revision` reconciled per host id.
    pub last_reconciled: HashMap<HostId, u64>,
    /// Last `server_info.process.host_id` seen when reconciling per host id.
    pub last_host_id: HashMap<HostId, Option<String>>,
    /// Unconnectable/skipped shared records from the latest reconcile pass (shown in Settings only).
    pub unconnectable: Vec<UnconnectableHost>,
    /// Diagnostic/conflict sets already announced as a notice (deduped like `keymap_notices`).
    pub diagnostic_notices: HashSet<String>,
}

impl Workspace {
    /// The endpoint whose Rex server provides the shared host list.
    pub fn shared_source_endpoint(&self) -> String {
        self.shared_hosts
            .source_override
            .clone()
            .unwrap_or_else(rex_rune::transport::local_endpoint)
    }

    /// Whether saved host `index` is this machine's shared-host source.
    /// A linked row (`shared.is_some()`) can never act as the shared-host source.
    pub fn is_shared_source(&self, index: usize) -> bool {
        self.settings.hosts.get(index).is_some_and(|h| {
            h.shared.is_none() && hosts::same_endpoint(&h.endpoint, &self.shared_source_endpoint())
        })
    }

    /// Saved host index of this machine's shared-host source, if present.
    pub fn shared_source_index(&self) -> Option<usize> {
        let source = self.shared_source_endpoint();
        self.settings
            .hosts
            .iter()
            .position(|h| h.shared.is_none() && hosts::same_endpoint(&h.endpoint, &source))
    }

    /// Whether saved host `index` is actively managed by a live shared-host source.
    /// Orphaned links whose source row is absent or whose sync is off are not locked read-only.
    pub fn is_shared_managed(&self, index: usize) -> bool {
        if !self.settings.sync_shared_hosts {
            return false;
        }
        let Some(link) = self
            .settings
            .hosts
            .get(index)
            .and_then(|h| h.shared.as_ref())
        else {
            return false;
        };
        let Some(source_idx) = self.shared_source_index() else {
            return false;
        };
        hosts::same_endpoint(&link.from, &self.settings.hosts[source_idx].endpoint)
    }

    /// Clear any `shared` link whose `from` endpoint does not match an active local source row
    /// (e.g. after removing or editing the source host, or changing `XDG_RUNTIME_DIR`).
    /// Returns whether any host row was unlinked.
    pub fn unlink_orphaned_shared_hosts(&mut self) -> bool {
        let active_source = self
            .settings
            .sync_shared_hosts
            .then(|| self.shared_source_index())
            .flatten()
            .map(|i| self.settings.hosts[i].endpoint.clone());
        let mut changed = false;
        for host in &mut self.settings.hosts {
            let orphaned = host.shared.as_ref().is_some_and(|link| {
                active_source
                    .as_deref()
                    .is_none_or(|src| !hosts::same_endpoint(&link.from, src))
            });
            if orphaned {
                host.shared = None;
                changed = true;
            }
        }
        if active_source.is_none() {
            self.shared_hosts.unconnectable.clear();
        }
        changed
    }

    /// Cached [`SharedHost`] record for linked saved host `index`, if currently loaded.
    pub fn shared_host_record(&self, index: usize) -> Option<&SharedHost> {
        let link = self.settings.hosts.get(index)?.shared.as_ref()?;
        let source_idx =
            self.settings.hosts.iter().position(|h| {
                h.shared.is_none() && hosts::same_endpoint(&h.endpoint, &link.from)
            })?;
        let list = self.hosts.get(source_idx)?.model.shared.list()?;
        list.hosts.iter().find(|r| r.id == link.id)
    }

    /// Toggle `sync_shared_hosts`. Turning it off unlinks all shared links locally with zero
    /// server commands; turning it back on reconciles from the cached list immediately (or
    /// requests/retries a read if not currently `Loaded`/`Unsupported`).
    pub fn set_sync_shared_hosts(&mut self, enabled: bool, tx: &mut Tx) {
        if self.settings.sync_shared_hosts == enabled {
            return;
        }
        self.settings.sync_shared_hosts = enabled;
        if !enabled {
            for host in &mut self.settings.hosts {
                host.shared = None;
            }
            self.shared_hosts.unconnectable.clear();
            self.shared_hosts.last_reconciled.clear();
            self.shared_hosts.last_host_id.clear();
            self.save();
            return;
        }
        self.save();
        if let Some(source_idx) = self.shared_source_index() {
            let id = self.hosts[source_idx].id;
            self.shared_hosts.last_reconciled.remove(&id);
            self.shared_hosts.last_host_id.remove(&id);
            match &self.hosts[source_idx].model.shared {
                SharedHostState::Loaded(_) => self.sync_shared_hosts(source_idx, tx),
                SharedHostState::Unsupported => {}
                SharedHostState::Unloaded
                | SharedHostState::Failed(_)
                | SharedHostState::Conflict(_) => {
                    self.send(source_idx, HostCommand::WatchSharedHosts);
                }
            }
        }
    }

    /// Reconcile the local shared-host source's latest `hosts.list` into `self.settings.hosts`.
    pub(super) fn sync_shared_hosts(&mut self, index: usize, tx: &mut Tx) {
        if !self.settings.sync_shared_hosts || !self.is_shared_source(index) {
            return;
        }
        let id = self.hosts[index].id;
        let revision = self.hosts[index].model.shared_revision;
        let local_host_id = self.hosts[index]
            .model
            .server_info
            .process
            .reported()
            .and_then(|p| p.host_id.clone());
        if self.shared_hosts.last_reconciled.get(&id) == Some(&revision)
            && self.shared_hosts.last_host_id.get(&id) == Some(&local_host_id)
        {
            return;
        }
        self.shared_hosts.last_reconciled.insert(id, revision);
        self.shared_hosts
            .last_host_id
            .insert(id, local_host_id.clone());

        let state = self.hosts[index].model.shared.clone();
        match state {
            SharedHostState::Unloaded | SharedHostState::Unsupported => {
                self.shared_hosts.unconnectable.clear();
            }
            SharedHostState::Conflict(message) => {
                // Preserve last-known host rows and skip reconcile completely.
                let key = format!("conflict:{message}");
                if self.shared_hosts.diagnostic_notices.insert(key) {
                    self.notify(format!(
                        "Shared Rex host list could not be read: {message}. See Settings → Remote Hosts."
                    ));
                }
            }
            SharedHostState::Failed(message) => {
                // Preserve last-known host rows; Settings card reflects the read error.
                let key = format!("failed:{message}");
                if self.shared_hosts.diagnostic_notices.insert(key) {
                    self.notify(format!(
                        "Shared Rex host list read failed: {message} See Settings → Remote Hosts."
                    ));
                }
            }
            SharedHostState::Loaded(list) => {
                self.announce_shared_host_diagnostics(&list.diagnostics);
                let source = self.settings.hosts[index].endpoint.clone();
                let plan = hosts::reconcile(
                    &self.settings.hosts,
                    &source,
                    &list,
                    local_host_id.as_deref(),
                );
                self.shared_hosts.unconnectable = plan.unconnectable;
                let mut dirty = false;

                for upd in plan.updates {
                    if upd.endpoint_changed {
                        self.stop_install_for_host(upd.index);
                        let host = &mut self.settings.hosts[upd.index];
                        host.label = upd.label;
                        host.remote_control = false;
                        let old = std::mem::replace(&mut host.endpoint, upd.endpoint);
                        self.settings.sessions.remove(&old);
                        self.settings.recent_paths.remove(&old);
                        let state = self.spawn_host(upd.index, false);
                        let old_state = std::mem::replace(&mut self.hosts[upd.index], state);
                        self.forget_host(old_state);
                        if self.active == upd.index {
                            self.drag = None;
                            self.layer_drag = None;
                        }
                        self.update_visibility();
                    } else {
                        self.settings.hosts[upd.index].label = upd.label;
                    }
                    dirty = true;
                }

                for link in plan.links {
                    let host = &mut self.settings.hosts[link.index];
                    host.label = link.label;
                    host.shared = Some(link.shared);
                    dirty = true;
                }

                for unlink in plan.unlinks {
                    self.settings.hosts[unlink.index].shared = None;
                    self.inform(format!(
                        "{} was removed from the shared Rex host list and kept as a local host.",
                        unlink.label
                    ));
                    dirty = true;
                }

                for add in plan.additions {
                    self.settings.hosts.push(Host {
                        label: add.label,
                        endpoint: add.endpoint,
                        remote_control: false,
                        collapsed: false,
                        shell: None,
                        shared: Some(add.shared),
                    });
                    let new_index = self.settings.hosts.len() - 1;
                    self.connect_host(new_index, false);
                    dirty = true;
                }

                if dirty {
                    self.save();
                    self.update_title(tx);
                }
            }
        }
    }

    fn announce_shared_host_diagnostics(&mut self, diagnostics: &[SharedHostDiagnostic]) {
        if diagnostics.is_empty() {
            return;
        }
        let mut entries: Vec<String> = diagnostics
            .iter()
            .map(|d| {
                format!(
                    "{}|{}|{}",
                    d.severity,
                    d.source.as_deref().unwrap_or(""),
                    d.message
                )
            })
            .collect();
        entries.sort();
        entries.dedup();
        let count = diagnostics.len();
        let noun = diagnostic_noun(diagnostics);
        if self
            .shared_hosts
            .diagnostic_notices
            .insert(entries.join("\n"))
        {
            self.notify(format!(
                "{count} shared host {noun} from Rex; see Settings → Remote Hosts."
            ));
        }
    }
}

fn diagnostic_noun(diagnostics: &[SharedHostDiagnostic]) -> &'static str {
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity.eq_ignore_ascii_case("error"))
        .count();
    let single = diagnostics.len() == 1;
    if errors == diagnostics.len() {
        if single { "error" } else { "errors" }
    } else if errors > 0 {
        if single { "problem" } else { "problems" }
    } else if single {
        "warning"
    } else {
        "warnings"
    }
}

fn severity_label(severity: &str) -> (&'static str, u32) {
    if severity.eq_ignore_ascii_case("error") {
        ("Error", danger())
    } else {
        ("Warning", warning())
    }
}

fn status_summary(s: &Workspace) -> (String, String, ChipTone, String) {
    if !s.settings.sync_shared_hosts {
        return (
            "Shared host sync is turned off. Existing hosts stay local.".into(),
            "Off".into(),
            ChipTone::Off,
            String::new(),
        );
    }
    let Some(source_idx) = s.shared_source_index() else {
        return (
            "Add this machine's local Rex server to sync its shared host list.".into(),
            "No local host".into(),
            ChipTone::Off,
            String::new(),
        );
    };
    let source = &s.hosts[source_idx];
    let source_ep = &s.settings.hosts[source_idx].endpoint;
    match &source.model.shared {
        SharedHostState::Unloaded => {
            if source.model.connected {
                (
                    "Reading hosts from this machine's Rex server…".into(),
                    "Loading…".into(),
                    ChipTone::Off,
                    String::new(),
                )
            } else {
                (
                    "Waiting for this machine's Rex server to connect.".into(),
                    "Offline".into(),
                    ChipTone::Off,
                    String::new(),
                )
            }
        }
        SharedHostState::Unsupported => (
            "This Rex server does not expose a shared host list.".into(),
            "Unavailable".into(),
            ChipTone::Off,
            String::new(),
        ),
        SharedHostState::Conflict(message) => (
            format!("Could not read shared hosts: {message}"),
            "Conflict".into(),
            ChipTone::Attention,
            message.clone(),
        ),
        SharedHostState::Failed(message) => (
            format!("Could not read shared hosts: {message}"),
            "Error".into(),
            ChipTone::Attention,
            message.clone(),
        ),
        SharedHostState::Loaded(list) => {
            let synced = s
                .settings
                .hosts
                .iter()
                .filter(|h| {
                    h.shared
                        .as_ref()
                        .is_some_and(|link| hosts::same_endpoint(&link.from, source_ep))
                })
                .count();
            let unusable = s.shared_hosts.unconnectable.len();
            let diags = list.diagnostics.len();
            let diag_noun = diagnostic_noun(&list.diagnostics);
            let mut parts = Vec::new();
            if synced == 0 && unusable == 0 {
                parts.push("No remote hosts in this server's shared list.".to_owned());
            } else {
                parts.push(format!(
                    "{synced} shared {} synced from this machine's Rex server",
                    if synced == 1 { "host" } else { "hosts" }
                ));
                if unusable > 0 {
                    parts.push(format!("{unusable} unconnectable"));
                }
                if diags > 0 {
                    parts.push(format!("{diags} {diag_noun}"));
                }
            }
            let detail = if synced == 0 && unusable == 0 && diags == 0 {
                parts[0].clone()
            } else if synced == 0 && unusable == 0 {
                format!("No remote hosts in this server's shared list ({diags} {diag_noun}).")
            } else {
                format!("{}.", parts.join(" · "))
            };
            let chip_label = format!("{synced} synced");
            let (tone, why) = if diags > 0 || unusable > 0 {
                (ChipTone::Attention, detail.clone())
            } else {
                (ChipTone::Good, String::new())
            };
            (detail, chip_label, tone, why)
        }
    }
}

/// Settings → Remote Hosts → Shared host list section.
pub fn section(s: &Workspace) -> El {
    let (detail, chip_label, chip_tone, chip_why) = status_summary(s);
    let mut rows = vec![
        setting(
            "Use shared Rex host list",
            "Sync hosts from this machine's Rex server read-only (rex hosts and init.lua).",
            Switch::new(s.settings.sync_shared_hosts)
                .label("Use shared Rex host list")
                .hide_label()
                .on_toggle_tx(|s: &mut Workspace, tx: &mut Tx| {
                    s.set_sync_shared_hosts(!s.settings.sync_shared_hosts, tx)
                })
                .build(kit())
                .key("sync-shared-hosts"),
        ),
        setting(
            "Sync status",
            &detail,
            state_chip("shared-hosts-status", &chip_label, chip_tone, &chip_why),
        ),
    ];

    if s.settings.sync_shared_hosts
        && let Some(source_idx) = s.shared_source_index()
        && let Some(list) = s.hosts[source_idx].model.shared.list()
    {
        for (idx, diag) in list.diagnostics.iter().enumerate() {
            let (sev, color) = severity_label(&diag.severity);
            let title = match &diag.source {
                Some(src) => format!("{sev} · {src}"),
                None => sev.to_owned(),
            };
            rows.push(
                row_frame(true)
                    .key(format!("shared-host-diag:{idx}"))
                    .child(
                        col()
                            .grow(1.)
                            .min_w(0.)
                            .gap(2.)
                            .child(text(title).font(medium(12.)).color(rgb(color)).truncate())
                            .child(
                                text(diag.message.clone())
                                    .font(ui(12.))
                                    .color(rgb(muted()))
                                    .wrap(),
                            ),
                    ),
            );
        }
        for (idx, entry) in s.shared_hosts.unconnectable.iter().enumerate() {
            let mut detail = format!("Skipped: {}", entry.reason);
            if let Some(src) = &entry.source {
                detail.push_str(&format!(" · Declared in {src}"));
            }
            rows.push(
                row_frame(true)
                    .key(format!("shared-host-unconnectable:{idx}"))
                    .child(
                        col()
                            .grow(1.)
                            .min_w(0.)
                            .gap(2.)
                            .child(
                                text(entry.label.clone())
                                    .font(medium(13.))
                                    .color(rgb(ink()))
                                    .truncate(),
                            )
                            .child(text(detail).font(ui(11.5)).color(rgb(muted())).wrap()),
                    )
                    .child(state_chip(
                        &format!("shared-host-unconnectable-chip:{idx}"),
                        "Unconnectable",
                        ChipTone::Attention,
                        &entry.reason,
                    )),
            );
        }
    }

    kit_section(
        "Shared host list",
        [
            card(rows),
            footnote(
                "Hosts from rex hosts and init.lua sync read-only in this release. Local preferences such as default shell and remote control stay on this client.",
            ),
        ],
    )
}
