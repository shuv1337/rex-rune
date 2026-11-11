//! The actions that fill out the registry: Move pane, Resize pane, Close other panes, Go to
//! session and its numbers in the switcher, and the default keys for these, for tabs by
//! number and for the next and previous pane.
use super::pane_verb_tests::{Camera, framed_terminal};
use super::*;
use crate::app::{
    actions::{self, Remote, When},
    pane_moves, session_numbers, shortcuts_page,
};
use rex_linux::keymap::Overrides;
use serde_json::{Value, json};

fn mods(ctrl: bool, alt: bool, shift: bool) -> Mods {
    Mods {
        ctrl,
        alt,
        shift,
        ..Mods::default()
    }
}

/// Press `key` with `mods` in the window, as a keyboard would.
fn chord(rt: &mut Runtime<Workspace>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    for _ in 0..3 {
        rt.frame(1. / 60.);
    }
}

fn digit(n: u8) -> Key {
    Key::Text(n.to_string())
}

fn run(ws: &mut Workspace, action: &str, args: Value) {
    assert!(
        actions::run(ws, action, &args, &mut rune::Tx::default()),
        "{action} {args}"
    );
}

/// The sessions a host was asked to show, in order.
fn shown(rx: &flume::Receiver<HostCommand>) -> Vec<String> {
    rx.try_iter()
        .filter_map(|c| match c {
            HostCommand::Navigate { session, .. } => Some(session),
            _ => None,
        })
        .collect()
}

/// The split tab on screen with both panes live: `block:b` (focused, on the left) and
/// `block:c`. The second receiver has what the panes' terminals were sent.
fn split() -> (
    Runtime<Workspace>,
    flume::Receiver<HostCommand>,
    flume::Receiver<HostCommand>,
) {
    let (mut ws, host) = tabbed_workspace("split");
    let (sender, sent) = flume::unbounded();
    framed_terminal(&mut ws, "block:b", &sender, b"$ vim notes.md");
    framed_terminal(&mut ws, "block:c", &sender, b"$ tail -f server.log");
    (runtime(ws), host, sent)
}

/// Two connected hosts: test-0 with sessions s (on screen), t and v; test-1 with u.
fn hosts() -> (
    Workspace,
    flume::Receiver<HostCommand>,
    flume::Receiver<HostCommand>,
) {
    let mut ws = workspace_hosts(2);
    let first = capture_host(&mut ws, 0);
    let second = capture_host(&mut ws, 1);
    let model = &mut ws.hosts[0].model;
    model.connected = true;
    model.sessions = vec![session("s"), session("t"), session("v")];
    model.selected = Some("s".into());
    ws.hosts[1].model.connected = true;
    ws.hosts[1].model.sessions = vec![session("u")];
    (ws, first, second)
}

#[test]
fn every_new_action_is_registered_never_remote_and_checks_its_arguments() {
    for name in [
        pane_moves::MOVE,
        pane_moves::RESIZE,
        pane_moves::CLOSE_OTHERS,
        session_numbers::GOTO,
    ] {
        let spec = actions::find(name).unwrap_or_else(|| panic!("{name} is registered"));
        assert!(
            matches!(spec.remote(), Remote::Never(reason) if !reason.is_empty()),
            "{name} is never remote"
        );
        assert!(!spec.global(), "{name} does not run behind a dialog");
    }
    let accepts = |name: &str, args: Value| actions::find(name).unwrap().accepts(&args);
    for name in [pane_moves::MOVE, pane_moves::RESIZE] {
        for side in ["left", "right", "up", "down"] {
            assert!(accepts(name, json!({"direction": side})), "{name} {side}");
        }
        assert!(!accepts(name, Value::Null));
        assert!(!accepts(name, json!({})));
        assert!(!accepts(name, json!({"direction": "best_fit"})));
        assert!(!accepts(name, json!({"direction": "left", "by": 2})));
    }
    assert!(actions::find(pane_moves::RESIZE).unwrap().repeats());
    assert!(!actions::find(pane_moves::MOVE).unwrap().repeats());
    assert!(accepts(pane_moves::CLOSE_OTHERS, Value::Null));
    assert!(!accepts(pane_moves::CLOSE_OTHERS, json!({"all": true})));

    let goto = session_numbers::GOTO;
    assert!(accepts(goto, json!({"index": 1})));
    assert!(accepts(goto, json!({"index": -1})));
    assert!(accepts(goto, json!({"index": 3, "when": "switching"})));
    assert!(!accepts(goto, Value::Null));
    assert!(!accepts(goto, json!({"index": 0})));
    assert!(!accepts(goto, json!({"index": -2})));
    assert!(!accepts(goto, json!({"when": "switching"})));
    assert!(!accepts(goto, json!({"index": 1, "when": "searching"})));
    assert!(!accepts(goto, json!({"index": 1, "session_id": "s"})));

    // Find next and Find previous take the one condition and nothing else.
    for name in [crate::app::find::NEXT, crate::app::find::PREVIOUS] {
        assert!(accepts(name, json!({"when": "searching"})));
        assert!(!accepts(name, json!({"when": "switching"})));
        assert!(!accepts(name, json!({"when": "always"})));
    }
    // No other action takes a condition.
    assert!(!accepts("pane.zoom", json!({"when": "searching"})));
    assert_eq!(
        actions::when(&json!({"when": "searching"})),
        Some(When::Searching)
    );
    assert_eq!(actions::when(&json!({"index": 1})), None);
    assert_eq!(
        actions::without_when(&json!({"index": 1, "when": "switching"})),
        json!({"index": 1})
    );
    assert_eq!(
        actions::without_when(&json!({"when": "searching"})),
        Value::Null
    );

    // What is declared to the host says the same.
    let declared = actions::declarations();
    let schema = |name: &str| {
        declared
            .iter()
            .find(|action| action.name == name)
            .and_then(|action| action.args.clone())
            .unwrap_or(Value::Null)
    };
    assert_eq!(
        schema(pane_moves::MOVE)["properties"]["direction"]["enum"],
        json!(["left", "right", "up", "down"])
    );
    assert_eq!(schema(goto)["required"], json!(["index"]));
    assert_eq!(
        schema(goto)["properties"]["when"]["enum"],
        json!(["switching"])
    );
    assert_eq!(schema(pane_moves::CLOSE_OTHERS), Value::Null);
}

#[test]
fn the_new_actions_round_trip_through_shortcuts_json() {
    let file = json!([
        {"keys": "ctrl+b>h", "action": "pane.move", "args": {"direction": "left"}},
        {"keys": "ctrl+b>shift+l", "action": "pane.resize", "args": {"direction": "right"}},
        {"keys": "ctrl+b>o", "action": "pane.close_others"},
        {"keys": "ctrl+b>3", "action": "session.goto", "args": {"index": 3}},
        {"keys": "alt+3", "action": "session.goto", "args": {"index": 3, "when": "switching"}},
        {"keys": "ctrl+g", "action": "client.search.next"},
        {"keys": "n", "action": "client.search.next", "args": {"when": "searching"}},
        // Not a condition, and not an action that takes one.
        {"keys": "ctrl+b>x", "action": "client.search.next", "args": {"when": "idle"}},
        {"keys": "ctrl+b>y", "action": "pane.move", "args": {"direction": "left", "when": "searching"}},
    ]);
    let overrides = Overrides::parse(&file);
    assert!(overrides.diagnostics.is_empty(), "the file itself is read");
    let map = actions::resolve(&overrides);
    let problems: Vec<_> = map
        .diagnostics
        .iter()
        .map(|d| (d.key.clone().unwrap_or_default(), d.message.clone()))
        .collect();
    assert_eq!(
        problems,
        [
            (
                "ctrl+b y".to_owned(),
                "invalid arguments for \"pane.move\"".to_owned()
            ),
            (
                "ctrl+b x".to_owned(),
                "invalid arguments for \"client.search.next\"".to_owned()
            ),
        ]
    );
    let effective = actions::effective(&map, &[]);
    let runs = |keys: &[&str]| {
        effective
            .iter()
            .find(|b| b.binding.keys == keys)
            .map(|b| (b.binding.action.as_str(), b.binding.args.clone()))
    };
    assert_eq!(
        runs(&["ctrl+b", "h"]),
        Some(("pane.move", Some(json!({"direction": "left"}))))
    );
    assert_eq!(
        runs(&["ctrl+b", "shift+l"]),
        Some(("pane.resize", Some(json!({"direction": "right"}))))
    );
    assert_eq!(runs(&["ctrl+b", "o"]), Some(("pane.close_others", None)));
    assert_eq!(
        runs(&["alt+3"]),
        Some((
            "session.goto",
            Some(json!({"index": 3, "when": "switching"}))
        ))
    );
    assert_eq!(runs(&["ctrl+g"]), Some(("client.search.next", None)));
    assert_eq!(
        runs(&["ctrl+b", "x"]),
        None,
        "an invalid binding never runs"
    );
    // The layer sent to every host is the file's, argument for argument.
    let wire = serde_json::to_value(&overrides.layer).unwrap();
    assert_eq!(
        wire["bindings"][4]["args"],
        json!({"index": 3, "when": "switching"})
    );
    assert_eq!(
        serde_json::from_value::<rexrs::api::client::KeymapDefault>(wire).unwrap(),
        overrides.layer
    );
}

#[test]
fn move_pane_trades_places_with_the_pane_on_that_side() {
    let (mut rt, host, sent) = split();
    // The key, and the program gets none of it.
    chord(&mut rt, Key::Right, mods(true, true, true));
    let asked = requests(&host);
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].0, "session.swap_blocks");
    assert_eq!(
        asked[0].2,
        json!({"session_id": "s", "block_id": "block:b", "other_block_id": "block:c"})
    );
    assert!(sent_keys(&sent).is_empty());

    // Nothing lies left of, above or under the left pane: nothing is asked.
    for (key, side) in [(Key::Left, "left"), (Key::Up, "up"), (Key::Down, "down")] {
        chord(&mut rt, key, mods(true, true, true));
        rt.update(move |s, _| run(s, pane_moves::MOVE, json!({"direction": side})));
    }
    assert!(requests(&host).is_empty());
    assert!(sent_keys(&sent).is_empty(), "and the keys stay bound");

    // Commands offers the side that has a pane.
    rt.update(|s, _| s.picker(false));
    let rows: Vec<(String, bool, String)> = rt
        .state
        .entries()
        .into_iter()
        .filter(|e| e.label.starts_with("Move pane "))
        .map(|e| (e.label, e.enabled, e.detail))
        .collect();
    assert_eq!(
        rows,
        [
            (
                "Move pane left".to_owned(),
                false,
                "Ctrl Alt Shift Left".to_owned()
            ),
            (
                "Move pane right".to_owned(),
                true,
                "Ctrl Alt Shift Right".to_owned()
            ),
            (
                "Move pane up".to_owned(),
                false,
                "Ctrl Alt Shift Up".to_owned()
            ),
            (
                "Move pane down".to_owned(),
                false,
                "Ctrl Alt Shift Down".to_owned()
            ),
        ]
    );
    rt.update(|s, tx| {
        let row = s
            .entries()
            .into_iter()
            .find(|e| e.label == "Move pane right")
            .unwrap();
        s.perform(row.action, tx);
    });
    assert!(rt.state.panel.is_none(), "the list closes");
    assert_eq!(requests(&host)[0].0, "session.swap_blocks");

    // A zoomed pane has no pane beside it; an offline host is asked nothing.
    rt.update(|s, _| {
        let view = s.hosts[0].model.views.get_mut("s").unwrap();
        view.windows[1].zoomed_block_id = Some("block:b".into());
        run(s, pane_moves::MOVE, json!({"direction": "right"}));
        run(s, pane_moves::RESIZE, json!({"direction": "right"}));
    });
    assert!(requests(&host).is_empty());
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn resize_pane_steps_a_border_of_the_focused_pane_through_the_server() {
    let (mut ws, host) = tabbed_workspace("split");
    let resize = |ws: &mut Workspace, side: &str| {
        run(ws, pane_moves::RESIZE, json!({"direction": side}));
        requests(&host)
    };
    let asked = resize(&mut ws, "right");
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].0, "session.resize_pane");
    assert_eq!(
        asked[0].2,
        json!({"session_id": "s", "direction": "horizontal", "before_block_id": "block:b",
            "after_block_id": "block:c", "ratio": 0.55})
    );
    assert_eq!(resize(&mut ws, "left")[0].2["ratio"], 0.45);
    // Until the server reports a new layout, a repeat asks for the same ratio, never more.
    assert_eq!(resize(&mut ws, "left")[0].2["ratio"], 0.45);
    // A side-by-side split has no border above or under its panes.
    assert!(resize(&mut ws, "up").is_empty());
    assert!(resize(&mut ws, "down").is_empty());

    // The right pane's only border is the one on its left; it moves the way the key says.
    ws.hosts[0].model.views.get_mut("s").unwrap().windows[1].focused_block_id =
        Some("block:c".into());
    let asked = resize(&mut ws, "left");
    assert_eq!(asked[0].2["before_block_id"], "block:b");
    assert_eq!(asked[0].2["ratio"], 0.45);

    // The only pane of a tab has no border: Commands has the rows, unavailable.
    let (mut ws, host) = tabbed_workspace("single");
    ws.picker(false);
    let rows: Vec<_> = ws
        .entries()
        .into_iter()
        .filter(|e| e.label.starts_with("Resize pane "))
        .map(|e| (e.enabled, e.detail))
        .collect();
    assert_eq!(rows, vec![(false, String::new()); 4], "unbound by default");
    run(&mut ws, pane_moves::RESIZE, json!({"direction": "left"}));
    assert!(requests(&host).is_empty());
    std::fs::remove_file(&ws.settings_path).ok();
}

#[test]
fn close_other_panes_asks_first_and_closes_only_the_panes_it_named() {
    let (mut ws, host) = tabbed_workspace("split");
    let mut tx = rune::Tx::default();
    run(&mut ws, pane_moves::CLOSE_OTHERS, Value::Null);
    let panel = ws.panel.as_ref().expect("a confirmation");
    let PanelKind::Confirm {
        method,
        payload,
        description,
    } = &panel.kind
    else {
        panic!("a confirmation, not {:?}", panel.kind);
    };
    assert_eq!(panel.title, "Close the other pane in dev?");
    assert_eq!(method, "block.close");
    assert_eq!(payload["block_ids"], json!(["block:c"]));
    assert_eq!(
        description,
        "Ends the program in the other pane on test-0; Terminal stays open. Closing the app itself keeps programs running."
    );
    assert!(
        requests(&host).is_empty(),
        "nothing closes before the answer"
    );
    ws.submit(&mut tx);
    let asked = requests(&host);
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].0, "block.close");
    assert_eq!(
        asked[0].2,
        json!({"session_id": "s", "block_id": "block:c"})
    );
    assert!(ws.panel.is_none());

    // Escape keeps every pane.
    run(&mut ws, pane_moves::CLOSE_OTHERS, Value::Null);
    ws.cancel_panel(&mut tx);
    assert!(requests(&host).is_empty());

    // A pane that closed while the question was open is not asked for again, and a pane
    // opened meanwhile is not closed.
    run(&mut ws, pane_moves::CLOSE_OTHERS, Value::Null);
    let view = ws.hosts[0].model.views.get_mut("s").unwrap();
    view.windows[1].layers[0].blocks[1].block_id = Some("block:new".into());
    ws.submit(&mut tx);
    assert!(requests(&host).is_empty());

    // With the pane that was to stay gone, nothing closes.
    run(&mut ws, pane_moves::CLOSE_OTHERS, Value::Null);
    let view = ws.hosts[0].model.views.get_mut("s").unwrap();
    view.windows[1].layers[0].blocks.remove(0);
    view.windows[1].focused_block_id = Some("block:new".into());
    ws.submit(&mut tx);
    assert!(requests(&host).is_empty());
    assert_eq!(
        ws.notices.last().map(|n| n.text.as_str()),
        Some("That pane is no longer available.")
    );

    // A tab with one pane has no others: the row waits and the action asks nothing.
    let (mut ws, host) = tabbed_workspace("single");
    ws.picker(false);
    let row = ws
        .entries()
        .into_iter()
        .find(|e| e.label == "Close other panes…")
        .expect("a Commands row");
    assert!(!row.enabled && row.danger);
    ws.cancel_panel(&mut tx);
    run(&mut ws, pane_moves::CLOSE_OTHERS, Value::Null);
    assert!(ws.panel.is_none());
    assert!(requests(&host).is_empty());
    std::fs::remove_file(&ws.settings_path).ok();
}

#[test]
fn three_other_panes_are_counted_in_the_question() {
    let (mut ws, _host) = tabbed_workspace("split");
    let view = ws.hosts[0].model.views.get_mut("s").unwrap();
    let blocks = &mut view.windows[1].layers[0].blocks;
    let mut more = blocks[1].clone();
    more.block_id = Some("block:d".into());
    blocks.push(more);
    run(&mut ws, pane_moves::CLOSE_OTHERS, Value::Null);
    let panel = ws.panel.as_ref().unwrap();
    assert_eq!(panel.title, "Close 2 other panes in dev?");
    let PanelKind::Confirm {
        payload,
        description,
        ..
    } = &panel.kind
    else {
        panic!()
    };
    assert_eq!(payload["block_ids"], json!(["block:c", "block:d"]));
    assert!(description.starts_with("Ends the program in each of the 2 other panes on test-0;"));
    std::fs::remove_file(&ws.settings_path).ok();
}

#[test]
fn go_to_session_counts_sessions_as_the_switcher_lists_them() {
    let (mut ws, first, second) = hosts();
    let goto = |ws: &mut Workspace, index: i64| {
        run(ws, session_numbers::GOTO, json!({"index": index}));
    };
    goto(&mut ws, 2);
    assert_eq!(shown(&first), ["t"]);
    // The fourth session is the second host's first.
    goto(&mut ws, 4);
    assert_eq!(shown(&second), ["u"]);
    assert_eq!(ws.active, 1);
    goto(&mut ws, 3);
    assert_eq!(shown(&first), ["v"]);
    goto(&mut ws, -1);
    assert_eq!(shown(&second), ["u"], "the last");
    // A number past the end goes nowhere.
    goto(&mut ws, 5);
    assert!(shown(&first).is_empty() && shown(&second).is_empty());
    std::fs::remove_file(&ws.settings_path).ok();
}

/// The switcher's session rows as (label, hint).
fn switcher_rows(rt: &Runtime<Workspace>) -> Vec<(String, String)> {
    rt.state
        .entries()
        .into_iter()
        .filter(|e| matches!(e.action, Intent::Select(..)))
        .map(|e| (e.label, e.detail))
        .collect()
}

#[test]
fn the_switcher_numbers_its_rows_and_ctrl_with_a_number_picks_one() {
    let (mut ws, first, second) = hosts();
    let (sender, sent) = flume::unbounded();
    live_terminal(&mut ws, "block:a", &sender);
    ws.hosts[0].model.views.insert(
        "s".into(),
        serde_json::from_value(json!({
            "revision": 1, "session_id": "s", "active_window_id": "w",
            "windows": [{"window_id": "w", "focused_block_id": "block:a",
                "layers": [{"kind": "tiled", "layout": {"block_id": "block:a"},
                    "blocks": [{"block_id": "block:a"}]}]}]
        }))
        .unwrap(),
    );
    let mut rt = runtime(ws);

    // With the switcher closed the key is the program's.
    chord(&mut rt, digit(2), mods(true, false, false));
    assert_eq!(sent_keys(&sent).len(), 1, "Ctrl 2 reached the terminal");
    assert!(shown(&first).is_empty());

    rt.update(|s, _| s.picker(true));
    rt.frame(1. / 60.);
    assert_eq!(
        switcher_rows(&rt),
        [
            ("s-label".to_owned(), "Ctrl 1".to_owned()),
            ("t-label".to_owned(), "Ctrl 2".to_owned()),
            ("v-label".to_owned(), "Ctrl 3".to_owned()),
            ("u-label".to_owned(), "Ctrl 4".to_owned()),
        ]
    );
    // The number is drawn as keycaps at the row's end; the highlighted row has its buttons
    // there instead.
    assert!(has(&rt, "entry:1:keys"));
    chord(&mut rt, digit(4), mods(true, false, false));
    assert!(
        rt.state.panel.is_none(),
        "picking a row closes the switcher"
    );
    assert_eq!(shown(&second), ["u"]);
    assert!(sent_keys(&sent).is_empty(), "and the program gets no key");

    // A filter renumbers what is left, and a number typed without Ctrl is part of it.
    rt.update(|s, _| s.picker(true));
    rt.frame(1. / 60.);
    for c in ["l", "a", "b"] {
        press(&mut rt, Key::Text(c.into()));
    }
    press(&mut rt, digit(1));
    assert_eq!(rt.state.query(), "lab1");
    press(&mut rt, Key::Backspace);
    press(&mut rt, Key::Backspace);
    press(&mut rt, Key::Backspace);
    press(&mut rt, Key::Backspace);
    press(&mut rt, Key::Text("v".into()));
    assert_eq!(
        switcher_rows(&rt),
        [("v-label".to_owned(), "Ctrl 1".to_owned())]
    );
    chord(&mut rt, digit(1), mods(true, false, false));
    assert_eq!(shown(&first), ["v"]);
    assert!(rt.state.panel.is_none());

    // A number with no row leaves the switcher open; renaming a row takes no numbers.
    rt.update(|s, _| s.picker(true));
    rt.frame(1. / 60.);
    chord(&mut rt, digit(9), mods(true, false, false));
    assert!(rt.state.panel.is_some());
    rt.update(|s, _| s.start_rename(0, "t".into()));
    rt.frame(1. / 60.);
    assert!(
        switcher_rows(&rt).iter().all(|(_, hint)| hint.is_empty()),
        "no numbers while a row is a field"
    );
    chord(&mut rt, digit(2), mods(true, false, false));
    assert!(rt.state.panel.is_some());
    assert!(shown(&first).is_empty() && shown(&second).is_empty());
    assert!(sent_keys(&sent).is_empty());
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_rebound_or_removed_session_number_shows_on_its_row() {
    let (mut ws, first, _second) = hosts();
    ws.settings.shortcut_file.overrides = Overrides::parse(&json!([
        {"keys": "ctrl+1", "action": null},
        {"keys": "alt+2", "action": "session.goto", "args": {"index": 2}},
        {"keys": "ctrl+e", "action": "session.goto", "args": {"index": -1, "when": "switching"}},
    ]));
    ws.local_keymap = actions::resolve(&ws.settings.shortcut_file.overrides);
    ws.keymap_generation += 1;
    let mut rt = runtime(ws);
    rt.update(|s, _| s.picker(true));
    rt.frame(1. / 60.);
    assert_eq!(
        switcher_rows(&rt),
        [
            ("s-label".to_owned(), String::new()),
            ("t-label".to_owned(), "Alt 2".to_owned()),
            ("v-label".to_owned(), "Ctrl 3".to_owned()),
            // Its own number first; the key for the last session would show on a fifth row.
            ("u-label".to_owned(), "Ctrl 4".to_owned()),
        ]
    );
    // An unconditional binding also runs with the switcher closed.
    press(&mut rt, Key::Escape);
    chord(&mut rt, digit(2), mods(false, true, false));
    assert_eq!(shown(&first), ["t"]);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn ctrl_alt_numbers_go_to_tabs_and_brackets_to_the_next_and_previous_pane() {
    let (mut rt, host, sent) = split();
    let window = |call: &(String, String, Value)| {
        assert_eq!(call.0, "session.focus_window");
        call.2["window_id"].as_str().unwrap().to_owned()
    };
    chord(&mut rt, digit(1), mods(true, true, false));
    assert_eq!(window(&requests(&host)[0]), "single");
    chord(&mut rt, digit(2), mods(true, true, false));
    assert_eq!(window(&requests(&host)[0]), "split");
    // Nine is the last tab however many there are; a number with no tab does nothing.
    chord(&mut rt, digit(9), mods(true, true, false));
    assert_eq!(window(&requests(&host)[0]), "split");
    chord(&mut rt, digit(5), mods(true, true, false));
    assert!(requests(&host).is_empty());

    chord(&mut rt, Key::Text("]".into()), mods(true, true, false));
    chord(&mut rt, Key::Text("[".into()), mods(true, true, false));
    let asked: Vec<String> = requests(&host).into_iter().map(|call| call.0).collect();
    assert_eq!(
        asked,
        ["session.focus_next_pane", "session.focus_previous_pane"]
    );
    assert!(
        sent_keys(&sent).is_empty(),
        "none of these keys reached the program"
    );

    // Commands shows the brackets on the focus rows.
    rt.update(|s, _| s.picker(false));
    let hint = |label: &str| {
        rt.state
            .entries()
            .into_iter()
            .find(|e| e.label == label)
            .map(|e| e.detail)
    };
    assert_eq!(hint("Focus next pane").as_deref(), Some("Ctrl Alt ]"));
    assert_eq!(hint("Focus previous pane").as_deref(), Some("Ctrl Alt ["));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn settings_lists_the_new_actions_with_their_keys_and_when_they_run() {
    let map = actions::resolve(&Overrides::default());
    let rows = shortcuts_page::rows(&map, &[], &[]);
    let row = |title: &str| {
        let found: Vec<_> = rows.iter().filter(|row| row.title == title).collect();
        assert_eq!(found.len(), 1, "one row titled {title:?}");
        found[0]
    };
    let caps = |keys: &[&str]| {
        vec![vec![
            keys.iter().map(|k| (*k).to_owned()).collect::<Vec<_>>(),
        ]]
    };
    for (side, key) in [
        ("left", "Left"),
        ("right", "Right"),
        ("up", "Up"),
        ("down", "Down"),
    ] {
        let moved = row(&format!("Move pane {side}"));
        assert_eq!(moved.keys, caps(&["Ctrl", "Alt", "Shift", key]));
        assert_eq!(moved.category, "Panes");
        let resized = row(&format!("Resize pane {side}"));
        assert!(resized.keys.is_empty() && resized.notes.is_empty());
    }
    assert!(row("Close other panes").keys.is_empty());
    assert_eq!(row("Focus next pane").keys, caps(&["Ctrl", "Alt", "]"]));
    assert_eq!(row("Focus previous pane").keys, caps(&["Ctrl", "Alt", "["]));

    // A key that waits is on its action's one row, which says when it runs.
    let next = row("Find next");
    assert_eq!(next.keys, caps(&["F3"]));
    assert_eq!(next.notes, ["Only while the find bar is open"]);
    let previous = row("Find previous");
    assert_eq!(previous.keys, caps(&["Shift", "F3"]));
    let sessions: Vec<_> = rows
        .iter()
        .filter(|row| row.action == session_numbers::GOTO)
        .map(|row| (row.title.as_str(), row.keys.clone(), row.notes.clone()))
        .collect();
    assert_eq!(sessions.len(), 9);
    assert_eq!(
        sessions[0],
        (
            "Go to session 1",
            caps(&["Ctrl", "1"]),
            vec!["Only in the session switcher".to_owned()]
        )
    );
    assert_eq!(sessions[8].0, "Go to session 9");

    // With a key of the user's beside the default, each says which waits.
    let map = actions::resolve(&Overrides::parse(&json!([
        {"keys": "ctrl+g", "action": "client.search.next"},
        {"keys": "alt+1", "action": "session.goto", "args": {"index": -1}},
    ])));
    let rows = shortcuts_page::rows(&map, &[], &[]);
    let next = rows.iter().find(|row| row.title == "Find next").unwrap();
    assert_eq!(next.keys.len(), 2);
    assert_eq!(
        next.notes,
        [
            "Ctrl G from shortcuts.json",
            "F3 only while the find bar is open"
        ]
    );
    let last = rows
        .iter()
        .rfind(|row| row.action == session_numbers::GOTO)
        .unwrap();
    assert_eq!(last.title, "Go to last session");
    assert_eq!(last.notes, ["shortcuts.json"]);
}

/// The switcher with its numbers, Commands' Pane group and the Close other panes question,
/// as drawn (see `Camera`).
#[test]
fn the_switcher_numbers_and_the_pane_commands_draw() {
    let (mut ws, _first, _second) = hosts();
    let (sender, _sent) = flume::unbounded();
    ws.hosts[0].model.views.insert(
        "s".into(),
        serde_json::from_value(json!({
            "revision": 1, "session_id": "s", "label": "s-label", "active_window_id": "split",
            "windows": [
                {"window_id": "split", "label": "dev", "focused_block_id": "block:b",
                 "layers": [{"kind": "tiled", "layout": {"split": {"direction": "horizontal",
                    "ratio": 0.5, "before": {"block_id": "block:b"},
                    "after": {"split": {"direction": "vertical", "ratio": 0.5,
                        "before": {"block_id": "block:c"}, "after": {"block_id": "block:d"}}}}},
                    "blocks": [{"block_id": "block:b", "label": "editor"},
                        {"block_id": "block:c", "label": "logs"},
                        {"block_id": "block:d", "label": "tests"}]}]}
            ]
        }))
        .unwrap(),
    );
    framed_terminal(&mut ws, "block:b", &sender, b"$ vim notes.md");
    framed_terminal(&mut ws, "block:c", &sender, b"$ tail -f server.log");
    framed_terminal(&mut ws, "block:d", &sender, b"$ cargo test");
    let mut rt = Runtime::new(ws, view, (1024., 700.), Env::new(true, 2.));
    rt.on_key = Some(crate::app::input::on_key);
    let settle = |rt: &mut Runtime<Workspace>| {
        for _ in 0..90 {
            rt.frame(1. / 60.);
        }
    };
    settle(&mut rt);
    let mut camera = Camera::new();
    let mut snap = |rt: &mut Runtime<Workspace>, name: &str| {
        if let Some(camera) = &mut camera {
            camera.snap(rt, name);
        }
    };

    rt.update(|s, _| s.picker(true));
    settle(&mut rt);
    snap(&mut rt, "switcher-numbers");
    // Every row but the highlighted one ends with its keycaps, inside the list.
    let surface = super::pane_verb_tests::rect(&rt, "menu-surface");
    for row in 1..4 {
        let keys = super::pane_verb_tests::rect(&rt, &format!("entry:{row}:keys"));
        assert!(
            keys[0] >= surface[0] && keys[0] + keys[2] <= surface[0] + surface[2],
            "row {row}'s keys are inside the list: {keys:?} in {surface:?}"
        );
    }
    press(&mut rt, Key::Escape);

    rt.update(|s, _| s.picker(false));
    settle(&mut rt);
    for c in "pane".chars() {
        press(&mut rt, Key::Text(c.to_string()));
    }
    settle(&mut rt);
    snap(&mut rt, "commands-pane-search");
    press(&mut rt, Key::Escape);

    rt.update(|s, tx| assert!(actions::run(s, pane_moves::CLOSE_OTHERS, &Value::Null, tx)));
    settle(&mut rt);
    snap(&mut rt, "close-other-panes");
    assert_eq!(
        rt.state.panel.as_ref().unwrap().title,
        "Close 2 other panes in dev?"
    );
    // The panes that would close are outlined; the one that stays is not.
    assert!(has(&rt, "target-outline:block:c") && has(&rt, "target-outline:block:d"));
    assert!(!has(&rt, "target-outline") && !has(&rt, "target-outline:block:b"));
    press(&mut rt, Key::Escape);

    // Settings → Shortcuts: a key that waits says when on its action's row.
    rt.update(|s, tx| {
        s.perform(Intent::Settings, tx);
        s.open_section(crate::app::panel::SettingsSection::Shortcuts);
    });
    settle(&mut rt);
    click(&mut rt, "shortcut-filter");
    for c in "session".chars() {
        press(&mut rt, Key::Text(c.to_string()));
    }
    settle(&mut rt);
    snap(&mut rt, "shortcuts-sessions");
    for _ in 0..7 {
        press(&mut rt, Key::Backspace);
    }
    for c in "pane".chars() {
        press(&mut rt, Key::Text(c.to_string()));
    }
    settle(&mut rt);
    snap(&mut rt, "shortcuts-panes");
    for _ in 0..4 {
        press(&mut rt, Key::Backspace);
    }
    for c in "f3".chars() {
        press(&mut rt, Key::Text(c.to_string()));
    }
    settle(&mut rt);
    snap(&mut rt, "shortcuts-f3");
    std::fs::remove_file(&rt.state.settings_path).ok();
}
