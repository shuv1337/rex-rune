//! Synchronization of Rex's shared host list (`hosts.list`, `hosts_changed`, `hosts.put`,
//! `hosts.remove`) into the workspace and the Settings → Remote Hosts **Shared host list** card.
use super::{
    panel::PanelKind,
    settings_kit::{
        ChipTone, card, footnote, row_frame, section as kit_section, setting, state_chip,
    },
    theme::*,
    workspace::{HostId, Workspace},
};
use rex_rune::{
    connection::{
        HostCommand,
        hosts::{SharedHostState, WriteOutcome, WriteResult},
    },
    hosts::{self, SharedHost, SharedHostDiagnostic, UnconnectableHost},
    settings::{Host, SharedLink},
};
use rune::{El, Tx, col, text};
use rune_kit::Switch;
use std::collections::{HashMap, HashSet};

/// One in-flight shared-host write sent to the local Rex server (`hosts.put` / `hosts.remove`).
/// Local `settings.hosts` is never mutated ahead of the server's confirmation.
#[derive(Clone, Debug)]
pub enum PendingSharedWrite {
    Add {
        label: String,
        endpoint: String,
        source_endpoint: String,
    },
    Edit {
        shared_id: String,
        source_endpoint: String,
        shell: Option<String>,
        apply_shell: bool,
        shell_changed: bool,
        previous_endpoint: String,
        next_endpoint: String,
        ssh_review_endpoint: Option<String>,
    },
    Remove {
        shared_id: String,
        source_endpoint: String,
        label: String,
        /// Set when an earlier `hosts.list` refresh already observed the host absent and removed
        /// the local row so the subsequent `WriteOutcome::Removed` does not duplicate notices or
        /// leave a stale `self_removed` entry.
        applied: bool,
    },
}

impl PendingSharedWrite {
    fn targets_shared_id(&self, id: &str) -> bool {
        match self {
            Self::Add { .. } => false,
            Self::Edit { shared_id, .. } | Self::Remove { shared_id, .. } => shared_id == id,
        }
    }
}

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
    /// Monotonic client-side request id for `PutSharedHost` / `RemoveSharedHost`.
    pub next_write_id: u64,
    /// In-flight shared-host writes awaiting their `WriteResult`, keyed by request id.
    pub pending_writes: HashMap<u64, PendingSharedWrite>,
    /// Shared host IDs whose removal this client initiated and the server confirmed, so a
    /// subsequent `hosts_changed` / `hosts.list` never fires Rule 5's unlink-and-keep notice.
    pub self_removed: HashSet<String>,
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
            self.shared_hosts.pending_writes.clear();
            self.shared_hosts.self_removed.clear();
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

    /// Whether the local shared-host source is enabled, connected, and `Loaded`, so new hosts
    /// can be saved to Rex's shared host list (`hosts.put`).
    pub fn can_save_shared_host(&self) -> bool {
        if !self.settings.sync_shared_hosts {
            return false;
        }
        let Some(source_idx) = self.shared_source_index() else {
            return false;
        };
        let Some(source) = self.hosts.get(source_idx) else {
            return false;
        };
        source.model.connected && matches!(source.model.shared, SharedHostState::Loaded(_))
    }

    /// Whether a shared-host write for `shared_id` is currently in flight.
    pub fn has_pending_shared_write_for(&self, shared_id: &str) -> bool {
        self.shared_hosts
            .pending_writes
            .values()
            .any(|p| p.targets_shared_id(shared_id))
    }

    /// Whether an in-flight shared Add or Edit write already targets `endpoint` canonically
    /// (excluding writes for `excluding_shared_id`, e.g. when editing that same host).
    pub fn has_pending_shared_write_endpoint(
        &self,
        endpoint: &str,
        excluding_shared_id: Option<&str>,
    ) -> bool {
        self.shared_hosts.pending_writes.values().any(|p| match p {
            PendingSharedWrite::Add { endpoint: ep, .. } => hosts::same_endpoint(ep, endpoint),
            PendingSharedWrite::Edit {
                shared_id,
                next_endpoint,
                ..
            } => {
                excluding_shared_id != Some(shared_id.as_str())
                    && hosts::same_endpoint(next_endpoint, endpoint)
            }
            PendingSharedWrite::Remove { .. } => false,
        })
    }

    /// Why saved host `index` cannot currently be edited (label/endpoint) or removed via the
    /// shared Rex host list. Returns `None` for local hosts and for stored (`source.is_none()`)
    /// shared hosts whose local Rex server is connected and `Loaded`.
    pub fn shared_write_blocked(&self, index: usize) -> Option<String> {
        if !self.is_shared_managed(index) {
            return None;
        }
        let link = self.settings.hosts.get(index)?.shared.as_ref()?;
        let source_idx = self.shared_source_index()?;
        let source = self.hosts.get(source_idx)?;
        if !source.model.connected {
            return Some("Local Rex server is offline (read-only)".into());
        }
        match &source.model.shared {
            SharedHostState::Unloaded => Some("Waiting for shared Rex host list…".into()),
            SharedHostState::Unsupported => {
                Some("This Rex server does not support shared host writes".into())
            }
            SharedHostState::Conflict(_) => {
                Some("Shared host list has a conflict (read-only)".into())
            }
            SharedHostState::Failed(_) => {
                Some("Shared host list failed to load (read-only)".into())
            }
            SharedHostState::Loaded(list) => {
                let Some(record) = list.hosts.iter().find(|r| r.id == link.id) else {
                    return Some("Host is not in the loaded shared host list".into());
                };
                if let Some(src) = &record.source {
                    return Some(format!("Declared in {src} (read-only)"));
                }
                None
            }
        }
    }

    fn alloc_shared_write_id(&mut self) -> u64 {
        self.shared_hosts.next_write_id += 1;
        self.shared_hosts.next_write_id
    }

    /// Send an epoch-scoped `hosts.put` to add a new stored shared host. Does not mutate
    /// `settings.hosts` until the server confirms.
    pub fn start_shared_add(&mut self, label: String, endpoint: String) -> Result<(), String> {
        if !self.can_save_shared_host() {
            return Err(
                "Local Rex server is not ready to save shared hosts. Turn off \"Save to shared Rex host list\" to add locally, or reconnect first."
                    .into(),
            );
        }
        let source_idx = self
            .shared_source_index()
            .ok_or_else(|| "No local Rex server configured for shared hosts.".to_owned())?;
        let source_endpoint = self.settings.hosts[source_idx].endpoint.clone();
        let epoch = self.hosts[source_idx].model.connection_epoch;
        let client_label = self.settings.client_label();
        let payload = hosts::build_put_create(&label, &endpoint, Some(&client_label));
        let id = self.alloc_shared_write_id();
        self.shared_hosts.pending_writes.insert(
            id,
            PendingSharedWrite::Add {
                label,
                endpoint,
                source_endpoint,
            },
        );
        self.send(
            source_idx,
            HostCommand::PutSharedHost {
                id,
                epoch,
                host: payload,
            },
        );
        Ok(())
    }

    /// Send an epoch-scoped `hosts.put` to update an existing stored shared host, preserving all
    /// unknown JSON fields and unrelated endpoints from its cached `SharedHost::raw`. Does not
    /// mutate `settings.hosts` until the server confirms.
    pub fn start_shared_edit(
        &mut self,
        index: usize,
        label: String,
        endpoint: String,
        shell: Option<String>,
        apply_shell: bool,
        ssh_review_endpoint: Option<String>,
    ) -> Result<(), String> {
        if let Some(reason) = self.shared_write_blocked(index) {
            return Err(reason);
        }
        let link = self
            .settings
            .hosts
            .get(index)
            .and_then(|h| h.shared.clone())
            .ok_or_else(|| "Host is no longer linked to the shared host list.".to_owned())?;
        if self.has_pending_shared_write_for(&link.id) {
            return Err("A shared host change for this host is already in progress.".into());
        }
        let record = self
            .shared_host_record(index)
            .cloned()
            .ok_or_else(|| "Host is not in the loaded shared host list.".to_owned())?;
        if let Some(src) = &record.source {
            return Err(format!("Declared in {src} (read-only)"));
        }
        let source_idx = self
            .shared_source_index()
            .ok_or_else(|| "No local Rex server configured for shared hosts.".to_owned())?;
        let source_endpoint = self.settings.hosts[source_idx].endpoint.clone();
        let epoch = self.hosts[source_idx].model.connection_epoch;
        let previous_endpoint = self.settings.hosts[index].endpoint.clone();
        let shell_changed = apply_shell && self.settings.hosts[index].shell != shell;
        let payload = hosts::build_put_update(&record, &label, &previous_endpoint, &endpoint);
        let id = self.alloc_shared_write_id();
        self.shared_hosts.pending_writes.insert(
            id,
            PendingSharedWrite::Edit {
                shared_id: link.id,
                source_endpoint,
                shell,
                apply_shell,
                shell_changed,
                previous_endpoint,
                next_endpoint: endpoint,
                ssh_review_endpoint,
            },
        );
        self.send(
            source_idx,
            HostCommand::PutSharedHost {
                id,
                epoch,
                host: payload,
            },
        );
        Ok(())
    }

    /// Send an epoch-scoped `hosts.remove` for an existing stored shared host. Does not delete
    /// the local row until the server confirms.
    pub fn start_shared_remove(&mut self, index: usize) -> Result<(), String> {
        if self.settings.hosts.len() <= 1 || index >= self.settings.hosts.len() {
            return Err("The workspace needs at least one host.".into());
        }
        if let Some(reason) = self.shared_write_blocked(index) {
            return Err(reason);
        }
        let link = self.settings.hosts[index]
            .shared
            .clone()
            .ok_or_else(|| "Host is no longer linked to the shared host list.".to_owned())?;
        if self.has_pending_shared_write_for(&link.id) {
            return Err("A shared host change for this host is already in progress.".into());
        }
        let record = self
            .shared_host_record(index)
            .ok_or_else(|| "Host is not in the loaded shared host list.".to_owned())?;
        if let Some(src) = &record.source {
            return Err(format!("Declared in {src} (read-only)"));
        }
        let source_idx = self
            .shared_source_index()
            .ok_or_else(|| "No local Rex server configured for shared hosts.".to_owned())?;
        let source_endpoint = self.settings.hosts[source_idx].endpoint.clone();
        let epoch = self.hosts[source_idx].model.connection_epoch;
        let label = self.settings.hosts[index].label.clone();
        let id = self.alloc_shared_write_id();
        self.shared_hosts.pending_writes.insert(
            id,
            PendingSharedWrite::Remove {
                shared_id: link.id.clone(),
                source_endpoint,
                label,
                applied: false,
            },
        );
        self.send(
            source_idx,
            HostCommand::RemoveSharedHost {
                id,
                epoch,
                host_id: link.id,
            },
        );
        Ok(())
    }

    /// Apply completed `hosts.put` / `hosts.remove` outcomes delivered by the source worker.
    /// Runs before `sync_shared_hosts` so confirmed edits/adds/removals settle cleanly ahead of
    /// the refreshed `hosts.list` reconciliation.
    pub(super) fn apply_shared_writes(&mut self, results: Vec<WriteResult>, tx: &mut Tx) {
        for result in results {
            let Some(pending) = self.shared_hosts.pending_writes.remove(&result.id) else {
                continue;
            };
            match (pending, result.outcome) {
                (
                    PendingSharedWrite::Add {
                        endpoint,
                        source_endpoint,
                        ..
                    },
                    WriteOutcome::Put { host: record, .. },
                ) => {
                    let chosen_endpoint = if record
                        .expanded_endpoints()
                        .iter()
                        .any(|ep| hosts::same_endpoint(ep, &endpoint))
                    {
                        endpoint.clone()
                    } else if let Some(valid) = record.first_valid_endpoint() {
                        valid
                    } else {
                        endpoint.clone()
                    };
                    let link = SharedLink {
                        id: record.id.clone(),
                        from: source_endpoint.clone(),
                    };
                    let added_label = record.label.clone();
                    if let Some(existing) = self.settings.hosts.iter().position(|h| {
                        h.shared.as_ref().is_some_and(|l| {
                            l.id == link.id && hosts::same_endpoint(&l.from, &source_endpoint)
                        })
                    }) {
                        self.settings.hosts[existing].label = record.label;
                        self.active = existing;
                        self.update_visibility();
                        self.save();
                        self.update_title(tx);
                    } else if let Some(existing) = self
                        .settings
                        .hosts
                        .iter()
                        .position(|h| hosts::same_endpoint(&h.endpoint, &chosen_endpoint))
                    {
                        // Match Rule 3: only link an unlinked non-source row in place. If another
                        // shared record already owns this canonical endpoint, leave it untouched
                        // and let reconcile surface the duplicate as Skipped.
                        if self.settings.hosts[existing].shared.is_none()
                            && !hosts::same_endpoint(
                                &self.settings.hosts[existing].endpoint,
                                &source_endpoint,
                            )
                        {
                            self.settings.hosts[existing].label = record.label;
                            self.settings.hosts[existing].shared = Some(link);
                            self.active = existing;
                            self.update_visibility();
                            self.save();
                            self.update_title(tx);
                        }
                    } else {
                        self.add_host(
                            Host {
                                label: record.label,
                                endpoint: chosen_endpoint,
                                remote_control: false,
                                prompts: true,
                                collapsed: false,
                                shell: None,
                                shared: Some(link),
                            },
                            tx,
                        );
                    }
                    self.inform(format!("Added {added_label} to the shared Rex host list."));
                }
                (PendingSharedWrite::Add { label, .. }, WriteOutcome::Failed { message, .. }) => {
                    self.notify(format!("Could not add shared host {label}: {message}"));
                }
                (
                    PendingSharedWrite::Edit {
                        shared_id,
                        source_endpoint,
                        shell,
                        apply_shell,
                        previous_endpoint,
                        next_endpoint,
                        ssh_review_endpoint,
                        ..
                    },
                    WriteOutcome::Put { host: record, .. },
                ) => {
                    let mut endpoint_applied = false;
                    if let Some(row_idx) = self.settings.hosts.iter().position(|h| {
                        h.shared.as_ref().is_some_and(|l| {
                            l.id == shared_id && hosts::same_endpoint(&l.from, &source_endpoint)
                        })
                    }) {
                        let target_endpoint =
                            if hosts::same_endpoint(&previous_endpoint, &next_endpoint) {
                                self.settings.hosts[row_idx].endpoint.clone()
                            } else if record
                                .expanded_endpoints()
                                .iter()
                                .any(|ep| hosts::same_endpoint(ep, &next_endpoint))
                            {
                                next_endpoint.clone()
                            } else {
                                record
                                    .first_valid_endpoint()
                                    .unwrap_or_else(|| next_endpoint.clone())
                            };
                        let endpoint_changed = !hosts::same_endpoint(
                            &self.settings.hosts[row_idx].endpoint,
                            &target_endpoint,
                        );
                        // Preserve one-row-per-canonical-endpoint: never mutate onto an endpoint
                        // already owned by another saved row or the source server; let reconcile
                        // surface the collision as Skipped instead.
                        let collides = endpoint_changed
                            && (hosts::same_endpoint(&target_endpoint, &source_endpoint)
                                || self.settings.hosts.iter().enumerate().any(|(i, h)| {
                                    i != row_idx
                                        && hosts::same_endpoint(&h.endpoint, &target_endpoint)
                                }));
                        if endpoint_changed && !collides {
                            self.stop_install_for_host(row_idx);
                            let host = &mut self.settings.hosts[row_idx];
                            host.label = record.label;
                            if apply_shell {
                                host.shell = shell;
                            }
                            host.remote_control = false;
                            let old = std::mem::replace(&mut host.endpoint, target_endpoint);
                            self.settings.sessions.remove(&old);
                            self.settings.recent_paths.remove(&old);
                            let state = self.spawn_host(row_idx, false);
                            let old_state = std::mem::replace(&mut self.hosts[row_idx], state);
                            self.forget_host(old_state);
                            if self.active == row_idx {
                                self.drag = None;
                                self.layer_drag = None;
                            }
                            self.update_visibility();
                            endpoint_applied = true;
                        } else {
                            self.settings.hosts[row_idx].label = record.label;
                            if apply_shell {
                                self.settings.hosts[row_idx].shell = shell;
                            }
                            endpoint_applied = !endpoint_changed;
                        }
                        self.save();
                        self.update_title(tx);
                    }
                    if let Some(ep) = ssh_review_endpoint {
                        if endpoint_applied {
                            self.ssh_setup.attachment.remove(&ep);
                            self.inform(
                                "Saved the reported socket endpoint to the shared Rex host list. Connecting through SSH without starting a server.",
                            );
                        } else {
                            self.notify(
                                "Updated the shared Rex host list, but kept this host's current endpoint because the new endpoint is already in the workspace.",
                            );
                        }
                    }
                }
                (
                    PendingSharedWrite::Edit { shell_changed, .. },
                    WriteOutcome::Failed { message, .. },
                ) => {
                    if shell_changed {
                        self.notify(format!(
                            "Could not update shared host (no changes, including default shell, were saved): {message}"
                        ));
                    } else {
                        self.notify(format!(
                            "Could not update shared host (no changes were saved): {message}"
                        ));
                    }
                }
                (
                    PendingSharedWrite::Remove {
                        shared_id,
                        source_endpoint,
                        label,
                        applied,
                    },
                    WriteOutcome::Removed { removed, .. },
                ) => {
                    if applied {
                        continue;
                    }
                    let still_in_loaded_list = self
                        .shared_source_index()
                        .and_then(|src| self.hosts.get(src))
                        .and_then(|h| h.model.shared.list())
                        .is_some_and(|list| list.hosts.iter().any(|r| r.id == shared_id));
                    if still_in_loaded_list {
                        self.shared_hosts.self_removed.insert(shared_id.clone());
                    }
                    let removed_row = self
                        .settings
                        .hosts
                        .iter()
                        .position(|h| {
                            h.shared.as_ref().is_some_and(|l| {
                                l.id == shared_id && hosts::same_endpoint(&l.from, &source_endpoint)
                            })
                        })
                        .inspect(|&row_idx| {
                            self.remove_confirmed_shared_host_row(row_idx, tx);
                        })
                        .is_some();
                    if removed {
                        self.inform(format!(
                            "Removed {label} from the shared Rex host list. Its sessions keep running there."
                        ));
                    } else if removed_row {
                        self.inform(format!(
                            "Removed {label} from this workspace; it was already absent from the shared Rex host list."
                        ));
                    }
                }
                (
                    PendingSharedWrite::Remove { label, applied, .. },
                    WriteOutcome::Failed { message, .. },
                ) => {
                    if !applied {
                        self.notify(format!("Could not remove shared host {label}: {message}"));
                    }
                }
                (_, WriteOutcome::Put { .. } | WriteOutcome::Removed { .. }) => {}
            }
        }
    }

    fn remove_confirmed_shared_host_row(&mut self, index: usize, tx: &mut Tx) {
        if self.settings.hosts.len() <= 1 || index >= self.hosts.len() {
            return;
        }
        self.stop_install_for_host(index);
        self.navigation.cancel();
        if self.ports_target().is_some() {
            self.close_panel(tx);
        }
        let state = self.hosts.remove(index);
        self.forget_host(state);
        let host = self.settings.hosts.remove(index);
        self.settings.sessions.remove(&host.endpoint);
        self.settings.recent_paths.remove(&host.endpoint);
        // Removing a host shifts every higher `host: usize` index; clear every non-Settings panel
        // (matching `remove_host`) so no dialog, menu, download, or form targets a shifted index.
        if self
            .panel
            .as_ref()
            .is_some_and(|p| !matches!(p.kind, PanelKind::Settings))
        {
            self.panel = None;
        }
        self.drag = None;
        self.layer_drag = None;
        if self.active == index {
            self.active = 0;
        } else if self.active > index {
            self.active -= 1;
        }
        self.unlink_orphaned_shared_hosts();
        self.update_visibility();
        self.save();
        self.update_title(tx);
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
            self.shared_hosts.pending_writes.clear();
            self.shared_hosts.self_removed.clear();
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

                let mut self_removed_indices = Vec::new();
                for unlink in plan.unlinks {
                    let pending_remove_label = self
                        .shared_hosts
                        .pending_writes
                        .values_mut()
                        .find_map(|pending| match pending {
                            PendingSharedWrite::Remove {
                                shared_id,
                                source_endpoint,
                                label,
                                applied,
                            } if !*applied
                                && *shared_id == unlink.id
                                && hosts::same_endpoint(source_endpoint, &source) =>
                            {
                                *applied = true;
                                Some(label.clone())
                            }
                            _ => None,
                        });
                    if self.shared_hosts.self_removed.remove(&unlink.id) {
                        self_removed_indices.push(unlink.index);
                    } else if let Some(label) = pending_remove_label {
                        self_removed_indices.push(unlink.index);
                        self.inform(format!(
                            "Removed {label} from the shared Rex host list. Its sessions keep running there."
                        ));
                    } else {
                        self.settings.hosts[unlink.index].shared = None;
                        self.inform(format!(
                            "{} was removed from the shared Rex host list and kept as a local host.",
                            unlink.label
                        ));
                        dirty = true;
                    }
                }
                // Prune any self_removed ids no longer present in the loaded list.
                self.shared_hosts
                    .self_removed
                    .retain(|id| list.hosts.iter().any(|h| h.id == *id));

                for add in plan.additions {
                    self.settings.hosts.push(Host {
                        label: add.label,
                        endpoint: add.endpoint,
                        remote_control: false,
                        prompts: true,
                        collapsed: false,
                        shell: None,
                        shared: Some(add.shared),
                    });
                    let new_index = self.settings.hosts.len() - 1;
                    self.connect_host(new_index, false);
                    dirty = true;
                }

                // Remove any self-initiated removals in descending index order so indices stay valid.
                self_removed_indices.sort_unstable();
                for row_idx in self_removed_indices.into_iter().rev() {
                    self.remove_confirmed_shared_host_row(row_idx, tx);
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
            let skipped_dup = s
                .shared_hosts
                .unconnectable
                .iter()
                .filter(|u| u.duplicate)
                .count();
            let unusable = s.shared_hosts.unconnectable.len() - skipped_dup;
            let diags = list.diagnostics.len();
            let diag_noun = diagnostic_noun(&list.diagnostics);
            let mut parts = Vec::new();
            if synced == 0 && unusable == 0 && skipped_dup == 0 {
                parts.push("No remote hosts in this server's shared list.".to_owned());
            } else {
                parts.push(format!(
                    "{synced} shared {} synced from this machine's Rex server",
                    if synced == 1 { "host" } else { "hosts" }
                ));
                if unusable > 0 {
                    parts.push(format!("{unusable} unconnectable"));
                }
                if skipped_dup > 0 {
                    parts.push(format!("{skipped_dup} skipped"));
                }
                if diags > 0 {
                    parts.push(format!("{diags} {diag_noun}"));
                }
            }
            let detail = if synced == 0 && unusable == 0 && skipped_dup == 0 && diags == 0 {
                parts[0].clone()
            } else if synced == 0 && unusable == 0 && skipped_dup == 0 {
                format!("No remote hosts in this server's shared list ({diags} {diag_noun}).")
            } else {
                format!("{}.", parts.join(" · "))
            };
            let chip_label = format!("{synced} synced");
            let (tone, why) = if diags > 0 || unusable > 0 || skipped_dup > 0 {
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
            "Sync hosts with this machine's Rex server (rex hosts and init.lua).",
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
            let mut detail = if entry.duplicate {
                format!("Skipped: {}", entry.reason)
            } else {
                format!("Unconnectable: {}", entry.reason)
            };
            if let Some(src) = &entry.source {
                detail.push_str(&format!(" · Declared in {src}"));
            }
            let chip_label = if entry.duplicate {
                "Skipped"
            } else {
                "Unconnectable"
            };
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
                        chip_label,
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
                "Stored rex hosts can be added, edited, and removed from Rex Rune while this server is connected. Hosts declared in init.lua are read-only. Local preferences such as default shell and remote control stay on this client.",
            ),
        ],
    )
}
