//! Headless UI and workspace reconciliation tests for #145 Slice 1 (read-only shared Rex host list).
use super::*;
use crate::app::panel::SettingsSection;
use rex_rune::{
    connection::{
        HostModel,
        hosts::SharedHostState,
        server_info::{Answer, Process},
    },
    hosts::{SharedHost, SharedHostDiagnostic, SharedHostList},
    settings::{MutedTab, SharedLink, recent_paths::RecentPath},
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

#[test]
fn linking_and_renaming_preserve_all_local_preferences_and_suppress_self() {
    let mut ws = workspace_hosts(2);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let _rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);

    // Pre-existing local host with every local preference populated.
    ws.settings.hosts[1].label = "My Old Studio".into();
    ws.settings.hosts[1].endpoint = "https://studio.tail.ts.net:443/".into();
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
                    &["self.ts.net"],
                    Some("local-uuid"),
                    None,
                ),
                // Matches pre-existing host[1] via canonical endpoint.
                host_record(
                    "host:studio",
                    "Studio",
                    &["studio.tail.ts.net"],
                    Some("remote-1"),
                    Some("init.lua:2"),
                ),
                // New shared host appended automatically.
                host_record("host:lab", "Lab", &["lab.ts.net"], Some("remote-2"), None),
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
    assert_eq!(lab.endpoint, "https://lab.ts.net");
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
                    &["studio.tail.ts.net"],
                    Some("remote-1"),
                    Some("init.lua:2"),
                ),
                host_record("host:lab", "Lab", &["lab.ts.net"], Some("remote-2"), None),
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

    let old_ep = "https://old-box.ts.net".to_owned();
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
                &["new-box.ts.net"],
                None,
                None,
            )],
            diagnostics: Vec::new(),
        }),
        None,
    );

    let host = &ws.settings.hosts[1];
    assert_eq!(host.label, "Box Moved");
    assert_eq!(host.endpoint, "https://new-box.ts.net");
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
    ws.settings.hosts[1].endpoint = "https://studio.ts.net".into();
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
    ws.settings.hosts[1].endpoint = "https://studio.ts.net".into();
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h1".into(),
            hosts: vec![host_record(
                "host:studio",
                "Studio",
                &["studio.ts.net"],
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
}

#[test]
fn linked_host_edit_and_remove_are_guarded_while_local_shell_editing_works() {
    let mut ws = workspace_hosts(2);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let _rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);

    ws.settings.hosts[1].label = "Studio".into();
    ws.settings.hosts[1].endpoint = "https://studio.ts.net".into();
    ws.settings.hosts[1].shared = Some(SharedLink {
        id: "host:studio".into(),
        from: source_ep,
    });
    ws.active = 1;

    let mut tx = rune::Tx::default();
    // Intent::EditHost and Intent::RemoveHost refuse cleanly without opening destructive/edit panels.
    ws.perform(Intent::EditHost(1), &mut tx);
    assert!(ws.panel.is_none());
    ws.perform(Intent::RemoveHost(1), &mut tx);
    assert!(ws.panel.is_none());
    ws.remove_host(1, &mut tx);
    assert_eq!(
        ws.settings.hosts.len(),
        2,
        "linked host must not be removed"
    );

    // Sidebar host menu disables Edit host… and Remove host… while keeping Default shell… enabled.
    ws.host_menu(1, (100., 100.));
    let menu = ws.entries();
    let shell_row = menu.iter().find(|e| e.label == "Default shell…").unwrap();
    let edit_row = menu.iter().find(|e| e.label == "Edit host…").unwrap();
    let remove_row = menu.iter().find(|e| e.label == "Remove host…").unwrap();
    assert!(shell_row.enabled);
    assert!(!edit_row.enabled);
    assert!(edit_row.detail.contains("Managed by rex hosts"));
    assert!(!remove_row.enabled);
    assert!(remove_row.detail.contains("Managed by rex hosts"));

    // Commands also disables host.edit and host.remove for the linked active host.
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
    assert!(!cmd_remove.enabled);
    assert!(cmd_shell.enabled);

    // Local shell editing still works end-to-end!
    ws.perform(Intent::HostShell(1), &mut tx);
    assert!(matches!(
        ws.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Form(Form::HostShell(1)))
    ));
    ws.panel.as_mut().unwrap().inputs[0].text = "/bin/zsh".into();
    ws.submit(&mut tx);
    assert_eq!(ws.settings.hosts[1].shell.as_deref(), Some("/bin/zsh"));
}

#[test]
fn settings_card_shows_diagnostics_provenance_conflict_search_and_fits_640x480() {
    let mut ws = workspace_hosts(2);
    let source_ep = ws.settings.hosts[0].endpoint.clone();
    ws.shared_hosts.source_override = Some(source_ep.clone());
    let _rx0 = stub_host(&mut ws, 0);
    let _rx1 = stub_host(&mut ws, 1);

    ws.settings.hosts[1].label = "Studio".into();
    ws.settings.hosts[1].endpoint = "https://studio.ts.net".into();
    push_shared_state(
        &mut ws,
        0,
        SharedHostState::Loaded(SharedHostList {
            hash: "h1".into(),
            hosts: vec![
                host_record(
                    "host:studio",
                    "Studio",
                    &["studio.ts.net"],
                    None,
                    Some("init.lua:2"),
                ),
                host_record("host:bad", "UnreachableBox", &[], None, Some("init.lua:9")),
            ],
            diagnostics: vec![SharedHostDiagnostic {
                severity: "warning".into(),
                message: "rex.host: endpoints is empty".into(),
                source: Some("init.lua:9".into()),
            }],
        }),
        None,
    );

    // Diagnostic notice announced once per distinct set.
    let notice_count = ws
        .notices
        .iter()
        .filter(|n| n.text.contains("shared host warning"))
        .count();
    assert_eq!(notice_count, 1);

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
        texts
            .iter()
            .any(|t| t.contains("rex.host: endpoints is empty")),
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
}
