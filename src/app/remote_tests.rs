use super::*;
use crate::app::actions::{self, Remote};
use rex_rune::connection::perform::{Outcome, Pending, Request};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub(super) fn pending(action: &str, args: Value) -> Pending {
    Pending { epoch: 0, received: Instant::now(), request: serde_json::from_value::<Request>(json!({
        "call_id":"1", "action":action, "args":args,
        "requester":{"client_id":"cli", "info":{"name":"rex"}, "principal":{"display":"uid 1000"}}
    })).unwrap() }
}
pub(super) fn reply(rx: &flume::Receiver<HostCommand>) -> Outcome {
    let replies: Vec<_> = rx
        .try_iter()
        .filter_map(|c| match c {
            HostCommand::PerformReply { outcome, .. } => Some(outcome),
            _ => None,
        })
        .collect();
    assert_eq!(replies.len(), 1);
    replies.into_iter().next().unwrap()
}
pub(super) fn code(outcome: Outcome, expected: &str) {
    assert!(
        matches!(outcome, Outcome::Error { code, .. } if code == expected),
        "{outcome:?}"
    );
}

#[test]
fn remote_set_is_exact_and_every_denial_is_described() {
    let mut allowed: Vec<_> = actions::SPECS
        .iter()
        .filter(|s| matches!(s.remote(), Remote::View { .. }))
        .map(|s| s.name)
        .collect();
    allowed.sort();
    assert_eq!(
        allowed,
        [
            "client.sidebar.toggle",
            "client.tab.goto",
            "client.tab.next",
            "client.tab.previous",
            "pane.focus",
            "pane.scroll",
            "pane.zoom",
            "session.select"
        ]
    );
    let mut tab_state: Vec<_> = actions::SPECS
        .iter()
        .filter(|s| matches!(s.remote(), Remote::TabState))
        .map(|s| s.name)
        .collect();
    tab_state.sort();
    assert_eq!(tab_state, ["client.tab.mute"]);
    assert!(matches!(
        actions::find("client.ports").unwrap().remote(),
        Remote::Never(_)
    ));
    let declarations = actions::declarations();
    for action in &declarations {
        if matches!(
            actions::find(&action.name).unwrap().remote(),
            Remote::Never(_)
        ) {
            assert!(
                action
                    .description
                    .as_deref()
                    .unwrap()
                    .contains("Not available remotely")
            );
        }
    }
    let mute = declarations
        .iter()
        .find(|a| a.name == "client.tab.mute")
        .expect("client.tab.mute declaration");
    let desc = mute.description.as_deref().expect("mute description");
    assert!(!desc.contains("Not available remotely"), "{desc}");
    assert!(
        desc.contains("window_id") && desc.contains("muted"),
        "{desc}"
    );
    let schema = mute.args.as_ref().expect("mute schema");
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"]["window_id"]["maxLength"], 128);
    assert_eq!(schema["properties"]["session_id"]["maxLength"], 128);
    assert_eq!(schema["properties"]["muted"]["type"], "boolean");
}

#[test]
fn remote_default_off_and_all_hazards_are_denied_without_side_effects() {
    let (mut ws, rx) = tabbed_workspace("single");
    let sidebar = ws.sidebar.is_open();
    for enabled in [false, true] {
        ws.settings.hosts[0].remote_control = enabled;
        for spec in actions::SPECS {
            if enabled && matches!(spec.remote(), Remote::View { .. }) {
                continue;
            }
            ws.remote_perform(0, pending(spec.name, Value::Null), &mut rune::Tx::default());
            let commands: Vec<_> = rx.try_iter().collect();
            assert_eq!(commands.len(), 1, "{} must send only a reply", spec.name);
            let HostCommand::PerformReply { outcome, .. } = commands.into_iter().next().unwrap()
            else {
                panic!()
            };
            let expected = match spec.remote() {
                Remote::Never(_) => "not_supported",
                Remote::TabState if enabled => "bad_request",
                Remote::View { .. } | Remote::TabState => "refused",
            };
            code(outcome, expected);
            assert!(ws.panel.is_none());
            assert_eq!(ws.sidebar.is_open(), sidebar);
            assert!(ws.settings.muted_tabs.is_empty());
        }
    }
    assert_eq!(ws.notices.len(), 1, "off notice once per host per launch");
    assert!(ws.notices[0].text.contains("rex (uid 1000)"));
}

#[test]
fn unknown_and_download_remote_actions_never_dispatch() {
    let (mut ws, rx) = tabbed_workspace("single");
    let sidebar = ws.sidebar.is_open();
    for enabled in [false, true] {
        ws.settings.hosts[0].remote_control = enabled;
        // API 13 adds a server method, not permission to download through client.perform.
        for action in ["com.superlogical.terminal.download_begin", "future.action"] {
            ws.remote_perform(
                0,
                pending(action, json!({"path":"/private/file"})),
                &mut rune::Tx::default(),
            );
            let commands: Vec<_> = rx.try_iter().collect();
            assert_eq!(commands.len(), 1, "must send only a refusal");
            let HostCommand::PerformReply { outcome, .. } = commands.into_iter().next().unwrap()
            else {
                panic!()
            };
            code(outcome, "not_supported");
            assert!(ws.panel.is_none());
            assert_eq!(ws.sidebar.is_open(), sidebar);
        }
    }
}

#[test]
fn remote_guards_and_argument_checks_do_not_dispatch() {
    let (mut ws, rx) = tabbed_workspace("single");
    ws.settings.hosts[0].remote_control = true;
    for (action, args) in [
        ("client.sidebar.toggle", json!({"extra":1})),
        ("pane.focus", json!({"direction":"sideways"})),
        ("pane.scroll", json!({"lines":1.5})),
        ("client.tab.goto", json!({"index":0})),
        ("client.tab.goto", json!({"index":99})),
    ] {
        ws.remote_perform(0, pending(action, args), &mut rune::Tx::default());
        code(reply(&rx), "bad_request");
    }
    let action = "client.tab.next";
    for field in ["session", "block", "typing", "panel", "expired", "epoch"] {
        let mut p = pending(action, Value::Null);
        match field {
            "session" => p.request.session_id = Some("other".into()),
            "block" => p.request.block_id = Some("other".into()),
            "typing" => ws.last_keypress = Some(Instant::now()),
            "panel" => ws.perform(Intent::Settings, &mut rune::Tx::default()),
            "expired" => p.received -= Duration::from_secs(6),
            "epoch" => p.epoch = 1,
            _ => unreachable!(),
        }
        rx.drain();
        ws.remote_perform(0, p, &mut rune::Tx::default());
        if field == "epoch" {
            assert!(rx.is_empty());
        } else {
            code(
                reply(&rx),
                if field == "expired" {
                    "internal"
                } else {
                    "refused"
                },
            );
        }
        ws.last_keypress = None;
        ws.panel = None;
    }
}

#[test]
fn remote_view_actions_use_the_local_handlers_and_announce_requester() {
    for (name, args, method) in [
        ("client.tab.next", Value::Null, "session.focus_next_window"),
        (
            "client.tab.previous",
            Value::Null,
            "session.focus_previous_window",
        ),
        (
            "pane.focus",
            json!({"direction":"right"}),
            "session.focus_direction",
        ),
        ("pane.zoom", Value::Null, "session.zoom_block"),
        ("client.tab.goto", json!({"index":2}), "navigate"),
        ("client.sidebar.toggle", Value::Null, "sidebar"),
    ] {
        let (mut ws, rx) = tabbed_workspace("single");
        ws.settings.hosts[0].remote_control = true;
        ws.remote_perform(0, pending(name, args), &mut rune::Tx::default());
        let mut performed = 0;
        let mut dispatched = method == "sidebar" && ws.sidebar.is_open();
        for command in rx.try_iter() {
            match command {
                HostCommand::PerformReply { outcome, .. } => {
                    assert_eq!(outcome, Outcome::Performed);
                    performed += 1;
                }
                HostCommand::Request { method: m, .. } => dispatched |= m == method,
                HostCommand::Navigate { window, .. } => {
                    dispatched |= method == "navigate" && window.as_deref() == Some("split")
                }
                _ => {}
            }
        }
        assert!(dispatched, "{name}");
        assert_eq!(performed, 1);
        assert_eq!(ws.notices.len(), 1);
        assert!(
            ws.notices[0]
                .text
                .contains(&format!("test-0: rex (uid 1000) performed {name}"))
        );
        assert!(!ws.notices[0].sticky);
    }
}

#[test]
fn remote_scroll_is_local_and_requester_principal_is_display_only() {
    let (mut ws, rx) = tabbed_workspace("single");
    ws.settings.hosts[0].remote_control = true;
    let sender = ws.hosts[0].connection.commands.clone();
    live_terminal(&mut ws, "block:a", &sender);
    for principal in [
        json!({"kind":"anonymous"}),
        json!({"kind":"tailscale","login_name":"agent@example.test"}),
    ] {
        let mut p = pending("pane.scroll", json!({"lines":-10}));
        p.request.requester.principal = principal;
        ws.remote_perform(0, p, &mut rune::Tx::default());
        assert!(matches!(
            rx.recv().unwrap(),
            HostCommand::Terminal {
                command: rex_rune::connection::Command::Scroll(-10),
                ..
            }
        ));
        assert_eq!(reply(&rx), Outcome::Performed);
    }
}

#[test]
fn remote_actions_cannot_take_focus_from_a_search_field() {
    let (mut ws, rx) = tabbed_workspace("single");
    ws.settings.hosts[0].remote_control = true;
    let (sender, terminal) = flume::unbounded();
    live_terminal(&mut ws, "block:a", &sender);
    let key = ws.term_key("block:a");
    let mut rt = runtime(ws);
    ctrl_shift(&mut rt, 'f');
    for c in ["a", "b", "c"] {
        press(&mut rt, Key::Text(c.into()));
    }
    let focus = focused(&rt);
    assert!(focus.as_ref().unwrap().contains("search-field:block:a"));
    // Even after a pause, every remote action must leave the field alone.
    rt.state.last_keypress = Some(Instant::now() - Duration::from_secs(2));
    rx.drain();
    for (action, args) in [
        ("client.tab.next", Value::Null),
        ("client.sidebar.toggle", Value::Null),
        ("pane.scroll", json!({"lines": 1})),
    ] {
        rt.update(|s, tx| s.remote_perform(0, pending(action, args), tx));
        code(reply(&rx), "refused");
        assert_eq!(focused(&rt), focus);
    }
    press(&mut rt, Key::Text("d".into()));
    assert_eq!(
        rt.state.terminals[&key].search.as_ref().unwrap().text,
        "abcd"
    );
    assert!(
        sent_keys(&terminal).is_empty(),
        "no field input reaches a shell"
    );
}

#[test]
fn search_edits_and_consumed_keys_record_activity() {
    let (mut ws, _rx) = tabbed_workspace("single");
    let (sender, _) = flume::unbounded();
    live_terminal(&mut ws, "block:a", &sender);
    let mut rt = runtime(ws);
    ctrl_shift(&mut rt, 'f');
    rt.state.last_keypress = None;
    press(&mut rt, Key::Text("a".into()));
    assert!(rt.state.last_keypress.is_some());
    rt.state.last_keypress = None;
    press(&mut rt, Key::Left);
    assert!(
        rt.state.last_keypress.is_some(),
        "cursor movement is activity too"
    );
    rt.state.last_keypress = None;
    // IME edits the TextInput directly, exercising on_change without search_key.
    rt.ime(rune::Ime::Commit("b".into()));
    rt.frame(1. / 60.);
    assert!(rt.state.last_keypress.is_some());
}

#[test]
fn remote_actions_preserve_pending_key_sequences_even_after_a_pause() {
    let (mut ws, rx) = tabbed_workspace("single");
    ws.settings.hosts[0].remote_control = true;
    ws.local_keymap = actions::resolve(&rex_rune::keymap::Overrides::parse(&json!([
        {"keys": "ctrl+b>c", "action": "client.sidebar.toggle"}
    ])));
    let mut rt = runtime(ws);
    rt.key(KeyEvent {
        key: Key::Command('b'),
        mods: Mods {
            ctrl: true,
            cmd: true,
            ..Mods::default()
        },
    });
    rt.frame(1. / 60.);
    assert_eq!(rt.state.bindings.sequence.depth, 1);
    assert!(
        rt.state.last_keypress.is_some(),
        "consumed prefix counts as activity"
    );
    rt.state.last_keypress = Some(Instant::now() - Duration::from_secs(2));
    rx.drain();
    for action in ["client.tab.next", "client.sidebar.toggle"] {
        rt.update(|s, tx| s.remote_perform(0, pending(action, Value::Null), tx));
        code(reply(&rx), "refused");
        assert_eq!(rt.state.bindings.sequence.depth, 1);
    }
    let sidebar = rt.state.sidebar.is_open();
    rt.state.last_keypress = None;
    press(&mut rt, Key::Text("c".into()));
    assert_eq!(rt.state.bindings.sequence.depth, 0);
    assert_ne!(
        rt.state.sidebar.is_open(),
        sidebar,
        "the original tail still runs"
    );
    assert!(
        rt.state.last_keypress.is_some(),
        "consumed shortcut counts as activity"
    );
}

#[test]
fn remote_notices_evict_oldest_non_sticky_notice_before_warnings() {
    let (mut ws, rx) = tabbed_workspace("single");
    ws.settings.hosts[0].remote_control = true;
    ws.notify("sticky warning");
    ws.inform("old information");
    ws.notify("second warning");
    ws.remote_perform(
        0,
        pending("client.sidebar.toggle", Value::Null),
        &mut rune::Tx::default(),
    );
    assert_eq!(reply(&rx), Outcome::Performed);
    assert_eq!(ws.notices.len(), 3);
    assert_eq!(ws.notices[0].text, "sticky warning");
    assert_eq!(ws.notices[1].text, "second warning");
    assert!(
        ws.notices[2]
            .text
            .contains("performed client.sidebar.toggle")
    );
    ws.notify("third warning");
    ws.inform("new information");
    assert!(
        ws.notices.iter().all(|n| n.sticky),
        "information cannot displace three warnings"
    );
    ws.notify("fourth warning");
    assert_eq!(ws.notices.len(), 3);
    assert_eq!(
        ws.notices[0].text, "second warning",
        "all-sticky overflow evicts the oldest"
    );
}

#[test]
fn client_label_form_trims_and_rejects_empty_without_freezing_the_default() {
    let mut ws = workspace();
    assert!(ws.settings.client_label.is_none());
    let mut tx = rune::Tx::default();
    ws.perform(Intent::ClientLabel, &mut tx);
    assert_eq!(
        ws.panel.as_ref().unwrap().inputs[0].text,
        rex_rune::settings::hostname()
    );
    ws.panel.as_mut().unwrap().inputs[0] = rune::TextEdit::new("  \n  ");
    ws.submit(&mut tx);
    assert!(ws.panel.as_ref().unwrap().error.is_some());
    assert!(ws.settings.client_label.is_none());
    ws.panel.as_mut().unwrap().inputs[0] = rune::TextEdit::new("  My laptop  ");
    ws.submit(&mut tx);
    assert_eq!(ws.settings.client_label.as_deref(), Some("My laptop"));
    assert_eq!(
        Settings::load(&ws.settings_path)
            .unwrap()
            .client_label
            .as_deref(),
        Some("My laptop")
    );
    std::fs::remove_file(&ws.settings_path).unwrap();
}

#[test]
fn remote_control_switch_is_per_host_and_survives_save() {
    let mut ws = workspace_hosts(2);
    let rx = capture_host(&mut ws, 1);
    ws.hosts[1].model.connected = true;
    ws.settings.hosts[1].remote_control = true;
    ws.remote_perform(
        1,
        pending("client.sidebar.toggle", Value::Null),
        &mut rune::Tx::default(),
    );
    code(reply(&rx), "refused");
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.settings_section = crate::app::panel::SettingsSection::Hosts;
    let mut rt = runtime(ws);
    assert!(has(&rt, "remote-control:0"));
    assert!(has(&rt, "remote-control:1"));
    // The root eases to the window's size; the switches sit at the dialog's trailing edge.
    for _ in 0..60 {
        rt.frame(1. / 60.);
    }
    click(&mut rt, "remote-control:0");
    assert!(Settings::load(&rt.state.settings_path).unwrap().hosts[0].remote_control);
    let endpoint = rt.state.settings.hosts[0].endpoint.clone();
    rt.state.edit_host(0, "renamed".into(), endpoint, None);
    assert!(rt.state.settings.hosts[0].remote_control);
    rt.state.edit_host(
        0,
        "changed".into(),
        "unix:///nonexistent/changed.sock".into(),
        None,
    );
    assert!(!rt.state.settings.hosts[0].remote_control);
    assert!(rt.state.settings.hosts[1].remote_control);
}

fn two_host_mute_workspace() -> (
    Workspace,
    flume::Receiver<HostCommand>,
    flume::Receiver<HostCommand>,
) {
    let mut ws = workspace_hosts(2);
    let rx0 = capture_host(&mut ws, 0);
    let rx1 = capture_host(&mut ws, 1);
    for (host, prefix) in [(0, "h0"), (1, "h1")] {
        let model = &mut ws.hosts[host].model;
        model.connected = true;
        model.sessions = vec![
            session(&format!("{prefix}-s")),
            session(&format!("{prefix}-bg")),
        ];
        model.selected = Some(format!("{prefix}-s"));
        model.views.insert(
            format!("{prefix}-s"),
            serde_json::from_value(json!({
                "revision": 1,
                "session_id": format!("{prefix}-s"),
                "active_window_id": format!("{prefix}-main"),
                "windows": [
                    {
                        "window_id": format!("{prefix}-main"),
                        "label": "main",
                        "focused_block_id": format!("{prefix}-b-main"),
                        "layers": [{"kind": "tiled", "layout": {"block_id": format!("{prefix}-b-main")},
                            "blocks": [{"block_id": format!("{prefix}-b-main")}]}]
                    },
                    {
                        "window_id": format!("{prefix}-worker"),
                        "label": "worker",
                        "focused_block_id": format!("{prefix}-b-worker"),
                        "layers": [{"kind": "tiled", "layout": {"block_id": format!("{prefix}-b-worker")},
                            "blocks": [{"block_id": format!("{prefix}-b-worker")}]}]
                    }
                ]
            }))
            .expect("selected view"),
        );
        model.views.insert(
            format!("{prefix}-bg"),
            serde_json::from_value(json!({
                "revision": 1,
                "session_id": format!("{prefix}-bg"),
                "active_window_id": format!("{prefix}-bg-main"),
                "windows": [
                    {
                        "window_id": format!("{prefix}-bg-main"),
                        "label": "bg-main",
                        "focused_block_id": format!("{prefix}-b-bg-main"),
                        "layers": [{"kind": "tiled", "layout": {"block_id": format!("{prefix}-b-bg-main")},
                            "blocks": [{"block_id": format!("{prefix}-b-bg-main")}]}]
                    },
                    {
                        "window_id": format!("{prefix}-bg-worker"),
                        "label": "bg-worker",
                        "focused_block_id": format!("{prefix}-b-bg-worker"),
                        "layers": [{"kind": "tiled", "layout": {"block_id": format!("{prefix}-b-bg-worker")},
                            "blocks": [{"block_id": format!("{prefix}-b-bg-worker")}]}]
                    }
                ]
            }))
            .expect("background view"),
        );
        model.background_views.insert(format!("{prefix}-bg"));
    }
    ws.settings.hosts[0].remote_control = true;
    ws.settings.hosts[1].remote_control = true;
    (ws, rx0, rx1)
}

#[test]
fn remote_tab_mute_works_unattended_across_panels_search_sequences_typing_and_offscreen_host() {
    let (mut ws, rx0, rx1) = two_host_mute_workspace();
    let (sender, _) = flume::unbounded();
    live_terminal(&mut ws, "h0-b-main", &sender);
    ws.local_keymap = actions::resolve(&rex_rune::keymap::Overrides::parse(&json!([
        {"keys": "ctrl+b>c", "action": "client.sidebar.toggle"}
    ])));
    let mut rt = runtime(ws);

    // 1. Focused search field + active keypress: host 1 (off-screen) mutes its own tab.
    ctrl_shift(&mut rt, 'f');
    press(&mut rt, Key::Text("a".into()));
    let search_focus = focused(&rt);
    assert!(
        search_focus
            .as_ref()
            .unwrap()
            .contains("search-field:h0-b-main")
    );
    rt.state.last_keypress = Some(Instant::now());
    rx1.drain();
    rt.update(|s, tx| {
        s.remote_perform(
            1,
            pending(
                "client.tab.mute",
                json!({"window_id": "h1-worker", "muted": true}),
            ),
            tx,
        );
    });
    assert_eq!(reply(&rx1), Outcome::Performed);
    assert!(rt.state.tab_muted(1, "h1-worker"));
    assert_eq!(rt.state.active, 0, "active host did not change");
    assert_eq!(rt.state.hosts[0].model.selected.as_deref(), Some("h0-s"));
    assert_eq!(
        rt.state.hosts[0]
            .model
            .window()
            .unwrap()
            .window_id
            .as_deref(),
        Some("h0-main")
    );
    assert_eq!(focused(&rt), search_focus, "search field kept focus");
    assert!(
        rt.state
            .notices
            .iter()
            .any(|n| n.text == "test-1: rex (uid 1000) muted \"worker\""),
        "{:?}",
        rt.state.notices.iter().map(|n| &n.text).collect::<Vec<_>>()
    );

    // 2. Settings panel open: host 0 mutes without closing the panel.
    press(&mut rt, Key::Escape);
    rt.update(|s, tx| s.perform(Intent::Settings, tx));
    assert!(rt.state.panel.is_some());
    rx0.drain();
    rt.update(|s, tx| {
        s.remote_perform(
            0,
            pending(
                "client.tab.mute",
                json!({"window_id": "h0-worker", "muted": true}),
            ),
            tx,
        );
    });
    assert_eq!(reply(&rx0), Outcome::Performed);
    assert!(rt.state.tab_muted(0, "h0-worker"));
    assert!(rt.state.panel.is_some(), "panel stayed open");

    // 3. Pending key sequence: remote mute succeeds and leaves the sequence intact.
    press(&mut rt, Key::Escape);
    assert!(rt.state.panel.is_none());
    rt.key(KeyEvent {
        key: Key::Command('b'),
        mods: Mods {
            ctrl: true,
            cmd: true,
            ..Mods::default()
        },
    });
    rt.frame(1. / 60.);
    assert_eq!(rt.state.bindings.sequence.depth, 1);
    rx0.drain();
    rt.update(|s, tx| {
        s.remote_perform(
            0,
            pending(
                "client.tab.mute",
                json!({"window_id": "h0-worker", "muted": false}),
            ),
            tx,
        );
    });
    assert_eq!(reply(&rx0), Outcome::Performed);
    assert!(!rt.state.tab_muted(0, "h0-worker"));
    assert_eq!(
        rt.state.bindings.sequence.depth, 1,
        "pending sequence untouched"
    );
    let sidebar = rt.state.sidebar.is_open();
    press(&mut rt, Key::Text("c".into()));
    assert_eq!(rt.state.bindings.sequence.depth, 0);
    assert_ne!(
        rt.state.sidebar.is_open(),
        sidebar,
        "the pending sequence tail still runs"
    );
}

#[test]
fn remote_tab_mute_in_background_views_suppresses_activity_without_attaching_session() {
    use rex_rune::connection::activity::ProgramState::*;
    let (mut ws, rx0, _rx1) = two_host_mute_workspace();
    ws.track_activity(0);
    // Remote mute worker tab inside background session h0-bg.
    ws.remote_perform(
        0,
        pending(
            "client.tab.mute",
            json!({"window_id": "h0-bg-worker", "muted": true}),
        ),
        &mut rune::Tx::default(),
    );
    let commands: Vec<_> = rx0.try_iter().collect();
    assert_eq!(commands.len(), 1, "must not send Navigate or Select");
    assert!(matches!(
        commands[0],
        HostCommand::PerformReply {
            outcome: Outcome::Performed,
            ..
        }
    ));
    assert_eq!(ws.hosts[0].model.selected.as_deref(), Some("h0-s"));
    assert!(ws.hosts[0].model.background_views.contains("h0-bg"));
    assert!(ws.tab_muted(0, "h0-bg-worker"));

    // Activity arrives on both bg-worker (muted) and bg-main (unmuted).
    set_activity(&mut ws, "h0-b-bg-worker", &[("", record(Blocked, true))]);
    set_activity(&mut ws, "h0-b-bg-main", &[("", record(Error, true))]);

    // Session word reflects only the unmuted main tab, not the higher-priority muted worker.
    let (word, _, tab) = ws.session_word(0, "h0-bg").expect("bg-main status visible");
    assert_eq!(word, "Error");
    assert_eq!(tab, "bg-main");

    // Activity chip counts only bg-main (1 error), never the muted Blocked worker.
    let chip = ws.activity_chip().expect("chip shows unmuted error");
    assert!(!chip.needs_you);
    assert_eq!(chip.count, 1);

    // Activity list includes bg-main and omits bg-worker.
    let entries = ws.activity_entries("");
    let titles: Vec<_> = entries.iter().map(|e| e.label.as_str()).collect();
    assert_eq!(titles, ["bg-main"]);

    // Desktop notifications only fire for bg-main, never for muted bg-worker.
    let notified: Vec<_> = ws
        .outbox
        .iter()
        .filter_map(|n| n.target.as_ref().and_then(|t| t.window.as_deref()))
        .collect();
    assert_eq!(notified, ["h0-bg-main"]);
}

#[test]
fn remote_tab_mute_enforces_args_host_isolation_envelope_lifecycle_and_idempotency() {
    let (mut ws, rx0, rx1) = two_host_mute_workspace();
    // Explicit window_id and muted are both required remotely.
    for bad in [
        Value::Null,
        json!({}),
        json!({"window_id": "h0-worker"}),
        json!({"muted": true}),
        json!({"session_id": "h0-s", "muted": true}),
        json!({"window_id": "   ", "muted": true}),
        json!({"window_id": "a".repeat(129), "muted": true}),
        json!({"window_id": "h0-worker", "muted": "yes"}),
        json!({"window_id": "h0-worker", "muted": true, "extra": 1}),
    ] {
        ws.remote_perform(
            0,
            pending("client.tab.mute", bad.clone()),
            &mut rune::Tx::default(),
        );
        code(reply(&rx0), "bad_request");
    }

    // Unknown window or unknown session -> not_found.
    for missing in [
        json!({"window_id": "no-such-tab", "muted": true}),
        json!({"session_id": "no-such-session", "window_id": "h0-worker", "muted": true}),
        json!({"session_id": "h0-bg", "window_id": "h0-worker", "muted": true}),
    ] {
        ws.remote_perform(
            0,
            pending("client.tab.mute", missing),
            &mut rune::Tx::default(),
        );
        code(reply(&rx0), "not_found");
    }

    // Cross-host isolation: host 1 cannot mute or probe host 0's window ID.
    ws.remote_perform(
        1,
        pending(
            "client.tab.mute",
            json!({"window_id": "h0-worker", "muted": true}),
        ),
        &mut rune::Tx::default(),
    );
    code(reply(&rx1), "not_found");
    assert!(!ws.tab_muted(0, "h0-worker"));

    // Envelope confinement: mismatched session_id or block_id -> refused.
    let mut bad_session = pending(
        "client.tab.mute",
        json!({"window_id": "h0-worker", "muted": true}),
    );
    bad_session.request.session_id = Some("h0-bg".into());
    ws.remote_perform(0, bad_session, &mut rune::Tx::default());
    code(reply(&rx0), "refused");
    assert!(!ws.tab_muted(0, "h0-worker"));

    let mut bad_block = pending(
        "client.tab.mute",
        json!({"window_id": "h0-worker", "muted": true}),
    );
    bad_block.request.block_id = Some("h0-b-main".into());
    ws.remote_perform(0, bad_block, &mut rune::Tx::default());
    code(reply(&rx0), "refused");
    assert!(!ws.tab_muted(0, "h0-worker"));

    // Matching envelope session_id + block_id succeeds.
    let mut good = pending(
        "client.tab.mute",
        json!({"window_id": "h0-worker", "muted": true}),
    );
    good.request.session_id = Some("h0-s".into());
    good.request.block_id = Some("h0-b-worker".into());
    ws.remote_perform(0, good, &mut rune::Tx::default());
    assert_eq!(reply(&rx0), Outcome::Performed);
    assert!(ws.tab_muted(0, "h0-worker"));
    assert_eq!(ws.notices.len(), 1);
    assert_eq!(
        ws.notices[0].text,
        "test-0: rex (uid 1000) muted \"worker\""
    );

    // Idempotent repeat: replies Performed, skips redundant notice and skips rewriting disk.
    ws.notices.clear();
    std::fs::remove_file(&ws.settings_path).ok();
    ws.remote_perform(
        0,
        pending(
            "client.tab.mute",
            json!({"window_id": "h0-worker", "muted": true}),
        ),
        &mut rune::Tx::default(),
    );
    assert_eq!(reply(&rx0), Outcome::Performed);
    assert!(ws.notices.is_empty(), "idempotent repeat emits no notice");
    assert!(
        !ws.settings_path.exists(),
        "idempotent repeat skips redundant save"
    );

    // Unmute changes state, persists, and names the tab in notice.
    ws.remote_perform(
        0,
        pending(
            "client.tab.mute",
            json!({"window_id": "h0-worker", "muted": false}),
        ),
        &mut rune::Tx::default(),
    );
    assert_eq!(reply(&rx0), Outcome::Performed);
    assert!(!ws.tab_muted(0, "h0-worker"));
    assert_eq!(ws.notices.len(), 1);
    assert_eq!(
        ws.notices[0].text,
        "test-0: rex (uid 1000) unmuted \"worker\""
    );
    assert!(ws.settings_path.exists());

    // Stale epoch drops silently; expired and disconnected return internal.
    let mut stale = pending(
        "client.tab.mute",
        json!({"window_id": "h0-worker", "muted": true}),
    );
    stale.epoch = 99;
    ws.remote_perform(0, stale, &mut rune::Tx::default());
    assert!(rx0.is_empty());

    let mut expired = pending(
        "client.tab.mute",
        json!({"window_id": "h0-worker", "muted": true}),
    );
    expired.received -= Duration::from_secs(6);
    ws.remote_perform(0, expired, &mut rune::Tx::default());
    code(reply(&rx0), "internal");

    ws.hosts[0].model.connected = false;
    ws.remote_perform(
        0,
        pending(
            "client.tab.mute",
            json!({"window_id": "h0-worker", "muted": true}),
        ),
        &mut rune::Tx::default(),
    );
    code(reply(&rx0), "internal");
}

#[test]
fn remote_tab_mute_fetches_unloaded_listed_session_and_rejects_missing_or_disconnected() {
    use rex_rune::connection::ViewFetchOutcome;

    // 1. Success path: unloaded listed session triggers FetchView, then mutes on arrival.
    let (mut ws, rx0, _rx1) = two_host_mute_workspace();
    ws.hosts[0].model.sessions.push(session("h0-new"));
    ws.remote_perform(
        0,
        pending(
            "client.tab.mute",
            json!({"session_id": "h0-new", "window_id": "w-new", "muted": true}),
        ),
        &mut rune::Tx::default(),
    );
    let commands: Vec<_> = rx0.try_iter().collect();
    assert_eq!(commands.len(), 1);
    let HostCommand::FetchView { id, session: s, .. } = &commands[0] else {
        panic!("expected FetchView");
    };
    let fetch_id = *id;
    assert_eq!(s, "h0-new");
    assert!(!ws.poll_remote_mutes(Instant::now()));

    ws.hosts[0].model.views.insert(
        "h0-new".into(),
        serde_json::from_value(json!({
            "revision": 1,
            "session_id": "h0-new",
            "active_window_id": "w-new",
            "windows": [{
                "window_id": "w-new",
                "label": "fresh-worker",
                "focused_block_id": "b-new",
                "layers": [{"kind": "tiled", "layout": {"block_id": "b-new"},
                    "blocks": [{"block_id": "b-new"}]}]
            }]
        }))
        .unwrap(),
    );
    ws.hosts[0].model.background_views.insert("h0-new".into());
    ws.hosts[0]
        .model
        .view_fetches
        .push((fetch_id, ViewFetchOutcome::Loaded));
    assert!(ws.poll_remote_mutes(Instant::now()));
    assert_eq!(reply(&rx0), Outcome::Performed);
    assert!(ws.tab_muted(0, "w-new"));
    assert_eq!(ws.hosts[0].model.selected.as_deref(), Some("h0-s"));

    // 2. Bogus window_id in unloaded listed session -> not_found after view loads (zero false positives).
    let (mut ws, rx0, _rx1) = two_host_mute_workspace();
    ws.hosts[0].model.sessions.push(session("h0-new"));
    ws.remote_perform(
        0,
        pending(
            "client.tab.mute",
            json!({"session_id": "h0-new", "window_id": "bogus", "muted": true}),
        ),
        &mut rune::Tx::default(),
    );
    let HostCommand::FetchView { id, .. } = rx0.try_recv().unwrap() else {
        panic!("expected FetchView");
    };
    ws.hosts[0].model.views.insert(
        "h0-new".into(),
        serde_json::from_value(json!({"revision": 1, "windows": []})).unwrap(),
    );
    ws.hosts[0]
        .model
        .view_fetches
        .push((id, ViewFetchOutcome::Loaded));
    assert!(ws.poll_remote_mutes(Instant::now()));
    code(reply(&rx0), "not_found");
    assert!(!ws.tab_muted(0, "bogus"));

    // 3. Disconnect drops pending fetch silently without replying across reconnect.
    let (mut ws, rx0, _rx1) = two_host_mute_workspace();
    ws.hosts[0].model.sessions.push(session("h0-new"));
    ws.remote_perform(
        0,
        pending(
            "client.tab.mute",
            json!({"session_id": "h0-new", "window_id": "w-new", "muted": true}),
        ),
        &mut rune::Tx::default(),
    );
    rx0.drain();
    ws.hosts[0].model.connected = false;
    assert!(ws.poll_remote_mutes(Instant::now()));
    assert!(rx0.is_empty());

    // 4. Timeout returns internal.
    let (mut ws, rx0, _rx1) = two_host_mute_workspace();
    ws.hosts[0].model.sessions.push(session("h0-new"));
    ws.remote_perform(
        0,
        pending(
            "client.tab.mute",
            json!({"session_id": "h0-new", "window_id": "w-new", "muted": true}),
        ),
        &mut rune::Tx::default(),
    );
    rx0.drain();
    assert!(ws.poll_remote_mutes(Instant::now() + Duration::from_secs(6)));
    code(reply(&rx0), "internal");
}

#[test]
fn remote_tab_mute_post_fetch_rechecks_consent_oracle_concurrency_and_host_lifecycle() {
    use rex_rune::connection::ViewFetchOutcome;

    let unloaded_view = |session_id: &str, window_id: &str, block_id: &str| {
        serde_json::from_value(json!({
            "revision": 1,
            "session_id": session_id,
            "active_window_id": window_id,
            "windows": [{
                "window_id": window_id,
                "label": window_id,
                "focused_block_id": block_id,
                "layers": [{"kind": "tiled", "layout": {"block_id": block_id},
                    "blocks": [{"block_id": block_id}]}]
            }]
        }))
        .unwrap()
    };

    // (a) Toggling Remote control off while FetchView is in flight refuses immediately.
    let (mut ws, rx0, _rx1) = two_host_mute_workspace();
    ws.hosts[0].model.sessions.push(session("h0-new"));
    ws.remote_perform(
        0,
        pending(
            "client.tab.mute",
            json!({"session_id": "h0-new", "window_id": "w-new", "muted": true}),
        ),
        &mut rune::Tx::default(),
    );
    rx0.drain();
    ws.toggle_remote_control(0);
    code(reply(&rx0), "refused");
    assert!(ws.settings.muted_tabs.is_empty());
    assert!(ws.remote_mutes.is_empty());

    // (b) Two concurrent FetchView completions (first Failed, second Loaded) arriving
    // before sync() both resolve deterministically without overwriting.
    let (mut ws, rx0, _rx1) = two_host_mute_workspace();
    ws.hosts[0].model.sessions.push(session("h0-fail"));
    ws.hosts[0].model.sessions.push(session("h0-ok"));
    let mut p1 = pending(
        "client.tab.mute",
        json!({"session_id": "h0-fail", "window_id": "w-fail", "muted": true}),
    );
    p1.request.call_id = "call-fail".into();
    let mut p2 = pending(
        "client.tab.mute",
        json!({"session_id": "h0-ok", "window_id": "w-ok", "muted": true}),
    );
    p2.request.call_id = "call-ok".into();
    ws.remote_perform(0, p1, &mut rune::Tx::default());
    ws.remote_perform(0, p2, &mut rune::Tx::default());
    let fetches: Vec<u64> = rx0
        .try_iter()
        .filter_map(|c| match c {
            HostCommand::FetchView { id, .. } => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(fetches.len(), 2);
    {
        let mut shared = ws.hosts[0].connection.model.lock().unwrap();
        *shared = ws.hosts[0].model.clone();
        shared
            .views
            .insert("h0-ok".into(), unloaded_view("h0-ok", "w-ok", "b-ok"));
        shared.background_views.insert("h0-ok".into());
        shared
            .view_fetches
            .push((fetches[0], ViewFetchOutcome::Failed));
        shared
            .view_fetches
            .push((fetches[1], ViewFetchOutcome::Loaded));
    }
    ws.sync(0, &mut rune::Tx::default());
    let replies: Vec<(String, Outcome)> = rx0
        .try_iter()
        .filter_map(|c| match c {
            HostCommand::PerformReply {
                call_id, outcome, ..
            } => Some((call_id, outcome)),
            _ => None,
        })
        .collect();
    assert_eq!(replies.len(), 2, "{replies:?}");
    assert_eq!(replies[0].0, "call-fail");
    code(replies[0].1.clone(), "internal");
    assert_eq!(replies[1], ("call-ok".into(), Outcome::Performed));
    assert!(ws.tab_muted(0, "w-ok") && !ws.tab_muted(0, "w-fail"));
    assert!(
        ws.hosts[0]
            .connection
            .model
            .lock()
            .unwrap()
            .view_fetches
            .is_empty(),
        "sync drains shared.view_fetches"
    );

    // (c) Mismatched envelope session_id with nonexistent args.session_id -> refused
    // (no cross-session existence oracle).
    let (mut ws, rx0, _rx1) = two_host_mute_workspace();
    let mut oracle = pending(
        "client.tab.mute",
        json!({"session_id": "does-not-exist", "window_id": "h0-worker", "muted": true}),
    );
    oracle.request.session_id = Some("h0-s".into());
    ws.remote_perform(0, oracle, &mut rune::Tx::default());
    code(reply(&rx0), "refused");

    // (d) Post-fetch envelope block_id mismatch -> refused; post-fetch envelope session_id
    // mismatch (window actually lives in another session while env_session was unloaded) -> refused.
    let (mut ws, rx0, _rx1) = two_host_mute_workspace();
    ws.hosts[0].model.sessions.push(session("h0-new"));
    let mut bad_block = pending(
        "client.tab.mute",
        json!({"session_id": "h0-new", "window_id": "w-new", "muted": true}),
    );
    bad_block.request.block_id = Some("wrong-block".into());
    ws.remote_perform(0, bad_block, &mut rune::Tx::default());
    let HostCommand::FetchView { id, .. } = rx0.try_recv().unwrap() else {
        panic!("expected FetchView");
    };
    ws.hosts[0]
        .model
        .views
        .insert("h0-new".into(), unloaded_view("h0-new", "w-new", "b-new"));
    ws.hosts[0]
        .model
        .view_fetches
        .push((id, ViewFetchOutcome::Loaded));
    assert!(ws.poll_remote_mutes(Instant::now()));
    code(reply(&rx0), "refused");
    assert!(!ws.tab_muted(0, "w-new"));

    let (mut ws, rx0, _rx1) = two_host_mute_workspace();
    ws.hosts[0].model.sessions.push(session("h0-unloaded"));
    let mut env_mismatch = pending(
        "client.tab.mute",
        json!({"window_id": "w-later", "muted": true}),
    );
    env_mismatch.request.session_id = Some("h0-unloaded".into());
    ws.remote_perform(0, env_mismatch, &mut rune::Tx::default());
    let HostCommand::FetchView { id, .. } = rx0.try_recv().unwrap() else {
        panic!("expected FetchView");
    };
    // Suppose w-later actually appeared in h0-s while h0-unloaded loaded empty.
    ws.hosts[0].model.views.insert(
        "h0-unloaded".into(),
        serde_json::from_value(json!({"revision": 1, "windows": []})).unwrap(),
    );
    ws.hosts[0]
        .model
        .views
        .get_mut("h0-s")
        .unwrap()
        .windows
        .push(
            serde_json::from_value(json!({
                "window_id": "w-later",
                "label": "w-later",
                "focused_block_id": "b-later",
                "layers": [{"kind": "tiled", "layout": {"block_id": "b-later"},
                    "blocks": [{"block_id": "b-later"}]}]
            }))
            .unwrap(),
        );
    ws.hosts[0]
        .model
        .view_fetches
        .push((id, ViewFetchOutcome::Loaded));
    assert!(ws.poll_remote_mutes(Instant::now()));
    code(reply(&rx0), "refused");
    assert!(!ws.tab_muted(0, "w-later"));

    // (e) ViewFetchOutcome::Failed (session listed -> internal) vs ViewFetchOutcome::NotFound
    // (or session removed -> not_found).
    for (outcome, drop_session, expected) in [
        (ViewFetchOutcome::Failed, false, "internal"),
        (ViewFetchOutcome::NotFound, false, "not_found"),
        (ViewFetchOutcome::Failed, true, "not_found"),
    ] {
        let (mut ws, rx0, _rx1) = two_host_mute_workspace();
        ws.hosts[0].model.sessions.push(session("h0-new"));
        ws.remote_perform(
            0,
            pending(
                "client.tab.mute",
                json!({"session_id": "h0-new", "window_id": "w-new", "muted": true}),
            ),
            &mut rune::Tx::default(),
        );
        let HostCommand::FetchView { id, .. } = rx0.try_recv().unwrap() else {
            panic!("expected FetchView");
        };
        if drop_session {
            ws.hosts[0]
                .model
                .sessions
                .retain(|s| s.session_id != "h0-new");
        }
        ws.hosts[0].model.view_fetches.push((id, outcome));
        assert!(ws.poll_remote_mutes(Instant::now()));
        code(reply(&rx0), expected);
    }

    // (f1) Host removed mid-fetch -> dropped cleanly by HostId without panic or reply.
    let (mut ws, rx0, rx1) = two_host_mute_workspace();
    ws.hosts[1].model.sessions.push(session("h1-new"));
    ws.remote_perform(
        1,
        pending(
            "client.tab.mute",
            json!({"session_id": "h1-new", "window_id": "w-new", "muted": true}),
        ),
        &mut rune::Tx::default(),
    );
    rx1.drain();
    ws.hosts.remove(1);
    ws.settings.hosts.remove(1);
    assert!(ws.poll_remote_mutes(Instant::now()));
    assert!(ws.remote_mutes.is_empty());
    assert!(rx0.is_empty());

    // (f2) Host reordered mid-fetch -> still replies on the original host's channel and mutes it.
    let (mut ws, rx0, rx1) = two_host_mute_workspace();
    ws.hosts[1].model.sessions.push(session("h1-new"));
    ws.remote_perform(
        1,
        pending(
            "client.tab.mute",
            json!({"session_id": "h1-new", "window_id": "w-new", "muted": true}),
        ),
        &mut rune::Tx::default(),
    );
    let HostCommand::FetchView { id, .. } = rx1.try_recv().unwrap() else {
        panic!("expected FetchView on host 1");
    };
    ws.hosts.swap(0, 1);
    ws.settings.hosts.swap(0, 1);
    // Original host 1 is now at index 0.
    ws.hosts[0]
        .model
        .views
        .insert("h1-new".into(), unloaded_view("h1-new", "w-new", "b-new"));
    ws.hosts[0]
        .model
        .view_fetches
        .push((id, ViewFetchOutcome::Loaded));
    assert!(ws.poll_remote_mutes(Instant::now()));
    assert_eq!(reply(&rx1), Outcome::Performed);
    assert!(rx0.is_empty());
    assert!(ws.tab_muted(0, "w-new"));
    assert!(!ws.tab_muted(1, "w-new"));
}
