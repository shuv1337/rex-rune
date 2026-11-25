use super::{InputAction::*, *};
use crate::terminal::Renderer;
use ghostty_vt::{Terminal, TerminalOptions};

struct Rig {
    term: Terminal,
    renderer: Renderer,
    frame: Frame,
    p: Predictor,
    mode: LocalEcho,
    rtt: Option<u32>,
    now: Instant,
    reset_seq: u64,
    acked: u64,
}
impl Rig {
    fn new(mode: LocalEcho, text: &str) -> Self {
        let mut term = Terminal::new(TerminalOptions {
            cols: 240,
            rows: 4,
            ..Default::default()
        })
        .unwrap();
        term.vt_write(text.as_bytes());
        let mut renderer = Renderer::new().unwrap();
        let mut frame = renderer.frame(&term).unwrap();
        frame.alt_screen = false;
        frame.viewport_active = true;
        Self {
            term,
            renderer,
            frame,
            p: Predictor::default(),
            mode,
            rtt: Some(90),
            now: Instant::now(),
            reset_seq: 0,
            acked: 0,
        }
    }
    fn input(&mut self, a: InputAction) {
        if a == Reset {
            self.reset_seq += 1;
            self.p
                .sent_reset(self.reset_seq, Some(&self.frame), self.now, self.rtt);
            return;
        }
        self.p
            .on_input(a, Some(&self.frame), self.mode, self.rtt, self.now);
    }
    fn server(&mut self, text: &str) {
        self.term.vt_write(text.as_bytes());
        let mut next = self.renderer.frame(&self.term).unwrap();
        next.alt_screen =
            self.term.active_screen().unwrap() == ghostty_vt::screen::Screen::Alternate;
        next.viewport_active = self.term.is_viewport_active().unwrap();
        next.acked_input = self.acked;
        next.output_position = self.frame.output_position + text.len() as u64;
        self.p
            .reconcile(&self.frame, &next, self.mode, self.rtt, self.now);
        self.frame = next;
    }
    fn overlay(&self) -> Option<Overlay> {
        self.p.overlay(&self.frame, self.mode, self.rtt, self.now)
    }
    fn poll(&mut self) -> (bool, Option<Instant>) {
        self.p.poll(self.mode, self.rtt, self.now)
    }
    fn confirm(&mut self) {
        self.input(Insert('a'));
        assert!(self.overlay().is_none());
        self.server("a");
        assert!(self.p.confirmed);
    }
    fn secret(&mut self) {
        self.input(Insert('s'));
        assert!(self.overlay().is_none());
        assert!(!self.p.confirmed);
    }
}
const MODES: [LocalEcho; 2] = [LocalEcho::Adaptive, LocalEcho::Always];

#[test]
fn unconfirmed_evidence_requires_a_surviving_unmasked_change_and_cursor_advance() {
    for mode in MODES {
        for (baseline, input, server) in [
            ("", '*', "*"),
            ("", ' ', " "),
            ("", 'x', "*"),
            ("", 'x', " "),
            ("", 'x', "x\r"),
            ("x\r", 'x', "\x1b[C"),
            ("", 'x', "\x1b[C"),
        ] {
            let mut r = Rig::new(mode, baseline);
            r.input(Insert(input));
            r.server(server);
            r.secret();
        }
        for observed in ["", "x", "x\x08 \x08"] {
            let mut r = Rig::new(mode, "");
            r.input(Insert('x'));
            r.input(Backspace);
            r.server(observed);
            r.secret();
        }
        let mut r = Rig::new(mode, "");
        r.confirm();
        for ch in ['*', ' '] {
            r.input(Insert(ch));
            assert!(r.overlay().is_some());
        }
    }
}
#[test]
fn control_barrier_requires_new_server_bytes_even_after_disengagement() {
    for mode in MODES {
        let mut r = Rig::new(mode, "");
        r.confirm();
        r.rtt = Some(5);
        r.input(Reset);
        r.rtt = Some(80);
        for _ in 0..2 {
            r.secret();
            assert!(!r.p.has_pending());
            r.server("");
        }
        r.acked = r.reset_seq;
        r.now += Duration::from_millis(5);
        r.server("\rPassword: ");
        r.secret();
        assert!(r.p.has_pending());
        r.server("*");
        r.secret();
    }
}
#[test]
fn stale_pre_enter_echo_cannot_confirm_a_password_character() {
    let mut r = Rig::new(LocalEcho::Always, "");
    r.input(Insert('l'));
    r.input(Insert('s'));
    r.input(Reset);
    r.server("l");
    r.input(Insert('s'));
    r.server("s");
    assert!(!r.p.confirmed);
    r.input(Insert('p'));
    assert!(r.overlay().is_none());
    assert!(!r.p.has_pending());
}
#[test]
fn enter_requires_its_own_ack_new_output_and_elapsed_rtt() {
    for mode in MODES {
        let mut r = Rig::new(mode, "");
        r.input(Reset);
        r.acked = r.reset_seq;
        r.input(Reset);
        r.now += Duration::from_millis(89);
        r.server("s");
        r.input(Insert('x'));
        assert!(!r.p.has_pending());
        r.acked = r.reset_seq;
        r.server("\r\n$ ");
        r.input(Insert('x'));
        assert!(!r.p.has_pending());
        r.now += Duration::from_millis(1);
        r.confirm();
        r.input(Insert('b'));
        assert!(r.overlay().is_some());
    }
}
#[test]
fn output_and_screen_boundaries_revoke_an_idle_or_pending_epoch() {
    for mode in MODES {
        for pending in [false, true] {
            for output in [
                "\x1b[2K\r",
                "\x1b]2;changed\x07",
                "\n",
                "\x1b[?1049h",
                "\x1b[?9h",
                "\x1b[?1000h",
                "\x1b[?1002h",
                "\x1b[?1003h",
            ] {
                let mut r = Rig::new(mode, "");
                r.confirm();
                if pending {
                    r.input(Insert('p'));
                    assert!(r.overlay().is_some());
                }
                r.server(output);
                r.secret();
                if output.starts_with("\x1b[?") {
                    assert!(r.frame.alt_screen || r.frame.mouse_tracking, "{output:?}");
                    r.server("s");
                    r.secret();
                    assert!(!r.p.has_pending());
                }
            }
        }
    }
}
#[test]
fn scrollback_dimensions_overflow_and_late_echo_revoke_before_rendering() {
    for mode in MODES {
        let mut r = Rig::new(mode, "");
        r.confirm();
        r.input(Insert('b'));
        let mut f = r.frame.clone();
        f.viewport_active = false;
        r.p.reconcile(&r.frame, &f, mode, r.rtt, r.now);
        r.frame = f;
        r.secret();
        assert!(!r.p.has_pending());
        let mut r = Rig::new(mode, "");
        r.confirm();
        r.input(Insert('b'));
        r.term.resize(100, 3, 8, 16).unwrap();
        r.server("b");
        r.secret();
        let mut r = Rig::new(mode, "");
        r.confirm();
        for _ in 0..=MAX_PENDING {
            r.input(Insert('x'));
        }
        assert!(!r.p.has_pending());
        r.secret();
        let mut r = Rig::new(mode, "");
        r.input(Insert('a'));
        r.now += MIN_EXPIRE;
        r.server("a");
        r.secret();
    }
}
#[test]
fn styling_graphemes_links_and_wide_cells_are_never_overwritten_or_shifted() {
    for mode in MODES {
        for boundary in [
            "\x1b[1mB",
            "\x1b[2mB",
            "\x1b[48;2;0;0;0m ",
            "界",
            "e\u{301}",
            "\x1b]8;;https://example.com\x1b\\x\x1b]8;;\x1b\\",
            "https://example.com",
        ] {
            let mut r = Rig::new(mode, &format!("   {boundary}\x1b[0m\r"));
            r.confirm();
            r.input(Insert('b'));
            r.input(Insert('c'));
            assert!(r.overlay().is_none(), "{boundary}");
        }
        let mut r = Rig::new(mode, "界");
        r.confirm();
        r.input(Backspace);
        assert!(r.overlay().is_some());
        r.input(Backspace);
        assert!(r.overlay().is_none());
    }
}
#[test]
fn split_and_coalesced_plain_edits_keep_right_prompt_and_cursor() {
    for mode in MODES {
        let mut r = Rig::new(mode, "         right\r");
        r.input(Insert('a'));
        r.input(Insert('b'));
        r.input(Insert('c'));
        r.server("ab");
        let ov = r.overlay().unwrap();
        assert_eq!((ov.start_col, ov.cursor), (2, (3, 0)));
        r.input(Backspace);
        r.input(Insert('d'));
        r.server("d");
        assert!(r.p.confirmed);
        assert!(r.overlay().is_none());
        assert_eq!(r.poll(), (false, None));
    }
}
#[test]
fn adaptive_hysteresis_reset_deadlines_and_unknown_latency() {
    let mut r = Rig::new(LocalEcho::Adaptive, "");
    r.rtt = Some(29);
    r.input(Insert('a'));
    assert!(!r.p.has_pending());
    r.server("a");
    r.rtt = Some(30);
    r.confirm();
    r.rtt = Some(20);
    r.input(Insert('b'));
    assert!(!r.overlay().unwrap().cells[0].underline);
    let sent = r.now;
    assert_eq!(r.poll(), (false, Some(sent + UNDERLINE_AGE)));
    r.now += UNDERLINE_AGE;
    assert_eq!(r.poll(), (true, Some(sent + MIN_EXPIRE)));
    assert!(r.overlay().unwrap().cells[0].underline);
    assert_eq!(r.poll(), (false, Some(sent + MIN_EXPIRE)));
    r.rtt = Some(19);
    assert!(r.overlay().is_none());
    assert_eq!(r.poll(), (true, None));
    assert!(!r.p.confirmed);
    r.rtt = Some(80);
    r.confirm();
    r.input(Insert('q'));
    assert!(r.overlay().unwrap().cells[0].underline);
    assert_eq!(r.poll(), (false, Some(r.now + MIN_EXPIRE)));
    r.rtt = None;
    assert_eq!(r.poll(), (true, None));
    r.rtt = Some(25);
    r.input(Insert('q'));
    assert!(!r.p.has_pending());
    r.p.reset();
    assert!(!r.p.adaptive);
}
fn key(name: &str, text: &str, mods: &str, consumed: &str, action: &str) -> Value {
    serde_json::json!({
        "key": name,
        "mods": mods,
        "consumed_mods": consumed,
        "text": text,
        "unshifted_codepoint": text.chars().next().map_or(0, |c| c as u32),
        "composing": false,
        "action": action,
    })
}

#[test]
fn key_classification_accepts_printable_ascii_and_plain_backspace_only() {
    assert_eq!(
        classify_key(&key("key_a", "a", "", "", "press")),
        InputAction::Insert('a')
    );
    assert_eq!(
        classify_key(&key("key_a", "A", "shift", "shift", "repeat")),
        InputAction::Insert('A')
    );
    assert_eq!(
        classify_key(&key("key_a", "", "", "", "release")),
        InputAction::Ignore
    );
    assert_eq!(
        classify_key(&key("backspace", "", "", "", "press")),
        InputAction::Backspace
    );
    assert_eq!(
        classify_key(&key("backspace", "", "ctrl", "", "press")),
        InputAction::Reset
    );
    assert_eq!(
        classify_key(&key("key_c", "", "ctrl", "", "press")),
        InputAction::Reset
    );
    assert_eq!(
        classify_key(&key("key_b", "b", "alt", "", "press")),
        InputAction::Reset
    );
    // Option-composed ASCII on macOS (e.g. `@` via Option+L on German layout).
    assert_eq!(
        classify_key(&key("key_l", "@", "alt", "alt", "press")),
        InputAction::Insert('@')
    );
    assert_eq!(
        classify_key(&key("enter", "\n", "", "", "press")),
        InputAction::Reset
    );
    assert_eq!(
        classify_key(&key("tab", "\t", "", "", "press")),
        InputAction::Reset
    );
    assert_eq!(
        classify_key(&key("escape", "", "", "", "press")),
        InputAction::Reset
    );
}
