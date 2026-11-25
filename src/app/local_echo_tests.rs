use super::*;
use crate::app::{input, theme};
use std::time::Instant;

const OVERLAY: &str = "gblock:a/local-echo";
const HELPER: &str = "Show pending keystrokes while waiting for the server to echo them.";
fn size() -> (f32, f32) {
    if std::env::var_os("REX_M180_MINIMUM").is_some() {
        (640., 480.)
    } else {
        (1024., 700.)
    }
}
fn real(ws: Workspace) -> Runtime<Workspace> {
    Runtime::with_text(ws, view, size(), Env::new(true, 2.), app_fonts())
}
fn find_setting(rt: &mut Runtime<Workspace>) {
    rt.update(|s, tx| {
        s.perform(Intent::Settings, tx);
        s.settings_nav.search.text = "local echo".into();
        s.settings_search_changed();
        let i = s
            .settings_nav
            .hits()
            .iter()
            .position(|h| h.label == "Local echo" && h.page == SettingsSection::Terminal)
            .unwrap();
        s.open_settings_hit(i, tx);
    });
    settle(rt);
    let control = element(rt, "local-echo");
    assert_eq!(control.visible, control.rect);
    for text in ["Local echo", HELPER] {
        let words = rt
            .elements()
            .into_iter()
            .find(|e| {
                !e.exiting
                    && e.text.as_deref() == Some(text)
                    && e.path.contains("settings-scroll:terminal")
            })
            .unwrap();
        assert!(!words.text_cut);
        assert_eq!(words.visible, words.rect);
        assert!(words.rect[0] + words.rect[2] <= control.rect[0] + 1.);
        if text == HELPER {
            assert!(words.wrap);
            let layout = words.text_size.unwrap();
            assert!(layout[1] <= words.rect[3] + 1.);
        }
    }
}
#[test]
fn local_echo_search_popup_fit_and_local_persistence() {
    for size in [(1024., 700.), (640., 480.)] {
        let (mut ws, host) = tabbed_workspace("single");
        let dir = tempfile::TempDir::new().unwrap();
        ws.settings_path = dir.path().join("workspace.json");
        let (tx, rx) = flume::unbounded();
        framed_terminal(&mut ws, "block:a", &tx, b"$ ");
        let mut rt = runtime_at(ws, size);
        assert_eq!(rt.state.settings.terminal.local_echo, LocalEcho::Off);
        find_setting(&mut rt);
        for mode in [LocalEcho::Adaptive, LocalEcho::Always, LocalEcho::Off] {
            choose(&mut rt, "local-echo", mode.name());
            assert_eq!(
                Settings::load(&rt.state.settings_path)
                    .unwrap()
                    .terminal
                    .local_echo,
                mode
            );
        }
        assert!(other_commands(&host).is_empty() && other_commands(&rx).is_empty());
    }
}
#[test]
#[ignore = "run individually with REX_TEST_SNAP_DIR"]
fn local_echo_settings_draw() {
    let _fonts = rune::text::with_families("Inter", "IBM Plex Mono");
    let mut camera = Camera::new().unwrap();
    let mut rt = real(workspace());
    find_setting(&mut rt);
    camera.snap(&mut rt, &format!("local-echo-settings-{}", size().0));
    click(&mut rt, "local-echo");
    settle(&mut rt);
    camera.snap(&mut rt, &format!("local-echo-menu-{}", size().0));
}

struct Pane {
    ws: Workspace,
    terminal: ghostty_vt::Terminal,
    renderer: Renderer,
    sent: flume::Receiver<HostCommand>,
    _host: flume::Receiver<HostCommand>,
}
impl Pane {
    fn new(mode: LocalEcho) -> Self {
        let (mut ws, host) = tabbed_workspace("single");
        let (tx, sent) = flume::unbounded();
        ws.settings.terminal.font.spacing_unit =
            rex_rune::settings::typography::SpacingUnit::Percent;
        ws.settings.terminal.font.spacing = 7;
        let terminal = framed_terminal(
            &mut ws,
            "block:a",
            &tx,
            "$         \x1b[48;2;18;68;105m界\x1b[0m\r\x1b[2C".as_bytes(),
        );
        ws.set_local_echo(mode);
        let t = ws.active_terminal_mut().unwrap();
        t.echo_test_latency = Some(90);
        t.echo_test_clock = Some(Instant::now());
        let mut pane = Self {
            ws,
            terminal,
            renderer: Renderer::new().unwrap(),
            sent,
            _host: host,
        };
        pane.server("");
        pane
    }
    fn server(&mut self, output: &str) {
        self.terminal.vt_write(output.as_bytes());
        let mut f = self.renderer.frame(&self.terminal).unwrap();
        f.alt_screen = false;
        f.viewport_active = true;
        let t = self.ws.active_terminal_mut().unwrap();
        f.output_position = t.frame.as_ref().map_or(1, |f| f.output_position + 1);
        t.connection.model.lock().unwrap().frame = Some(Arc::new(f));
        t.refresh();
    }
    fn confirmed(&mut self) {
        key(&mut self.ws, 'a');
        self.server("a");
        key(&mut self.ws, 'b');
    }
}
fn key(ws: &mut Workspace, ch: char) {
    let t = ws.active_terminal_mut().unwrap();
    assert!(t.try_send(Command::Key(
        serde_json::json!({"key":format!("key_{ch}"),"text":ch.to_string(),"action":"press"})
    )));
}

#[test]
fn local_echo_inert_overlay_passes_drag_to_authoritative_grid() {
    for mode in [LocalEcho::Always, LocalEcho::Adaptive] {
        let mut p = Pane::new(mode);
        p.confirmed();
        let mut rt = runtime(p.ws);
        let layer = element(&rt, OVERLAY);
        assert!(layer.inert);
        let [x, y, w, h] = layer.rect;
        rt.pointer_move(x + w / 2., y + h / 2.);
        rt.pointer_down(x + w / 2., y + h / 2.);
        rt.pointer_move(x + 3. * w, y + h / 2.);
        rt.pointer_up(x + 3. * w, y + h / 2.);
        settle(&mut rt);
        assert!(p.sent.try_iter().any(|c| matches!(
            c,
            HostCommand::Terminal {
                command: Command::Selection(_),
                ..
            }
        )));
        assert!(!has(&rt, OVERLAY));
        rt.update(|s, _| key(s, 's'));
        assert!(!has(&rt, OVERLAY));
    }
}
#[test]
fn local_echo_lifecycle_boundaries_revoke_before_typing_again() {
    for mode in [LocalEcho::Always, LocalEcho::Adaptive] {
        for boundary in [
            "window",
            "find",
            "paste",
            "resize",
            "copy",
            "ime cancel",
            "ime disabled",
            "ime commit",
            "disconnect",
            "resume",
            "switch mode",
            "modal",
        ] {
            let mut p = Pane::new(mode);
            p.confirmed();
            let mut rt = runtime(p.ws);
            assert!(has(&rt, OVERLAY), "{mode:?} {boundary} initial");
            rt.update(move |s, tx| {
                match boundary {
                    "window" => {
                        s.window_focus(false);
                        s.window_focus(true);
                    }
                    "find" => {
                        s.open_search();
                        s.close_search(&s.term_key("block:a"), tx);
                    }
                    "paste" => s
                        .active_terminal_mut()
                        .unwrap()
                        .send(Command::Paste("abc".into())),
                    "resize" => s
                        .active_terminal_mut()
                        .unwrap()
                        .send(Command::Resize(80, 24, 9, 16)),
                    "copy" => {
                        s.active_terminal_mut().unwrap().begin_copy_mode((9., 16.));
                        s.active_terminal_mut().unwrap().clear_copy_mode_active();
                    }
                    "ime cancel" | "ime disabled" | "ime commit" => {
                        input::on_ime(s, &rune::Ime::Preedit("n".into(), None, vec![]), tx);
                        input::on_ime(
                            s,
                            &match boundary {
                                "ime cancel" => rune::Ime::Preedit(String::new(), None, vec![]),
                                "ime disabled" => rune::Ime::Disabled,
                                _ => rune::Ime::Commit("n".into()),
                            },
                            tx,
                        );
                    }
                    "disconnect" | "resume" => {
                        let t = s.active_terminal_mut().unwrap();
                        if boundary == "disconnect" {
                            t.connection.model.lock().unwrap().connected = false;
                            t.refresh();
                            t.connection.model.lock().unwrap().connected = true;
                        } else {
                            t.connection.model.lock().unwrap().stream_epoch += 1;
                        }
                        t.refresh();
                    }
                    "switch mode" => s.set_local_echo(if mode == LocalEcho::Adaptive {
                        LocalEcho::Always
                    } else {
                        LocalEcho::Adaptive
                    }),
                    "modal" => {
                        use rex_rune::connection::prompt::{PromptEvent, PromptEventKind};
                        let now = Instant::now();
                        let offer = super::super::prompt_tests::input_prompt("call", "Name");
                        s.process_prompt_events_at(
                            0,
                            [PromptEvent {
                                epoch: s.model().connection_epoch,
                                kind: PromptEventKind::Requested(offer),
                            }]
                            .into(),
                            now,
                            tx,
                        );
                        assert!(s.is_prompt_open());
                        s.cancel_open_prompt(Some(tx));
                    }
                    _ => unreachable!(),
                }
                key(s, 's');
            });
            rt.frame(1. / 60.);
            assert!(!has(&rt, OVERLAY), "{mode:?} {boundary}");
        }
    }
}

#[test]
#[ignore = "run individually with REX_TEST_SNAP_DIR"]
fn local_echo_overlay_draw() {
    let _fonts = rune::text::with_families("Inter", "IBM Plex Mono");
    let mut camera = Camera::new().unwrap();
    let mut p = Pane::new(LocalEcho::Adaptive);
    p.confirmed();
    key(&mut p.ws, 'c');
    let mut rt = real(p.ws);
    settle(&mut rt);
    let grid = rect(&rt, "grid:block:a");
    let ov = rect(&rt, OVERLAY);
    let m = rt.state.metrics();
    let cw = m.cell.0;
    assert!(cw.fract().abs() > 0.001);
    assert!((ov[0] - grid[0] - 3. * cw).abs() < 1.);
    assert!((ov[2] - 2. * cw).abs() < 1.);
    let pixels = camera.snap(&mut rt, &format!("local-echo-overlay-{}", size().0));
    let width = (size().0 * rt.env.scale) as usize;
    let t = rt.state.active_terminal().unwrap();
    let body = t.body(&rt.state.term_colors());
    let fg = t.frame.as_ref().unwrap().fg;
    let contains = |box_: [f32; 4], color: u32| {
        let want = theme::rgb(color)
            .premul_srgb(1.)
            .map(|v| (v * 255.).round() as u8);
        let [x, y, w, h] = box_;
        let scale = rt.env.scale;
        ((y * scale).ceil() as usize..((y + h) * scale).floor() as usize).any(|j| {
            ((x * scale).ceil() as usize..((x + w) * scale).floor() as usize)
                .any(|i| pixels[4 * (j * width + i)..][..4] == want)
        })
    };
    assert!(contains(ov, body));
    assert!(contains(ov, theme::mix(fg, body, 0.45)), "underline colour");
    assert!(
        contains([grid[0] + 10. * cw, ov[1], 2. * cw, ov[3]], 0x124469),
        "authoritative wide glyph background"
    );
}

#[test]
fn local_echo_host_wake_revokes_focus_and_program_epochs_after_queue_drains() {
    for mode in [LocalEcho::Adaptive, LocalEcho::Always] {
        for boundary in ["pane", "tab", "host", "title", "process", "program"] {
            let mut p = Pane::new(mode);
            p.confirmed();
            p.server("b");
            let s = &mut p.ws;
            // Install the same pre-wake model in the mock worker and its UI replica.
            let pane = s.active_terminal().unwrap().connection.clone();
            let host = &mut s.hosts[0];
            host.model.terminals.insert("block:a".into(), pane);
            let mut next = host.model.clone();
            match boundary {
                "title" => {
                    next.titles
                        .insert("block:a".into(), ("Password".into(), None));
                }
                "program" => {
                    next.programs.insert("block:a".into(), "ssh".into());
                }
                "process" => {
                    next.foreground.insert("block:a".into(), vec![41]);
                }
                "pane" => {
                    next.views.get_mut("s").unwrap().windows[0].focused_block_id =
                        Some("block:other".into())
                }
                "tab" => next.views.get_mut("s").unwrap().active_window_id = Some("split".into()),
                "host" => {}
                _ => unreachable!(),
            }
            let original = host.model.clone();
            *host.connection.model.lock().unwrap() = next;
            let mut tx = rune::Tx::default();
            if boundary == "host" {
                let mut second = workspace_hosts(2);
                s.settings.hosts.push(second.settings.hosts.pop().unwrap());
                s.hosts.push(second.hosts.pop().unwrap());
                s.activate_host(1, &mut tx);
                s.activate_host(0, &mut tx);
            } else {
                s.sync(0, &mut tx);
                if matches!(boundary, "pane" | "tab") {
                    *s.hosts[0].connection.model.lock().unwrap() = original;
                    s.sync(0, &mut tx);
                }
            }
            key(s, 's');
            let rt = runtime(p.ws);
            assert!(!has(&rt, OVERLAY), "{mode:?} {boundary}");
        }
    }
}

#[test]
fn local_echo_enter_skips_a_pre_enter_frame_waiting_for_its_ui_wake() {
    for mode in [LocalEcho::Adaptive, LocalEcho::Always] {
        let mut p = Pane::new(mode);
        p.confirmed();
        p.server("b");
        let t = p.ws.active_terminal_mut().unwrap();
        let mut queued = (**t.frame.as_ref().unwrap()).clone();
        queued.output_position += 1;
        t.connection.model.lock().unwrap().frame = Some(Arc::new(queued));
        t.send(Command::Key(
            serde_json::json!({"key":"enter","text":"\n","action":"press"}),
        ));
        t.refresh();
        key(&mut p.ws, 's');
        p.server("s");
        key(&mut p.ws, 'z');
        assert!(!has(&runtime(p.ws), OVERLAY));
    }
}

#[test]
fn local_echo_enter_tracks_a_reply_only_in_opt_in_modes() {
    for mode in [LocalEcho::Off, LocalEcho::Always, LocalEcho::Adaptive] {
        let mut p = Pane::new(mode);
        p.ws.active_terminal_mut().unwrap().send(Command::Key(
            serde_json::json!({"key":"enter","text":"\n","action":"press"}),
        ));
        let HostCommand::Terminal { command, .. } = p.sent.try_recv().unwrap() else {
            panic!("expected terminal input");
        };
        let key = match (mode, command) {
            (LocalEcho::Off, Command::Key(k)) => k,
            (LocalEcho::Always | LocalEcho::Adaptive, Command::EchoReset(tracked)) => {
                let Command::Key(k) = tracked.0 else {
                    panic!("expected tracked key");
                };
                k
            }
            _ => panic!("wrong receipt for {mode:?}"),
        };
        assert_eq!(key["key"], "enter");
    }
}
