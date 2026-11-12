//! The Activity entry point shares the list's inventory, never its read receipts.
use super::activity_list_tests::fixture;
use super::pane_verb_tests::{framed_terminal, rect};
use super::*;
use crate::app::{
    actions,
    activity_list::ACTION,
    notify::NoteTarget,
    theme::{danger, warning},
};
use rex_linux::{
    connection::activity::{BlockedKind, ProgramState, StatusRecord, TerminalProgress},
    settings::interface::ToolbarBehavior,
};
use serde_json::json;

fn state(ws: &mut Workspace, host: usize, block: &str, state: ProgramState) {
    ws.hosts[host].model.activity.insert(
        block.into(),
        [(
            String::new(),
            StatusRecord {
                state,
                live: true,
                ..Default::default()
            },
        )]
        .into(),
    );
}
fn label(ws: &Workspace) -> Option<String> {
    ws.activity_chip().map(|s| s.label())
}
fn settle(rt: &mut Runtime<Workspace>) {
    // Fixed frame budget, including the production active-tab reveal path.
    for _ in 0..6 {
        rt.update(|s, tx| {
            crate::app::poll(s, 0., tx);
        });
        for _ in 0..30 {
            rt.frame(1. / 60.);
        }
    }
}
fn wake(rt: &mut Runtime<Workspace>, f: impl FnOnce(&mut Workspace)) {
    rt.update(|s, tx| {
        f(s);
        for host in 0..s.hosts.len() {
            *s.hosts[host].connection.model.lock().unwrap() = s.hosts[host].model.clone();
            s.sync(host, tx);
        }
    });
    settle(rt);
}
fn bind(ws: &mut Workspace) {
    ws.settings.shortcut_file.overrides = rex_linux::keymap::Overrides::parse(&json!([
        {"keys":"ctrl+alt+a","action":ACTION},
        {"keys":"ctrl+b>c","action":"client.tab.new"}
    ]));
    ws.local_keymap = actions::resolve(&ws.settings.shortcut_file.overrides);
    ws.keymap_generation += 1;
}
fn prefix(rt: &mut Runtime<Workspace>) {
    let mods = Mods {
        ctrl: true,
        ..Mods::default()
    };
    rt.physical_key_input(
        rune::PhysicalInput::Key {
            key: rune::PhysicalKeyCode::KeyB,
            mods,
            pressed: true,
            repeat: false,
        },
        KeyEvent {
            key: Key::Command('b'),
            mods,
        },
        None,
    );
    settle(rt);
}
fn download(ws: &mut Workspace) {
    let mut t = rex_linux::download::Transfer::new(
        1,
        "s".into(),
        "front".into(),
        "~/build.log".into(),
        std::env::temp_dir(),
        1,
    );
    t.name = Some("build.log".into());
    t.state = rex_linux::download::State::Receiving {
        received: 42,
        hint: 100,
    };
    ws.hosts[0].model.downloads.push(t);
}

#[test]
fn inventory_states_priority_words_and_host_session_identity() {
    let (mut ws, _rx) = fixture();
    assert_eq!(label(&ws).as_deref(), Some("2 need you"));
    assert_eq!(ws.activity_chip().unwrap().tone(), warning());
    assert_eq!(ws.activity_chip().unwrap().short(), "Need 2");
    // Same tab and session ids on the other host remain distinct.
    state(&mut ws, 0, "front", ProgramState::Blocked);
    assert_eq!(
        label(&ws).as_deref(),
        Some("2 need you"),
        "on-screen never counts"
    );
    ws.window_focused = false;
    assert_eq!(
        label(&ws).as_deref(),
        Some("2 need you"),
        "not a focus/read receipt"
    );
    state(&mut ws, 1, "front", ProgramState::Blocked);
    state(&mut ws, 1, "build", ProgramState::Blocked);
    assert_eq!(label(&ws).as_deref(), Some("4 need you"));
    assert_eq!(
        ws.activity_entries("")
            .iter()
            .filter(|e| e.group == "Needs you")
            .count(),
        5
    );
    for kind in [
        None,
        Some(BlockedKind::Auth),
        Some(BlockedKind::Question),
        Some(BlockedKind::Permission),
    ] {
        ws.hosts[1]
            .model
            .activity
            .get_mut("build")
            .unwrap()
            .get_mut("")
            .unwrap()
            .kind = kind;
        assert_eq!(label(&ws).as_deref(), Some("4 need you"));
    }
    ws.hosts[1].model.connected = false;
    assert_eq!(label(&ws).as_deref(), Some("1 needs you"));
    ws.settings.muted_tabs.push(rex_linux::settings::MutedTab {
        endpoint: ws.settings.hosts[0].endpoint.clone(),
        session: "s".into(),
        window: "blocked".into(),
    });
    assert_eq!(label(&ws).as_deref(), Some("1 error"));
    assert_eq!(ws.activity_chip().unwrap().tone(), danger());
    assert_eq!(ws.activity_chip().unwrap().short(), "1 error");
    state(&mut ws, 0, "build", ProgramState::Error);
    assert_eq!(label(&ws).as_deref(), Some("2 errors"));
    ws.settings.notifications.show_in_tabs = false;
    assert!(ws.activity_chip().is_none());
    assert!(
        ws.activity_entries("").iter().any(|e| e.group == "Error"),
        "list policy unchanged"
    );
    ws.settings.notifications.show_in_tabs = true;
    for s in [
        ProgramState::Idle,
        ProgramState::Working,
        ProgramState::Done,
    ] {
        state(&mut ws, 0, "build", s);
        state(&mut ws, 0, "error", s);
        assert!(
            ws.activity_chip().is_none(),
            "no Working, Done or Bell chip"
        );
    }
    ws.hosts[0]
        .model
        .progress
        .insert("build".into(), TerminalProgress::Error(None));
    assert!(
        ws.activity_chip().is_none(),
        "OSC 9;4 alone is not listed activity"
    );
    // A layout absent from the session inventory is not something the list can list.
    let extra = ws.hosts[0].model.views["other"].clone();
    ws.hosts[0].model.views.insert("not-listed".into(), extra);
    state(&mut ws, 0, "build", ProgramState::Blocked);
    assert_eq!(label(&ws).as_deref(), Some("1 needs you"));
    ws.hosts[0].model.views.remove("other");
    assert!(ws.activity_chip().is_none());
}

#[test]
fn live_chip_click_is_local_preserves_latches_notes_focus_and_pane_geometry() {
    let _families = rune::text::with_families("Inter", "Inter");
    let (mut ws, hosts) = fixture();
    for h in &mut ws.hosts {
        h.model.activity.clear();
    }
    let (sender, terminal) = flume::unbounded();
    framed_terminal(&mut ws, "front", &sender, b"$ ");
    let connection = ws.terminals[&ws.term_key("front")].connection.clone();
    ws.hosts[0]
        .model
        .terminals
        .insert("front".into(), connection);
    let note = NoteTarget {
        host: ws.hosts[1].id,
        session: "s".into(),
        window: Some("done".into()),
    };
    ws.notified_tabs.insert(note.clone());
    let mut rt = runtime(ws);
    wake(&mut rt, |_| {});
    let pane = rect(&rt, "pane:front");
    let grid = rect(&rt, "grid:front");
    for h in &hosts {
        h.drain();
    }
    terminal.drain();
    let done = rt.state.unseen_done.clone();
    let bells = rt.state.unseen_bells.clone();
    assert!(!has(&rt, "activity-chip"));
    wake(&mut rt, |s| state(s, 1, "build", ProgramState::Blocked));
    assert!(has(&rt, "activity-chip"));
    assert_eq!(rect(&rt, "pane:front"), pane);
    assert_eq!(rect(&rt, "grid:front"), grid);
    let chip = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("activity-chip"))
        .unwrap();
    assert_eq!(chip.role, "button");
    assert_eq!(chip.label.as_deref(), Some("Activity: 1 needs you"));
    assert_eq!(chip.help.as_deref(), Some("1 needs you · Show activity"));
    click(&mut rt, "activity-chip");
    settle(&mut rt);
    assert!(matches!(
        rt.state.panel.as_ref().unwrap().kind,
        PanelKind::Activity
    ));
    assert_eq!(rect(&rt, "pane:front"), pane);
    assert_eq!(rect(&rt, "grid:front"), grid);
    let chip_focus = focused(&rt);
    assert!(chip_focus.is_some());
    press(&mut rt, Key::Text("b".into()));
    assert_eq!(
        rt.state.query(),
        "b",
        "typing immediately filters after a chip click"
    );
    press(&mut rt, Key::Escape);
    rt.update(|s, tx| {
        actions::run(s, ACTION, &serde_json::Value::Null, tx);
    });
    settle(&mut rt);
    assert_eq!(focused(&rt), chip_focus, "same field as Commands/action");
    press(&mut rt, Key::Escape);
    assert!(focused(&rt).is_none());
    wake(&mut rt, |s| state(s, 1, "error", ProgramState::Blocked));
    assert_eq!(label(&rt.state).as_deref(), Some("2 need you"));
    wake(&mut rt, |s| {
        state(s, 1, "build", ProgramState::Working);
        state(s, 1, "error", ProgramState::Working);
    });
    assert!(!has(&rt, "activity-chip"));
    assert_eq!(rt.state.unseen_done, done);
    assert_eq!(rt.state.unseen_bells, bells);
    assert!(rt.state.notified_tabs.contains(&note));
    assert_eq!(rect(&rt, "pane:front"), pane);
    assert_eq!(rect(&rt, "grid:front"), grid);
    for h in &hosts {
        assert!(
            h.is_empty(),
            "no host commands across appearance/click/disappearance"
        );
    }
    assert!(terminal.is_empty(), "no PTY input, resize or claim");
}

#[test]
fn chip_room_policy_tooltip_and_coexistence_keep_active_tab_actions_and_drag() {
    let _families = rune::text::with_families("Inter", "Inter");
    for width in [1024., 640.] {
        for sidebar in [false, true] {
            let (mut ws, _rx) = fixture();
            bind(&mut ws);
            download(&mut ws);
            ws.hosts[0]
                .model
                .views
                .get_mut("s")
                .unwrap()
                .active_window_id = Some("idle".into());
            if sidebar {
                ws.settings.vertical_tabs = true;
                ws.sidebar.toggle();
            }
            let mut rt = runtime_at(ws, (width, 700.));
            settle(&mut rt);
            let chip = rt
                .elements()
                .into_iter()
                .find(|e| e.key.as_deref() == Some("activity-chip"))
                .unwrap();
            assert_eq!(
                chip.help.as_deref(),
                Some("2 need you · Show activity · Ctrl Alt A")
            );
            assert!(rt.elements().iter().any(|e| e.text.as_deref()
                == Some(if width < 1000. {
                    "Need 2"
                } else {
                    "2 need you"
                })));
            for pending in [false, true] {
                if pending {
                    prefix(&mut rt);
                }
                let expected = width >= 1000. || !pending;
                assert_eq!(has(&rt, "activity-chip"), expected);
                assert_eq!(has(&rt, "download-chip"), expected);
                assert_eq!(has(&rt, "pending-sequence"), pending);
                let drag = rect(&rt, "title-drag");
                assert!(drag[2] >= 48.);
                let close = rect(&rt, "close-app");
                assert!(
                    close[0] + close[2] <= width + 0.5,
                    "close {close:?} width {width}"
                );
                let commands = rect(&rt, "commands");
                assert!(drag[0] + drag[2] <= commands[0] + 0.5);
                if !sidebar {
                    let strip = rect(&rt, "tabs");
                    let active = rect(&rt, "tab:idle");
                    assert!(
                        strip[2] >= 40.,
                        "width {width} sidebar {sidebar} pending {pending}: strip {strip:?}; chips {:?}; identity {:?}",
                        rt.elements()
                            .iter()
                            .filter(|e| matches!(
                                e.key.as_deref(),
                                Some("activity-chip" | "download-chip" | "pending-sequence")
                            ))
                            .map(|e| (&e.key, e.rect))
                            .collect::<Vec<_>>(),
                        rect(&rt, "identity")
                    );
                    assert!(
                        active[0] + 40. <= strip[0] + strip[2]
                            && active[0] + active[2] >= strip[0] + 40.,
                        "active {active:?}, strip {strip:?}"
                    );
                }
            }
            press(&mut rt, Key::Escape);
            rt.update(|s, _| {
                for h in &mut s.hosts {
                    h.model.activity.clear();
                }
            });
            settle(&mut rt);
            assert!(!has(&rt, "activity-chip"));
            if !sidebar {
                for show in [false, true] {
                    if show {
                        rt.update(|s, _| state(s, 1, "build", ProgramState::Blocked));
                        settle(&mut rt);
                    }
                    let strip = rect(&rt, "tabs");
                    let active = rect(&rt, "tab:idle");
                    assert!(
                        active[0] + 40. <= strip[0] + strip[2]
                            && active[0] + active[2] >= strip[0] + 40.,
                        "active tab revealed after chip show={show}: {active:?} in {strip:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn every_toolbar_mode_floats_or_contains_the_same_chip_without_resizing_panes() {
    let _families = rune::text::with_families("Inter", "Inter");
    for mode in [
        ToolbarBehavior::Always,
        ToolbarBehavior::OnHover,
        ToolbarBehavior::Hidden,
    ] {
        let (mut ws, _rx) = fixture();
        ws.settings.interface.toolbar = mode;
        let mut rt = runtime(ws);
        settle(&mut rt);
        let pane = rect(&rt, "pane:front");
        assert!(has(&rt, "activity-chip"));
        assert_eq!(has(&rt, "toolbar"), mode == ToolbarBehavior::Always);
        if mode != ToolbarBehavior::Always {
            assert!(has(&rt, "pending-sequence-float"));
            rt.update(|s, _| s.toggle_toolbar());
            settle(&mut rt);
            assert!(has(&rt, "toolbar"));
            assert!(has(&rt, "activity-chip"));
            assert!(!has(&rt, "pending-sequence-float"));
            assert_eq!(rect(&rt, "pane:front"), pane);
        }
        rt.update(|s, _| {
            for h in &mut s.hosts {
                h.model.activity.clear();
            }
        });
        settle(&mut rt);
        assert!(!has(&rt, "activity-chip"));
        assert_eq!(rect(&rt, "pane:front"), pane);
    }
}
