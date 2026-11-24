//! Headless UI and workspace reconciliation tests for #145 Slices 1 & 2 (shared Rex host list
//! read-only sync, diagnostics, deduplication, and epoch-scoped `hosts.put` / `hosts.remove`).
use super::*;
use crate::app::panel::SettingsSection;
use rex_rune::{
    connection::{
        HostModel,
        hosts::{SharedHostState, WriteKind, WriteOutcome, WriteResult},
        server_info::{Answer, Process},
    },
    hosts::{SharedHost, SharedHostDiagnostic, SharedHostList},
    settings::{MutedTab, SharedLink, recent_paths::RecentPath},
    ssh::setup,
};
use serde_json::json;

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..90 {
        rt.frame(1. / 60.);
    }
}

fn rect(rt: &Runtime<Workspace>, key: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing {key}"))
        .rect
}

fn host_record(
    id: &str,
    label: &str,
    endpoints: &[&str],
    host_id: Option<&str>,
    source: Option<&str>,
) -> SharedHost {
    let mut obj = json!({
        "id": id,
        "label": label,
        "endpoints": endpoints,
    });
    if let Some(hid) = host_id {
        obj["host_id"] = json!(hid);
    }
    if let Some(src) = source {
        obj["source"] = json!(src);
    }
    SharedHost::from_value(&obj).unwrap()
}

/// Replace host `index`'s connection with a dummy channel so tests can inspect every command sent
/// and inject `HostModel` updates deterministically without touching sockets.
fn stub_host(ws: &mut Workspace, index: usize) -> flume::Receiver<HostCommand> {
    let (tx, rx) = flume::unbounded();
    let (_wake_tx, wake_rx) = flume::bounded(1);
    let model = std::sync::Arc::new(std::sync::Mutex::new(HostModel {
        connected: true,
        status: "Connected".into(),
        ..HostModel::default()
    }));
    ws.hosts[index].connection = HostConnection {
        model,
        commands: tx,
        wake: wake_rx,
    };
    ws.hosts[index].model.connected = true;
    ws.hosts[index].model.status = "Connected".into();
    rx
}

fn push_shared_state(
    ws: &mut Workspace,
    source_idx: usize,
    state: SharedHostState,
    local_host_id: Option<&str>,
) {
    {
        let mut m = ws.hosts[source_idx].connection.model.lock().unwrap();
        m.connected = true;
        m.status = "Connected".into();
        m.shared = state;
        m.shared_revision += 1;
        if let Some(hid) = local_host_id {
            m.server_info.process = Answer::Reported(Process {
                host_id: Some(hid.into()),
                ..Process::default()
            });
        }
    }
    ws.sync(source_idx, &mut rune::Tx::default());
}

fn push_shared_write(ws: &mut Workspace, source_idx: usize, result: WriteResult) {
    {
        let mut m = ws.hosts[source_idx].connection.model.lock().unwrap();
        m.shared_writes.push(result);
    }
    ws.sync(source_idx, &mut rune::Tx::default());
}

#[test]
fn linking_and_renaming_preserve_all_local_preferences_and_suppress_self() {
    let mut ws = workspace_hosts(2);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let _rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);

    // Pre-existing local host with every local preference populated.
    ws.settings.hosts[1].label = "My Old Studio".into();
    ws.settings.hosts[1].endpoint = "https://studio.invalid:443/".into();
    ws.settings.hosts[1].shell = Some("/bin/zsh".into());
    ws.settings.hosts[1].collapsed = true;
    ws.settings.hosts[1].remote_control = true;
    let saved_ep = ws.settings.hosts[1].endpoint.clone();
    ws.settings
        .sessions
        .insert(saved_ep.clone(), "session:keep".into());
    ws.settings.recent_paths.insert(
        saved_ep.clone(),
        vec![RecentPath {
            path: "/srv/code".into(),
            directory: true,
        }],
    );
    ws.settings.muted_tabs.push(MutedTab {
        endpoint: saved_ep.clone(),
        session: "session:keep".into(),
        window: "win:1".into(),
    });

    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h1".into(),
            hosts: vec![
                // Self record matching local server's host_id must be suppressed.
                host_record(
                    "host:self",
                    "This Machine",
                    &["self.invalid"],
                    Some("local-uuid"),
                    None,
                ),
                // Matches pre-existing host[1] via canonical endpoint.
                host_record(
                    "host:studio",
                    "Studio",
                    &["studio.invalid"],
                    Some("remote-1"),
                    Some("init.lua:2"),
                ),
                // New shared host appended automatically.
                host_record("host:lab", "Lab", &["lab.invalid"], Some("remote-2"), None),
                // Unconnectable host stays out of settings.hosts (no broken worker).
                host_record(
                    "host:bad",
                    "Broken",
                    &["ftp://only-ftp"],
                    None,
                    Some("init.lua:9"),
                ),
            ],
            diagnostics: Vec::new(),
        }),
        Some("local-uuid"),
    );

    assert_eq!(
        ws.settings.hosts.len(),
        3,
        "local + linked studio + appended lab"
    );
    assert_eq!(ws.hosts.len(), 3);
    assert_eq!(ws.shared_hosts.unconnectable.len(), 1);
    assert_eq!(ws.shared_hosts.unconnectable[0].label, "Broken");

    // Pre-existing host linked without duplicating or losing any local preference.
    let studio = &ws.settings.hosts[1];
    assert_eq!(studio.label, "Studio");
    assert_eq!(studio.endpoint, saved_ep);
    assert_eq!(
        studio.shared,
        Some(SharedLink {
            id: "host:studio".into(),
            from: source_ep.clone(),
        })
    );
    assert_eq!(studio.shell.as_deref(), Some("/bin/zsh"));
    assert!(studio.collapsed);
    assert!(studio.remote_control);
    assert_eq!(
        ws.settings.sessions.get(&saved_ep).map(String::as_str),
        Some("session:keep")
    );
    assert_eq!(
        ws.settings.recent_paths.get(&saved_ep).map(Vec::len),
        Some(1)
    );
    assert_eq!(ws.settings.muted_tabs.len(), 1);

    // Appended host is linked and uses first valid expanded endpoint.
    let lab = &ws.settings.hosts[2];
    assert_eq!(lab.label, "Lab");
    assert_eq!(lab.endpoint, "https://lab.invalid");
    assert_eq!(
        lab.shared,
        Some(SharedLink {
            id: "host:lab".into(),
            from: source_ep.clone(),
        })
    );

    // Rename on server updates label while keeping all local preferences intact.
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h2".into(),
            hosts: vec![
                host_record(
                    "host:studio",
                    "Studio Primary",
                    &["studio.invalid"],
                    Some("remote-1"),
                    Some("init.lua:2"),
                ),
                host_record("host:lab", "Lab", &["lab.invalid"], Some("remote-2"), None),
            ],
            diagnostics: Vec::new(),
        }),
        Some("local-uuid"),
    );
    assert_eq!(ws.settings.hosts[1].label, "Studio Primary");
    assert_eq!(ws.settings.hosts[1].shell.as_deref(), Some("/bin/zsh"));
    assert!(ws.settings.hosts[1].collapsed);
    assert!(ws.settings.hosts[1].remote_control);
    assert_eq!(
        ws.settings.sessions.get(&saved_ep).map(String::as_str),
        Some("session:keep")
    );
}

#[test]
fn endpoint_change_resets_remote_control_and_drops_sessions_and_recent_paths() {
    let mut ws = workspace_hosts(2);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let _rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);

    let old_ep = "https://old-box.invalid".to_owned();
    ws.settings.hosts[1].label = "Box".into();
    ws.settings.hosts[1].endpoint = old_ep.clone();
    ws.settings.hosts[1].shell = Some("fish".into());
    ws.settings.hosts[1].collapsed = true;
    ws.settings.hosts[1].remote_control = true;
    ws.settings.hosts[1].shared = Some(SharedLink {
        id: "host:box".into(),
        from: source_ep,
    });
    ws.settings
        .sessions
        .insert(old_ep.clone(), "session:old".into());
    ws.settings.recent_paths.insert(
        old_ep.clone(),
        vec![RecentPath {
            path: "/old/path".into(),
            directory: true,
        }],
    );
    let old_id = ws.hosts[1].id;

    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h-moved".into(),
            hosts: vec![host_record(
                "host:box",
                "Box Moved",
                &["new-box.invalid"],
                None,
                None,
            )],
            diagnostics: Vec::new(),
        }),
        None,
    );

    let host = &ws.settings.hosts[1];
    assert_eq!(host.label, "Box Moved");
    assert_eq!(host.endpoint, "https://new-box.invalid");
    assert!(
        !host.remote_control,
        "remote_control must reset on endpoint change"
    );
    assert_eq!(host.shell.as_deref(), Some("fish"), "shell is preserved");
    assert!(host.collapsed, "collapsed is preserved");
    assert!(!ws.settings.sessions.contains_key(&old_ep));
    assert!(!ws.settings.recent_paths.contains_key(&old_ep));
    assert_ne!(
        ws.hosts[1].id, old_id,
        "host worker must reconnect under fresh id"
    );
}

#[test]
fn departed_shared_host_unlinks_and_keeps_local_with_notice_and_conflict_preserves_rows() {
    let mut ws = workspace_hosts(2);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let _rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);

    ws.settings.hosts[1].label = "Studio".into();
    ws.settings.hosts[1].endpoint = "https://studio.invalid".into();
    ws.settings.hosts[1].shared = Some(SharedLink {
        id: "host:studio".into(),
        from: source_ep.clone(),
    });

    // 1. Conflict must preserve last-known linked row and NOT unlink or delete it.
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Conflict("hosts/everywhere.json is not a JSON object".into()),
        None,
    );
    assert_eq!(ws.settings.hosts.len(), 2);
    assert!(ws.settings.hosts[1].shared.is_some());
    assert!(ws.notices.iter().any(|n| {
        n.text
            .contains("hosts/everywhere.json is not a JSON object")
    }));

    // 2. Successfully read list without host:studio unlinks and keeps it as a local host.
    ws.notices.clear();
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h-empty".into(),
            hosts: Vec::new(),
            diagnostics: Vec::new(),
        }),
        None,
    );
    assert_eq!(
        ws.settings.hosts.len(),
        2,
        "must never silently delete departed host"
    );
    assert_eq!(ws.settings.hosts[1].shared, None);
    assert_eq!(ws.settings.hosts[1].label, "Studio");
    assert!(
        ws.notices
            .iter()
            .any(|n| n.text.contains("Studio") && n.text.contains("kept as a local host"))
    );
}

#[test]
fn toggle_off_unlinks_locally_with_zero_server_commands_and_toggle_on_reconciles_from_cache() {
    let mut ws = workspace_hosts(2);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let rx0 = stub_host(&mut ws, 0);
    let rx1 = stub_host(&mut ws, 1);

    ws.settings.hosts[1].label = "Studio".into();
    ws.settings.hosts[1].endpoint = "https://studio.invalid".into();
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h1".into(),
            hosts: vec![host_record(
                "host:studio",
                "Studio",
                &["studio.invalid"],
                None,
                None,
            )],
            diagnostics: Vec::new(),
        }),
        None,
    );
    assert!(ws.settings.hosts[1].shared.is_some());
    rx0.drain();
    rx1.drain();

    // Turn off: unlinks locally, persists false, sends ZERO server commands!
    let mut tx = rune::Tx::default();
    ws.set_sync_shared_hosts(false, &mut tx);
    assert!(!ws.settings.sync_shared_hosts);
    assert_eq!(ws.settings.hosts[1].shared, None);
    assert!(
        rx0.is_empty(),
        "toggling off must send zero server commands"
    );
    assert!(
        rx1.is_empty(),
        "toggling off must send zero server commands"
    );
    assert!(!Settings::load(&ws.settings_path).unwrap().sync_shared_hosts);

    // Turn back on: reconciles immediately from cached Loaded list with zero extra commands!
    ws.set_sync_shared_hosts(true, &mut tx);
    assert!(ws.settings.sync_shared_hosts);
    assert_eq!(
        ws.settings.hosts[1].shared,
        Some(SharedLink {
            id: "host:studio".into(),
            from: source_ep,
        })
    );
    assert_eq!(
        ws.settings.hosts.len(),
        2,
        "must re-link without duplicating"
    );
    assert!(
        rx0.is_empty(),
        "reconciling from cached list sends zero commands"
    );

    // When cached state is Failed, toggling off->on sends WatchSharedHosts to retry!
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Failed("hosts.list request timed out.".into()),
        None,
    );
    rx0.drain();
    ws.set_sync_shared_hosts(false, &mut tx);
    assert!(rx0.is_empty());
    ws.set_sync_shared_hosts(true, &mut tx);
    let cmds: Vec<_> = rx0.try_iter().collect();
    assert!(
        cmds.iter()
            .any(|c| matches!(c, HostCommand::WatchSharedHosts)),
        "toggling back on after Failed must send WatchSharedHosts to retry"
    );
}

#[test]
fn declared_init_lua_host_remains_strictly_read_only_while_local_shell_editing_works() {
    let mut ws = workspace_hosts(2);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);

    ws.settings.hosts[1].label = "Studio".into();
    ws.settings.hosts[1].endpoint = "https://studio.invalid".into();
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h-decl".into(),
            hosts: vec![host_record(
                "host:studio",
                "Studio",
                &["studio.invalid"],
                None,
                Some("init.lua:2"),
            )],
            diagnostics: Vec::new(),
        }),
        None,
    );
    ws.active = 1;
    rx0.drain();

    let mut tx = rune::Tx::default();
    // Intent::EditHost and Intent::RemoveHost refuse cleanly without opening panels or sending writes.
    ws.perform(Intent::EditHost(1), &mut tx);
    assert!(ws.panel.is_none());
    ws.perform(Intent::RemoveHost(1), &mut tx);
    assert!(ws.panel.is_none());
    ws.remove_host(1, &mut tx);
    assert_eq!(
        ws.settings.hosts.len(),
        2,
        "declared host must not be removed"
    );
    assert!(rx0.is_empty());

    // Direct edit_host call changing label or endpoint is also refused (shell-only still works).
    ws.edit_host(
        1,
        "Renamed".into(),
        "https://other.invalid".into(),
        Some("fish".into()),
    );
    assert_eq!(ws.settings.hosts[1].label, "Studio");
    assert_eq!(ws.settings.hosts[1].endpoint, "https://studio.invalid");
    assert_eq!(ws.settings.hosts[1].shell, None);
    ws.edit_host(
        1,
        "Studio".into(),
        "https://studio.invalid".into(),
        Some("fish".into()),
    );
    assert_eq!(ws.settings.hosts[1].shell.as_deref(), Some("fish"));

    // Sidebar host menu disables Edit host… and Remove host… naming init.lua:2 while keeping Default shell… enabled.
    ws.host_menu(1, (100., 100.));
    let menu = ws.entries();
    let shell_row = menu.iter().find(|e| e.label == "Default shell…").unwrap();
    let edit_row = menu.iter().find(|e| e.label == "Edit host…").unwrap();
    let remove_row = menu.iter().find(|e| e.label == "Remove host…").unwrap();
    assert!(shell_row.enabled);
    assert!(!edit_row.enabled);
    assert!(
        edit_row
            .detail
            .contains("Declared in init.lua:2 (read-only)")
    );
    assert!(!remove_row.enabled);
    assert!(
        remove_row
            .detail
            .contains("Declared in init.lua:2 (read-only)")
    );

    // Commands also disables host.edit and host.remove for the declared active host.
    ws.picker(false);
    let cmds = ws.entries();
    let cmd_edit = cmds
        .iter()
        .find(|e| e.id.as_deref() == Some("host.edit"))
        .unwrap();
    let cmd_remove = cmds
        .iter()
        .find(|e| e.id.as_deref() == Some("host.remove"))
        .unwrap();
    let cmd_shell = cmds
        .iter()
        .find(|e| e.id.as_deref() == Some("host.shell"))
        .unwrap();
    assert!(!cmd_edit.enabled);
    assert!(
        cmd_edit
            .detail
            .contains("Declared in init.lua:2 (read-only)")
    );
    assert!(!cmd_remove.enabled);
    assert!(
        cmd_remove
            .detail
            .contains("Declared in init.lua:2 (read-only)")
    );
    assert!(cmd_shell.enabled);

    // Local shell editing still works end-to-end with zero server writes!
    ws.perform(Intent::HostShell(1), &mut tx);
    assert!(matches!(
        ws.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Form(Form::HostShell(1)))
    ));
    ws.panel.as_mut().unwrap().inputs[0].text = "/bin/zsh".into();
    ws.submit(&mut tx);
    assert_eq!(ws.settings.hosts[1].shell.as_deref(), Some("/bin/zsh"));
    assert!(rx0.is_empty());
}

#[test]
fn save_ssh_socket_and_stale_edit_form_cannot_mutate_declared_or_newly_linked_host() {
    let mut ws = workspace_hosts(2);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);

    // (a) save_ssh_socket on a declared init.lua SSH host must not overwrite endpoint or send PutSharedHost.
    let ssh_ep = "ssh://me@studio.invalid".to_owned();
    ws.settings.hosts[1].label = "Studio".into();
    ws.settings.hosts[1].endpoint = ssh_ep.clone();
    ws.settings.hosts[1].remote_control = true;
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h-decl-ssh".into(),
            hosts: vec![host_record(
                "host:studio",
                "Studio",
                &[&ssh_ep],
                None,
                Some("init.lua:2"),
            )],
            diagnostics: Vec::new(),
        }),
        None,
    );
    ws.hosts[1].model.connected = false;
    let id_before = ws.hosts[1].id;
    ws.ssh_setup.attachment.insert(
        ssh_ep.clone(),
        setup::Attachment::Review {
            reason: "custom socket".into(),
            endpoint: Some("ssh://me@studio.invalid/run/custom.sock".into()),
        },
    );
    rx0.drain();
    let mut tx = rune::Tx::default();
    ws.save_ssh_socket(&ssh_ep, &mut tx);
    assert_eq!(ws.settings.hosts[1].endpoint, ssh_ep);
    assert!(ws.settings.hosts[1].remote_control);
    assert_eq!(ws.hosts[1].id, id_before);
    assert!(rx0.is_empty(), "declared host must send zero PutSharedHost");
    assert!(
        ws.notices
            .iter()
            .any(|n| n.text.contains("Declared in init.lua:2 (read-only)")),
        "expected declared read-only notice, got {:?}",
        ws.notices.iter().map(|n| &n.text).collect::<Vec<_>>()
    );
    assert!(
        !ws.notices
            .iter()
            .any(|n| n.text.contains("Saved the reported socket endpoint"))
    );

    // (b) Form::EditHost opened BEFORE a refresh links the host, then submitted AFTER linking.
    ws.settings.hosts[1].shared = None;
    ws.settings
        .sessions
        .insert(ssh_ep.clone(), "session:keep".into());
    ws.perform(Intent::EditHost(1), &mut tx);
    assert!(matches!(
        ws.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Form(Form::EditHost(1)))
    ));
    // Refresh links host 1 while the edit form is open!
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h-link".into(),
            hosts: vec![host_record(
                "host:studio",
                "Studio From Rex",
                &[&ssh_ep],
                None,
                None,
            )],
            diagnostics: Vec::new(),
        }),
        None,
    );
    assert!(ws.is_shared_managed(1));
    // User edits name and endpoint in the stale open form and clicks Save.
    {
        let panel = ws.panel.as_mut().unwrap();
        panel.inputs[0].text = "Local Override".into();
        panel.inputs[1].text = "https://hijack.invalid".into();
    }
    rx0.drain();
    ws.submit(&mut tx);
    assert_eq!(ws.settings.hosts[1].label, "Studio From Rex");
    assert_eq!(ws.settings.hosts[1].endpoint, ssh_ep);
    assert!(ws.settings.hosts[1].remote_control);
    assert_eq!(
        ws.settings.sessions.get(&ssh_ep).map(String::as_str),
        Some("session:keep")
    );
    assert_eq!(ws.hosts[1].id, id_before);
    assert!(
        rx0.is_empty(),
        "stale local EditHost form must not send PutSharedHost after host became linked"
    );
    assert!(
        ws.panel.as_ref().and_then(|p| p.error.as_deref()).is_some(),
        "stale form must explain why it refused"
    );
}

#[test]
fn orphaned_shared_links_unlink_when_source_disappears_changes_or_on_startup() {
    // 1. Removing the source host unlinks hosts previously linked to it.
    let mut ws = workspace_hosts(2);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let _rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);
    ws.settings.hosts[1].label = "Studio".into();
    ws.settings.hosts[1].endpoint = "https://studio.invalid".into();
    ws.settings.hosts[1].shared = Some(SharedLink {
        id: "host:studio".into(),
        from: source_ep.clone(),
    });
    assert!(ws.is_shared_managed(1));

    let mut tx = rune::Tx::default();
    ws.remove_host(0, &mut tx);
    assert_eq!(ws.settings.hosts.len(), 1);
    assert_eq!(
        ws.settings.hosts[0].shared, None,
        "removing source host must unlink orphaned shared hosts"
    );
    assert!(!ws.is_shared_managed(0));

    // 2. Editing the source host's endpoint unlinks hosts linked to the old source.
    let mut ws2 = workspace_hosts(2);
    let source_ep2 = ws2.settings.hosts[0].endpoint.clone();
    ws2.shared_hosts.source_override = Some(source_ep2.clone());
    let _rx0 = stub_host(&mut ws2, 0);
    let _rx1 = stub_host(&mut ws2, 1);
    ws2.settings.hosts[1].label = "Studio".into();
    ws2.settings.hosts[1].endpoint = "https://studio.invalid".into();
    ws2.settings.hosts[1].shared = Some(SharedLink {
        id: "host:studio".into(),
        from: source_ep2,
    });
    ws2.edit_host(
        0,
        "Moved Local".into(),
        "unix:///nonexistent/other-local.sock".into(),
        None,
    );
    assert_eq!(
        ws2.settings.hosts[1].shared, None,
        "editing source endpoint must unlink orphaned shared hosts"
    );
    assert!(!ws2.is_shared_managed(1));

    // 3. Launching with a saved linked host whose `from` doesn't match the current source
    // unlinks it on startup so it is never permanently locked read-only.
    let mut ws3 = workspace_hosts(2);
    ws3.settings.hosts[1].endpoint = "https://studio.invalid".into();
    ws3.settings.hosts[1].shared = Some(SharedLink {
        id: "host:studio".into(),
        from: "unix:///nonexistent/old-vanished-source.sock".into(),
    });
    ws3.save();
    let relaunched = relaunch(&ws3);
    assert_eq!(
        relaunched.settings.hosts[1].shared, None,
        "startup must unlink orphaned links from a vanished source"
    );
    assert!(!relaunched.is_shared_managed(1));
}

#[test]
fn settings_card_shows_diagnostics_error_severity_provenance_conflict_failed_search_and_fits_640x480()
 {
    let mut ws = workspace_hosts(2);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let _rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);

    ws.settings.hosts[1].label = "Studio".into();
    ws.settings.hosts[1].endpoint = "https://studio.invalid".into();
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h1".into(),
            hosts: vec![
                host_record(
                    "host:studio",
                    "Studio",
                    &["studio.invalid"],
                    None,
                    Some("init.lua:2"),
                ),
                host_record("host:bad", "UnreachableBox", &[], None, Some("init.lua:9")),
            ],
            diagnostics: vec![SharedHostDiagnostic {
                severity: "error".into(),
                message: "rex.host: invalid label".into(),
                source: Some("init.lua:9".into()),
            }],
        }),
        None,
    );

    // Error diagnostic notice reflects error severity (not "warning"), announced once.
    assert!(
        ws.notices
            .iter()
            .any(|n| n.text.contains("1 shared host error from Rex")),
        "expected error wording in notices: {:?}",
        ws.notices.iter().map(|n| &n.text).collect::<Vec<_>>()
    );

    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(SettingsSection::Hosts);

    // Settings search finds Remote Hosts for all required terms.
    for query in ["shared host", "rex hosts", "sync"] {
        ws.settings_nav.search.text = query.into();
        ws.settings_search_changed();
        assert!(
            ws.settings_nav
                .hits()
                .iter()
                .any(|hit| hit.page == SettingsSection::Hosts),
            "search {query:?} missed Remote Hosts: {:?}",
            ws.settings_nav.hits()
        );
    }
    ws.clear_settings_search();

    // Render at 640x480 minimum window and verify controls, diagnostics, provenance, and bounds.
    let mut rt = runtime_at(ws, (640., 480.));
    settle(&mut rt);
    let scroll = rect(&rt, "settings-scroll:hosts");
    let elements = rt.elements();
    let texts: Vec<&str> = elements.iter().filter_map(|e| e.text.as_deref()).collect();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("Shared via rex hosts") && t.contains("Declared in init.lua:2")),
        "missing provenance text in {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("rex.host: invalid label")),
        "missing diagnostic message in {texts:?}"
    );
    assert!(
        texts.contains(&"UnreachableBox"),
        "missing unconnectable host in {texts:?}"
    );
    for key in [
        "sync-shared-hosts",
        "shared-hosts-status",
        "shared-host-diag:0",
        "shared-host-unconnectable:0",
        "host-shell-btn:1",
    ] {
        let r = rect(&rt, key);
        assert!(
            r[0] >= scroll[0] - 0.5 && r[0] + r[2] <= scroll[0] + scroll[2] + 0.5,
            "{key} overflows 640x480 page: {r:?} not in {scroll:?}"
        );
    }

    // Conflict state renders the broken-file error clearly in Settings → Remote Hosts.
    rt.update(|s, _| {
        push_shared_state(
            s,
            0,
            SharedHostState::Conflict("hosts/everywhere.json is not a JSON object".into()),
            None,
        );
    });
    settle(&mut rt);
    let conflict_texts: Vec<String> = rt.elements().into_iter().filter_map(|e| e.text).collect();
    assert!(
        conflict_texts
            .iter()
            .any(|t| t.contains("hosts/everywhere.json is not a JSON object")),
        "conflict message not shown: {conflict_texts:?}"
    );
    assert!(
        conflict_texts.iter().any(|t| t == "Conflict"),
        "Conflict chip not shown"
    );

    // Failed state renders Error chip and reason (never stuck on Loading).
    rt.update(|s, _| {
        push_shared_state(
            s,
            0,
            SharedHostState::Failed("hosts.list request timed out.".into()),
            None,
        );
    });
    settle(&mut rt);
    let failed_texts: Vec<String> = rt.elements().into_iter().filter_map(|e| e.text).collect();
    assert!(
        failed_texts
            .iter()
            .any(|t| t.contains("hosts.list request timed out.")),
        "failed message not shown: {failed_texts:?}"
    );
    assert!(
        failed_texts.iter().any(|t| t == "Error"),
        "Error chip not shown"
    );
}

#[test]
fn editing_stored_shared_host_preserves_raw_waits_for_server_applies_safety_and_keeps_shell_local()
{
    let mut ws = workspace_hosts(2);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);
    {
        let mut m = ws.hosts[0].connection.model.lock().unwrap();
        m.connection_epoch = 7;
    }

    let old_ep = "https://studio.invalid".to_owned();
    ws.settings.hosts[1].label = "Studio".into();
    ws.settings.hosts[1].endpoint = old_ep.clone();
    ws.settings.hosts[1].remote_control = true;
    ws.settings.hosts[1].collapsed = true;
    ws.settings
        .sessions
        .insert(old_ep.clone(), "session:old".into());
    ws.settings.recent_paths.insert(
        old_ep.clone(),
        vec![RecentPath {
            path: "/srv/studio".into(),
            directory: true,
        }],
    );

    let rich_record = SharedHost::from_value(&json!({
        "id": "host:studio",
        "label": "Studio",
        "endpoints": ["studio.invalid", "ssh://alice@studio.invalid"],
        "icon": "server",
        "added_from": "laptop",
        "host_id": "11111111-2222-3333-4444-555555555555",
        "custom_extra": {"nested": [1, 2, 3]}
    }))
    .unwrap();
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h1".into(),
            hosts: vec![rich_record],
            diagnostics: Vec::new(),
        }),
        None,
    );
    ws.active = 1;
    rx0.drain();

    // Sidebar menu & Commands enable Edit host… and Remove host… for stored shared host.
    ws.host_menu(1, (100., 100.));
    let menu = ws.entries();
    assert!(
        menu.iter()
            .find(|e| e.label == "Edit host…")
            .unwrap()
            .enabled
    );
    assert!(
        menu.iter()
            .find(|e| e.label == "Remove host…")
            .unwrap()
            .enabled
    );
    ws.picker(false);
    let cmds = ws.entries();
    assert!(
        cmds.iter()
            .find(|e| e.id.as_deref() == Some("host.edit"))
            .unwrap()
            .enabled
    );
    assert!(
        cmds.iter()
            .find(|e| e.id.as_deref() == Some("host.remove"))
            .unwrap()
            .enabled
    );

    let mut tx = rune::Tx::default();

    // 1. Shell-only edit stays local and immediate with ZERO PutSharedHost commands.
    ws.perform(Intent::EditHost(1), &mut tx);
    assert!(matches!(
        ws.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Form(Form::EditHost(1)))
    ));
    ws.panel.as_mut().unwrap().inputs[2].text = "/bin/zsh".into();
    ws.submit(&mut tx);
    assert_eq!(ws.settings.hosts[1].shell.as_deref(), Some("/bin/zsh"));
    assert!(
        rx0.is_empty(),
        "shell-only edit must send zero PutSharedHost"
    );

    // 2. Label + endpoint + shell edit sends epoch-scoped PutSharedHost with preserved raw fields
    // and unrelated endpoints, and does NOT mutate local state ahead of server reply.
    let old_host_id = ws.hosts[1].id;
    ws.perform(Intent::EditHost(1), &mut tx);
    {
        let panel = ws.panel.as_mut().unwrap();
        panel.inputs[0].text = "Studio Pro".into();
        panel.inputs[1].text = "https://studio-new.invalid:8443".into();
        panel.inputs[2].text = "fish".into();
    }
    ws.submit(&mut tx);
    assert_eq!(ws.settings.hosts[1].label, "Studio", "must wait for server");
    assert_eq!(
        ws.settings.hosts[1].endpoint, old_ep,
        "must wait for server"
    );
    assert_eq!(ws.settings.hosts[1].shell.as_deref(), Some("/bin/zsh"));
    assert!(ws.settings.hosts[1].remote_control);

    let cmd = rx0.try_recv().expect("PutSharedHost sent");
    let (req_id, epoch, payload) = match cmd {
        HostCommand::PutSharedHost { id, epoch, host } => (id, epoch, host),
        other => panic!(
            "expected PutSharedHost, got {:?}",
            std::mem::discriminant(&other)
        ),
    };
    assert_eq!(epoch, 7);
    assert_eq!(payload["id"], "host:studio");
    assert_eq!(payload["label"], "Studio Pro");
    assert_eq!(
        payload["endpoints"],
        json!([
            "https://studio-new.invalid:8443",
            "ssh://alice@studio.invalid"
        ])
    );
    assert_eq!(payload["icon"], "server");
    assert_eq!(payload["added_from"], "laptop");
    assert_eq!(payload["host_id"], "11111111-2222-3333-4444-555555555555");
    assert_eq!(payload["custom_extra"], json!({"nested": [1, 2, 3]}));

    // Concurrent edit while pending is refused cleanly without sending a second write.
    ws.perform(Intent::EditHost(1), &mut tx);
    assert!(ws.panel.is_none());
    assert!(rx0.is_empty());

    // 3. Server failure leaves local row 100% untouched and surfaces notice.
    push_shared_write(
        &mut ws,
        0,
        WriteResult {
            id: req_id,
            epoch: 7,
            kind: WriteKind::Put,
            outcome: WriteOutcome::Failed {
                code: Some("conflict".into()),
                message: "the label \"Studio Pro\" is already used".into(),
            },
        },
    );
    assert_eq!(ws.settings.hosts[1].label, "Studio");
    assert_eq!(ws.settings.hosts[1].endpoint, old_ep);
    assert_eq!(ws.settings.hosts[1].shell.as_deref(), Some("/bin/zsh"));
    assert!(ws.settings.hosts[1].remote_control);
    assert_eq!(ws.hosts[1].id, old_host_id);
    assert!(ws.notices.iter().any(|n| {
        n.text.contains("Studio Pro")
            && n.text.contains("already used")
            && n.text.contains("including default shell, were saved")
    }));

    // 4. Retry edit and deliver server confirmation + refreshed list: applies safety policy once.
    ws.perform(Intent::EditHost(1), &mut tx);
    {
        let panel = ws.panel.as_mut().unwrap();
        panel.inputs[0].text = "Studio Pro".into();
        panel.inputs[1].text = "https://studio-new.invalid:8443".into();
        panel.inputs[2].text = "fish".into();
    }
    ws.submit(&mut tx);
    let req_id2 = match rx0.try_recv().unwrap() {
        HostCommand::PutSharedHost { id, .. } => id,
        _ => panic!("expected PutSharedHost"),
    };
    let updated_record = SharedHost::from_value(&json!({
        "id": "host:studio",
        "label": "Studio Pro",
        "endpoints": ["https://studio-new.invalid:8443", "ssh://alice@studio.invalid"],
        "icon": "server",
        "added_from": "laptop",
        "host_id": "11111111-2222-3333-4444-555555555555",
        "custom_extra": {"nested": [1, 2, 3]}
    }))
    .unwrap();
    push_shared_write(
        &mut ws,
        0,
        WriteResult {
            id: req_id2,
            epoch: 7,
            kind: WriteKind::Put,
            outcome: WriteOutcome::Put {
                host: updated_record.clone(),
                hash: "h2".into(),
            },
        },
    );
    let new_host_id = ws.hosts[1].id;
    assert_ne!(new_host_id, old_host_id, "endpoint change must reconnect");
    assert_eq!(ws.settings.hosts[1].label, "Studio Pro");
    assert_eq!(
        ws.settings.hosts[1].endpoint,
        "https://studio-new.invalid:8443"
    );
    assert_eq!(ws.settings.hosts[1].shell.as_deref(), Some("fish"));
    assert!(ws.settings.hosts[1].collapsed);
    assert!(!ws.settings.hosts[1].remote_control);
    assert!(!ws.settings.sessions.contains_key(&old_ep));
    assert!(!ws.settings.recent_paths.contains_key(&old_ep));

    // Subsequent hosts.list refresh reconciles cleanly as a no-op (no double reconnect!).
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h2".into(),
            hosts: vec![updated_record],
            diagnostics: Vec::new(),
        }),
        None,
    );
    assert_eq!(ws.hosts[1].id, new_host_id, "must not double-reconnect");
}

#[test]
fn save_ssh_socket_routes_stored_shared_host_through_put_shared_host() {
    let mut ws = workspace_hosts(2);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);
    {
        ws.hosts[0]
            .connection
            .model
            .lock()
            .unwrap()
            .connection_epoch = 3;
    }

    let ssh_ep = "ssh://me@studio.invalid".to_owned();
    let proposed = "ssh://me@studio.invalid/run/custom.sock".to_owned();
    ws.settings.hosts[1].label = "Studio".into();
    ws.settings.hosts[1].endpoint = ssh_ep.clone();
    ws.settings.hosts[1].remote_control = true;
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h1".into(),
            hosts: vec![
                SharedHost::from_value(&json!({
                    "id": "host:studio",
                    "label": "Studio",
                    "endpoints": [&ssh_ep, "https://fallback.invalid"],
                    "custom": true
                }))
                .unwrap(),
            ],
            diagnostics: Vec::new(),
        }),
        None,
    );
    ws.hosts[1].model.connected = false;
    ws.ssh_setup.attachment.insert(
        ssh_ep.clone(),
        setup::Attachment::Review {
            reason: "custom socket".into(),
            endpoint: Some(proposed.clone()),
        },
    );
    rx0.drain();

    let mut tx = rune::Tx::default();
    ws.save_ssh_socket(&ssh_ep, &mut tx);
    // Must not mutate ahead of reply!
    assert_eq!(ws.settings.hosts[1].endpoint, ssh_ep);
    assert!(ws.ssh_setup.attachment.contains_key(&ssh_ep));

    let (req_id, payload) = match rx0.try_recv().expect("PutSharedHost sent") {
        HostCommand::PutSharedHost { id, epoch, host } => {
            assert_eq!(epoch, 3);
            (id, host)
        }
        _ => panic!("expected PutSharedHost"),
    };
    assert_eq!(
        payload["endpoints"],
        json!([&proposed, "https://fallback.invalid"])
    );
    assert_eq!(payload["custom"], json!(true));

    let updated = SharedHost::from_value(&json!({
        "id": "host:studio",
        "label": "Studio",
        "endpoints": [&proposed, "https://fallback.invalid"],
        "custom": true
    }))
    .unwrap();
    push_shared_write(
        &mut ws,
        0,
        WriteResult {
            id: req_id,
            epoch: 3,
            kind: WriteKind::Put,
            outcome: WriteOutcome::Put {
                host: updated,
                hash: "h2".into(),
            },
        },
    );
    assert_eq!(ws.settings.hosts[1].endpoint, proposed);
    assert!(!ws.settings.hosts[1].remote_control);
    assert!(!ws.ssh_setup.attachment.contains_key(&ssh_ep));
}

#[test]
fn removing_stored_shared_host_warns_waits_for_server_and_avoids_rule_5_notice() {
    let mut ws = workspace_hosts(2);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);
    {
        ws.hosts[0]
            .connection
            .model
            .lock()
            .unwrap()
            .connection_epoch = 4;
    }

    let studio_ep = "https://studio.invalid".to_owned();
    ws.settings.hosts[1].label = "Studio".into();
    ws.settings.hosts[1].endpoint = studio_ep.clone();
    ws.settings
        .sessions
        .insert(studio_ep.clone(), "session:s1".into());
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h1".into(),
            hosts: vec![host_record(
                "host:studio",
                "Studio",
                &["studio.invalid"],
                None,
                None,
            )],
            diagnostics: Vec::new(),
        }),
        None,
    );
    ws.active = 1;
    rx0.drain();

    // Open RemoveHost from Settings so we also verify Settings stays open after async confirmation!
    let mut tx = rune::Tx::default();
    ws.perform(Intent::Settings, &mut tx);
    ws.open_section(SettingsSection::Hosts);
    ws.perform(Intent::RemoveHost(1), &mut tx);
    assert!(matches!(
        ws.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::RemoveHost(1))
    ));
    assert!(
        rx0.is_empty(),
        "opening dialog must not send RemoveSharedHost"
    );

    // Render confirmation dialog and check shared-removal warning copy.
    let mut rt = runtime_at(ws, (640., 480.));
    settle(&mut rt);
    let texts: Vec<String> = rt.elements().into_iter().filter_map(|e| e.text).collect();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("Rex's shared host list for every client")),
        "missing shared-removal warning copy: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "Remove from shared list"),
        "missing shared danger button label: {texts:?}"
    );
    let dialog_r = rect(&rt, "dialog");
    assert!(
        dialog_r[0] >= 0.
            && dialog_r[1] >= 0.
            && dialog_r[0] + dialog_r[2] <= 640.5
            && dialog_r[1] + dialog_r[3] <= 480.5,
        "RemoveHost dialog overflows 640x480: {dialog_r:?}"
    );

    // Confirm removal: sends epoch-scoped RemoveSharedHost and does NOT delete locally yet!
    click(&mut rt, "confirm-close");
    assert_eq!(
        rt.state.settings.hosts.len(),
        2,
        "must not delete local row ahead of server confirmation"
    );
    let (req_id, epoch, host_id) = match rx0.try_recv().expect("RemoveSharedHost sent") {
        HostCommand::RemoveSharedHost { id, epoch, host_id } => (id, epoch, host_id),
        _ => panic!("expected RemoveSharedHost"),
    };
    assert_eq!(epoch, 4);
    assert_eq!(host_id, "host:studio");

    // 1. Server error keeps the row intact and notifies.
    rt.update(|s, _| {
        push_shared_write(
            s,
            0,
            WriteResult {
                id: req_id,
                epoch: 4,
                kind: WriteKind::Remove,
                outcome: WriteOutcome::Failed {
                    code: Some("conflict".into()),
                    message: "hosts/everywhere.json is not a JSON object".into(),
                },
            },
        );
    });
    assert_eq!(rt.state.settings.hosts.len(), 2);
    assert!(
        rt.state
            .notices
            .iter()
            .any(|n| n.text.contains("hosts/everywhere.json"))
    );

    // 2. Confirm again and deliver server success + refreshed empty list: removes cleanly without
    // Rule 5's "kept as a local host" notice and keeps Settings open!
    rt.update(|s, tx| {
        s.notices.clear();
        s.perform(Intent::RemoveHost(1), tx);
        s.submit(tx);
    });
    let req_id2 = match rx0.try_recv().expect("second RemoveSharedHost") {
        HostCommand::RemoveSharedHost { id, .. } => id,
        _ => panic!("expected RemoveSharedHost"),
    };
    rt.update(|s, _| {
        push_shared_write(
            s,
            0,
            WriteResult {
                id: req_id2,
                epoch: 4,
                kind: WriteKind::Remove,
                outcome: WriteOutcome::Removed {
                    host_id: "host:studio".into(),
                    removed: true,
                    hash: "h2".into(),
                },
            },
        );
        push_shared_state(
            s,
            0,
            SharedHostState::Loaded(SharedHostList {
                hash: "h2".into(),
                hosts: Vec::new(),
                diagnostics: Vec::new(),
            }),
            None,
        );
    });
    assert_eq!(rt.state.settings.hosts.len(), 1);
    assert_eq!(rt.state.active, 0);
    assert!(!rt.state.settings.sessions.contains_key(&studio_ep));
    assert!(
        matches!(
            rt.state.panel.as_ref().map(|p| &p.kind),
            Some(PanelKind::Settings)
        ),
        "Settings should remain open after confirmed shared removal"
    );
    assert!(
        !rt.state
            .notices
            .iter()
            .any(|n| n.text.contains("kept as a local host")),
        "self-initiated removal must never fire Rule 5's unlink-and-keep notice: {:?}",
        rt.state.notices.iter().map(|n| &n.text).collect::<Vec<_>>()
    );
}

#[test]
fn add_host_shared_toggle_sends_put_avoids_duplicates_supports_local_opt_out_and_fits_640x480() {
    let mut ws = workspace_hosts(1);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let rx0 = stub_host(&mut ws, 0);
    {
        ws.hosts[0]
            .connection
            .model
            .lock()
            .unwrap()
            .connection_epoch = 9;
    }
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h1".into(),
            hosts: vec![host_record(
                "host:existing",
                "Existing",
                &["existing.invalid"],
                None,
                None,
            )],
            diagnostics: Vec::new(),
        }),
        None,
    );
    rx0.drain();

    // Open AddHostManually while source is connected & Loaded -> shared toggle shown & ON.
    ws.perform(Intent::AddHostManually, &mut rune::Tx::default());
    assert!(ws.panel.as_ref().unwrap().save_shared);

    let mut rt = runtime_at(ws, (640., 480.));
    settle(&mut rt);
    let dialog_r = rect(&rt, "dialog");
    let switch_r = rect(&rt, "save-shared-host");
    assert!(
        dialog_r[0] >= 0.
            && dialog_r[1] >= 0.
            && dialog_r[0] + dialog_r[2] <= 640.5
            && dialog_r[1] + dialog_r[3] <= 480.5,
        "AddHost dialog with shared toggle overflows 640x480: {dialog_r:?}"
    );
    assert!(
        switch_r[0] >= dialog_r[0] && switch_r[0] + switch_r[2] <= dialog_r[0] + dialog_r[2],
        "shared switch overflows dialog: {switch_r:?} not in {dialog_r:?}"
    );

    // Canonical duplicate endpoint is rejected locally with zero PutSharedHost.
    rt.update(|s, tx| {
        let p = s.panel.as_mut().unwrap();
        p.inputs[0].text = "Dup".into();
        p.inputs[1].text = "HTTPS://EXISTING.INVALID:443/".into();
        s.submit(tx);
    });
    assert_eq!(
        rt.state.panel.as_ref().unwrap().error.as_deref(),
        Some("This endpoint is already in the workspace")
    );
    assert!(rx0.is_empty());

    // Valid submit with shared toggle ON sends PutSharedHost and does not add locally ahead of reply.
    rt.update(|s, tx| {
        let p = s.panel.as_mut().unwrap();
        p.inputs[0].text = "Lab".into();
        p.inputs[1].text = "https://lab.invalid".into();
        s.submit(tx);
    });
    assert!(rt.state.panel.is_none());
    assert_eq!(
        rt.state.settings.hosts.len(),
        2,
        "must not add ahead of reply"
    );
    let (req_id, epoch, payload) = match rx0.try_recv().expect("PutSharedHost sent") {
        HostCommand::PutSharedHost { id, epoch, host } => (id, epoch, host),
        _ => panic!("expected PutSharedHost"),
    };
    assert_eq!(epoch, 9);
    assert_eq!(payload["label"], "Lab");
    assert_eq!(payload["endpoints"], json!(["https://lab.invalid"]));
    assert!(payload.get("id").is_none());

    // Server confirms creation and subsequent hosts.list reconciles with zero duplicate rows.
    let created = SharedHost::from_value(&json!({
        "id": "host:lab",
        "label": "Lab",
        "endpoints": ["https://lab.invalid"]
    }))
    .unwrap();
    rt.update(|s, _| {
        push_shared_write(
            s,
            0,
            WriteResult {
                id: req_id,
                epoch: 9,
                kind: WriteKind::Put,
                outcome: WriteOutcome::Put {
                    host: created.clone(),
                    hash: "h2".into(),
                },
            },
        );
        push_shared_state(
            s,
            0,
            SharedHostState::Loaded(SharedHostList {
                hash: "h2".into(),
                hosts: vec![
                    host_record(
                        "host:existing",
                        "Existing",
                        &["existing.invalid"],
                        None,
                        None,
                    ),
                    created,
                ],
                diagnostics: Vec::new(),
            }),
            None,
        );
    });
    assert_eq!(
        rt.state.settings.hosts.len(),
        3,
        "no duplicate row on reconcile"
    );
    assert_eq!(rt.state.settings.hosts[2].label, "Lab");
    assert_eq!(
        rt.state.settings.hosts[2].shared,
        Some(SharedLink {
            id: "host:lab".into(),
            from: source_ep,
        })
    );

    // Turning the switch OFF adds locally immediately with ZERO PutSharedHost commands.
    rx0.drain();
    rt.update(|s, tx| {
        s.perform(Intent::AddHostManually, tx);
    });
    settle(&mut rt);
    click(&mut rt, "save-shared-host");
    assert!(!rt.state.panel.as_ref().unwrap().save_shared);
    rt.update(|s, tx| {
        let p = s.panel.as_mut().unwrap();
        p.inputs[0].text = "LocalOnly".into();
        p.inputs[1].text = "https://local-only.invalid".into();
        s.submit(tx);
    });
    assert!(
        !rx0.try_iter().any(|c| matches!(
            c,
            HostCommand::PutSharedHost { .. } | HostCommand::RemoveSharedHost { .. }
        )),
        "local-only add must send zero PutSharedHost"
    );
    assert_eq!(rt.state.settings.hosts.len(), 4);
    assert_eq!(rt.state.settings.hosts[3].shared, None);
}

#[test]
fn offline_conflict_and_unsupported_sources_refuse_shared_writes_and_stale_open_forms() {
    let mut ws = workspace_hosts(2);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);

    ws.settings.hosts[1].label = "Studio".into();
    ws.settings.hosts[1].endpoint = "https://studio.invalid".into();
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h1".into(),
            hosts: vec![host_record(
                "host:studio",
                "Studio",
                &["studio.invalid"],
                None,
                None,
            )],
            diagnostics: Vec::new(),
        }),
        None,
    );
    rx0.drain();
    let mut tx = rune::Tx::default();

    // Open EditHost while healthy, then source transitions to Conflict before submit -> refused!
    ws.perform(Intent::EditHost(1), &mut tx);
    assert!(matches!(
        ws.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Form(Form::EditHost(1)))
    ));
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Conflict("broken json".into()),
        None,
    );
    ws.panel.as_mut().unwrap().inputs[0].text = "Renamed".into();
    ws.submit(&mut tx);
    assert!(rx0.is_empty());
    assert_eq!(ws.settings.hosts[1].label, "Studio");
    assert!(
        ws.panel
            .as_ref()
            .and_then(|p| p.error.as_deref())
            .is_some_and(|e| e.contains("conflict"))
    );
    ws.close_panel(&mut tx);

    // Unsupported and offline states also refuse EditHost and RemoveHost cleanly.
    for make_unavailable in [
        (|s: &mut Workspace| push_shared_state(s, 0, SharedHostState::Unsupported, None))
            as fn(&mut Workspace),
        |s: &mut Workspace| {
            s.hosts[0].connection.model.lock().unwrap().connected = false;
            s.hosts[0].model.connected = false;
        },
    ] {
        make_unavailable(&mut ws);
        ws.perform(Intent::EditHost(1), &mut tx);
        assert!(ws.panel.is_none());
        ws.perform(Intent::RemoveHost(1), &mut tx);
        assert!(ws.panel.is_none());
        assert!(rx0.is_empty());
    }
}

#[test]
fn settings_card_distinguishes_skipped_duplicate_chip_from_unconnectable_and_edit_fits_640x480() {
    let mut ws = workspace_hosts(1);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep);
    let _rx0 = stub_host(&mut ws, 0);

    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h-chips".into(),
            hosts: vec![
                host_record("host:one", "One", &["dup.invalid"], None, None),
                host_record("host:two", "TwoDup", &["dup.invalid"], None, None),
                host_record("host:bad", "BadScheme", &["ftp://only-ftp"], None, None),
            ],
            diagnostics: Vec::new(),
        }),
        None,
    );

    let mut tx = rune::Tx::default();
    ws.perform(Intent::Settings, &mut tx);
    ws.open_section(SettingsSection::Hosts);
    let mut rt = runtime_at(ws, (640., 480.));
    settle(&mut rt);

    let texts: Vec<String> = rt.elements().into_iter().filter_map(|e| e.text).collect();
    assert!(
        texts.iter().any(|t| t == "Skipped"),
        "missing Skipped chip: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "Unconnectable"),
        "missing Unconnectable chip: {texts:?}"
    );

    // EditHost dialog for stored shared host also fits at 640x480.
    rt.update(|s, tx| s.perform(Intent::EditHost(1), tx));
    settle(&mut rt);
    let dialog_r = rect(&rt, "dialog");
    assert!(
        dialog_r[0] >= 0.
            && dialog_r[1] >= 0.
            && dialog_r[0] + dialog_r[2] <= 640.5
            && dialog_r[1] + dialog_r[3] <= 480.5,
        "EditHost dialog overflows 640x480: {dialog_r:?}"
    );
}

#[test]
fn async_shared_removal_clears_non_settings_host_indexed_panels_and_keeps_settings() {
    use crate::app::panel::SessionTarget;

    let mut ws = workspace_hosts(3);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep);
    let rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);
    let _rx2 = stub_host(&mut ws, 2);
    ws.hosts[0]
        .connection
        .model
        .lock()
        .unwrap()
        .connection_epoch = 2;

    ws.settings.hosts[1].label = "Alpha".into();
    ws.settings.hosts[1].endpoint = "https://alpha.invalid".into();
    ws.settings.hosts[2].label = "Beta".into();
    ws.settings.hosts[2].endpoint = "https://beta.invalid".into();
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h1".into(),
            hosts: vec![
                host_record("host:alpha", "Alpha", &["alpha.invalid"], None, None),
                host_record("host:beta", "Beta", &["beta.invalid"], None, None),
            ],
            diagnostics: Vec::new(),
        }),
        None,
    );
    rx0.drain();

    // Start removing host 1 (Alpha), then open a non-Settings host-indexed panel on host 2 (Beta).
    let mut tx = rune::Tx::default();
    ws.perform(Intent::RemoveHost(1), &mut tx);
    ws.submit(&mut tx);
    let req_id = match rx0.try_recv().expect("RemoveSharedHost sent") {
        HostCommand::RemoveSharedHost { id, .. } => id,
        _ => panic!("expected RemoveSharedHost"),
    };
    ws.open_panel(PanelKind::Session, "Beta session", &[]);
    ws.panel.as_mut().unwrap().session = Some(SessionTarget {
        host: 2,
        session: "session:beta".into(),
        label: "work".into(),
    });
    assert!(ws.panel.is_some());

    // Async confirmation removes host 1 (shifting Beta from 2 -> 1): non-Settings panel MUST clear
    // so it cannot panic or target a shifted host index!
    push_shared_write(
        &mut ws,
        0,
        WriteResult {
            id: req_id,
            epoch: 2,
            kind: WriteKind::Remove,
            outcome: WriteOutcome::Removed {
                host_id: "host:alpha".into(),
                removed: true,
                hash: "h2".into(),
            },
        },
    );
    assert_eq!(ws.settings.hosts.len(), 2);
    assert!(
        ws.panel.is_none(),
        "non-Settings host-indexed panel must be cleared on async host removal"
    );

    // By contrast, Settings stays open when Beta (now index 1) is removed asynchronously.
    rx0.drain();
    ws.perform(Intent::Settings, &mut tx);
    ws.open_section(SettingsSection::Hosts);
    ws.perform(Intent::RemoveHost(1), &mut tx);
    ws.submit(&mut tx);
    let req_id2 = match rx0.try_recv().expect("second RemoveSharedHost") {
        HostCommand::RemoveSharedHost { id, .. } => id,
        _ => panic!("expected RemoveSharedHost"),
    };
    assert!(matches!(
        ws.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));
    push_shared_write(
        &mut ws,
        0,
        WriteResult {
            id: req_id2,
            epoch: 2,
            kind: WriteKind::Remove,
            outcome: WriteOutcome::Removed {
                host_id: "host:beta".into(),
                removed: true,
                hash: "h3".into(),
            },
        },
    );
    assert_eq!(ws.settings.hosts.len(), 1);
    assert!(matches!(
        ws.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));
}

#[test]
fn shared_write_collision_guards_upfront_pending_and_reply_time_preserve_unique_endpoints() {
    let mut ws = workspace_hosts(3);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);
    let _rx2 = stub_host(&mut ws, 2);
    ws.hosts[0]
        .connection
        .model
        .lock()
        .unwrap()
        .connection_epoch = 5;

    ws.settings.hosts[1].label = "Alpha".into();
    ws.settings.hosts[1].endpoint = "https://alpha.invalid".into();
    ws.settings.hosts[2].label = "Beta".into();
    ws.settings.hosts[2].endpoint = "ssh://me@beta.invalid".into();
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h1".into(),
            hosts: vec![
                host_record("host:alpha", "Alpha", &["alpha.invalid"], None, None),
                host_record("host:beta", "Beta", &["ssh://me@beta.invalid"], None, None),
            ],
            diagnostics: Vec::new(),
        }),
        None,
    );
    rx0.drain();
    let mut tx = rune::Tx::default();

    // (a) Upfront pending-write duplicate rejection: start shared Add for https://raced.invalid.
    ws.perform(Intent::AddHostManually, &mut tx);
    {
        let p = ws.panel.as_mut().unwrap();
        p.inputs[0].text = "Raced".into();
        p.inputs[1].text = "https://raced.invalid".into();
    }
    ws.submit(&mut tx);
    let add_req_id = match rx0.try_recv().expect("Add PutSharedHost") {
        HostCommand::PutSharedHost { id, .. } => id,
        _ => panic!("expected PutSharedHost"),
    };

    // Concurrent second Add targeting the same canonical endpoint is refused upfront!
    ws.perform(Intent::AddHostManually, &mut tx);
    {
        let p = ws.panel.as_mut().unwrap();
        p.inputs[0].text = "Raced2".into();
        p.inputs[1].text = "HTTPS://RACED.INVALID:443/".into();
    }
    ws.submit(&mut tx);
    assert!(rx0.is_empty(), "duplicate pending Add must not hit wire");
    assert!(
        ws.panel
            .as_ref()
            .and_then(|p| p.error.as_deref())
            .is_some_and(|e| e.contains("already in progress"))
    );
    ws.close_panel(&mut tx);

    // Concurrent Edit targeting the pending Add's canonical endpoint is also refused upfront!
    ws.perform(Intent::EditHost(1), &mut tx);
    ws.panel.as_mut().unwrap().inputs[1].text = "https://raced.invalid/".into();
    ws.submit(&mut tx);
    assert!(rx0.is_empty(), "duplicate pending Edit must not hit wire");
    assert!(
        ws.panel
            .as_ref()
            .and_then(|p| p.error.as_deref())
            .is_some_and(|e| e.contains("already in progress"))
    );
    ws.close_panel(&mut tx);

    // (b) Reply-time Add collision with an ALREADY-LINKED row must NOT hijack that row!
    // Suppose while add_req_id was in flight, another shared record linked host[1] onto https://raced.invalid.
    ws.settings.hosts[1].endpoint = "https://raced.invalid".into();
    ws.notices.clear();
    let created = SharedHost::from_value(&json!({
        "id": "host:raced-new",
        "label": "Server Canonical Label",
        "endpoints": ["https://raced.invalid"]
    }))
    .unwrap();
    push_shared_write(
        &mut ws,
        0,
        WriteResult {
            id: add_req_id,
            epoch: 5,
            kind: WriteKind::Put,
            outcome: WriteOutcome::Put {
                host: created,
                hash: "h2".into(),
            },
        },
    );
    assert_eq!(ws.settings.hosts.len(), 3, "must not create duplicate row");
    assert_eq!(
        ws.settings.hosts[1].shared.as_ref().map(|l| l.id.as_str()),
        Some("host:alpha"),
        "already-linked row must never be hijacked by another shared record"
    );
    assert_eq!(ws.settings.hosts[1].label, "Alpha");
    assert!(
        ws.notices
            .iter()
            .any(|n| n.text.contains("Added Server Canonical Label")),
        "toast should use server-returned record.label"
    );

    // Restore host[1] endpoint for part (c).
    ws.settings.hosts[1].endpoint = "https://alpha.invalid".into();

    // (c) Reply-time Edit collision: start Edit moving Alpha to https://target.invalid, then
    // add another row owning https://target.invalid before the server replies.
    ws.perform(Intent::EditHost(1), &mut tx);
    {
        let p = ws.panel.as_mut().unwrap();
        p.inputs[0].text = "Alpha Renamed".into();
        p.inputs[1].text = "https://target.invalid".into();
        p.inputs[2].text = "zsh".into();
    }
    ws.submit(&mut tx);
    let edit_req_id = match rx0.try_recv().expect("Edit PutSharedHost") {
        HostCommand::PutSharedHost { id, .. } => id,
        _ => panic!("expected PutSharedHost"),
    };

    // Another row claims https://target.invalid while edit_req_id is in flight.
    ws.add_host(
        Host {
            label: "Claimed".into(),
            endpoint: "https://target.invalid".into(),
            remote_control: false,
            prompts: true,
            collapsed: false,
            shell: None,
            shared: None,
        },
        &mut tx,
    );
    let alpha_id_before = ws.hosts[1].id;
    let updated = SharedHost::from_value(&json!({
        "id": "host:alpha",
        "label": "Alpha Renamed",
        "endpoints": ["https://target.invalid"]
    }))
    .unwrap();
    push_shared_write(
        &mut ws,
        0,
        WriteResult {
            id: edit_req_id,
            epoch: 5,
            kind: WriteKind::Put,
            outcome: WriteOutcome::Put {
                host: updated.clone(),
                hash: "h3".into(),
            },
        },
    );
    assert_eq!(
        ws.settings.hosts[1].endpoint, "https://alpha.invalid",
        "colliding Edit reply must not mutate onto another row's canonical endpoint"
    );
    assert_eq!(
        ws.hosts[1].id, alpha_id_before,
        "must not respawn duplicate worker"
    );
    assert_eq!(ws.settings.hosts[1].label, "Alpha Renamed");
    assert_eq!(ws.settings.hosts[1].shell.as_deref(), Some("zsh"));

    // Subsequent hosts.list reconcile surfaces the colliding record cleanly as Skipped (duplicate).
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h3".into(),
            hosts: vec![
                updated,
                host_record("host:beta", "Beta", &["ssh://me@beta.invalid"], None, None),
            ],
            diagnostics: Vec::new(),
        }),
        None,
    );
    assert!(
        ws.shared_hosts
            .unconnectable
            .iter()
            .any(|u| u.id == "host:alpha" && u.duplicate),
        "reconcile must surface colliding edit as Skipped duplicate: {:?}",
        ws.shared_hosts.unconnectable
    );
}

#[test]
fn self_removal_when_hosts_list_arrives_before_write_outcome_avoids_rule_5_and_leaks() {
    let mut ws = workspace_hosts(2);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep);
    let rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);
    ws.hosts[0]
        .connection
        .model
        .lock()
        .unwrap()
        .connection_epoch = 6;

    ws.settings.hosts[1].label = "Studio".into();
    ws.settings.hosts[1].endpoint = "https://studio.invalid".into();
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h1".into(),
            hosts: vec![host_record(
                "host:studio",
                "Studio",
                &["studio.invalid"],
                None,
                None,
            )],
            diagnostics: Vec::new(),
        }),
        None,
    );
    rx0.drain();
    ws.notices.clear();

    let mut tx = rune::Tx::default();
    ws.perform(Intent::RemoveHost(1), &mut tx);
    ws.submit(&mut tx);
    let req_id = match rx0.try_recv().expect("RemoveSharedHost sent") {
        HostCommand::RemoveSharedHost { id, .. } => id,
        _ => panic!("expected RemoveSharedHost"),
    };

    // 1. hosts.list WITHOUT host:studio arrives BEFORE WriteOutcome::Removed!
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h2".into(),
            hosts: Vec::new(),
            diagnostics: Vec::new(),
        }),
        None,
    );
    assert_eq!(ws.settings.hosts.len(), 1, "row must be removed cleanly");
    assert!(
        !ws.notices
            .iter()
            .any(|n| n.text.contains("kept as a local host")),
        "must not fire Rule 5 unlink-and-keep notice: {:?}",
        ws.notices.iter().map(|n| &n.text).collect::<Vec<_>>()
    );
    let notice_count_after_list = ws.notices.len();
    assert!(ws.notices.iter().any(|n| {
        n.text
            .contains("Removed Studio from the shared Rex host list")
    }));

    // 2. WriteOutcome::Removed arrives afterward: no duplicate notice and no stale self_removed!
    push_shared_write(
        &mut ws,
        0,
        WriteResult {
            id: req_id,
            epoch: 6,
            kind: WriteKind::Remove,
            outcome: WriteOutcome::Removed {
                host_id: "host:studio".into(),
                removed: true,
                hash: "h2".into(),
            },
        },
    );
    assert_eq!(
        ws.notices.len(),
        notice_count_after_list,
        "must not emit duplicate removal notice"
    );
    assert!(
        ws.shared_hosts.self_removed.is_empty(),
        "self_removed must not leak stale IDs"
    );

    // 3. removed: false when cached Loaded list no longer has the ID also leaks zero self_removed.
    ws.shared_hosts.pending_writes.insert(
        999,
        crate::app::shared_hosts::PendingSharedWrite::Remove {
            shared_id: "host:already-gone".into(),
            source_endpoint: ws.settings.hosts[0].endpoint.clone(),
            label: "Ghost".into(),
            applied: false,
        },
    );
    ws.notices.clear();
    push_shared_write(
        &mut ws,
        0,
        WriteResult {
            id: 999,
            epoch: 6,
            kind: WriteKind::Remove,
            outcome: WriteOutcome::Removed {
                host_id: "host:already-gone".into(),
                removed: false,
                hash: "h2".into(),
            },
        },
    );
    assert!(ws.shared_hosts.self_removed.is_empty());
    assert!(
        ws.notices.is_empty(),
        "must not emit misleading Removed notice when host was already absent"
    );
}

#[test]
fn long_shared_host_form_errors_wrap_inside_640x480_dialog() {
    let mut ws = workspace_hosts(1);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep);
    let _rx0 = stub_host(&mut ws, 0);
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h1".into(),
            hosts: Vec::new(),
            diagnostics: Vec::new(),
        }),
        None,
    );

    let mut tx = rune::Tx::default();
    ws.perform(Intent::AddHostManually, &mut tx);
    assert!(ws.panel.as_ref().unwrap().save_shared);

    // Source goes offline while AddHost dialog is open, triggering the long offline error message.
    ws.hosts[0].connection.model.lock().unwrap().connected = false;
    ws.hosts[0].model.connected = false;
    {
        let p = ws.panel.as_mut().unwrap();
        p.inputs[0].text = "OfflineLab".into();
        p.inputs[1].text = "https://offline-lab.invalid".into();
    }
    ws.submit(&mut tx);
    assert!(
        ws.panel
            .as_ref()
            .and_then(|p| p.error.as_deref())
            .is_some_and(|e| e.len() > 100),
        "expected long offline shared-add error"
    );

    let mut rt = runtime_at(ws, (640., 480.));
    settle(&mut rt);
    let dlg_r = rect(&rt, "dialog");
    let err_r = rect(&rt, "form-error");
    assert!(
        dlg_r[0] >= 0.
            && dlg_r[1] >= 0.
            && dlg_r[0] + dlg_r[2] <= 640.5
            && dlg_r[1] + dlg_r[3] <= 480.5,
        "AddHost dialog with long error overflows 640x480: {dlg_r:?}"
    );
    assert!(
        err_r[0] >= dlg_r[0] - 0.5 && err_r[0] + err_r[2] <= dlg_r[0] + dlg_r[2] + 0.5,
        "form-error overflows dialog horizontally at 640x480: {err_r:?} not in {dlg_r:?}"
    );
}
