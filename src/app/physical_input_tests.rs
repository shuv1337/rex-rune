//! Public Rune input seam: the same path used by native keyboard events.
use rune::{Cx, El, Env, Key, KeyEvent, Mods, PhysicalInput, PhysicalKeyCode as Code, Runtime, el};

#[derive(Debug, PartialEq)]
struct Seen {
    key: Key,
    code: Option<Code>,
    base: Option<char>,
    repeat: bool,
    release: bool,
    ctrl: bool,
}
fn record_press(s: &mut Vec<Seen>, ev: &KeyEvent, tx: &mut rune::Tx) -> bool {
    s.push(Seen {
        key: ev.key.clone(),
        code: tx.physical_key(),
        base: tx.key_base(),
        repeat: tx.key_repeat(),
        release: false,
        ctrl: ev.mods.ctrl,
    });
    true
}
fn view(_: &Vec<Seen>, _: &mut Cx<Vec<Seen>>) -> El {
    el().size(300., 200.)
        .on_key(|s: &mut Vec<Seen>, ev, tx| {
            s.push(Seen {
                key: ev.key.clone(),
                code: tx.physical_key(),
                base: tx.key_base(),
                repeat: tx.key_repeat(),
                release: false,
                ctrl: ev.mods.ctrl,
            });
            true
        })
        .on_key_up(|s: &mut Vec<Seen>, ev, tx| {
            s.push(Seen {
                key: ev.key.clone(),
                code: tx.physical_key(),
                base: tx.key_base(),
                repeat: tx.key_repeat(),
                release: true,
                ctrl: ev.mods.ctrl,
            });
            true
        })
}
#[test]
fn physical_input_preserves_layout_repeat_and_release() {
    let mut rt = Runtime::new(Vec::new(), view, (300., 200.), Env::new(true, 1.));
    rt.on_key = Some(record_press);
    rt.frame(0.);
    let mods = Mods {
        ctrl: true,
        shift: true,
        ..Mods::default()
    };
    for (pressed, repeat) in [(true, false), (true, true), (false, false)] {
        rt.physical_key_input(
            PhysicalInput::Key {
                key: Code::KeyT,
                mods,
                pressed,
                repeat,
                text: pressed.then(|| "Е".into()),
                consumed_mods: Mods::default(),
            },
            KeyEvent {
                key: Key::Text("Е".into()),
                mods,
            },
            Some('е'),
        );
        rt.frame(0.);
    }
    assert_eq!(rt.state.len(), 3);
    for (seen, (repeat, release)) in
        rt.state
            .iter()
            .zip([(false, false), (true, false), (false, true)])
    {
        assert_eq!(
            seen,
            &Seen {
                key: Key::Text("Е".into()),
                code: Some(Code::KeyT),
                base: Some('е'),
                repeat,
                release,
                ctrl: true
            }
        );
    }
}

#[test]
fn physical_input_without_base_does_not_invent_a_layout() {
    let mut rt = Runtime::new(Vec::new(), view, (300., 200.), Env::new(true, 1.));
    rt.on_key = Some(record_press);
    rt.frame(0.);
    rt.physical_key_input(
        PhysicalInput::Key {
            key: Code::KeyK,
            mods: Mods::default(),
            pressed: true,
            repeat: false,
            text: Some("t".into()),
            consumed_mods: Mods::default(),
        },
        KeyEvent {
            key: Key::Text("t".into()),
            mods: Mods::default(),
        },
        None,
    );
    rt.frame(0.);
    assert_eq!(rt.state[0].base, None);
    assert_eq!(rt.state[0].code, Some(Code::KeyK));
    assert_eq!(rt.state[0].key, Key::Text("t".into()));
}
