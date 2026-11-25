//! Headless UI tests for server-side focus history (`client.focus_history.reverse`, API 19+):
//! walk-cursor vs toggle, reset on every interruption, duplicate in-flight suppression,
//! Commands gating, argument validation, shortcuts wiring, and remote/person-only guards.
use super::*;
use crate::app::actions::{self, FOCUS_HISTORY_REVERSE, Remote};
use rex_rune::connection::{
    FOCUS_HISTORY_DISCONNECTED, FOCUS_HISTORY_REQUIRES_API, FocusHistoryResult, FocusHistoryScope,
    HostCommand,
};
use serde_json::{Value, json};

fn focus_workspace() -> (
    Workspace,
    flume::Receiver<HostCommand>,
    flume::Receiver<HostCommand>,
) {
    let mut ws = workspace_hosts(2);
    let rx0 = capture_host(&mut ws, 0);
    let rx1 = capture_host(&mut ws, 1);
    let model = &mut ws.hosts[0].model;
    model.connected = true;
    model.connection_epoch = 7;
    model.protocol.api_revision = 21;
    model.sessions = vec![session("s"), session("t")];
    model.selected = Some("s".into());
    model.views.insert(
        "s".into(),
        serde_json::from_value(json!({
            "revision": 1,
            "session_id": "s",
            "label": "s-label",
            "active_window_id": "split",
            "windows": [
                {
                    "window_id": "single",
                    "label": "server",
                    "focused_block_id": "block:a",
                    "layers": [{
                        "kind": "tiled",
                        "layout": {"block_id": "block:a"},
                        "blocks": [{"block_id": "block:a"}]
                    }]
                },
                {
                    "window_id": "split",
                    "label": "dev",
                    "focused_block_id": "block:b",
                    "layers": [{
                        "kind": "tiled",
                        "layout": {
                            "split": {
                                "direction": "horizontal",
                                "ratio": 0.5,
                                "before": {"block_id": "block:b"},
                                "after": {"block_id": "block:c"}
                            }
                        },
                        "blocks": [{"block_id": "block:b"}, {"block_id": "block:c"}]
                    }]
                }
            ]
        }))
        .unwrap(),
    );
    ws.hosts[1].model.connected = true;
    ws.hosts[1].model.connection_epoch = 3;
    ws.hosts[1].model.protocol.api_revision = 21;
    ws.hosts[1].model.sessions = vec![session("u")];
    ws.hosts[1].model.selected = Some("u".into());
    (ws, rx0, rx1)
}

fn next_reverse(
    rx: &flume::Receiver<HostCommand>,
) -> Option<(u64, u64, FocusHistoryScope, Option<u64>)> {
    rx.try_iter().find_map(|cmd| match cmd {
        HostCommand::ReverseFocusHistory {
            epoch,
            id,
            scope,
            cursor,
            ..
        } => Some((epoch, id, scope, cursor)),
        _ => None,
    })
}

fn all_reverses(
    rx: &flume::Receiver<HostCommand>,
) -> Vec<(u64, u64, FocusHistoryScope, Option<u64>)> {
    rx.try_iter()
        .filter_map(|cmd| match cmd {
            HostCommand::ReverseFocusHistory {
                epoch,
                id,
                scope,
                cursor,
                ..
            } => Some((epoch, id, scope, cursor)),
            _ => None,
        })
        .collect()
}

fn deliver_result(ws: &mut Workspace, host: usize, result: FocusHistoryResult) {
    let mut model = ws.hosts[host].model.clone();
    model.focus_history_result = Some(result);
    *ws.hosts[host].connection.model.lock().unwrap() = model;
    ws.sync(host, &mut rune::Tx::default());
}

fn trigger(ws: &mut Workspace, args: Value) {
    assert!(actions::run(
        ws,
        FOCUS_HISTORY_REVERSE,
        &args,
        &mut rune::Tx::default()
    ));
}

#[test]
fn consecutive_presses_walk_with_cursor_and_ignore_duplicate_while_in_flight() {
    let (mut ws, rx0, _) = focus_workspace();

    // 1st press sends cursor: None.
    trigger(&mut ws, Value::Null);
    let (epoch1, id1, scope1, cur1) = next_reverse(&rx0).expect("first reverse");
    assert_eq!((epoch1, scope1, cur1), (7, FocusHistoryScope::Any, None));

    // Rapid duplicate press while id1 is still in flight sends nothing.
    trigger(&mut ws, Value::Null);
    assert!(
        all_reverses(&rx0).is_empty(),
        "in-flight duplicate must not send"
    );

    // Server returns cursor 41 -> immediate 2nd press sends cursor: Some(41).
    deliver_result(
        &mut ws,
        0,
        FocusHistoryResult {
            epoch: 7,
            id: id1,
            reversed: true,
            cursor: Some(41),
            session_id: Some("s".into()),
            window_id: Some("split".into()),
            block_id: Some("block:c".into()),
        },
    );
    trigger(&mut ws, json!({}));
    let (epoch2, id2, scope2, cur2) = next_reverse(&rx0).expect("second reverse");
    assert_eq!(
        (epoch2, scope2, cur2),
        (7, FocusHistoryScope::Any, Some(41))
    );
    assert_ne!(id2, id1);

    // Server returns cursor 40 -> immediate 3rd press sends cursor: Some(40).
    deliver_result(
        &mut ws,
        0,
        FocusHistoryResult {
            epoch: 7,
            id: id2,
            reversed: true,
            cursor: Some(40),
            session_id: Some("s".into()),
            window_id: Some("single".into()),
            block_id: Some("block:a".into()),
        },
    );
    trigger(&mut ws, json!({"scope": "any"}));
    let (epoch3, id3, scope3, cur3) = next_reverse(&rx0).expect("third reverse");
    assert_eq!(
        (epoch3, scope3, cur3),
        (7, FocusHistoryScope::Any, Some(40))
    );

    // reversed: false clears walk state so next press starts fresh with cursor: None.
    deliver_result(&mut ws, 0, FocusHistoryResult::not_reversed(7, id3));
    assert!(ws.focus_history.walk.is_none());
    trigger(&mut ws, Value::Null);
    let (_, _, _, cur4) = next_reverse(&rx0).expect("fresh reverse after end");
    assert_eq!(cur4, None);
}

#[test]
fn walk_cursor_resets_on_every_user_interruption() {
    let seed_cursor = |ws: &mut Workspace, rx: &flume::Receiver<HostCommand>| {
        trigger(ws, Value::Null);
        let (_, id, _, cur) = next_reverse(rx).expect("seed reverse");
        assert_eq!(cur, None);
        deliver_result(
            ws,
            0,
            FocusHistoryResult {
                epoch: 7,
                id,
                reversed: true,
                cursor: Some(55),
                session_id: Some("s".into()),
                window_id: Some("split".into()),
                block_id: Some("block:c".into()),
            },
        );
        assert_eq!(
            ws.focus_history.walk.as_ref().and_then(|w| w.cursor),
            Some(55)
        );
    };

    // 1. Typing a key to the terminal resets the walk cursor.
    {
        let (mut ws, rx0, _) = focus_workspace();
        let (term_tx, term_rx) = flume::unbounded();
        seed_cursor(&mut ws, &rx0);
        live_terminal(&mut ws, "block:b", &term_tx);
        let mut rt = runtime(ws);
        press(&mut rt, Key::Text("a".into()));
        assert!(!sent_keys(&term_rx).is_empty());
        assert!(rt.state.focus_history.walk.is_none());
        rt.update(|s, tx| {
            actions::run(s, FOCUS_HISTORY_REVERSE, &Value::Null, tx);
        });
        assert_eq!(next_reverse(&rx0).unwrap().3, None);
    }

    // 2. IME commit resets the walk cursor.
    {
        let (mut ws, rx0, _) = focus_workspace();
        let (term_tx, term_rx) = flume::unbounded();
        seed_cursor(&mut ws, &rx0);
        live_terminal(&mut ws, "block:b", &term_tx);
        let mut rt = rune::App::new(AppSpec {
            fonts: rune_theme_opencode::FONTS,
            families: crate::app::fonts::TEST_FAMILIES,
            ..AppSpec::new("Rex", (1024., 700.), placeholder, view)
        })
        .init(move || ws)
        .on_ime(crate::app::input::on_ime)
        .headless_runtime((1024., 700.), Env::new(true, 1.));
        rt.on_key = Some(crate::app::input::on_key);
        for _ in 0..4 {
            rt.frame(1. / 60.);
        }
        rt.ime(rune::Ime::Commit("x".into()));
        rt.frame(1. / 60.);
        assert!(term_rx.try_iter().any(|c| matches!(
            c,
            HostCommand::Terminal {
                command: rex_rune::connection::Command::Paste(ref text),
                ..
            } if text == "x"
        )));
        assert!(rt.state.focus_history.walk.is_none());
        rt.update(|s, tx| {
            actions::run(s, FOCUS_HISTORY_REVERSE, &Value::Null, tx);
        });
        assert_eq!(next_reverse(&rx0).unwrap().3, None);
    }

    // 3. Clicking a pane resets the walk cursor.
    {
        let (mut ws, rx0, _) = focus_workspace();
        let (term_tx, _) = flume::unbounded();
        seed_cursor(&mut ws, &rx0);
        live_terminal(&mut ws, "block:b", &term_tx);
        live_terminal(&mut ws, "block:c", &term_tx);
        let mut rt = runtime(ws);
        for _ in 0..30 {
            rt.frame(1. / 60.);
        }
        click(&mut rt, "pane-header:block:c");
        assert!(rt.state.focus_history.walk.is_none());
        rt.update(|s, tx| {
            actions::run(s, FOCUS_HISTORY_REVERSE, &Value::Null, tx);
        });
        assert_eq!(next_reverse(&rx0).unwrap().3, None);
    }

    // 4. Running another action resets the walk cursor.
    {
        let (mut ws, rx0, _) = focus_workspace();
        seed_cursor(&mut ws, &rx0);
        assert!(actions::run(
            &mut ws,
            "client.sidebar.toggle",
            &Value::Null,
            &mut rune::Tx::default()
        ));
        assert!(ws.focus_history.walk.is_none());
        trigger(&mut ws, Value::Null);
        assert_eq!(next_reverse(&rx0).unwrap().3, None);
    }

    // 5. Changing scope resets the walk cursor (sends cursor: None for new scope).
    {
        let (mut ws, rx0, _) = focus_workspace();
        seed_cursor(&mut ws, &rx0);
        trigger(&mut ws, json!({"scope": "tab"}));
        let (_, _, scope, cursor) = next_reverse(&rx0).expect("tab reverse");
        assert_eq!(scope, FocusHistoryScope::Tab);
        assert_eq!(cursor, None);
    }

    // 6. Opening a panel resets the walk cursor.
    {
        let (mut ws, rx0, _) = focus_workspace();
        seed_cursor(&mut ws, &rx0);
        ws.picker(false);
        assert!(ws.focus_history.walk.is_none());
        ws.close_panel(&mut rune::Tx::default());
        trigger(&mut ws, Value::Null);
        assert_eq!(next_reverse(&rx0).unwrap().3, None);
    }

    // 7. Entering copy mode and opening find bar reset the walk cursor.
    {
        let (mut ws, rx0, _) = focus_workspace();
        let (term_tx, _) = flume::unbounded();
        seed_cursor(&mut ws, &rx0);
        super::pane_verb_tests::framed_terminal(&mut ws, "block:b", &term_tx, b"$ ");
        ws.enter_copy_mode(&mut rune::Tx::default());
        assert!(ws.in_copy_mode());
        assert!(ws.focus_history.walk.is_none());
        ws.exit_copy_mode();
        seed_cursor(&mut ws, &rx0);
        super::pane_verb_tests::framed_terminal(&mut ws, "block:b", &term_tx, b"$ ");
        ws.open_search();
        assert!(ws.focus_history.walk.is_none());
        trigger(&mut ws, Value::Null);
        assert_eq!(next_reverse(&rx0).unwrap().3, None);
    }

    // 8. Switching active host resets the walk cursor.
    {
        let (mut ws, rx0, _) = focus_workspace();
        seed_cursor(&mut ws, &rx0);
        ws.activate_host(1, &mut rune::Tx::default());
        assert!(ws.focus_history.walk.is_none());
        ws.activate_host(0, &mut rune::Tx::default());
        trigger(&mut ws, Value::Null);
        assert_eq!(next_reverse(&rx0).unwrap().3, None);
    }

    // 9. Connection epoch change or disconnect resets the walk cursor.
    {
        let (mut ws, rx0, _) = focus_workspace();
        seed_cursor(&mut ws, &rx0);
        let mut model = ws.hosts[0].model.clone();
        model.connection_epoch = 8;
        *ws.hosts[0].connection.model.lock().unwrap() = model;
        ws.sync(0, &mut rune::Tx::default());
        assert!(ws.focus_history.walk.is_none());
        trigger(&mut ws, Value::Null);
        let (epoch, _, _, cur) = next_reverse(&rx0).expect("reverse after reconnect");
        assert_eq!((epoch, cur), (8, None));
    }

    // 10. Dragging a split divider resets the walk cursor.
    {
        let (mut ws, rx0, _) = focus_workspace();
        seed_cursor(&mut ws, &rx0);
        let mut rt = runtime(ws);
        for _ in 0..10 {
            rt.frame(1. / 60.);
        }
        let divider = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref().is_some_and(|k| k.starts_with("divider:")))
            .expect("split divider element")
            .rect;
        let (x, y) = (divider[0] + divider[2] / 2., divider[1] + divider[3] / 2.);
        rt.pointer_down(x, y);
        rt.pointer_move(x + 24., y);
        rt.pointer_up(x + 24., y);
        rt.frame(1. / 60.);
        assert!(rt.state.focus_history.walk.is_none());
        rt.update(|s, tx| {
            actions::run(s, FOCUS_HISTORY_REVERSE, &Value::Null, tx);
        });
        assert_eq!(next_reverse(&rx0).unwrap().3, None);
    }

    // 11. Dropping files onto a pane resets the walk cursor.
    {
        let (mut ws, rx0, _) = focus_workspace();
        let (term_tx, _) = flume::unbounded();
        seed_cursor(&mut ws, &rx0);
        live_terminal(&mut ws, "block:b", &term_tx);
        live_terminal(&mut ws, "block:c", &term_tx);
        ws.drop_files("block:c", &[std::path::PathBuf::from("/tmp/example.txt")]);
        assert!(ws.focus_history.walk.is_none());
        trigger(&mut ws, Value::Null);
        assert_eq!(next_reverse(&rx0).unwrap().3, None);
    }
}

#[test]
fn commands_gating_arguments_remote_policy_and_returned_actions() {
    let (mut ws, rx0, rx1) = focus_workspace();
    let spec = actions::find(FOCUS_HISTORY_REVERSE).unwrap();
    assert!(matches!(
        spec.remote(),
        Remote::Never(msg) if msg.contains("person at this computer")
    ));
    assert!(spec.requires_person());
    assert!(
        ws.shortcut_chords(FOCUS_HISTORY_REVERSE, &Value::Null)
            .is_empty()
    );

    // Valid arguments accepted; invalid arguments rejected before dispatch.
    for valid in [
        Value::Null,
        json!({}),
        json!({"scope": "any"}),
        json!({"scope": "pane"}),
        json!({"scope": "tab"}),
        json!({"scope": "tab_any_session"}),
        json!({"scope": "session"}),
    ] {
        assert!(spec.accepts(&valid), "valid args rejected: {valid}");
    }
    for invalid in [
        json!("pane"),
        json!(123),
        json!({"scope": "window"}),
        json!({"scope": ""}),
        json!({"scope": 1}),
        json!({"scope": "pane", "extra": true}),
        json!({"unknown": "pane"}),
    ] {
        assert!(!spec.accepts(&invalid), "invalid args accepted: {invalid}");
        assert!(!actions::run(
            &mut ws,
            FOCUS_HISTORY_REVERSE,
            &invalid,
            &mut rune::Tx::default()
        ));
    }
    assert!(all_reverses(&rx0).is_empty());

    // API revision gating in Commands and dispatch.
    for (connected, revision, reason) in [
        (false, 21, Some(FOCUS_HISTORY_DISCONNECTED)),
        (true, 10, Some(FOCUS_HISTORY_REQUIRES_API)),
        (true, 14, Some(FOCUS_HISTORY_REQUIRES_API)),
        (true, 18, Some(FOCUS_HISTORY_REQUIRES_API)),
        (true, 19, None),
        (true, 21, None),
    ] {
        ws.hosts[0].model.connected = connected;
        ws.hosts[0].model.protocol.api_revision = revision;
        ws.notices.clear();
        for id in [
            "focus_history.reverse",
            "focus_history.session",
            "focus_history.tab",
            "focus_history.pane",
        ] {
            let row = ws
                .commands()
                .into_iter()
                .find(|c| c.id == id)
                .unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(
                row.entry.enabled,
                reason.is_none(),
                "{id} at {connected}/{revision}"
            );
            if let Some(reason) = reason {
                assert_eq!(row.entry.detail, reason);
            }
        }
        if let Some(reason) = reason {
            trigger(&mut ws, Value::Null);
            assert!(all_reverses(&rx0).is_empty());
            assert_eq!(ws.notices.last().map(|n| n.text.as_str()), Some(reason));
        }
    }

    // Single session / single tab / single pane disable the narrow scoped rows on API 21.
    ws.hosts[0].model.connected = true;
    ws.hosts[0].model.protocol.api_revision = 21;
    ws.hosts[0].model.sessions.truncate(1);
    ws.hosts[0]
        .model
        .views
        .get_mut("s")
        .unwrap()
        .active_window_id = Some("single".into());
    ws.hosts[0]
        .model
        .views
        .get_mut("s")
        .unwrap()
        .windows
        .retain(|w| w.window_id.as_deref() == Some("single"));
    let enabled = |ws: &Workspace, id: &str| {
        ws.commands()
            .into_iter()
            .find(|c| c.id == id)
            .unwrap()
            .entry
            .enabled
    };
    assert!(enabled(&ws, "focus_history.reverse"));
    assert!(!enabled(&ws, "focus_history.session"));
    assert!(!enabled(&ws, "focus_history.tab"));
    assert!(!enabled(&ws, "focus_history.pane"));

    // Returned client action from server cannot trigger focus history.
    let mut model = ws.hosts[0].model.clone();
    model
        .client_actions
        .push(rex_rune::connection::actions::Queued {
            generation: ws.keymap_generation,
            session: model.selected.clone(),
            block: None,
            action: serde_json::from_value(json!({"action": FOCUS_HISTORY_REVERSE})).unwrap(),
        });
    *ws.hosts[0].connection.model.lock().unwrap() = model;
    ws.sync(0, &mut rune::Tx::default());
    assert!(ws.notices.iter().any(|n| {
        n.text
            .contains("only the person at this computer can step through focus history")
    }));
    assert!(all_reverses(&rx0).is_empty());
    assert!(all_reverses(&rx1).is_empty());
}
