//! Issue #129: the active key mode is named for as long as it lasts, and no longer.
use super::pane_verb_tests::{framed_terminal, rect};
use super::*;
use crate::app::actions;
use rex_rune::keymap::{Overrides, Resolved};
use rexrs::api::client::{Binding, KeymapResponse, Mode};
use rune::{PhysicalInput, PhysicalKeyCode as Code};
use serde_json::json;
use std::sync::Arc;

const LONG: &str = "a-mode-name-too-long-for-a-narrow-toolbar";

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..90 {
        rt.frame(1. / 60.);
    }
}

/// The host's keymap with modes to nest, a once mode, an ordinary key and a long name.
fn with_modes(ws: &mut Workspace, exclusive: bool) {
    let reply = KeymapResponse {
        bindings: vec![
            Binding::new(["ctrl+r"], "client.mode.enter").args(json!({"name":"resize"})),
            Binding::new(["ctrl+l"], "client.mode.enter").args(json!({"name":LONG})),
            Binding::new(["left"], "pane.resize")
                .args(json!({"direction":"left","amount":1}))
                .mode("resize"),
            Binding::new(["q"], "client.mode.exit_all").mode("resize"),
            Binding::new(["ctrl+shift+c"], "pane.copy").mode("resize"),
            Binding::new(["ctrl+shift+c"], "client.tab.new"),
            Binding::new(["n"], "client.mode.enter")
                .args(json!({"name":"nested"}))
                .mode("resize"),
            Binding::new(["a"], "client.mode.enter")
                .args(json!({"name":"once","once":true}))
                .mode("resize"),
            Binding::new(["x"], "client.mode.exit").mode("nested"),
            Binding::new(["right"], "pane.resize")
                .args(json!({"direction":"right","amount":1}))
                .mode("once"),
            Binding::new(["ctrl+b", "c"], "pane.balance"),
            Binding::new(["z"], "pane.close"),
        ],
        modes: vec![
            Mode {
                name: "resize".into(),
                exclusive,
                blocked: vec![Some(vec!["z".into()])],
            },
            Mode {
                name: "nested".into(),
                ..Default::default()
            },
            Mode {
                name: "once".into(),
                ..Default::default()
            },
            Mode {
                name: LONG.into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    ws.hosts[0].model.keymap = Some(Arc::new(rex_rune::connection::keymap::Snapshot {
        generation: ws.keymap_generation,
        resolved: Resolved::from_server(&reply, &Overrides::default()),
        reply,
    }));
    ws.hosts[0].model.connected = true;
}

/// A Ctrl chord as the window delivers it: a press on the physical key.
fn ctrl(rt: &mut Runtime<Workspace>, code: Code, character: char) {
    let ev = KeyEvent {
        key: Key::Command(character),
        mods: Mods {
            ctrl: true,
            ..Mods::default()
        },
    };
    rt.physical_key_input(
        PhysicalInput::Key {
            key: code,
            mods: ev.mods,
            pressed: true,
            repeat: false,
            text: None,
            consumed_mods: Mods::default(),
        },
        ev,
        None,
    );
}

/// The mode name as the chip draws it: none when no mode is active.
fn chip_text(rt: &Runtime<Workspace>) -> Option<String> {
    has(rt, "mode-chip").then(|| element_text(rt, "mode-name").join(""))
}

fn chip(rt: &Runtime<Workspace>) -> rune::ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some("mode-chip"))
        .expect("the mode chip")
}

#[test]
fn no_mode_draws_no_chip_and_reserves_no_space() {
    let (mut ws, _rx) = tabbed_workspace("split");
    with_modes(&mut ws, false);
    let mut rt = runtime(ws);
    settle(&mut rt);
    assert!(!has(&rt, "mode-chip"));
    assert!(
        rect(&rt, "title-drag")[2] >= 48.,
        "the drag space is the same without a chip"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_mode_names_itself_for_its_whole_life_and_leave_ends_it() {
    let (mut ws, rx) = tabbed_workspace("split");
    with_modes(&mut ws, false);
    let (sender, sent) = flume::unbounded();
    live_terminal(&mut ws, "block:b", &sender);
    let mut rt = runtime(ws);
    settle(&mut rt);
    assert_eq!(chip_text(&rt), None);
    let focus = focused(&rt);

    ctrl(&mut rt, Code::KeyR, 'r');
    assert_eq!(
        chip_text(&rt),
        None,
        "nothing is drawn before the frame that sees the mode"
    );
    rt.frame(1. / 60.);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));
    assert_eq!(focused(&rt), focus, "the chip takes no focus");
    assert_eq!(chip(&rt).label.as_deref(), Some("Key mode: resize"));
    assert_eq!(
        chip(&rt).help.as_deref(),
        Some("Key mode resize · Escape to leave")
    );

    // A notice would have gone by now; the mode has not.
    for _ in 0..6 * 60 {
        rt.frame(1. / 60.);
    }
    assert_eq!(
        chip_text(&rt).as_deref(),
        Some("resize"),
        "the name lasts as long as the mode, not as long as a notice"
    );

    press(&mut rt, Key::Escape);
    assert_eq!(chip_text(&rt), None, "Escape left, and the chip with it");

    // Escape to leave again, this time from the host's own exit-all binding.
    ctrl(&mut rt, Code::KeyR, 'r');
    rt.frame(1. / 60.);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));
    press(&mut rt, Key::Text("q".into()));
    assert_eq!(chip_text(&rt), None);
    assert!(
        requests(&rx).is_empty(),
        "leaving a mode asks the server nothing"
    );
    assert!(sent_keys(&sent).is_empty(), "and types nothing");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn nested_and_once_modes_name_the_innermost_and_fall_back_to_the_outer() {
    let (mut ws, rx) = tabbed_workspace("split");
    with_modes(&mut ws, false);
    let mut rt = runtime(ws);
    settle(&mut rt);

    ctrl(&mut rt, Code::KeyR, 'r');
    rt.frame(1. / 60.);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));
    press(&mut rt, Key::Text("n".into()));
    assert_eq!(chip_text(&rt).as_deref(), Some("nested"));
    press(&mut rt, Key::Text("x".into()));
    assert_eq!(
        chip_text(&rt).as_deref(),
        Some("resize"),
        "the outer mode is named again after the inner one leaves"
    );

    // A once mode is named like any other, and gone the moment its one key has run.
    press(&mut rt, Key::Text("a".into()));
    assert_eq!(chip_text(&rt).as_deref(), Some("once"));
    press(&mut rt, Key::Right);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));
    assert_eq!(requests(&rx)[0].0, "session.resize_pane");

    // Its one key may also be Escape.
    press(&mut rt, Key::Text("a".into()));
    press(&mut rt, Key::Escape);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));

    press(&mut rt, Key::Text("q".into()));
    assert_eq!(chip_text(&rt), None);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_pending_sequence_and_a_mode_share_the_bar_in_that_order() {
    let (mut ws, _rx) = tabbed_workspace("split");
    with_modes(&mut ws, false);
    let mut rt = runtime(ws);
    settle(&mut rt);
    ctrl(&mut rt, Code::KeyR, 'r');
    rt.frame(1. / 60.);
    ctrl(&mut rt, Code::KeyB, 'b');
    rt.frame(1. / 60.);
    assert_eq!(rt.state.bindings.sequence.depth, 1);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));
    assert!(has(&rt, "pending-sequence"), "the sequence chip too");
    assert!(
        rect(&rt, "mode-chip")[0] < rect(&rt, "pending-sequence")[0],
        "the mode comes before the sequence"
    );
    press(&mut rt, Key::Escape);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));
    assert!(
        !has(&rt, "pending-sequence"),
        "Escape cancelled only the prefix"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn nonexclusive_mode_copy_shadows_base_new_tab_without_a_selection() {
    let (mut ws, host) = tabbed_workspace("single");
    with_modes(&mut ws, false);
    let (sender, sent) = flume::unbounded();
    live_terminal(&mut ws, "block:a", &sender);
    let mut rt = runtime(ws);
    ctrl(&mut rt, Code::KeyR, 'r');
    rt.frame(1. / 60.);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));
    let copy = KeyEvent {
        key: Key::Command('c'),
        mods: Mods {
            ctrl: true,
            shift: true,
            ..Mods::default()
        },
    };
    rt.key(copy.clone());
    rt.key_up(copy);
    rt.frame(1. / 60.);
    assert!(requests(&host).is_empty(), "must not open a tab");
    let keys = sent_keys(&sent);
    assert_eq!(keys.len(), 2);
    for (payload, edge) in keys.iter().zip(["press", "release"]) {
        assert_eq!(payload["action"], edge);
        assert_eq!(payload["key"], "key_c");
    }
}

#[test]
fn an_exclusive_mode_keeps_its_name_while_it_swallows_keys() {
    let (mut ws, _rx) = tabbed_workspace("split");
    with_modes(&mut ws, true);
    let (sender, sent) = flume::unbounded();
    live_terminal(&mut ws, "block:b", &sender);
    let mut rt = runtime(ws);
    settle(&mut rt);
    ctrl(&mut rt, Code::KeyR, 'r');
    rt.frame(1. / 60.);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));
    press(&mut rt, Key::Text("w".into()));
    assert!(sent_keys(&sent).is_empty(), "the mode takes the plain key");
    let copy = KeyEvent {
        key: Key::Command('c'),
        mods: Mods {
            ctrl: true,
            shift: true,
            ..Mods::default()
        },
    };
    rt.key(copy.clone());
    rt.key_up(copy);
    rt.frame(1. / 60.);
    assert!(
        sent_keys(&sent).is_empty(),
        "an unavailable Copy stays in the mode"
    );
    assert_eq!(
        chip_text(&rt).as_deref(),
        Some("resize"),
        "a key that does nothing leaves the name that explains why"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn hint_off_keeps_the_mode_name_and_a_plain_key_still_reaches_the_program() {
    let (mut ws, _rx) = tabbed_workspace("split");
    ws.settings.terminal.key_sequence_hints = false;
    with_modes(&mut ws, false);
    let (sender, sent) = flume::unbounded();
    live_terminal(&mut ws, "block:b", &sender);
    let mut rt = runtime(ws);
    settle(&mut rt);
    ctrl(&mut rt, Code::KeyR, 'r');
    rt.frame(1. / 60.);
    assert_eq!(
        chip_text(&rt).as_deref(),
        Some("resize"),
        "the mode is not a key sequence hint"
    );
    // A non-exclusive mode falls through, so ordinary typing is unchanged.
    press(&mut rt, Key::Text("w".into()));
    assert_eq!(sent_keys(&sent).len(), 1);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_toolbar_with_no_row_floats_the_mode_and_never_moves_the_terminal() {
    use rex_rune::settings::interface::ToolbarBehavior;
    for toolbar in [ToolbarBehavior::Hidden, ToolbarBehavior::OnHover] {
        let (mut ws, _rx) = tabbed_workspace("single");
        ws.settings.interface.toolbar = toolbar;
        with_modes(&mut ws, false);
        let (sender, _sent) = flume::unbounded();
        framed_terminal(&mut ws, "block:a", &sender, b"$ make\r\nok");
        let mut rt = runtime(ws);
        settle(&mut rt);
        let grid = rect(&rt, "grid:block:a");
        assert!(
            !has(&rt, "toolbar") && !has(&rt, "mode-chip"),
            "{toolbar:?}"
        );
        ctrl(&mut rt, Code::KeyR, 'r');
        rt.frame(1. / 60.);
        assert!(!has(&rt, "toolbar"), "{toolbar:?}: no row comes back");
        assert_eq!(chip_text(&rt).as_deref(), Some("resize"), "{toolbar:?}");
        let float = rect(&rt, "pending-sequence-float");
        let chip = rect(&rt, "mode-chip");
        assert_eq!((float[0], float[1]), (10., 8.), "{toolbar:?}");
        assert_eq!(
            (chip[0], chip[1]),
            (float[0], float[1]),
            "{toolbar:?}: the chip is the float's first"
        );
        assert_eq!(
            rect(&rt, "grid:block:a"),
            grid,
            "{toolbar:?}: floating status reserves no terminal space"
        );
        press(&mut rt, Key::Escape);
        assert!(!has(&rt, "pending-sequence-float"), "{toolbar:?}");
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}

#[test]
fn a_long_name_truncates_at_both_sizes_and_leaves_the_controls_whole() {
    for size in [(1024., 700.), (640., 480.)] {
        let (mut ws, _rx) = tabbed_workspace("split");
        with_modes(&mut ws, false);
        let (sender, _sent) = flume::unbounded();
        for block in ["block:b", "block:c"] {
            framed_terminal(&mut ws, block, &sender, b"$ cargo test\r\nok");
        }
        let mut rt = runtime_at(ws, size);
        settle(&mut rt);
        let grid = rect(&rt, "grid:block:b");
        ctrl(&mut rt, Code::KeyR, 'r');
        rt.frame(1. / 60.);
        // A non-exclusive mode falls through to the ordinary map, which enters the long one.
        ctrl(&mut rt, Code::KeyL, 'l');
        rt.frame(1. / 60.);
        assert_eq!(chip_text(&rt).as_deref(), Some(LONG), "{size:?}");
        let name = chip(&rt).label.clone().unwrap_or_default();
        assert_eq!(name, format!("Key mode: {LONG}"), "{size:?}");
        assert_eq!(
            chip(&rt).help.as_deref(),
            Some(format!("Key mode {LONG} · Escape to leave").as_str()),
            "{size:?}: the tooltip keeps the name whole"
        );
        let chip_rect = rect(&rt, "mode-chip");
        assert!(
            chip_rect[2] <= 150.5,
            "{size:?}: the chip is bounded: {chip_rect:?}"
        );
        assert_eq!(
            rect(&rt, "grid:block:b"),
            grid,
            "{size:?}: the terminal keeps its rows"
        );
        let settings = rect(&rt, "settings");
        assert!(
            chip_rect[0] + chip_rect[2] <= settings[0] + 0.5,
            "{size:?}: the chip clears the bar's controls: {chip_rect:?} {settings:?}"
        );
        let close = rect(&rt, "close-app");
        assert!(
            close[0] + close[2] <= size.0 + 0.5,
            "{size:?}: Close app keeps its place"
        );
        for key in ["commands", "new-tab", "settings", "close-app"] {
            let r = rect(&rt, key);
            assert!(r[2] > 0. && r[0] >= 0., "{size:?}: {key} is drawn: {r:?}");
        }
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}

#[test]
fn switching_host_session_keymap_or_connection_ends_the_name_without_a_keypress() {
    let (mut ws, _rx) = tabbed_workspace("split");
    with_modes(&mut ws, false);
    let mut rt = runtime(ws);
    settle(&mut rt);
    ctrl(&mut rt, Code::KeyR, 'r');
    rt.frame(1. / 60.);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));

    // A pane may change under a mode; its host, session and keymap may not.
    rt.update(|s, _| {
        let view = s.hosts[0].model.views.get_mut("s").unwrap();
        let window = view
            .windows
            .iter_mut()
            .find(|w| w.window_id.as_deref() == Some("split"))
            .unwrap();
        window.focused_block_id = Some("block:c".into());
    });
    settle(&mut rt);
    assert_eq!(
        chip_text(&rt).as_deref(),
        Some("resize"),
        "another pane keeps the mode"
    );

    rt.update(|s, _| s.hosts[0].model.selected = Some("elsewhere".into()));
    settle(&mut rt);
    assert_eq!(chip_text(&rt), None, "another session ends it at once");

    rt.update(|s, _| s.hosts[0].model.selected = Some("s".into()));
    settle(&mut rt);
    ctrl(&mut rt, Code::KeyR, 'r');
    rt.frame(1. / 60.);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));
    rt.update(|s, _| s.hosts[0].model.connected = false);
    settle(&mut rt);
    assert_eq!(chip_text(&rt), None, "a lost connection ends it at once");

    rt.update(|s, _| s.hosts[0].model.connected = true);
    settle(&mut rt);
    ctrl(&mut rt, Code::KeyR, 'r');
    rt.frame(1. / 60.);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));
    rt.update(|s, _| s.keymap_generation += 1);
    settle(&mut rt);
    assert_eq!(chip_text(&rt), None, "another keymap ends it at once");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn another_host_does_not_keep_the_name_and_reload_clears_it() {
    let mut ws = workspace_hosts(2);
    with_modes(&mut ws, false);
    let mut rt = runtime(ws);
    settle(&mut rt);
    ctrl(&mut rt, Code::KeyR, 'r');
    rt.frame(1. / 60.);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));
    rt.update(|s, tx| s.activate_host(1, tx));
    settle(&mut rt);
    assert_eq!(
        chip_text(&rt),
        None,
        "the first host's mode is not the second host's"
    );

    rt.update(|s, tx| s.activate_host(0, tx));
    settle(&mut rt);
    assert_eq!(
        chip_text(&rt),
        None,
        "coming back is not entering the mode again"
    );
    assert!(
        rt.state.bindings.modes.is_empty(),
        "the stack ended with the host it belonged to"
    );
    ctrl(&mut rt, Code::KeyR, 'r');
    rt.frame(1. / 60.);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));
    rt.update(|s, tx| {
        assert!(actions::run(
            s,
            "client.config.reload",
            &serde_json::Value::Null,
            tx
        ));
    });
    settle(&mut rt);
    assert_eq!(chip_text(&rt), None, "Reload config ends every mode");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_title_row_and_its_strip_name_the_mode_too() {
    use rex_rune::settings::interface::ToolbarBehavior;
    use rune::ChromeInsets;
    for toolbar in [ToolbarBehavior::Always, ToolbarBehavior::Hidden] {
        let (mut ws, _rx) = tabbed_workspace("split");
        ws.unified_title = true;
        ws.settings.interface.toolbar = toolbar;
        with_modes(&mut ws, false);
        let (sender, _sent) = flume::unbounded();
        framed_terminal(&mut ws, "block:b", &sender, b"$ make");
        let mut rt = runtime_at(ws, (1024., 700.));
        let mut insets = ChromeInsets::default();
        insets.leading = 76.;
        insets.height = crate::app::platform::chrome::TITLE_ROW;
        rt.set_chrome_insets(insets);
        settle(&mut rt);
        ctrl(&mut rt, Code::KeyR, 'r');
        settle(&mut rt);
        let row = if toolbar == ToolbarBehavior::Always {
            "toolbar"
        } else {
            "title-strip"
        };
        assert!(has(&rt, row), "{toolbar:?}: {row}");
        assert_eq!(chip_text(&rt).as_deref(), Some("resize"), "{toolbar:?}");
        let (chip, bar) = (rect(&rt, "mode-chip"), rect(&rt, row));
        assert!(
            chip[0] >= bar[0]
                && chip[0] + chip[2] <= bar[0] + bar[2]
                && chip[1] >= bar[1]
                && chip[1] + chip[3] <= bar[1] + bar[3],
            "{toolbar:?}: the chip is in the row: {chip:?} {bar:?}"
        );
        press(&mut rt, Key::Escape);
        assert_eq!(chip_text(&rt), None, "{toolbar:?}");
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}

/// A mode an accepted action enters is named before any key, and the name is the state the
/// next key dispatches from.
#[test]
fn an_accepted_entry_names_the_mode_before_any_key() {
    let (mut ws, rx) = tabbed_workspace("split");
    with_modes(&mut ws, false);
    let mut rt = runtime(ws);
    settle(&mut rt);
    assert!(
        rt.state.bindings.context.is_none(),
        "no key has set an input context"
    );
    rt.update(|s, tx| {
        assert!(actions::run(
            s,
            "client.mode.enter",
            &json!({"name": "resize"}),
            tx
        ))
    });
    rt.frame(1. / 60.);
    assert_eq!(
        chip_text(&rt).as_deref(),
        Some("resize"),
        "the entry is accepted before a keypress, so it is named then too"
    );
    press(&mut rt, Key::Left);
    assert_eq!(
        requests(&rx).first().map(|r| r.0.as_str()),
        Some("session.resize_pane"),
        "the name and the routing are the same state"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// The same for a once mode: it is named from entry, and its one key ends it.
#[test]
fn an_accepted_once_entry_is_named_until_its_key_runs() {
    let (mut ws, rx) = tabbed_workspace("split");
    with_modes(&mut ws, false);
    let mut rt = runtime(ws);
    settle(&mut rt);
    rt.update(|s, tx| {
        assert!(actions::run(
            s,
            "client.mode.enter",
            &json!({"name": "once", "once": true}),
            tx
        ))
    });
    rt.frame(1. / 60.);
    assert_eq!(chip_text(&rt).as_deref(), Some("once"));
    press(&mut rt, Key::Right);
    assert_eq!(chip_text(&rt), None, "its one key ran it");
    assert_eq!(
        requests(&rx).first().map(|r| r.0.as_str()),
        Some("session.resize_pane")
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// A mode a host returns as a client action goes through the same entry, so it is named on the
/// wake that accepted it rather than on the next key.
#[test]
fn a_mode_returned_by_a_host_wake_is_named_before_any_key() {
    let (mut ws, rx) = tabbed_workspace("split");
    with_modes(&mut ws, false);
    *ws.hosts[0].connection.model.lock().unwrap() = ws.hosts[0].model.clone();
    let mut rt = runtime(ws);
    settle(&mut rt);
    assert!(rt.state.bindings.context.is_none());
    rt.update(|s, tx| {
        s.hosts[0]
            .connection
            .model
            .lock()
            .unwrap()
            .client_actions
            .push(rex_rune::connection::actions::Queued {
                generation: s.keymap_generation,
                session: Some("s".into()),
                block: Some("block:b".into()),
                action: rexrs::api::client::ClientAction {
                    action: "client.mode.enter".into(),
                    args: Some(json!({"name": "resize"})),
                },
            });
        s.sync(0, tx);
    });
    rt.frame(1. / 60.);
    assert_eq!(
        chip_text(&rt).as_deref(),
        Some("resize"),
        "an accepted returned action enters a mode this person can see"
    );
    press(&mut rt, Key::Left);
    assert!(
        requests(&rx).iter().any(|r| r.0 == "session.resize_pane"),
        "and the next key really uses it"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// A session the host selects and then selects back does not bring the old mode with it.
#[test]
fn a_session_round_trip_does_not_bring_the_old_mode_back() {
    let (mut ws, _rx) = tabbed_workspace("split");
    with_modes(&mut ws, false);
    *ws.hosts[0].connection.model.lock().unwrap() = ws.hosts[0].model.clone();
    let mut rt = runtime(ws);
    settle(&mut rt);
    ctrl(&mut rt, Code::KeyR, 'r');
    rt.frame(1. / 60.);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));

    rt.update(|s, tx| {
        s.hosts[0].connection.model.lock().unwrap().selected = Some("other".into());
        s.sync(0, tx);
    });
    settle(&mut rt);
    assert_eq!(chip_text(&rt), None, "another session has no mode");

    rt.update(|s, tx| {
        s.hosts[0].connection.model.lock().unwrap().selected = Some("s".into());
        s.sync(0, tx);
    });
    settle(&mut rt);
    assert_eq!(
        chip_text(&rt),
        None,
        "the session that had the mode does not keep it after a round trip"
    );
    assert!(rt.state.bindings.modes.is_empty());
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// A lost and regained connection, and a new server keymap revision, end the mode where they
/// change; coming back is not entering again.
#[test]
fn a_connection_or_server_keymap_round_trip_does_not_bring_the_mode_back() {
    let (mut ws, _rx) = tabbed_workspace("split");
    with_modes(&mut ws, false);
    *ws.hosts[0].connection.model.lock().unwrap() = ws.hosts[0].model.clone();
    let mut rt = runtime(ws);
    settle(&mut rt);
    ctrl(&mut rt, Code::KeyR, 'r');
    rt.frame(1. / 60.);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));

    rt.update(|s, tx| {
        s.hosts[0].connection.model.lock().unwrap().connected = false;
        s.sync(0, tx);
    });
    settle(&mut rt);
    assert_eq!(chip_text(&rt), None, "a lost connection has no mode");
    assert!(rt.state.bindings.modes.is_empty());

    rt.update(|s, tx| {
        s.hosts[0].connection.model.lock().unwrap().connected = true;
        s.sync(0, tx);
    });
    settle(&mut rt);
    assert_eq!(chip_text(&rt), None, "reconnecting is not entering again");

    ctrl(&mut rt, Code::KeyR, 'r');
    rt.frame(1. / 60.);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));
    rt.update(|s, tx| {
        s.hosts[0].connection.model.lock().unwrap().keymap_revision += 1;
        s.sync(0, tx);
    });
    settle(&mut rt);
    assert_eq!(chip_text(&rt), None, "another server keymap has no mode");
    assert!(rt.state.bindings.modes.is_empty());
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// A new local keymap is a change of its own; the mode ends with it.
#[test]
fn a_new_local_keymap_ends_the_mode_at_that_change() {
    let (mut ws, _rx) = tabbed_workspace("split");
    with_modes(&mut ws, false);
    let mut rt = runtime(ws);
    settle(&mut rt);
    ctrl(&mut rt, Code::KeyR, 'r');
    rt.frame(1. / 60.);
    assert_eq!(chip_text(&rt).as_deref(), Some("resize"));
    rt.update(|s, _tx| s.keymap_changed());
    settle(&mut rt);
    assert_eq!(chip_text(&rt), None);
    assert!(
        rt.state.bindings.modes.is_empty(),
        "the stack is gone, not merely hidden"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// The bar's live chips share one row with controls that must stay reachable: at the minimum
/// window size every mandatory control and both tab arrows keep their place.
fn bar_controls_fit(rt: &Runtime<Workspace>, width: f32, note: &str) {
    let mut end = 0.;
    for key in ["commands", "new-tab", "settings", "close-app"] {
        let r = rect(rt, key);
        assert!(
            r[2] > 0. && r[0] >= 0. && r[0] + r[2] <= width + 0.5,
            "{note}: {key} leaves the window: {r:?} of {width}"
        );
        assert!(
            r[0] >= end - 0.5,
            "{note}: {key} overlaps the control before it: {r:?}"
        );
        end = r[0] + r[2];
    }
    if has(rt, "tab-track") {
        let track = rect(rt, "tab-track");
        for key in ["previous-tabs", "next-tabs"] {
            if has(rt, key) {
                let arrow = rect(rt, key);
                assert!(
                    arrow[0] >= track[0] - 0.5 && arrow[0] + arrow[2] <= track[0] + track[2] + 0.5,
                    "{note}: {key} is outside its track: {arrow:?} in {track:?}"
                );
            }
        }
        if has(rt, "mode-chip") {
            let chip = rect(rt, "mode-chip");
            for key in ["previous-tabs", "next-tabs"] {
                if has(rt, key) {
                    let arrow = rect(rt, key);
                    assert!(
                        arrow[0] + arrow[2] <= chip[0] + 0.5,
                        "{note}: {key} covers the mode chip: {arrow:?} {chip:?}"
                    );
                }
            }
            assert!(
                chip[2] >= 40.,
                "{note}: the chip keeps room for its name: {chip:?}"
            );
            assert!(
                chip[0] + chip[2] <= rect(rt, "settings")[0] + 0.5,
                "{note}: the chip clears Settings: {chip:?}"
            );
            if has(rt, "pending-sequence") {
                assert!(
                    chip[0] + chip[2] <= rect(rt, "pending-sequence")[0] + 0.5,
                    "{note}: the sequence chip follows the mode chip"
                );
            }
        }
    }
}

fn with_download(ws: &mut Workspace) {
    let mut transfer = rex_rune::download::Transfer::new(
        1,
        "s".into(),
        "block:b".into(),
        "~/img-a.iso".into(),
        std::env::temp_dir(),
        1,
    );
    transfer.name = Some("img-a.iso".into());
    transfer.state = rex_rune::download::State::Receiving {
        received: 30,
        hint: 600,
    };
    ws.hosts[0].model.downloads.push(transfer);
}

#[test]
fn the_chip_row_keeps_the_controls_and_tab_arrows_at_640() {
    let (width, height) = (640., 480.);

    // A long mode alone: the arrows keep their whole hit areas inside the track.
    {
        let (mut ws, _rx) = tabbed_workspace("split");
        with_modes(&mut ws, false);
        let mut rt = runtime_at(ws, (width, height));
        settle(&mut rt);
        assert_eq!(
            rt.state.viewport[0], width,
            "the fixture width is the layout width"
        );
        ctrl(&mut rt, Code::KeyR, 'r');
        ctrl(&mut rt, Code::KeyL, 'l');
        settle(&mut rt);
        assert_eq!(chip_text(&rt).as_deref(), Some(LONG));
        bar_controls_fit(&rt, width, "a long mode");
        assert!(has(&rt, "previous-tabs") && has(&rt, "next-tabs"));
        std::fs::remove_file(&rt.state.settings_path).ok();
    }

    // The failing combination: a long mode and a pending sequence together.
    {
        let (mut ws, _rx) = tabbed_workspace("split");
        with_modes(&mut ws, false);
        let (sender, _sent) = flume::unbounded();
        for block in ["block:b", "block:c"] {
            framed_terminal(&mut ws, block, &sender, b"$ cargo test\r\nok");
        }
        let mut rt = runtime_at(ws, (width, height));
        settle(&mut rt);
        let grid = rect(&rt, "grid:block:b");
        ctrl(&mut rt, Code::KeyR, 'r');
        ctrl(&mut rt, Code::KeyL, 'l');
        ctrl(&mut rt, Code::KeyB, 'b');
        settle(&mut rt);
        assert_eq!(chip_text(&rt).as_deref(), Some(LONG));
        assert!(has(&rt, "pending-sequence"));
        bar_controls_fit(&rt, width, "a long mode and a pending sequence");
        let chip = rt
            .elements()
            .into_iter()
            .find(|e| !e.exiting && e.key.as_deref() == Some("mode-chip"))
            .unwrap();
        assert_eq!(
            chip.label.as_deref(),
            Some(format!("Key mode: {LONG}").as_str()),
            "the cut name keeps its whole accessible name"
        );
        assert_eq!(
            chip.help.as_deref(),
            Some(format!("Key mode {LONG} · Escape to leave").as_str()),
            "and its whole tooltip"
        );
        assert_eq!(
            rect(&rt, "grid:block:b"),
            grid,
            "the terminal keeps its rect"
        );
        std::fs::remove_file(&rt.state.settings_path).ok();
    }

    // A long host name as well: the identity chip gives up its extra width first.
    {
        let (mut ws, _rx) = tabbed_workspace("split");
        ws.settings.hosts[0].label = "a-long-host-name-on-the-identity-chip".into();
        with_modes(&mut ws, false);
        let mut rt = runtime_at(ws, (width, height));
        settle(&mut rt);
        ctrl(&mut rt, Code::KeyR, 'r');
        ctrl(&mut rt, Code::KeyL, 'l');
        ctrl(&mut rt, Code::KeyB, 'b');
        settle(&mut rt);
        assert_eq!(chip_text(&rt).as_deref(), Some(LONG));
        bar_controls_fit(
            &rt,
            width,
            "a long host, a long mode and a pending sequence",
        );
        assert!(
            rect(&rt, "identity")[2] >= 119.5,
            "the identity chip keeps its floor"
        );
        std::fs::remove_file(&rt.state.settings_path).ok();
    }

    // A download chip takes its room too, in its existing short narrow form.
    {
        let (mut ws, _rx) = tabbed_workspace("split");
        with_modes(&mut ws, false);
        with_download(&mut ws);
        let mut rt = runtime_at(ws, (width, height));
        settle(&mut rt);
        ctrl(&mut rt, Code::KeyR, 'r');
        ctrl(&mut rt, Code::KeyL, 'l');
        settle(&mut rt);
        assert_eq!(chip_text(&rt).as_deref(), Some(LONG));
        assert!(has(&rt, "download-chip"));
        bar_controls_fit(&rt, width, "a long mode and a download");
        std::fs::remove_file(&rt.state.settings_path).ok();
    }

    // With no mode the bar is what it was: a pending sequence still fits.
    {
        let (mut ws, _rx) = tabbed_workspace("split");
        with_modes(&mut ws, false);
        let mut rt = runtime_at(ws, (width, height));
        settle(&mut rt);
        assert!(!has(&rt, "mode-chip"));
        ctrl(&mut rt, Code::KeyB, 'b');
        settle(&mut rt);
        assert!(has(&rt, "pending-sequence"));
        bar_controls_fit(&rt, width, "a pending sequence with no mode");
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}

#[test]
fn downloads_keep_controls_reachable_with_modes_sidebar_tabs_and_actual_zoom() {
    for (width, height, scale) in [
        (640., 480., 1.),
        (960., 720., 1.5),
        (1024., 768., 1.5),
        (1200., 900., 1.5),
        (1024., 700., 1.),
    ] {
        for sidebar in [false, true] {
            for sequence in [false, true] {
                let (mut ws, _rx) = tabbed_workspace("split");
                ws.settings.ui_scale = scale;
                ws.settings.hosts[0].label = "a-very-long-host-name-to-test-truncation".into();
                ws.settings.vertical_tabs = sidebar;
                ws.sidebar.set_open(sidebar);
                with_modes(&mut ws, false);
                with_download(&mut ws);
                let name = "a-very-long-build-artifact-download.iso";
                ws.hosts[0].model.downloads[0].name = Some(name.into());
                let (sender, _sent) = flume::unbounded();
                for block in ["block:b", "block:c"] {
                    framed_terminal(&mut ws, block, &sender, b"$ cargo test\r\nok");
                }
                let mut rt = runtime_at(ws, (width, height));
                settle(&mut rt);
                assert_eq!(rt.state.ui_scale(), scale, "the zoom must not be clamped");
                let note =
                    format!("{width}x{height} @{scale}, sidebar={sidebar}, sequence={sequence}");
                bar_controls_fit(&rt, width, &format!("{note}: no mode control"));
                let grid = rect(&rt, "grid:block:b");
                ctrl(&mut rt, Code::KeyL, 'l');
                if sequence {
                    ctrl(&mut rt, Code::KeyB, 'b');
                }
                settle(&mut rt);
                assert_eq!(chip_text(&rt).as_deref(), Some(LONG));
                bar_controls_fit(&rt, width, &note);
                assert_eq!(rect(&rt, "grid:block:b"), grid, "{note}: unchanged grid");
                for key in ["commands", "new-tab", "settings", "close-app"] {
                    assert!(rect(&rt, key)[2] >= 26. * scale - 0.5, "{note}: {key}");
                }
                for key in ["previous-tabs", "next-tabs"] {
                    if has(&rt, key) {
                        assert!(rect(&rt, key)[2] >= 25.5 * scale, "{note}: {key}");
                    }
                }
                let download_visible = sidebar || !sequence || width / scale >= 1000.;
                assert_eq!(has(&rt, "download-chip"), download_visible, "{note}");
                if let Some(download) = rt
                    .elements()
                    .into_iter()
                    .find(|e| !e.exiting && e.key.as_deref() == Some("download-chip"))
                {
                    assert!(download.label.as_deref().unwrap().contains(name));
                    assert!(download.help.as_deref().unwrap().contains(name));
                }
                std::fs::remove_file(&rt.state.settings_path).ok();
            }
        }
    }
}

/// The name-only composition is the narrow one; a wide bar still names the chip's own word.
#[test]
fn a_narrow_bar_drops_the_mode_word_and_a_wide_one_keeps_it() {
    for (size, word) in [((1024., 700.), true), ((640., 480.), false)] {
        let (mut ws, _rx) = tabbed_workspace("split");
        with_modes(&mut ws, false);
        let mut rt = runtime_at(ws, size);
        settle(&mut rt);
        ctrl(&mut rt, Code::KeyR, 'r');
        rt.frame(1. / 60.);
        assert_eq!(chip_text(&rt).as_deref(), Some("resize"), "{size:?}");
        assert_eq!(
            element_text(&rt, "mode-chip").contains(&"mode".to_owned()),
            word,
            "{size:?}: the mode word"
        );
        let chip = rt
            .elements()
            .into_iter()
            .find(|e| !e.exiting && e.key.as_deref() == Some("mode-chip"))
            .unwrap();
        assert_eq!(
            chip.label.as_deref(),
            Some("Key mode: resize"),
            "{size:?}: the accessible name keeps the word"
        );
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}

#[test]
fn mode_chip_draws_with_downloads_at_narrow_effective_widths() {
    let Some(mut camera) = super::pane_verb_tests::Camera::new() else {
        return;
    };
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");
    for (prefix, size, scale, sidebar) in [
        ("mode-download-sidebar", (640., 480.), 1., true),
        ("mode-download-zoom", (1024., 768.), 1.5, false),
    ] {
        let mut rt = painted_runtime(size, false);
        rt.update(|s, _| {
            s.settings.vertical_tabs = sidebar;
            s.sidebar.set_open(sidebar);
            s.settings.ui_scale = scale;
            with_download(s);
            s.hosts[0].model.downloads[0].name =
                Some("a-very-long-build-artifact-download.iso".into());
        });
        settle(&mut rt);
        assert_eq!(rt.state.ui_scale(), scale);
        camera.snap(&mut rt, &format!("{prefix}-before"));
        ctrl(&mut rt, Code::KeyL, 'l');
        settle(&mut rt);
        bar_controls_fit(&rt, size.0, prefix);
        camera.snap(&mut rt, &format!("{prefix}-mode"));
        ctrl(&mut rt, Code::KeyB, 'b');
        settle(&mut rt);
        bar_controls_fit(&rt, size.0, prefix);
        camera.snap(&mut rt, &format!("{prefix}-sequence"));
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}

/// Real fonts are process-global: run each capture test on its own with `REX_TEST_SNAP_DIR`.
#[test]
fn mode_chip_draws_with_a_pending_sequence() {
    let Some(mut camera) = super::pane_verb_tests::Camera::new() else {
        return;
    };
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");
    for (prefix, size) in [
        ("mode-chip-sequence", (1024., 700.)),
        ("mode-chip-sequence-minimum", (640., 480.)),
    ] {
        let mut rt = painted_runtime(size, false);
        ctrl(&mut rt, Code::KeyR, 'r');
        ctrl(&mut rt, Code::KeyL, 'l');
        ctrl(&mut rt, Code::KeyB, 'b');
        settle(&mut rt);
        camera.snap(&mut rt, prefix);
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}

/// Real fonts are process-global: run each capture test on its own with `REX_TEST_SNAP_DIR`.
#[test]
fn mode_chip_draws() {
    draw("mode-chip", (1024., 700.));
}

/// Real fonts are process-global: run each capture test on its own with `REX_TEST_SNAP_DIR`.
#[test]
fn mode_chip_draws_at_the_minimum_size() {
    draw("mode-chip-minimum", (640., 480.));
}

fn draw(prefix: &str, size: (f32, f32)) {
    let Some(mut camera) = super::pane_verb_tests::Camera::new() else {
        return;
    };
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");
    let mut rt = painted_runtime(size, false);
    camera.snap(&mut rt, &format!("{prefix}-none"));
    ctrl(&mut rt, Code::KeyR, 'r');
    settle(&mut rt);
    camera.snap(&mut rt, &format!("{prefix}-resize"));
    ctrl(&mut rt, Code::KeyL, 'l');
    settle(&mut rt);
    camera.snap(&mut rt, &format!("{prefix}-long-name"));
    std::fs::remove_file(&rt.state.settings_path).ok();

    // The title row macOS draws instead of a separate toolbar row.
    let mut title = painted_runtime(size, true);
    ctrl(&mut title, Code::KeyR, 'r');
    settle(&mut title);
    camera.snap(&mut title, &format!("{prefix}-title-row"));
    std::fs::remove_file(&title.state.settings_path).ok();
}

fn painted_runtime(size: (f32, f32), unified_title: bool) -> Runtime<Workspace> {
    let fonts = crate::app::fonts::load("IBM Plex Mono", &[]);
    let (mut ws, _rx) = tabbed_workspace("split");
    ws.unified_title = unified_title;
    ws.title_inset = if unified_title { 76. } else { 0. };
    with_modes(&mut ws, false);
    let (sender, _sent) = flume::unbounded();
    for block in ["block:b", "block:c"] {
        super::pane_verb_tests::framed_terminal(
            &mut ws,
            block,
            &sender,
            b"$ cargo test\r\nrunning 12 tests",
        );
    }
    let mut rt = Runtime::with_text(
        ws,
        view,
        size,
        Env::new(true, 1.),
        rune::text::TextSystem::new(fonts.files, fonts.families),
    );
    rt.on_key = Some(crate::app::input::on_key);
    settle(&mut rt);
    rt
}
