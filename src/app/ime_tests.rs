//! Headless workspace tests for terminal IME anchoring, preedit/commit/disable, gates,
//! multi-pane isolation, and pointer hit-test transparency when the hardware cursor is hidden.
use super::*;
use rex_rune::connection::{Command, Connection, HostCommand, TerminalSender};
use rune::{App, AppSpec, Env, Ime, Key, Runtime};

fn ime_runtime(ws: Workspace) -> Runtime<Workspace> {
    let size = (1024., 700.);
    let mut rt = App::new(AppSpec::new("Rex", size, placeholder, view))
        .init(move || ws)
        .on_ime(crate::app::input::on_ime)
        .headless_runtime(size, Env::new(true, 1.));
    rt.on_key = Some(crate::app::input::on_key);
    for _ in 0..4 {
        rt.frame(1. / 60.);
    }
    rt
}

fn attach_terminal(
    ws: &mut Workspace,
    block: &str,
    sender: &flume::Sender<HostCommand>,
    terminal: &ghostty_vt::Terminal,
) {
    let connection = Connection {
        model: Default::default(),
        commands: TerminalSender::new(sender.clone(), "s".into(), block.into()),
        wake: flume::unbounded().1,
    };
    let frame = rex_rune::terminal::Renderer::new()
        .unwrap()
        .frame(terminal)
        .unwrap();
    {
        let mut m = connection.model.lock().unwrap();
        m.connected = true;
        m.control_connected = true;
        m.frame = Some(std::sync::Arc::new(frame));
    }
    let key = ws.term_key(block);
    let mut term = crate::app::term::Term::new(connection.clone());
    term.refresh();
    ws.terminals.insert(key, term);
    ws.hosts[0].model.visible = true;
    ws.hosts[0].model.terminals.insert(block.into(), connection);
    *ws.hosts[0].connection.model.lock().unwrap() = ws.hosts[0].model.clone();
}

fn push_frame(rt: &mut Runtime<Workspace>, block: &str, terminal: &ghostty_vt::Terminal) {
    let frame = rex_rune::terminal::Renderer::new()
        .unwrap()
        .frame(terminal)
        .unwrap();
    rt.update(|s, _| {
        let key = s.term_key(block);
        let term = s.terminals.get_mut(&key).unwrap();
        term.connection.model.lock().unwrap().frame = Some(std::sync::Arc::new(frame));
        term.refresh();
    });
    rt.frame(1. / 60.);
}

fn rect_of(rt: &Runtime<Workspace>, key: &str) -> Option<[f32; 4]> {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .map(|e| e.rect)
}

fn assert_rect_near(actual: Option<[f32; 4]>, expected: [f32; 4]) {
    let actual = actual.expect("expected rect");
    for i in 0..4 {
        assert!(
            (actual[i] - expected[i]).abs() <= 1.,
            "rect mismatch at index {i}: actual={actual:?}, expected={expected:?}"
        );
    }
}

#[test]
fn hidden_cursor_keeps_ime_enabled_renders_preedit_commits_and_clears_on_disable() {
    let (mut ws, _host_rx) = tabbed_workspace("single");
    let (sender, sent) = flume::unbounded();
    let mut terminal = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
        cols: 40,
        rows: 10,
        ..Default::default()
    })
    .unwrap();
    terminal.vt_write(b"prompt\x1b[?25l\x1b[3;5H");
    attach_terminal(&mut ws, "block:a", &sender, &terminal);

    let mut rt = ime_runtime(ws);
    let grid = rect_of(&rt, "grid:block:a").expect("grid laid out");
    let cell = rt.state.metrics().cell;
    let expected = [grid[0] + 4. * cell.0, grid[1] + 2. * cell.1, cell.0, cell.1];

    assert_rect_near(rt.ime_area, expected);
    let stable_area = rt.ime_area.expect("IME area enabled");
    assert!(
        rect_of(&rt, "gblock:a/cursor").is_none(),
        "hidden hardware cursor draws no cursor element"
    );
    assert_rect_near(rect_of(&rt, "gblock:a/ime"), expected);

    // An empty preedit outside composition leaves the hidden-cursor anchor enabled.
    rt.ime(Ime::Preedit(String::new(), None));
    rt.frame(1. / 60.);
    assert_eq!(rt.ime_area, Some(stable_area));
    assert!(rect_of(&rt, "gblock:a/preedit").is_none());
    assert!(sent.try_iter().next().is_none());

    // Preedit renders inline at the anchor while the cursor stays hidden.
    rt.ime(Ime::Preedit("ni".into(), None));
    rt.frame(1. / 60.);
    let preedit = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some("gblock:a/preedit"))
        .expect("preedit rendered");
    assert_eq!(preedit.text.as_deref(), Some("ni"));
    assert!(
        (preedit.rect[0] - expected[0]).abs() <= 1. && (preedit.rect[1] - expected[1]).abs() <= 1.,
        "preedit positioned at anchor: preedit={:?}, expected={expected:?}",
        preedit.rect
    );

    // Commit sends one Command::Paste and clears the preedit.
    rt.ime(Ime::Commit("你好".into()));
    rt.frame(1. / 60.);
    let pastes: Vec<String> = sent
        .try_iter()
        .filter_map(|cmd| match cmd {
            HostCommand::Terminal {
                command: Command::Paste(text),
                ..
            } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(pastes, ["你好"]);
    assert!(
        rt.state.terminals[&rt.state.term_key("block:a")]
            .preedit
            .is_empty()
    );
    assert!(rect_of(&rt, "gblock:a/preedit").is_none());

    // Empty preedit and Ime::Disabled both clear open preedit without sending a paste.
    rt.ime(Ime::Preedit("かん".into(), None));
    rt.frame(1. / 60.);
    assert!(rect_of(&rt, "gblock:a/preedit").is_some());
    rt.ime(Ime::Preedit(String::new(), None));
    rt.frame(1. / 60.);
    assert!(rect_of(&rt, "gblock:a/preedit").is_none());
    assert_eq!(rt.ime_area, Some(stable_area));
    assert!(sent.try_iter().next().is_none());

    rt.ime(Ime::Preedit("かん".into(), None));
    rt.frame(1. / 60.);
    assert!(rect_of(&rt, "gblock:a/preedit").is_some());
    rt.ime(Ime::Disabled);
    rt.frame(1. / 60.);
    assert!(
        rt.state.terminals[&rt.state.term_key("block:a")]
            .preedit
            .is_empty()
    );
    assert!(rect_of(&rt, "gblock:a/preedit").is_none());
    assert!(sent.try_iter().next().is_none());

    // Rapid hidden -> visible -> hidden transitions never drop ime_area.
    for (seq, expect_cursor) in [
        (&b"\x1b[?25h"[..], true),
        (&b"\x1b[?25l"[..], false),
        (&b"\x1b[?25h"[..], true),
        (&b"\x1b[?25l"[..], false),
    ] {
        terminal.vt_write(seq);
        push_frame(&mut rt, "block:a", &terminal);
        assert_eq!(rt.ime_area, Some(stable_area));
        assert_eq!(rect_of(&rt, "gblock:a/cursor").is_some(), expect_cursor);
    }
}

#[test]
fn ime_anchor_respects_panel_window_focus_find_field_and_split_focus_gates() {
    let (mut ws, _host_rx) = tabbed_workspace("split");
    let (sender, _sent) = flume::unbounded();
    let mut term_b = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
        cols: 30,
        rows: 8,
        ..Default::default()
    })
    .unwrap();
    term_b.vt_write(b"left\x1b[?25l");
    let mut term_c = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
        cols: 30,
        rows: 8,
        ..Default::default()
    })
    .unwrap();
    term_c.vt_write(b"right\x1b[?25l");
    attach_terminal(&mut ws, "block:b", &sender, &term_b);
    attach_terminal(&mut ws, "block:c", &sender, &term_c);

    let mut rt = ime_runtime(ws);
    let grid_b = rect_of(&rt, "grid:block:b").expect("focused split grid");
    let anchor_b = rect_of(&rt, "gblock:b/ime").expect("focused split IME anchor");
    assert!(
        rect_of(&rt, "gblock:c/ime").is_none(),
        "unfocused split pane emits no IME anchor"
    );
    assert_rect_near(rt.ime_area, anchor_b);
    let area_b = rt.ime_area.expect("focused split IME area");
    assert!(
        anchor_b[0] >= grid_b[0] && anchor_b[0] < grid_b[0] + grid_b[2],
        "IME anchor lies inside the focused split pane"
    );

    // Unfocusing the window removes the grid IME anchor and disables IME.
    rt.window_focus(false);
    rt.frame(1. / 60.);
    assert!(rect_of(&rt, "gblock:b/ime").is_none());
    assert_eq!(rt.ime_area, None);
    rt.window_focus(true);
    rt.frame(1. / 60.);
    assert_eq!(rt.ime_area, Some(area_b));

    // Focusing the find bar removes the grid IME anchor (the find field owns IME).
    ctrl_shift(&mut rt, 'f');
    assert!(
        rect_of(&rt, "gblock:b/ime").is_none(),
        "grid yields IME while find field is focused"
    );
    press(&mut rt, Key::Escape);
    assert_eq!(
        rect_of(&rt, "gblock:b/ime"),
        Some(anchor_b),
        "closing find restores the grid IME anchor"
    );

    // Opening a modal dialog removes the grid IME anchor (confirm has no text field -> None).
    rt.update(|s, _| {
        s.panel = Some(crate::app::panel::Panel::new(
            PanelKind::Confirm {
                method: "session.close_window".into(),
                payload: serde_json::json!({}),
                description: "Ends programs.".into(),
            },
            "Close this tab?",
            &[],
        ));
    });
    rt.frame(1. / 60.);
    assert!(rect_of(&rt, "gblock:b/ime").is_none());
    assert_eq!(rt.ime_area, None);
}

#[test]
fn pointer_drag_starting_on_the_ime_anchor_cell_still_selects_text() {
    let (mut ws, _host_rx) = tabbed_workspace("single");
    let (sender, sent) = flume::unbounded();
    let mut terminal = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
        cols: 40,
        rows: 5,
        ..Default::default()
    })
    .unwrap();
    // Place hidden cursor right on top of the text we will drag across.
    terminal.vt_write(b"hello world\x1b[1;3H\x1b[?25l");
    attach_terminal(&mut ws, "block:a", &sender, &terminal);

    let mut rt = ime_runtime(ws);
    let anchor = rect_of(&rt, "gblock:a/ime").expect("IME anchor over row 0, col 2");
    let cell = rt.state.metrics().cell;
    let start_x = anchor[0] + cell.0 * 0.5;
    let start_y = anchor[1] + cell.1 * 0.5;
    let end_x = start_x + cell.0 * 4.;

    // Drain any initial resize commands before dragging.
    while sent.try_recv().is_ok() {}

    rt.pointer_down(start_x, start_y);
    rt.pointer_move(end_x, start_y);
    rt.pointer_up(end_x, start_y);
    rt.frame(1. / 60.);

    let selection_events = sent
        .try_iter()
        .filter(|cmd| {
            matches!(
                cmd,
                HostCommand::Terminal {
                    command: Command::Selection(_),
                    ..
                }
            )
        })
        .count();
    assert!(
        selection_events >= 3,
        "drag starting on the IME anchor reached the grid selection handler ({selection_events} events)"
    );
    let key = rt.state.term_key("block:a");
    assert_eq!(
        rt.state.terminals[&key].copy_text().as_deref(),
        Some("llo w")
    );
}
