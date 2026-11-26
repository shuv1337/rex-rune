use super::*;
use PhysicalKeyCode as C;

#[test]
fn physical_codes_use_ghostty_names() {
    for (code, name) in [
        (C::KeyA, "key_a"),
        (C::Digit7, "digit_7"),
        (C::ArrowUp, "arrow_up"),
        (C::PageDown, "page_down"),
        (C::BracketLeft, "bracket_left"),
        (C::Backquote, "backquote"),
        (C::F12, "f12"),
        (C::Numpad0, "numpad_0"),
        (C::NumpadAdd, "numpad_add"),
        (C::IntlBackslash, "intl_backslash"),
        (C::Enter, "enter"),
    ] {
        assert_eq!(code_name(code), name);
    }
}

#[test]
fn key_payloads() {
    let shift = Mods {
        shift: true,
        ..Default::default()
    };
    let ctrl = Mods {
        ctrl: true,
        cmd: true,
        ..Default::default()
    };
    let typed = KeyEvent {
        key: Key::Text("A".into()),
        mods: shift,
    };
    let p = payload(&typed, Some(C::KeyA), Some('a'), Action::Press).unwrap();
    assert_eq!(p["key"], "key_a");
    assert_eq!(p["mods"], "shift");
    assert_eq!(p["consumed_mods"], "shift");
    assert_eq!(p["text"], "A");
    assert_eq!(p["unshifted_codepoint"], 'a' as u32);

    let interrupt = KeyEvent {
        key: Key::Command('c'),
        mods: ctrl,
    };
    let p = payload(&interrupt, Some(C::KeyC), None, Action::Press).unwrap();
    assert_eq!(
        (p["key"].as_str(), p["mods"].as_str(), p["text"].as_str()),
        (Some("key_c"), Some("ctrl"), Some(""))
    );

    let enter = KeyEvent {
        key: Key::Enter,
        mods: Mods::default(),
    };
    let p = payload(&enter, Some(C::Enter), None, Action::Repeat).unwrap();
    assert_eq!(
        (p["text"].as_str(), p["action"].as_str()),
        (Some("\n"), Some("repeat"))
    );
    let p = payload(&enter, None, None, Action::Release).unwrap();
    assert_eq!(
        (p["key"].as_str(), p["text"].as_str()),
        (Some("enter"), Some(""))
    );
}

#[test]
fn shift_enter_keeps_shift_for_the_encoder() {
    // With shift marked consumed, Ghostty's Kitty encoder sends a bare `\r` and apps
    // cannot tell Shift+Enter from Enter.
    let shift_enter = KeyEvent {
        key: Key::Enter,
        mods: Mods {
            shift: true,
            ..Default::default()
        },
    };
    let p = payload(&shift_enter, Some(C::Enter), None, Action::Press).unwrap();
    assert_eq!(
        (
            p["mods"].as_str(),
            p["consumed_mods"].as_str(),
            p["text"].as_str()
        ),
        (Some("shift"), Some(""), Some("\n"))
    );
}

#[test]
fn composed_space_keeps_the_accent_and_physical_position() {
    let completed = KeyEvent {
        key: Key::Text("~".into()),
        mods: Mods::default(),
    };
    let last = payload(&completed, Some(C::Space), Some(' '), Action::Press).unwrap();
    assert_eq!(
        (
            last["key"].as_str(),
            last["text"].as_str(),
            last["composing"].as_bool()
        ),
        (Some("space"), Some("~"), Some(false))
    );
}

const NONE: Mods = Mods {
    shift: false,
    cmd: false,
    alt: false,
    ctrl: false,
    logo: false,
};
const SHIFT: Mods = Mods {
    shift: true,
    ..NONE
};
/// Rune sets both off macOS.
const CTRL: Mods = Mods {
    ctrl: true,
    cmd: true,
    ..NONE
};
const ALT: Mods = Mods { alt: true, ..NONE };

fn with(a: Mods, b: Mods) -> Mods {
    Mods {
        shift: a.shift || b.shift,
        cmd: a.cmd || b.cmd,
        alt: a.alt || b.alt,
        ctrl: a.ctrl || b.ctrl,
        logo: a.logo || b.logo,
    }
}

fn ev(key: Key, mods: Mods) -> KeyEvent {
    KeyEvent { key, mods }
}

/// (key, mods, consumed_mods, text, unshifted_codepoint)
fn fields(p: &Value) -> (&str, &str, &str, &str, u64) {
    (
        p["key"].as_str().unwrap(),
        p["mods"].as_str().unwrap(),
        p["consumed_mods"].as_str().unwrap(),
        p["text"].as_str().unwrap(),
        p["unshifted_codepoint"].as_u64().unwrap(),
    )
}

fn text(t: &str) -> Key {
    Key::Text(t.into())
}

#[test]
fn caps_remapped_to_escape_sends_escape_on_every_edge() {
    for (mods, name) in [(NONE, ""), (CTRL, "ctrl"), (ALT, "alt"), (SHIFT, "shift")] {
        for (action, edge) in [
            (Action::Press, "press"),
            (Action::Repeat, "repeat"),
            (Action::Release, "release"),
        ] {
            let p = payload(&ev(Key::Escape, mods), Some(C::CapsLock), None, action).unwrap();
            assert_eq!(fields(&p), ("escape", name, "", "", 0));
            assert_eq!(p["action"], edge);
        }
    }
}

#[test]
fn named_keys_follow_the_layout_except_for_distinct_keypad_functions() {
    for (key, physical, name, typed) in [
        (Key::Backspace, C::CapsLock, "backspace", ""),
        (Key::Tab, C::CapsLock, "tab", "\t"),
        (Key::Enter, C::CapsLock, "enter", "\n"),
        (Key::Delete, C::CapsLock, "delete", ""),
        (Key::Left, C::Numpad4, "arrow_left", ""),
        (Key::Up, C::CapsLock, "arrow_up", ""),
        (Key::Down, C::CapsLock, "arrow_down", ""),
        (Key::Right, C::CapsLock, "arrow_right", ""),
        (Key::Home, C::CapsLock, "home", ""),
        (Key::End, C::CapsLock, "end", ""),
        (Key::PageUp, C::CapsLock, "page_up", ""),
        (Key::PageDown, C::CapsLock, "page_down", ""),
        (Key::Function(1), C::CapsLock, "f1", ""),
        (Key::Enter, C::NumpadEnter, "numpad_enter", "\n"),
        (Key::Backspace, C::NumpadBackspace, "numpad_backspace", ""),
        (Key::Escape, C::NumpadEnter, "escape", ""),
    ] {
        let p = payload(&ev(key, NONE), Some(physical), None, Action::Press).unwrap();
        assert_eq!(fields(&p), (name, "", "", typed, 0));
    }
}

#[test]
fn modified_keys_keep_their_modifiers_in_the_payload() {
    type Row<'a> = (
        &'a str,
        KeyEvent,
        Option<C>,
        Option<char>,
        (&'a str, &'a str, &'a str, &'a str, u64),
    );
    let rows: Vec<Row> = vec![
        (
            "layout A on physical Q",
            ev(text("a"), NONE),
            Some(C::KeyQ),
            Some('a'),
            ("key_q", "", "", "a", 97),
        ),
        (
            "layout Ctrl+C on physical I",
            ev(Key::Command('c'), CTRL),
            Some(C::KeyI),
            Some('c'),
            ("key_i", "ctrl", "", "", 99),
        ),
        (
            "Tab",
            ev(Key::Tab, NONE),
            Some(C::Tab),
            None,
            ("tab", "", "", "\t", 0),
        ),
        (
            "Shift+Tab",
            ev(Key::Tab, SHIFT),
            Some(C::Tab),
            None,
            ("tab", "shift", "", "\t", 0),
        ),
        (
            "Enter",
            ev(Key::Enter, NONE),
            Some(C::Enter),
            None,
            ("enter", "", "", "\n", 0),
        ),
        (
            "Shift+Enter",
            ev(Key::Enter, SHIFT),
            Some(C::Enter),
            None,
            ("enter", "shift", "", "\n", 0),
        ),
        (
            "Ctrl+Enter",
            ev(Key::Enter, CTRL),
            Some(C::Enter),
            None,
            ("enter", "ctrl", "", "", 0),
        ),
        (
            "Alt+Enter",
            ev(Key::Enter, ALT),
            Some(C::Enter),
            None,
            ("enter", "alt", "", "", 0),
        ),
        (
            "Ctrl+Shift+Enter",
            ev(Key::Enter, with(SHIFT, CTRL)),
            Some(C::Enter),
            None,
            ("enter", "shift+ctrl", "", "", 0),
        ),
        (
            "Space",
            ev(text(" "), NONE),
            Some(C::Space),
            Some(' '),
            ("space", "", "", " ", 32),
        ),
        (
            "Shift+Space",
            ev(text(" "), SHIFT),
            Some(C::Space),
            Some(' '),
            ("space", "shift", "", " ", 32),
        ),
        (
            "Shift+Up",
            ev(Key::Up, SHIFT),
            Some(C::ArrowUp),
            None,
            ("arrow_up", "shift", "", "", 0),
        ),
        (
            "Ctrl+Left",
            ev(Key::Left, CTRL),
            Some(C::ArrowLeft),
            None,
            ("arrow_left", "ctrl", "", "", 0),
        ),
        (
            "Alt+Right",
            ev(Key::Right, ALT),
            Some(C::ArrowRight),
            None,
            ("arrow_right", "alt", "", "", 0),
        ),
        (
            "Shift+Home",
            ev(Key::Home, SHIFT),
            Some(C::Home),
            None,
            ("home", "shift", "", "", 0),
        ),
        (
            "Ctrl+End",
            ev(Key::End, CTRL),
            Some(C::End),
            None,
            ("end", "ctrl", "", "", 0),
        ),
        (
            "Shift+PageUp",
            ev(Key::PageUp, SHIFT),
            Some(C::PageUp),
            None,
            ("page_up", "shift", "", "", 0),
        ),
        (
            "Ctrl+PageDown",
            ev(Key::PageDown, CTRL),
            Some(C::PageDown),
            None,
            ("page_down", "ctrl", "", "", 0),
        ),
        (
            "Shift+Backspace",
            ev(Key::Backspace, SHIFT),
            Some(C::Backspace),
            None,
            ("backspace", "shift", "", "", 0),
        ),
        (
            "Ctrl+Backspace",
            ev(Key::Backspace, CTRL),
            Some(C::Backspace),
            None,
            ("backspace", "ctrl", "", "", 0),
        ),
        (
            "Alt+Backspace",
            ev(Key::Backspace, ALT),
            Some(C::Backspace),
            None,
            ("backspace", "alt", "", "", 0),
        ),
        (
            "Shift+Delete",
            ev(Key::Delete, SHIFT),
            Some(C::Delete),
            None,
            ("delete", "shift", "", "", 0),
        ),
        (
            "Ctrl+Delete",
            ev(Key::Delete, CTRL),
            Some(C::Delete),
            None,
            ("delete", "ctrl", "", "", 0),
        ),
        (
            "Shift+a",
            ev(text("A"), SHIFT),
            Some(C::KeyA),
            Some('a'),
            ("key_a", "shift", "shift", "A", 97),
        ),
        (
            "Shift+1",
            ev(text("!"), SHIFT),
            Some(C::Digit1),
            Some('1'),
            ("digit_1", "shift", "shift", "!", 49),
        ),
        (
            "Russian Shift+ф",
            ev(text("Ф"), SHIFT),
            Some(C::KeyA),
            Some('ф'),
            ("key_a", "shift", "shift", "Ф", 'ф' as u64),
        ),
        // xkb counts Shift as consumed on a cased key even when Caps Lock cancels it.
        (
            "Caps+Shift+a",
            ev(text("a"), SHIFT),
            Some(C::KeyA),
            Some('a'),
            ("key_a", "shift", "shift", "a", 97),
        ),
        // AltGr is a layout level, not a Rune modifier.
        (
            "AltGr+2 (sv)",
            ev(text("@"), NONE),
            Some(C::Digit2),
            Some('2'),
            ("digit_2", "", "", "@", 50),
        ),
        (
            "Alt+Shift+a",
            ev(text("A"), with(ALT, SHIFT)),
            Some(C::KeyA),
            Some('a'),
            ("key_a", "shift+alt", "shift", "A", 97),
        ),
        (
            "logical Shift+Space",
            ev(text(" "), SHIFT),
            None,
            None,
            ("space", "shift", "", " ", 0),
        ),
        (
            "logical Shift+A",
            ev(text("A"), SHIFT),
            None,
            None,
            ("key_a", "shift", "shift", "A", 0),
        ),
    ];
    for (label, event, code, base, expected) in rows {
        let p = payload(&event, code, base, Action::Press).unwrap();
        assert_eq!(fields(&p), expected, "{label}");
        assert_eq!(p["action"], "press", "{label}");
    }
}

#[test]
fn shifted_dead_key_composition_consumes_shift_for_the_composed_text() {
    let composed = payload(
        &ev(text("Ä"), SHIFT),
        Some(C::KeyA),
        Some('a'),
        Action::Press,
    )
    .unwrap();
    assert_eq!(fields(&composed), ("key_a", "shift", "shift", "Ä", 97));
    assert_eq!(composed["composing"], false);
}

#[test]
fn press_repeat_and_release_carry_the_same_identity() {
    for (label, event, code, base) in [
        ("Shift+Tab", ev(Key::Tab, SHIFT), C::Tab, None),
        ("Shift+Space", ev(text(" "), SHIFT), C::Space, Some(' ')),
        ("Shift+a", ev(text("A"), SHIFT), C::KeyA, Some('a')),
        ("Ctrl+Left", ev(Key::Left, CTRL), C::ArrowLeft, None),
    ] {
        let [press, repeat, release] = [Action::Press, Action::Repeat, Action::Release]
            .map(|action| payload(&event, Some(code), base, action).unwrap());
        let identity = |p: &Value| {
            let (key, mods, consumed, _, unshifted) = fields(p);
            (
                key.to_owned(),
                mods.to_owned(),
                consumed.to_owned(),
                unshifted,
            )
        };
        assert_eq!(identity(&repeat), identity(&press), "{label} repeat");
        assert_eq!(identity(&release), identity(&press), "{label} release");
        assert_eq!(
            [&press, &repeat, &release].map(|p| p["action"].as_str().unwrap()),
            ["press", "repeat", "release"],
            "{label}"
        );
        assert_eq!(release["text"], "", "{label} release types nothing");
    }
}

/// The bytes Ghostty's encoder (the library the Rex server encodes `client.key` with) writes
/// for a payload, in legacy mode or with Kitty flags `CSI > 3 u`.
fn encode(p: &Value, kitty: bool) -> Vec<u8> {
    use ghostty_vt::key::{self as g, Encoder, Event, KittyKeyFlags};
    let key = match p["key"].as_str().unwrap() {
        "tab" => g::Key::Tab,
        "enter" => g::Key::Enter,
        "space" => g::Key::Space,
        "arrow_up" => g::Key::ArrowUp,
        "arrow_left" => g::Key::ArrowLeft,
        "arrow_right" => g::Key::ArrowRight,
        "home" => g::Key::Home,
        "end" => g::Key::End,
        "page_up" => g::Key::PageUp,
        "page_down" => g::Key::PageDown,
        "backspace" => g::Key::Backspace,
        "delete" => g::Key::Delete,
        "key_a" => g::Key::A,
        "digit_1" => g::Key::Digit1,
        other => panic!("no Ghostty key for {other}"),
    };
    let mods = |field: &str| {
        p[field]
            .as_str()
            .unwrap()
            .split('+')
            .filter(|m| !m.is_empty())
            .fold(g::Mods::empty(), |all, m| {
                all | match m {
                    "shift" => g::Mods::SHIFT,
                    "ctrl" => g::Mods::CTRL,
                    "alt" => g::Mods::ALT,
                    "super" => g::Mods::SUPER,
                    other => panic!("unknown modifier {other}"),
                }
            })
    };
    let action = match p["action"].as_str().unwrap() {
        "press" => g::Action::Press,
        "repeat" => g::Action::Repeat,
        _ => g::Action::Release,
    };
    let text = p["text"].as_str().unwrap();
    let mut event = Event::new().unwrap();
    event
        .set_action(action)
        .set_key(key)
        .set_mods(mods("mods"))
        .set_consumed_mods(mods("consumed_mods"))
        .set_utf8((!text.is_empty()).then(|| text.to_owned()));
    if let Some(c) =
        char::from_u32(p["unshifted_codepoint"].as_u64().unwrap() as u32).filter(|&c| c != '\0')
    {
        event.set_unshifted_codepoint(c);
    }
    let mut encoder = Encoder::new().unwrap();
    if kitty {
        encoder.set_kitty_flags(KittyKeyFlags::DISAMBIGUATE | KittyKeyFlags::REPORT_EVENTS);
    }
    let mut out = Vec::new();
    encoder.encode_to_vec(&event, &mut out).unwrap();
    out
}

#[test]
fn modified_keys_encode_distinctly_in_legacy_and_kitty_modes() {
    use Action::{Press, Release, Repeat};
    // (label, event, code, base, action, legacy bytes, Kitty bytes); `None` = not checked.
    // Legacy Shift+Backspace is a bare DEL: the legacy encoding has no way to say Shift there.
    type Row<'a> = (
        &'a str,
        KeyEvent,
        C,
        Option<char>,
        Action,
        Option<&'a [u8]>,
        &'a [u8],
    );
    let rows: Vec<Row> = vec![
        (
            "Tab",
            ev(Key::Tab, NONE),
            C::Tab,
            None,
            Press,
            Some(b"\t"),
            b"\t",
        ),
        (
            "Shift+Tab",
            ev(Key::Tab, SHIFT),
            C::Tab,
            None,
            Press,
            Some(b"\x1b[Z"),
            b"\x1b[9;2u",
        ),
        (
            "Shift+Tab repeat",
            ev(Key::Tab, SHIFT),
            C::Tab,
            None,
            Repeat,
            None,
            b"\x1b[9;2:2u",
        ),
        (
            "Enter",
            ev(Key::Enter, NONE),
            C::Enter,
            None,
            Press,
            Some(b"\r"),
            b"\r",
        ),
        (
            "Shift+Enter",
            ev(Key::Enter, SHIFT),
            C::Enter,
            None,
            Press,
            Some(b"\x1b[27;2;13~"),
            b"\x1b[13;2u",
        ),
        (
            "Ctrl+Enter",
            ev(Key::Enter, CTRL),
            C::Enter,
            None,
            Press,
            Some(b"\x1b[27;5;13~"),
            b"\x1b[13;5u",
        ),
        (
            "Alt+Enter",
            ev(Key::Enter, ALT),
            C::Enter,
            None,
            Press,
            Some(b"\x1b\r"),
            b"\x1b[13;3u",
        ),
        (
            "Ctrl+Shift+Enter",
            ev(Key::Enter, with(CTRL, SHIFT)),
            C::Enter,
            None,
            Press,
            Some(b"\x1b[27;6;13~"),
            b"\x1b[13;6u",
        ),
        (
            "Space",
            ev(text(" "), NONE),
            C::Space,
            Some(' '),
            Press,
            Some(b" "),
            b" ",
        ),
        (
            "Shift+Space",
            ev(text(" "), SHIFT),
            C::Space,
            Some(' '),
            Press,
            Some(b" "),
            b"\x1b[32;2u",
        ),
        (
            "Shift+Space release",
            ev(text(" "), SHIFT),
            C::Space,
            Some(' '),
            Release,
            None,
            b"\x1b[32;2:3u",
        ),
        (
            "Shift+Up",
            ev(Key::Up, SHIFT),
            C::ArrowUp,
            None,
            Press,
            Some(b"\x1b[1;2A"),
            b"\x1b[1;2:1A",
        ),
        (
            "Ctrl+Left",
            ev(Key::Left, CTRL),
            C::ArrowLeft,
            None,
            Press,
            Some(b"\x1b[1;5D"),
            b"\x1b[1;5:1D",
        ),
        (
            "Alt+Right",
            ev(Key::Right, ALT),
            C::ArrowRight,
            None,
            Press,
            Some(b"\x1b[1;3C"),
            b"\x1b[1;3:1C",
        ),
        (
            "Shift+Home",
            ev(Key::Home, SHIFT),
            C::Home,
            None,
            Press,
            Some(b"\x1b[1;2H"),
            b"\x1b[1;2:1H",
        ),
        (
            "Ctrl+End",
            ev(Key::End, CTRL),
            C::End,
            None,
            Press,
            Some(b"\x1b[1;5F"),
            b"\x1b[1;5:1F",
        ),
        (
            "Shift+PageUp",
            ev(Key::PageUp, SHIFT),
            C::PageUp,
            None,
            Press,
            Some(b"\x1b[5;2~"),
            b"\x1b[5;2~",
        ),
        (
            "Ctrl+PageDown",
            ev(Key::PageDown, CTRL),
            C::PageDown,
            None,
            Press,
            Some(b"\x1b[6;5~"),
            b"\x1b[6;5~",
        ),
        (
            "Backspace",
            ev(Key::Backspace, NONE),
            C::Backspace,
            None,
            Press,
            Some(b"\x7f"),
            b"\x7f",
        ),
        (
            "Shift+Backspace",
            ev(Key::Backspace, SHIFT),
            C::Backspace,
            None,
            Press,
            Some(b"\x7f"),
            b"\x1b[127;2u",
        ),
        (
            "Ctrl+Backspace",
            ev(Key::Backspace, CTRL),
            C::Backspace,
            None,
            Press,
            Some(b"\x08"),
            b"\x1b[127;5u",
        ),
        (
            "Alt+Backspace",
            ev(Key::Backspace, ALT),
            C::Backspace,
            None,
            Press,
            Some(b"\x1b\x7f"),
            b"\x1b[127;3u",
        ),
        (
            "Shift+Delete",
            ev(Key::Delete, SHIFT),
            C::Delete,
            None,
            Press,
            Some(b"\x1b[3;2~"),
            b"\x1b[3;2~",
        ),
        (
            "Ctrl+Delete",
            ev(Key::Delete, CTRL),
            C::Delete,
            None,
            Press,
            Some(b"\x1b[3;5~"),
            b"\x1b[3;5~",
        ),
        (
            "Shift+a",
            ev(text("A"), SHIFT),
            C::KeyA,
            Some('a'),
            Press,
            Some(b"A"),
            b"A",
        ),
        (
            "Shift+a release",
            ev(text("A"), SHIFT),
            C::KeyA,
            Some('a'),
            Release,
            None,
            b"\x1b[97;2:3u",
        ),
        (
            "Shift+1",
            ev(text("!"), SHIFT),
            C::Digit1,
            Some('1'),
            Press,
            Some(b"!"),
            b"!",
        ),
        (
            "Alt+Shift+a",
            ev(text("A"), with(ALT, SHIFT)),
            C::KeyA,
            Some('a'),
            Press,
            None,
            b"\x1b[97;4u",
        ),
    ];
    let show = |b: &[u8]| String::from_utf8_lossy(b).escape_debug().to_string();
    for (label, event, code, base, action, legacy, kitty) in rows {
        let p = payload(&event, Some(code), base, action).unwrap();
        if let Some(legacy) = legacy {
            assert_eq!(show(&encode(&p, false)), show(legacy), "{label} (legacy)");
        }
        assert_eq!(show(&encode(&p, true)), show(kitty), "{label} (Kitty)");
    }
}

#[test]
fn a_composed_press_and_its_plain_release_may_differ_in_consumption_but_not_in_bytes() {
    // Dead `~` then Shift+Space: winit gives the press the composed text and the release only
    // the plain key, so each decides on its own text. Ghostty ignores consumption on a
    // release, so the Kitty release is the same either way.
    let press = payload(
        &ev(text("~"), SHIFT),
        Some(C::Space),
        Some(' '),
        Action::Press,
    )
    .unwrap();
    let release = payload(
        &ev(text(" "), SHIFT),
        Some(C::Space),
        Some(' '),
        Action::Release,
    )
    .unwrap();
    assert_eq!(fields(&press), ("space", "shift", "shift", "~", 32));
    assert_eq!(fields(&release), ("space", "shift", "", "", 32));
    assert_eq!(encode(&press, true), b"~");
    let mut consumed_release = release.clone();
    consumed_release["consumed_mods"] = "shift".into();
    assert_eq!(encode(&release, true), b"\x1b[32;2:3u");
    assert_eq!(encode(&consumed_release, true), encode(&release, true));
}
