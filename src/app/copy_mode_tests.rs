//! Headless UI tests and real-font visual captures for Vim copy mode (#149 slice 2).
use super::*;
use crate::app::{
    actions::{self, Remote},
    copy_mode::{self, Prefix, SearchDirection},
    panel::SettingsSection,
    select_copy,
};
use rex_rune::{
    connection::{Command, Connection, HostCommand, TerminalSender},
    keymap::Overrides,
    settings::interface::ToolbarBehavior,
    terminal::{
        Renderer,
        copy_mode::{Controller, CopyModeInput, CopyModeOp, CopyMotion, CopyUnit, MAX_COUNT},
        search,
    },
};
use rune::{
    App, AppSpec, Env, Ime, Key, KeyEvent, Mods, PhysicalInput, PhysicalKeyCode as Code,
    PointerButton, Runtime,
};
use serde_json::{Value, json};
use std::sync::Arc;

/// Live terminal harness backed by a real Ghostty `Terminal`, `copy_mode::Controller`,
/// `search::Controller`, and `Renderer` so headless UI tests verify end-to-end state.
struct Harness {
    terminal: ghostty_vt::Terminal,
    copy: Controller,
    search: search::Controller,
    renderer: Renderer,
    selection_revision: u64,
    block: String,
}

impl Harness {
    fn new(cols: u16, rows: u16, block: &str, output: &[u8]) -> Self {
        let mut terminal = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
            cols,
            rows,
            max_scrollback: 1 << 20,
        })
        .unwrap();
        terminal.vt_write(output);
        Self {
            terminal,
            copy: Controller::default(),
            search: search::Controller::default(),
            renderer: Renderer::new().unwrap(),
            selection_revision: 0,
            block: block.to_owned(),
        }
    }

    fn attach(&mut self, ws: &mut Workspace, sender: &flume::Sender<HostCommand>) {
        let connection = Connection {
            model: Default::default(),
            commands: TerminalSender::new(sender.clone(), "s".into(), self.block.clone()),
            wake: flume::unbounded().1,
        };
        let frame = self.build_frame();
        {
            let mut m = connection.model.lock().unwrap();
            m.connected = true;
            m.control_connected = true;
            m.frame = Some(Arc::new(frame));
        }
        let key = ws.term_key(&self.block);
        let mut term = crate::app::term::Term::new(connection.clone());
        term.refresh();
        ws.terminals.insert(key, term);
        ws.hosts[0].model.visible = true;
        ws.hosts[0]
            .model
            .terminals
            .insert(self.block.clone(), connection);
        *ws.hosts[0].connection.model.lock().unwrap() = ws.hosts[0].model.clone();
    }

    fn build_frame(&mut self) -> rex_rune::terminal::Frame {
        for _ in 0..1000 {
            if !self.search.update(&self.terminal).unwrap() {
                break;
            }
        }
        self.copy.sync(&mut self.terminal).unwrap();
        let mut frame = self.renderer.frame(&self.terminal).unwrap();
        frame.selection_revision = self.selection_revision;
        frame.copy = self.copy.view(&self.terminal).unwrap();
        frame.search = self.search.results(&self.terminal).unwrap();
        frame
    }

    /// Drain all terminal commands queued on `rx` for this harness's block, apply
    /// copy-mode/search/scroll operations against the real Ghostty terminal, push the
    /// resulting frame + `copy_yank` + `copy_ended_epoch` into `rt`, and wake the UI.
    /// Returns every non-CopyMode/Search/Scroll/Resize/CursorStyle command observed so
    /// tests can assert zero PTY leaks.
    fn pump(
        &mut self,
        rt: &mut Runtime<Workspace>,
        rx: &flume::Receiver<HostCommand>,
    ) -> Vec<Command> {
        let mut other = Vec::new();
        let mut yank = None;
        let mut pending_search: Option<(u64, Vec<bool>)> = None;
        for cmd in rx.try_iter() {
            let HostCommand::Terminal { block, command, .. } = cmd else {
                continue;
            };
            if block != self.block {
                continue;
            }
            match command {
                Command::CopyMode(input) => {
                    let is_to_match = matches!(input.op, CopyModeOp::ToMatch);
                    let outcome = self
                        .copy
                        .apply(&mut self.terminal, Some(&self.search), input)
                        .unwrap();
                    if outcome.accepted {
                        self.selection_revision = input.revision;
                        if let Some(y) = outcome.yank {
                            yank = Some(y);
                        }
                        if is_to_match && self.search.has_needle() && !self.search.is_settled() {
                            pending_search = Some((input.epoch, Vec::new()));
                        } else {
                            pending_search = None;
                        }
                    }
                }
                Command::Search(needle) => {
                    pending_search = None;
                    self.search
                        .set_needle(Some(&self.terminal), needle)
                        .unwrap();
                }
                Command::SearchStep(older) => {
                    if self.search.has_needle()
                        && !self.search.is_settled()
                        && let Some(epoch) = self.copy.active_epoch()
                    {
                        pending_search
                            .get_or_insert((epoch, Vec::new()))
                            .1
                            .push(older);
                    } else {
                        self.search.step(&self.terminal, older).unwrap();
                        if self.copy.is_active() {
                            self.copy
                                .move_to_match(&mut self.terminal, &self.search)
                                .unwrap();
                        }
                    }
                }
                Command::Scroll(delta) => {
                    pending_search = None;
                    let before = self.terminal.scrollbar().unwrap().offset as usize;
                    self.terminal
                        .scroll_viewport(ghostty_vt::terminal::ScrollViewport::Delta(delta));
                    self.copy
                        .after_viewport_scroll_delta(&mut self.terminal, before, delta)
                        .unwrap();
                }
                Command::Resize(..) | Command::CursorStyle(_) => {}
                leaked => other.push(leaked),
            }
        }
        // Mirror `run_data`'s tick arm: settle search first, then finish any queued ToMatch/steps.
        for _ in 0..1000 {
            if !self.search.update(&self.terminal).unwrap() {
                break;
            }
        }
        if let Some((epoch, steps)) = pending_search.take()
            && self.copy.active_epoch() == Some(epoch)
            && self.search.has_needle()
        {
            for older in steps {
                self.search.step(&self.terminal, older).unwrap();
            }
            self.copy
                .move_to_match(&mut self.terminal, &self.search)
                .unwrap();
        }
        let frame = Arc::new(self.build_frame());
        let ended = self.copy.ended_epoch();
        let block = self.block.clone();
        rt.update(|s, tx| {
            let key = s.term_key(&block);
            if let Some(term) = s.terminals.get(&key) {
                let mut m = term.connection.model.lock().unwrap();
                m.frame = Some(frame);
                if let Some(y) = yank {
                    m.copy_yank = Some(y);
                }
                if ended > m.copy_ended_epoch {
                    m.copy_ended_epoch = ended;
                }
            }
            s.terminal_woke(&key, tx);
        });
        rt.frame(1. / 60.);
        other
    }
}

fn copy_runtime(
    ws: Workspace,
    size: (f32, f32),
) -> (Runtime<Workspace>, rune::clipboard::MemoryClipboard) {
    let mut rt = App::new(AppSpec::new("Rex", size, placeholder, view))
        .init(move || ws)
        .on_ime(crate::app::input::on_ime)
        .headless_runtime(size, Env::new(true, 1.));
    rt.on_key = Some(crate::app::input::on_key);
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    for _ in 0..8 {
        rt.frame(1. / 60.);
    }
    (rt, clipboard)
}

fn key_ev(key: Key, mods: Mods) -> KeyEvent {
    KeyEvent { key, mods }
}

fn send_key(
    rt: &mut Runtime<Workspace>,
    code: Option<Code>,
    ev: KeyEvent,
    down: bool,
    repeat: bool,
) {
    if let Some(code) = code {
        rt.physical_key_input(
            PhysicalInput::Key {
                key: code,
                mods: ev.mods,
                pressed: down,
                repeat,
            },
            ev,
            None,
        );
    } else if down {
        rt.key(ev);
    } else {
        rt.key_up(ev);
    }
    rt.frame(1. / 60.);
}

fn tap_text(rt: &mut Runtime<Workspace>, text: &str) {
    let shift = text.len() == 1 && text.chars().next().is_some_and(|c| c.is_ascii_uppercase());
    let mods = Mods {
        shift,
        ..Mods::default()
    };
    let ev = key_ev(Key::Text(text.into()), mods);
    rt.key(ev.clone());
    rt.key_up(ev);
    rt.frame(1. / 60.);
}

fn tap_ctrl(rt: &mut Runtime<Workspace>, code: Code, c: char) {
    let ev = key_ev(
        Key::Command(c),
        Mods {
            ctrl: true,
            cmd: true,
            ..Mods::default()
        },
    );
    send_key(rt, Some(code), ev.clone(), true, false);
    send_key(rt, Some(code), ev, false, false);
}

fn tap_ctrl_shift(rt: &mut Runtime<Workspace>, code: Code, c: char) {
    let ev = key_ev(
        Key::Command(c),
        Mods {
            ctrl: true,
            cmd: true,
            shift: true,
            ..Mods::default()
        },
    );
    send_key(rt, Some(code), ev.clone(), true, false);
    send_key(rt, Some(code), ev, false, false);
}

fn tap_key(rt: &mut Runtime<Workspace>, code: Code, key: Key) {
    let ev = key_ev(key, Mods::default());
    send_key(rt, Some(code), ev.clone(), true, false);
    send_key(rt, Some(code), ev, false, false);
}

fn drain_pty(rx: &flume::Receiver<HostCommand>) -> Vec<String> {
    rx.try_iter()
        .filter_map(|cmd| match cmd {
            HostCommand::Terminal { command, .. } => match command {
                Command::Key(v) => Some(format!("Key({v})")),
                Command::KeyWithPrefix { key, prefix } => {
                    Some(format!("KeyWithPrefix({prefix:?},{key})"))
                }
                Command::SendKey(v) => Some(format!("SendKey({v})")),
                Command::Paste(s) => Some(format!("Paste({s:?})")),
                Command::Mouse(v) => Some(format!("Mouse({v})")),
                Command::Wheel(v) => Some(format!("Wheel({v})")),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn drain_copy_ops(rx: &flume::Receiver<HostCommand>) -> Vec<CopyModeInput> {
    rx.try_iter()
        .filter_map(|cmd| match cmd {
            HostCommand::Terminal {
                command: Command::CopyMode(input),
                ..
            } => Some(input),
            _ => None,
        })
        .collect()
}

fn mode_chip_text(rt: &Runtime<Workspace>) -> Option<String> {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some("mode-name"))
        .and_then(|e| e.text)
}

fn rect_of(rt: &Runtime<Workspace>, key: &str) -> Option<[f32; 4]> {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .map(|e| e.rect)
}

#[test]
fn single_action_registration_keymap_shortcuts_and_entry_guards() {
    let spec = actions::find(copy_mode::ACTION).expect("pane.copy_mode registered");
    assert_eq!(spec.title(), "Copy mode");
    assert!(matches!(spec.remote(), Remote::Never(_)));
    assert!(spec.accepts(&Value::Null));
    assert!(spec.accepts(&json!({})));
    assert!(!spec.accepts(&json!({"unexpected": 1})));

    // Single recordable row in Settings -> Shortcuts, zero copy-* action bloat.
    let (mut ws, _host_rx) = tabbed_workspace("single");
    let copy_rows: Vec<_> = ws
        .shortcut_rows()
        .into_iter()
        .filter(|r| r.action == copy_mode::ACTION)
        .collect();
    assert_eq!(copy_rows.len(), 1);
    assert_eq!(copy_rows[0].title, "Copy mode");
    assert!(copy_rows[0].recordable);
    assert_eq!(copy_rows[0].keys, vec![vec![vec!["Ctrl", "Shift", "X"]]]);
    assert!(
        !ws.shortcut_rows()
            .iter()
            .any(|r| r.action.starts_with("copy."))
    );

    // Without a frame, executing pane.copy_mode informs cleanly and does not enter.
    let mut tx = rune::Tx::default();
    assert!(actions::run(
        &mut ws,
        copy_mode::ACTION,
        &Value::Null,
        &mut tx
    ));
    assert!(!ws.in_copy_mode());
    assert!(!ws.notices.is_empty());

    // With a framed live pane, entering cancels layer drag, clears preedit, allocates
    // monotonic epochs > copy_ended_epoch, and is idempotent.
    let (sender, sent) = flume::unbounded();
    let mut harness = Harness::new(40, 10, "block:a", b"hello world");
    harness.attach(&mut ws, &sender);
    {
        let key = ws.term_key("block:a");
        let term = ws.terminals.get_mut(&key).unwrap();
        term.preedit = "compose".into();
        term.connection.model.lock().unwrap().copy_ended_epoch = 4;
    }
    assert!(actions::run(
        &mut ws,
        copy_mode::ACTION,
        &Value::Null,
        &mut tx
    ));
    assert!(ws.in_copy_mode());
    let active = ws.active_copy_mode().unwrap();
    assert_eq!(active.epoch, 5, "epoch must be > copy_ended_epoch");
    assert!(
        ws.terminals[&ws.term_key("block:a")].preedit.is_empty(),
        "IME preedit cleared on entry"
    );
    let ops = drain_copy_ops(&sent);
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].op, CopyModeOp::Begin);
    assert_eq!(ops[0].epoch, 5);

    // Second invocation on the same pane is idempotent (no second Begin).
    assert!(actions::run(
        &mut ws,
        copy_mode::ACTION,
        &Value::Null,
        &mut tx
    ));
    assert!(drain_copy_ops(&sent).is_empty());
    assert_eq!(ws.active_copy_mode().unwrap().epoch, 5);

    // Exit and re-enter allocates epoch 6.
    ws.exit_copy_mode();
    assert!(!ws.in_copy_mode());
    assert!(actions::run(
        &mut ws,
        copy_mode::ACTION,
        &Value::Null,
        &mut tx
    ));
    assert_eq!(ws.active_copy_mode().unwrap().epoch, 6);
}

#[test]
fn zero_pty_leaks_across_all_copy_mode_states_and_input_paths() {
    let (mut ws, _host_rx) = tabbed_workspace("single");
    // Enable PRIMARY selection and mouse tracking + bind client.key.forward and pane.send_key.
    ws.settings.terminal.primary_selection = true;
    ws.settings.shortcut_file.overrides = Overrides::parse(&json!([
        {"keys": "ctrl+l", "action": "client.key.forward"},
        {"keys": "ctrl+alt+z", "action": "pane.send_key", "args": {"key": "ctrl+c"}}
    ]));
    ws.local_keymap = actions::resolve(&ws.settings.shortcut_file.overrides);
    ws.keymap_generation += 1;

    let (sender, sent) = flume::unbounded();
    // Turn on mouse tracking (1000 + 1002 + 1006) in the terminal.
    let mut harness = Harness::new(
        40,
        8,
        "block:a",
        b"\x1b[?1000h\x1b[?1002h\x1b[?1006halpha beta gamma\r\nsecond line\r\nthird line",
    );
    harness.attach(&mut ws, &sender);
    let (mut rt, clipboard) = copy_runtime(ws, (1024., 700.));
    clipboard.set_text("should-not-paste");
    select_copy::memory::set("primary-should-not-paste".into());
    let _ = drain_pty(&sent);

    // Enter copy mode via default Ctrl+Shift+X.
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(rt.state.in_copy_mode());
    assert!(harness.pump(&mut rt, &sent).is_empty());

    // IME anchor must be disabled while copy mode is active.
    assert!(
        rect_of(&rt, "gblock:a/ime").is_none(),
        "IME anchor disabled during copy mode"
    );

    // Exercise hostile keys across Normal, Char, Line, Block, pending count, pending g,
    // pending y, and open-but-unfocused find bar.
    let prepare_state = |rt: &mut Runtime<Workspace>, harness: &mut Harness, idx: usize| {
        if !rt.state.in_copy_mode() {
            tap_ctrl_shift(rt, Code::KeyX, 'x');
            assert!(harness.pump(rt, &sent).is_empty());
        }
        // Reset pending state/selection cleanly before preparing target state.
        let had_unit = rt
            .state
            .active_copy_mode()
            .is_some_and(|cm| cm.unit.is_some());
        rt.update(|s, _| {
            if let Some(cm) = s.copy_mode.as_mut() {
                cm.count = None;
                cm.prefix = None;
            }
        });
        if had_unit {
            tap_key(rt, Code::Escape, Key::Escape);
            assert!(harness.pump(rt, &sent).is_empty());
        }
        match idx {
            0 => {} // Normal
            1 => tap_text(rt, "v"),
            2 => tap_text(rt, "V"),
            3 => tap_ctrl(rt, Code::KeyV, 'v'),
            4 => {
                tap_text(rt, "1");
                tap_text(rt, "2");
            }
            5 => tap_text(rt, "g"),
            6 => tap_text(rt, "y"),
            7 => {
                // Open find bar, type needle, press Enter so find bar is open-but-unfocused.
                tap_text(rt, "/");
                tap_text(rt, "a");
                tap_key(rt, Code::Enter, Key::Enter);
                assert!(rt.state.in_copy_mode());
                let key = rt.state.term_key("block:a");
                assert!(
                    rt.state.terminals[&key]
                        .search
                        .as_ref()
                        .is_some_and(|e| !e.focused)
                );
            }
            _ => unreachable!(),
        }
        assert!(harness.pump(rt, &sent).is_empty());
    };

    for state_idx in 0..8 {
        prepare_state(&mut rt, &mut harness, state_idx);
        // Unbound letters/punctuation, Backspace, dead keys, modifier-only keys,
        // client.key.forward (Ctrl+L), pane.paste (Ctrl+Shift+V), pane.send_key (Ctrl+Alt+Z).
        for ch in ["a", "i", "x", ":", "!", "@", " "] {
            tap_text(&mut rt, ch);
        }
        tap_key(&mut rt, Code::Backspace, Key::Backspace);
        send_key(
            &mut rt,
            None,
            key_ev(Key::Dead(Some('^')), Mods::default()),
            true,
            false,
        );
        send_key(
            &mut rt,
            Some(Code::ShiftLeft),
            key_ev(
                Key::Modifier {
                    modifier: rune::Modifier::ShiftLeft,
                    pressed: true,
                },
                Mods {
                    shift: true,
                    ..Mods::default()
                },
            ),
            true,
            false,
        );
        tap_ctrl(&mut rt, Code::KeyL, 'l'); // client.key.forward
        tap_ctrl_shift(&mut rt, Code::KeyV, 'v'); // pane.paste
        let send_key_ev = key_ev(
            Key::Command('z'),
            Mods {
                ctrl: true,
                cmd: true,
                alt: true,
                ..Mods::default()
            },
        );
        send_key(&mut rt, Some(Code::KeyZ), send_key_ev.clone(), true, false);
        send_key(&mut rt, Some(Code::KeyZ), send_key_ev, false, false);
        if state_idx == 0 || state_idx == 4 || state_idx == 7 {
            tap_key(&mut rt, Code::Enter, Key::Enter);
        }
        // IME Preedit + Commit must be swallowed with zero Command::Paste.
        rt.ime(Ime::Preedit("漢字".into(), None));
        rt.ime(Ime::Commit("漢字".into()));
        rt.frame(1. / 60.);
        assert!(
            rt.state.terminals[&rt.state.term_key("block:a")]
                .preedit
                .is_empty()
        );
        let leaked = harness.pump(&mut rt, &sent);
        assert!(
            leaked.is_empty(),
            "state {state_idx} leaked {} PTY commands",
            leaked.len()
        );
    }

    // Middle-click (PRIMARY paste) and mouse-tracking wheel/motion/Shift+right-click never leak.
    prepare_state(&mut rt, &mut harness, 0);
    let grid = rect_of(&rt, "grid:block:a").unwrap();
    let cx = grid[0] + 20.;
    let cy = grid[1] + 20.;
    rt.pointer_move(cx, cy);
    rt.frame(1. / 60.);
    rt.button_down(cx, cy, PointerButton::Middle);
    rt.button_up(cx, cy, PointerButton::Middle);
    rt.frame(1. / 60.);
    rt.wheel(cx, cy, 40.);
    rt.frame(1. / 60.);
    // Shift+right-click while in copy mode stays in copy mode, does not open the pane menu,
    // and never leaks a mouse-tracking press/release to the PTY.
    rt.physical_input(PhysicalInput::Modifiers(Mods {
        shift: true,
        ..Mods::default()
    }));
    rt.secondary_down(cx, cy);
    rt.secondary_up(cx, cy);
    rt.frame(1. / 60.);
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    rt.frame(1. / 60.);
    assert!(
        rt.state.in_copy_mode(),
        "Shift+right-click in copy mode must keep copy mode active"
    );
    assert!(
        rt.state.panel.is_none(),
        "Shift+right-click in copy mode must not open pane menu"
    );
    assert!(
        drain_pty(&sent).is_empty(),
        "middle-click/wheel/motion/shift+right-click must not leak to PTY"
    );

    // File drops onto a pane while in copy mode must NOT highlight drop_hover or upload to PTY.
    let drop_paths = vec![std::path::PathBuf::from("/tmp/should-not-upload.txt")];
    rt.drag_files(drop_paths.clone(), Some((cx, cy)));
    rt.frame(1. / 60.);
    assert!(
        rt.state.drop_hover.is_none(),
        "copy mode must suppress drop hover highlight"
    );
    rt.drop_files(drop_paths, Some((cx, cy)));
    rt.frame(1. / 60.);
    assert!(
        rt.state.in_copy_mode(),
        "file drop must not alter copy mode"
    );
    assert!(
        !_host_rx
            .try_iter()
            .any(|c| matches!(c, HostCommand::Upload { .. })),
        "file drop in copy mode must never emit HostCommand::Upload"
    );

    // Left-click inside pane exits copy mode without leaking mouse-tracking press.
    assert!(
        rt.state.in_copy_mode(),
        "must still be in copy mode immediately before left-click exit"
    );
    rt.pointer_down(cx, cy);
    rt.pointer_up(cx, cy);
    rt.frame(1. / 60.);
    assert!(!rt.state.in_copy_mode());
    let leaked = drain_pty(&sent);
    assert!(
        leaked.is_empty(),
        "left-click exit must not leak mouse-tracking press to PTY: {leaked:?}"
    );

    // Held motion keys (`j` and `Ctrl+D`) whose press stayed in copy mode MUST NOT leak
    // key repeats to the PTY if copy mode ends automatically via a non-keyboard trigger.
    for (code, ev) in [
        (Code::KeyJ, key_ev(Key::Text("j".into()), Mods::default())),
        (
            Code::KeyD,
            key_ev(
                Key::Command('d'),
                Mods {
                    ctrl: true,
                    cmd: true,
                    ..Mods::default()
                },
            ),
        ),
    ] {
        tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
        assert!(harness.pump(&mut rt, &sent).is_empty());
        assert!(rt.state.in_copy_mode());
        // Initial press is handled by copy mode (leaves consumed=false so it can repeat).
        send_key(&mut rt, Some(code), ev.clone(), true, false);
        assert!(harness.pump(&mut rt, &sent).is_empty());
        // Automatic worker exit (e.g. program switches to alternate screen / eviction).
        let epoch = rt.state.active_copy_mode().unwrap().epoch;
        rt.update(|s, tx| {
            let key = s.term_key("block:a");
            s.terminals[&key]
                .connection
                .model
                .lock()
                .unwrap()
                .copy_ended_epoch = epoch;
            s.terminal_woke(&key, tx);
        });
        assert!(!rt.state.in_copy_mode());
        // Subsequent repeats and release from the same held key must NEVER reach the PTY!
        send_key(&mut rt, Some(code), ev.clone(), true, true);
        send_key(&mut rt, Some(code), ev.clone(), true, true);
        send_key(&mut rt, Some(code), ev, false, false);
        assert!(
            drain_pty(&sent).is_empty(),
            "held {code:?} repeat after automatic copy-mode exit leaked to PTY"
        );
    }

    // Releasing a modifier that was pressed BEFORE entering copy mode still forwards its
    // release without terminating copy mode (prevents stuck modifiers on kitty clients).
    let ctrl_down = key_ev(
        Key::Modifier {
            modifier: rune::Modifier::ControlLeft,
            pressed: true,
        },
        Mods {
            ctrl: true,
            cmd: true,
            ..Mods::default()
        },
    );
    let ctrl_up = key_ev(
        Key::Modifier {
            modifier: rune::Modifier::ControlLeft,
            pressed: false,
        },
        Mods::default(),
    );
    send_key(&mut rt, Some(Code::ControlLeft), ctrl_down, true, false);
    let _ = drain_pty(&sent);
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(harness.pump(&mut rt, &sent).is_empty());
    assert!(rt.state.in_copy_mode());
    send_key(&mut rt, Some(Code::ControlLeft), ctrl_up, false, false);
    assert!(
        rt.state.in_copy_mode(),
        "draining pre-entry modifier release must not exit copy mode"
    );
    assert!(
        drain_copy_ops(&sent).is_empty(),
        "pre-entry modifier release must not send CopyModeOp::End"
    );
    tap_text(&mut rt, "q");
    assert!(!rt.state.in_copy_mode());
    let _ = harness.pump(&mut rt, &sent);

    // Key repeat + key release after exiting with q, y, Escape must NEVER leak to PTY.
    for (exit_code, exit_ev) in [
        (Code::KeyQ, key_ev(Key::Text("q".into()), Mods::default())),
        (Code::Escape, key_ev(Key::Escape, Mods::default())),
        (
            Code::KeyY,
            key_ev(
                Key::Text("Y".into()),
                Mods {
                    shift: true,
                    ..Mods::default()
                },
            ),
        ),
    ] {
        tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
        let _ = harness.pump(&mut rt, &sent);
        assert!(rt.state.in_copy_mode());
        // Press exits copy mode.
        send_key(&mut rt, Some(exit_code), exit_ev.clone(), true, false);
        assert!(!rt.state.in_copy_mode());
        // Held repeat right after exit must stay swallowed!
        send_key(&mut rt, Some(exit_code), exit_ev.clone(), true, true);
        // Key release after exit must stay swallowed!
        send_key(&mut rt, Some(exit_code), exit_ev, false, false);
        assert!(
            drain_pty(&sent).is_empty(),
            "repeat/release after {exit_code:?} leaked to PTY"
        );
    }

    // Defense-in-depth in Term::send: Paste/SendKey/Reset/Clear exits copy mode first.
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    let _ = drain_copy_ops(&sent);
    rt.update(|s, _| {
        let key = s.term_key("block:a");
        s.terminals
            .get_mut(&key)
            .unwrap()
            .send(Command::Paste("direct".into()));
    });
    let commands: Vec<_> = sent
        .try_iter()
        .filter_map(|c| match c {
            HostCommand::Terminal { command, .. } => Some(command),
            _ => None,
        })
        .collect();
    assert!(matches!(
        commands.first(),
        Some(Command::CopyMode(CopyModeInput {
            op: CopyModeOp::End,
            ..
        }))
    ));
    assert!(matches!(commands.get(1), Some(Command::Paste(s)) if s == "direct"));
}

#[test]
fn layout_independent_symbols_and_modifier_preservation() {
    let (mut ws, _host_rx) = tabbed_workspace("single");
    let (sender, sent) = flume::unbounded();
    let mut harness = Harness::new(40, 8, "block:a", b"  alpha beta\r\nsecond");
    harness.attach(&mut ws, &sender);
    let (mut rt, _clipboard) = copy_runtime(ws, (1024., 700.));

    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    let _ = drain_copy_ops(&sent);

    // Send logical `$` and `^` with non-US physical codes (e.g. AZERTY/QWERTZ).
    send_key(
        &mut rt,
        Some(Code::BracketRight),
        key_ev(Key::Text("$".into()), Mods::default()),
        true,
        false,
    );
    send_key(
        &mut rt,
        Some(Code::BracketLeft),
        key_ev(Key::Text("^".into()), Mods::default()),
        true,
        false,
    );
    // Count `5` followed by Shift modifier press followed by `G` -> LineNumber(5).
    tap_text(&mut rt, "5");
    send_key(
        &mut rt,
        Some(Code::ShiftLeft),
        key_ev(
            Key::Modifier {
                modifier: rune::Modifier::ShiftLeft,
                pressed: true,
            },
            Mods {
                shift: true,
                ..Mods::default()
            },
        ),
        true,
        false,
    );
    assert_eq!(rt.state.active_copy_mode().unwrap().count, Some(5));
    tap_text(&mut rt, "G");

    let ops: Vec<CopyModeOp> = drain_copy_ops(&sent).into_iter().map(|i| i.op).collect();
    assert_eq!(
        ops,
        [
            CopyModeOp::Move {
                motion: CopyMotion::LineEnd,
                count: 1
            },
            CopyModeOp::Move {
                motion: CopyMotion::FirstNonBlank,
                count: 1
            },
            CopyModeOp::Move {
                motion: CopyMotion::LineNumber(5),
                count: 1
            },
        ]
    );

    // macOS Option+4 (`$`) and Windows/Linux AltGr+4 (`$`, `^`) dispatch symbol motions,
    // while Option/AltGr letters (`Alt+j`, `Ctrl+Alt+j`) stay swallowed without moving.
    send_key(
        &mut rt,
        Some(Code::Digit4),
        key_ev(
            Key::Text("$".into()),
            Mods {
                alt: true,
                ..Mods::default()
            },
        ),
        true,
        false,
    );
    send_key(
        &mut rt,
        Some(Code::Digit4),
        key_ev(
            Key::Text("$".into()),
            Mods {
                ctrl: true,
                alt: true,
                ..Mods::default()
            },
        ),
        true,
        false,
    );
    send_key(
        &mut rt,
        Some(Code::KeyJ),
        key_ev(
            Key::Text("j".into()),
            Mods {
                alt: true,
                ..Mods::default()
            },
        ),
        true,
        false,
    );
    send_key(
        &mut rt,
        Some(Code::KeyJ),
        key_ev(
            Key::Text("j".into()),
            Mods {
                ctrl: true,
                alt: true,
                ..Mods::default()
            },
        ),
        true,
        false,
    );
    let alt_ops: Vec<CopyModeOp> = drain_copy_ops(&sent).into_iter().map(|i| i.op).collect();
    assert_eq!(
        alt_ops,
        [
            CopyModeOp::Move {
                motion: CopyMotion::LineEnd,
                count: 1
            },
            CopyModeOp::Move {
                motion: CopyMotion::LineEnd,
                count: 1
            },
        ],
        "Option and AltGr `$` must dispatch LineEnd while Alt/AltGr letters stay swallowed"
    );

    // Logical `/` and `?` with non-US physical codes open forward and backward search.
    send_key(
        &mut rt,
        Some(Code::Digit7),
        key_ev(
            Key::Text("/".into()),
            Mods {
                shift: true,
                ..Mods::default()
            },
        ),
        true,
        false,
    );
    assert_eq!(
        rt.state.active_copy_mode().unwrap().search_direction,
        SearchDirection::Forward
    );
    tap_key(&mut rt, Code::Escape, Key::Escape);
    send_key(
        &mut rt,
        Some(Code::KeyM),
        key_ev(Key::Text("?".into()), Mods::default()),
        true,
        false,
    );
    assert_eq!(
        rt.state.active_copy_mode().unwrap().search_direction,
        SearchDirection::Backward
    );
}

#[test]
fn full_vim_workflow_counts_prefixes_yank_clipboard_primary_and_escape_hierarchy() {
    let (mut ws, _host_rx) = tabbed_workspace("single");
    ws.settings.terminal.copy_on_selection = true;
    ws.settings.terminal.primary_selection = true;
    let (sender, sent) = flume::unbounded();
    let mut harness = Harness::new(
        40,
        6,
        "block:a",
        b"line one alpha\r\nline two beta\r\nline three gamma\r\nline four delta",
    );
    harness.attach(&mut ws, &sender);
    let (mut rt, clipboard) = copy_runtime(ws, (1024., 700.));
    clipboard.set_text("sentinel");
    select_copy::memory::set("primary-sentinel".into());

    // Enter copy mode and move to top (`gg`).
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_text(&mut rt, "g");
    assert_eq!(rt.state.active_copy_mode().unwrap().prefix, Some(Prefix::G));
    assert_eq!(mode_chip_text(&rt).as_deref(), Some("copy · g"));
    tap_text(&mut rt, "g");
    assert!(harness.pump(&mut rt, &sent).is_empty());

    // Count clamping (99999j -> MAX_COUNT) and bare 0 -> LineStart.
    for ch in ["9", "9", "9", "9", "9"] {
        tap_text(&mut rt, ch);
    }
    assert_eq!(rt.state.active_copy_mode().unwrap().count, Some(MAX_COUNT));
    assert_eq!(mode_chip_text(&rt).as_deref(), Some("copy · 9999"));
    // Escape clears pending count first without exiting copy mode.
    tap_key(&mut rt, Code::Escape, Key::Escape);
    assert!(rt.state.in_copy_mode());
    assert_eq!(rt.state.active_copy_mode().unwrap().count, None);

    // Visual selection motions NEVER trigger copy_on_selection!
    tap_text(&mut rt, "0");
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_text(&mut rt, "v");
    assert_eq!(mode_chip_text(&rt).as_deref(), Some("copy · visual"));
    tap_text(&mut rt, "e");
    assert!(harness.pump(&mut rt, &sent).is_empty());
    assert_eq!(
        clipboard.text().as_deref().map(String::as_str),
        Some("sentinel"),
        "visual motion must not trigger copy_on_selection"
    );

    // Escape in visual mode clears selection and stays in copy mode.
    tap_key(&mut rt, Code::Escape, Key::Escape);
    assert!(rt.state.in_copy_mode());
    assert_eq!(rt.state.active_copy_mode().unwrap().unit, None);
    assert!(harness.pump(&mut rt, &sent).is_empty());

    // Select word from start of line with `0`, `v`, `e`, swap ends with `o`, and yank with `y`.
    tap_text(&mut rt, "0");
    tap_text(&mut rt, "v");
    tap_text(&mut rt, "e");
    tap_text(&mut rt, "o");
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_text(&mut rt, "y");
    assert!(!rt.state.in_copy_mode());
    assert!(harness.pump(&mut rt, &sent).is_empty());
    assert_eq!(
        clipboard.text().as_deref().map(String::as_str),
        Some("line")
    );
    assert_eq!(select_copy::memory::get(), "line");

    // Duplicate wake with same yank (epoch, revision) does not re-copy after clipboard changes.
    clipboard.set_text("user-replaced");
    rt.update(|s, tx| {
        let key = s.term_key("block:a");
        s.terminal_woke(&key, tx);
    });
    assert_eq!(
        clipboard.text().as_deref().map(String::as_str),
        Some("user-replaced")
    );

    // `2yy` yanks two lines and exits cleanly.
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_text(&mut rt, "g");
    tap_text(&mut rt, "g");
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_text(&mut rt, "2");
    tap_text(&mut rt, "y");
    assert_eq!(mode_chip_text(&rt).as_deref(), Some("copy · 2y"));
    tap_text(&mut rt, "y");
    assert!(!rt.state.in_copy_mode());
    assert!(harness.pump(&mut rt, &sent).is_empty());
    assert_eq!(
        clipboard.text().as_deref().map(String::as_str),
        Some("line one alpha\nline two beta")
    );

    // `Y` yanks current line immediately; `Ctrl+Shift+C` yanks visual selection.
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_text(&mut rt, "g");
    tap_text(&mut rt, "g");
    tap_text(&mut rt, "j");
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_text(&mut rt, "Y");
    assert!(!rt.state.in_copy_mode());
    assert!(harness.pump(&mut rt, &sent).is_empty());
    assert_eq!(
        clipboard.text().as_deref().map(String::as_str),
        Some("line two beta")
    );

    // Ctrl+Shift+C with visual selection yanks it and exits; without visual selection is no-op.
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(harness.pump(&mut rt, &sent).is_empty());
    clipboard.set_text("keep-me");
    tap_ctrl_shift(&mut rt, Code::KeyC, 'c');
    assert!(
        rt.state.in_copy_mode(),
        "Ctrl+Shift+C without selection stays in copy mode"
    );
    assert_eq!(
        clipboard.text().as_deref().map(String::as_str),
        Some("keep-me")
    );
    tap_text(&mut rt, "g");
    tap_text(&mut rt, "g");
    tap_text(&mut rt, "V");
    assert_eq!(mode_chip_text(&rt).as_deref(), Some("copy · line"));
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_ctrl_shift(&mut rt, Code::KeyC, 'c');
    assert!(!rt.state.in_copy_mode());
    assert!(harness.pump(&mut rt, &sent).is_empty());
    assert_eq!(
        clipboard.text().as_deref().map(String::as_str),
        Some("line one alpha")
    );

    // `q` always exits immediately even with visual block + count active.
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_ctrl(&mut rt, Code::KeyV, 'v');
    assert_eq!(mode_chip_text(&rt).as_deref(), Some("copy · block"));
    tap_text(&mut rt, "3");
    assert_eq!(mode_chip_text(&rt).as_deref(), Some("copy · block · 3"));
    tap_text(&mut rt, "q");
    assert!(!rt.state.in_copy_mode());

    // `Ctrl+C` mirrors Escape hierarchy (prefix -> selection -> exit).
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_text(&mut rt, "v");
    tap_text(&mut rt, "g");
    tap_ctrl(&mut rt, Code::KeyC, 'c');
    assert!(rt.state.in_copy_mode());
    assert_eq!(rt.state.active_copy_mode().unwrap().prefix, None);
    assert_eq!(
        rt.state.active_copy_mode().unwrap().unit,
        Some(CopyUnit::Char)
    );
    tap_ctrl(&mut rt, Code::KeyC, 'c');
    assert!(rt.state.in_copy_mode());
    assert_eq!(rt.state.active_copy_mode().unwrap().unit, None);
    tap_ctrl(&mut rt, Code::KeyC, 'c');
    assert!(!rt.state.in_copy_mode());
}

#[test]
fn search_handoff_forward_backward_stepping_and_exit_cleanup() {
    let (mut ws, _host_rx) = tabbed_workspace("single");
    let (sender, sent) = flume::unbounded();
    let mut harness = Harness::new(
        40,
        6,
        "block:a",
        b"needle first\r\nfiller\r\nneedle second\r\nfiller\r\nneedle third",
    );
    harness.attach(&mut ws, &sender);
    let (mut rt, _clipboard) = copy_runtime(ws, (1024., 700.));

    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(harness.pump(&mut rt, &sent).is_empty());

    // `?` backward search -> Enter jumps straight to the first selected match without skipping.
    tap_text(&mut rt, "?");
    for ch in ["n", "e", "e", "d", "l", "e"] {
        tap_text(&mut rt, ch);
    }
    assert!(harness.pump(&mut rt, &sent).is_empty());
    let key = rt.state.term_key("block:a");
    let selected_before = rt.state.terminals[&key]
        .frame
        .as_ref()
        .unwrap()
        .search
        .as_ref()
        .unwrap()
        .index;
    assert_eq!(selected_before, Some(0), "newest match selected initially");

    tap_key(&mut rt, Code::Enter, Key::Enter);
    assert!(harness.pump(&mut rt, &sent).is_empty());
    assert!(rt.state.in_copy_mode());
    assert!(
        rt.state.terminals[&key]
            .search
            .as_ref()
            .is_some_and(|e| !e.focused),
        "find bar stays visible but blurred"
    );
    let frame = rt.state.terminals[&key].frame.as_ref().unwrap();
    assert_eq!(
        frame.search.as_ref().unwrap().index,
        Some(0),
        "Enter must not skip the first match"
    );
    assert_eq!(
        frame.copy.unwrap().cursor,
        frame.search.as_ref().unwrap().selected.unwrap().start,
        "copy cursor jumped straight to selected match"
    );

    // Under `?` (Backward): `n` steps older (index 0 -> 1), `N` steps newer (index 1 -> 0).
    tap_text(&mut rt, "n");
    assert!(harness.pump(&mut rt, &sent).is_empty());
    assert_eq!(
        rt.state.terminals[&key]
            .frame
            .as_ref()
            .unwrap()
            .search
            .as_ref()
            .unwrap()
            .index,
        Some(1)
    );
    tap_text(&mut rt, "N");
    assert!(harness.pump(&mut rt, &sent).is_empty());
    assert_eq!(
        rt.state.terminals[&key]
            .frame
            .as_ref()
            .unwrap()
            .search
            .as_ref()
            .unwrap()
            .index,
        Some(0)
    );

    // Under `/` (Forward): `N` steps older (index 0 -> 1), `n` steps newer (index 1 -> 0).
    tap_text(&mut rt, "/");
    for ch in ["n", "e", "e", "d", "l", "e"] {
        tap_text(&mut rt, ch);
    }
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_key(&mut rt, Code::Enter, Key::Enter);
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_text(&mut rt, "N");
    assert!(harness.pump(&mut rt, &sent).is_empty());
    assert_eq!(
        rt.state.terminals[&key]
            .frame
            .as_ref()
            .unwrap()
            .search
            .as_ref()
            .unwrap()
            .index,
        Some(1)
    );
    tap_text(&mut rt, "n");
    assert!(harness.pump(&mut rt, &sent).is_empty());
    assert_eq!(
        rt.state.terminals[&key]
            .frame
            .as_ref()
            .unwrap()
            .search
            .as_ref()
            .unwrap()
            .index,
        Some(0)
    );

    // Exiting copy mode clears search opened during copy mode so highlights do not linger.
    tap_text(&mut rt, "q");
    assert!(!rt.state.in_copy_mode());
    assert!(harness.pump(&mut rt, &sent).is_empty());
    assert!(rt.state.terminals[&key].search.is_none());
    assert!(
        rt.state.terminals[&key]
            .frame
            .as_ref()
            .unwrap()
            .search
            .is_none()
    );

    // Escape inside focused find bar closes find bar and stays in copy mode.
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_text(&mut rt, "/");
    tap_text(&mut rt, "x");
    tap_key(&mut rt, Code::Escape, Key::Escape);
    assert!(rt.state.in_copy_mode());
    assert!(rt.state.terminals[&key].search.is_none());
    assert!(harness.pump(&mut rt, &sent).is_empty());

    // Fast `/` + typing + `Enter` in the SAME pump (zero pre-tick) settles and jumps to match.
    tap_text(&mut rt, "g");
    tap_text(&mut rt, "g");
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_text(&mut rt, "/");
    for ch in ["n", "e", "e", "d", "l", "e"] {
        tap_text(&mut rt, ch);
    }
    tap_key(&mut rt, Code::Enter, Key::Enter);
    assert!(harness.pump(&mut rt, &sent).is_empty());
    {
        let frame = rt.state.terminals[&key].frame.as_ref().unwrap();
        let search = frame.search.as_ref().unwrap();
        assert_eq!(search.index, Some(0));
        assert_eq!(frame.copy.unwrap().cursor, search.selected.unwrap().start);
    }

    // Exit copy mode (preserving `find.last = "needle"` and clearing `term.search`), re-enter,
    // and press `2N` (default Forward direction -> N is older, count=2 -> index 1) without
    // opening the find bar first.
    tap_text(&mut rt, "q");
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_text(&mut rt, "2");
    tap_text(&mut rt, "N");
    assert!(harness.pump(&mut rt, &sent).is_empty());
    {
        let frame = rt.state.terminals[&key].frame.as_ref().unwrap();
        let search = frame.search.as_ref().unwrap();
        assert_eq!(
            search.index,
            Some(1),
            "2N re-opening find.last must land on 2nd older match"
        );
        assert_eq!(frame.copy.unwrap().cursor, search.selected.unwrap().start);
    }

    // Re-open with `n` (Forward direction -> !older, count=1 -> wraps from newest to oldest index 2).
    tap_text(&mut rt, "q");
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(harness.pump(&mut rt, &sent).is_empty());
    tap_text(&mut rt, "n");
    assert!(harness.pump(&mut rt, &sent).is_empty());
    {
        let frame = rt.state.terminals[&key].frame.as_ref().unwrap();
        let search = frame.search.as_ref().unwrap();
        assert_eq!(
            search.index,
            Some(2),
            "n in Forward direction re-opening find.last must land on oldest match (index 2)"
        );
        assert_eq!(frame.copy.unwrap().cursor, search.selected.unwrap().start);
    }
}

#[test]
fn lifecycle_auto_exit_is_immediate_across_focus_tabs_sessions_hosts_layers_and_worker() {
    let (mut ws, _host_rx) = tabbed_workspace("split");
    let (sender, sent) = flume::unbounded();
    let mut h_b = Harness::new(40, 8, "block:b", b"left pane");
    let mut h_c = Harness::new(40, 8, "block:c", b"right pane");
    h_b.attach(&mut ws, &sender);
    h_c.attach(&mut ws, &sender);
    let (mut rt, _clipboard) = copy_runtime(ws, (1024., 700.));

    // 1. Non-focus-leaving global shortcuts keep copy mode active.
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(rt.state.in_copy_mode());
    tap_ctrl_shift(&mut rt, Code::KeyB, 'b'); // toggle sidebar
    assert!(rt.state.in_copy_mode());

    // 2. Switching tabs exits immediately.
    rt.update(|s, tx| s.perform(Intent::Tab("single".into()), tx));
    assert!(!rt.state.in_copy_mode());

    // 3. Focusing another pane exits immediately.
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(rt.state.in_copy_mode());
    let grid_c = rect_of(&rt, "grid:block:c").unwrap();
    rt.pointer_down(grid_c[0] + 10., grid_c[1] + 10.);
    rt.pointer_up(grid_c[0] + 10., grid_c[1] + 10.);
    rt.frame(1. / 60.);
    assert!(!rt.state.in_copy_mode());

    // 4. PromptStep exits cleanly.
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(rt.state.in_copy_mode());
    rt.update(|s, tx| {
        actions::run(s, "client.prompt.previous", &Value::Null, tx);
    });
    assert!(!rt.state.in_copy_mode());

    // 5. Worker auto-exit (copy_ended_epoch >= active.epoch) exits immediately on wake.
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    let epoch = rt.state.active_copy_mode().unwrap().epoch;
    rt.update(|s, tx| {
        let key = s.term_key("block:b");
        s.terminals[&key]
            .connection
            .model
            .lock()
            .unwrap()
            .copy_ended_epoch = epoch;
        s.terminal_woke(&key, tx);
    });
    assert!(!rt.state.in_copy_mode());

    // 6. Reconnect / restore_generation bump exits immediately on wake.
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(rt.state.in_copy_mode());
    rt.update(|s, tx| {
        let key = s.term_key("block:b");
        s.terminals[&key]
            .connection
            .model
            .lock()
            .unwrap()
            .restore_generation += 1;
        s.terminal_woke(&key, tx);
    });
    assert!(!rt.state.in_copy_mode());

    // 7. Server-side minimized floating layer exits immediately on sync().
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(rt.state.in_copy_mode());
    rt.update(|s, tx| {
        {
            let mut m = s.hosts[0].connection.model.lock().unwrap();
            let win = &mut m.views.get_mut("s").unwrap().windows[1];
            win.layers[0].kind = "floating".into();
            win.layers[0].state = Some("minimized".into());
        }
        s.sync(0, tx);
    });
    assert!(!rt.state.in_copy_mode());

    // 8. Opening a panel (Commands / Settings) exits immediately.
    rt.update(|s, tx| {
        {
            let mut m = s.hosts[0].connection.model.lock().unwrap();
            let win = &mut m.views.get_mut("s").unwrap().windows[1];
            win.layers[0].kind = "tiled".into();
            win.layers[0].state = None;
        }
        s.sync(0, tx);
    });
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(rt.state.in_copy_mode());
    tap_ctrl_shift(&mut rt, Code::KeyP, 'p');
    assert!(rt.state.panel.is_some());
    assert!(!rt.state.in_copy_mode());
    let _ = drain_pty(&sent);
}

#[test]
fn cursor_rendering_wide_cells_hollow_focus_narrow_bar_hidden_toolbar_and_title_row() {
    let (mut ws, _host_rx) = tabbed_workspace("single");
    let (sender, sent) = flume::unbounded();
    let mut harness = Harness::new(40, 8, "block:a", "ab世界cd\r\nsecond\r\n│█box".as_bytes());
    harness.attach(&mut ws, &sender);
    let (mut rt, _clipboard) = copy_runtime(ws, (640., 480.));

    let grid_before = rect_of(&rt, "grid:block:a").unwrap();
    tap_ctrl_shift(&mut rt, Code::KeyX, 'x');
    assert!(harness.pump(&mut rt, &sent).is_empty());

    // Zero grid shift and toolbar controls stay inside 640x480.
    assert_eq!(rect_of(&rt, "grid:block:a").unwrap(), grid_before);
    for control in ["settings", "close-app", "mode-chip"] {
        let r = rect_of(&rt, control).unwrap_or_else(|| panic!("missing {control}"));
        assert!(
            r[0] >= 0. && r[0] + r[2] <= 640.5,
            "{control} clipped: {r:?}"
        );
    }

    // Hardware cursor is hidden; copy cursor is drawn.
    assert!(rect_of(&rt, "gblock:a/cursor").is_none());
    let cw = rt.state.metrics().cell.0;
    let narrow_cursor = rect_of(&rt, "gblock:a/copy-cursor").expect("copy cursor rendered");
    assert!((narrow_cursor[2] - cw).abs() < 0.5);

    // Move cursor onto wide CJK character '世' (row 0, col 2) -> cursor is 2 cells wide.
    tap_text(&mut rt, "g");
    tap_text(&mut rt, "g");
    tap_text(&mut rt, "0");
    tap_text(&mut rt, "l");
    tap_text(&mut rt, "l");
    assert!(harness.pump(&mut rt, &sent).is_empty());
    let wide_cursor = rect_of(&rt, "gblock:a/copy-cursor").expect("wide copy cursor");
    assert!(
        (wide_cursor[2] - 2. * cw).abs() < 0.5,
        "wide cursor width {} expected {}",
        wide_cursor[2],
        2. * cw
    );

    // Move cursor onto box-drawing sprite '│' on row 2 (y > 0): the ON_ACCENT sprite
    // container MUST render at `py` (same y as copy-cursor), NEVER at row 0 (`y = 0`).
    tap_text(&mut rt, "j");
    tap_text(&mut rt, "j");
    tap_text(&mut rt, "0");
    assert!(harness.pump(&mut rt, &sent).is_empty());
    let sprite_cursor = rect_of(&rt, "gblock:a/copy-cursor").expect("sprite copy cursor");
    let sprite_layer =
        rect_of(&rt, "gblock:a/copy-cursor-sprite").expect("copy cursor sprite container");
    assert!(
        sprite_cursor[1] > grid_before[1] + rt.state.metrics().cell.1,
        "copy cursor must be on row 2 (> row 0): cursor={sprite_cursor:?} grid={grid_before:?}"
    );
    assert!(
        (sprite_layer[0] - sprite_cursor[0]).abs() < 0.5
            && (sprite_layer[1] - sprite_cursor[1]).abs() < 0.5,
        "copy cursor sprite must align with cursor at py, not y=0: sprite={sprite_layer:?} cursor={sprite_cursor:?}"
    );

    // Window blur keeps copy cursor element rendered (hollow outline).
    rt.update(|s, _| s.window_focus(false));
    rt.frame(1. / 60.);
    assert!(rect_of(&rt, "gblock:a/copy-cursor").is_some());
    rt.update(|s, _| s.window_focus(true));
    rt.frame(1. / 60.);

    // Toolbar hidden floats the mode chip cleanly.
    rt.update(|s, _| s.settings.interface.toolbar = ToolbarBehavior::Hidden);
    rt.frame(1. / 60.);
    assert!(rect_of(&rt, "mode-chip").is_some());

    // macOS unified title row shows the mode chip cleanly.
    rt.update(|s, _| {
        s.settings.interface.toolbar = ToolbarBehavior::Always;
        s.unified_title = true;
        s.title_inset = 76.;
    });
    rt.frame(1. / 60.);
    assert!(rect_of(&rt, "mode-chip").is_some());

    // Settings -> Shortcuts opens cleanly and shows the single Copy mode row.
    rt.update(|s, tx| {
        s.perform(Intent::Settings, tx);
        s.open_section(SettingsSection::Shortcuts);
    });
    rt.frame(1. / 60.);
    assert!(has(&rt, "shortcut:pane.copy_mode"));
}

/// Real-font visual captures for copy mode (`REX_TEST_SNAP_DIR`).
/// Run individually: `REX_TEST_SNAP_DIR=... cargo test --locked --all-features copy_mode_draw`
#[test]
fn copy_mode_draw() {
    use super::pane_verb_tests::Camera;
    let Some(mut camera) = Camera::new() else {
        return;
    };
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");

    type Drive<'a> =
        &'a dyn Fn(&mut Runtime<Workspace>, &mut Harness, &flume::Receiver<HostCommand>);
    let run_scene = |camera: &mut Camera,
                     name: &str,
                     size: (f32, f32),
                     rows: u16,
                     keep_bg: bool,
                     output: &[u8],
                     drive: Drive<'_>| {
        let (mut ws, _host_rx) = tabbed_workspace("single");
        ws.settings.terminal.keep_program_background = keep_bg;
        let (sender, sent) = flume::unbounded();
        let mut harness = Harness::new(56, rows, "block:a", output);
        harness.attach(&mut ws, &sender);
        let mut rt = Runtime::new(ws, view, size, Env::new(true, 2.));
        rt.on_key = Some(crate::app::input::on_key);
        for _ in 0..12 {
            rt.frame(1. / 60.);
        }
        drive(&mut rt, &mut harness, &sent);
        for _ in 0..12 {
            rt.frame(1. / 60.);
        }
        camera.snap(&mut rt, name);
    };

    // 1. 1024x700 with ASCII + wide CJK cursor + block selection.
    run_scene(
        &mut camera,
        "copy-mode-1024-block-cjk",
        (1024., 700.),
        12,
        false,
        "alpha beta gamma delta\r\n01 世界 hello world\r\n02 block select row\r\n03 fourth line here"
            .as_bytes(),
        &|rt, harness, sent| {
            tap_ctrl_shift(rt, Code::KeyX, 'x');
            let _ = harness.pump(rt, sent);
            tap_text(rt, "g");
            tap_text(rt, "g");
            tap_text(rt, "0");
            tap_text(rt, "j");
            tap_text(rt, "3");
            tap_text(rt, "l");
            let _ = harness.pump(rt, sent);
            tap_ctrl(rt, Code::KeyV, 'v');
            tap_text(rt, "j");
            tap_text(rt, "l");
            tap_text(rt, "l");
            let _ = harness.pump(rt, sent);
        },
    );

    // 2. 640x480 narrow window with visual line selection.
    run_scene(
        &mut camera,
        "copy-mode-640-narrow",
        (640., 480.),
        10,
        false,
        b"narrow viewport line 1\r\nnarrow viewport line 2\r\nnarrow viewport line 3",
        &|rt, harness, sent| {
            tap_ctrl_shift(rt, Code::KeyX, 'x');
            let _ = harness.pump(rt, sent);
            tap_text(rt, "g");
            tap_text(rt, "g");
            tap_text(rt, "V");
            tap_text(rt, "j");
            let _ = harness.pump(rt, sent);
        },
    );

    // 3. Cropped pane (frame rows > visible rows).
    run_scene(
        &mut camera,
        "copy-mode-cropped",
        (640., 480.),
        40,
        false,
        b"top 01\r\ntop 02\r\ntop 03\r\nmid 04\r\nmid 05\r\nmid 06\r\nmid 07\r\nmid 08\r\nmid 09\r\nmid 10\r\nmid 11\r\nmid 12\r\nmid 13\r\nmid 14\r\nmid 15\r\nmid 16\r\nmid 17\r\nmid 18\r\nmid 19\r\nmid 20\r\nmid 21\r\nmid 22\r\nmid 23\r\nmid 24\r\nbottom 25",
        &|rt, harness, sent| {
            tap_ctrl_shift(rt, Code::KeyX, 'x');
            let _ = harness.pump(rt, sent);
            tap_text(rt, "g");
            tap_text(rt, "g");
            tap_text(rt, "0");
            tap_text(rt, "v");
            tap_text(rt, "e");
            let _ = harness.pump(rt, sent);
        },
    );

    // 4. Light OSC 11 keep_program_background (high-contrast ACCENT + ON_ACCENT cursor).
    run_scene(
        &mut camera,
        "copy-mode-light-osc11",
        (1024., 700.),
        10,
        true,
        b"\x1b]11;#f5f5f0\x07\x1b]10;#1a1a1a\x07Light OSC 11 background\r\nCursor stays legible here",
        &|rt, harness, sent| {
            tap_ctrl_shift(rt, Code::KeyX, 'x');
            let _ = harness.pump(rt, sent);
            tap_text(rt, "g");
            tap_text(rt, "g");
            tap_text(rt, "w");
            let _ = harness.pump(rt, sent);
        },
    );
}
