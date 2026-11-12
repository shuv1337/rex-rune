//! Which key is the platform's own for shortcuts.
//!
//! A Mac's shortcuts are on Command, which reaches no program in the terminal; Linux and
//! Windows use Control. Rune reports the difference in [`Mods`]: `cmd` is the Command key
//! on macOS and comes with Control everywhere else.
use rex_rune::keymap::{self, Modifiers};
use rune::Mods;

/// Choose this platform's shortcuts. Called once at startup, before settings are read.
/// Headless tests never call it, so one set of key presses tests every platform against
/// the Control-key defaults.
pub fn init() {
    if cfg!(target_os = "macos") {
        keymap::use_command_key();
    }
}

/// A key event's modifiers as a shortcut spells them, where shortcuts are on Command (`cmd`
/// is its own modifier, spelled `cmd`) or on Control (`cmd` is only Rune's other name for
/// Control).
pub fn modifiers_on(m: Mods, command: bool) -> Modifiers {
    Modifiers {
        ctrl: m.ctrl || (m.cmd && !command),
        alt: m.alt,
        shift: m.shift,
        logo: m.logo || (m.cmd && command),
    }
}

/// Whether the platform's shortcut modifier is held, for the few keys that are not
/// configurable (Enter in Commands, the pane menu, a click that opens a link).
pub fn primary(m: Mods) -> bool {
    primary_on(m, keymap::command_key())
}

/// [`primary`] where shortcuts are on Command or on Control.
pub fn primary_on(m: Mods, command: bool) -> bool {
    let modifiers = modifiers_on(m, command);
    if command {
        modifiers.logo
    } else {
        modifiers.ctrl
    }
}

/// What the platform's shortcut modifier is called, in text the app writes itself.
pub fn primary_name() -> &'static str {
    if keymap::command_key() { "Cmd" } else { "Ctrl" }
}

/// What the Alt key is called on this platform's keyboard.
pub fn alt_name() -> &'static str {
    if keymap::command_key() {
        "Option"
    } else {
        "Alt"
    }
}

/// Whether Control is held, for a key that goes to the program in the terminal. Where
/// shortcuts are on Command, Command is not Control and is sent as Super; elsewhere `cmd`
/// is only set along with Control.
pub fn control(m: Mods) -> bool {
    m.ctrl || (m.cmd && !keymap::command_key())
}

/// A key the system's window menu owns where the window has the system's frame: Command
/// with W closes the window and with M minimizes it, when the view does not take the key.
/// `None` on a platform whose shortcuts are on Control, and for every other key.
pub fn window_menu_key(key: &rune::Key, m: Mods) -> Option<WindowMenuKey> {
    window_menu_key_on(key, m, keymap::command_key())
}

pub fn window_menu_key_on(key: &rune::Key, m: Mods, command: bool) -> Option<WindowMenuKey> {
    let plain_command = command && m.cmd && !m.ctrl && !m.alt && !m.shift;
    match key {
        rune::Key::Command('w') if plain_command => Some(WindowMenuKey::Close),
        rune::Key::Command('m') if plain_command => Some(WindowMenuKey::Minimize),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowMenuKey {
    Close,
    Minimize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_is_its_own_modifier_only_where_shortcuts_are_on_it() {
        // What a Mac keyboard sends for Command+K, and for Control+K.
        let command_k = Mods {
            cmd: true,
            logo: true,
            ..Default::default()
        };
        let control_k = Mods {
            ctrl: true,
            ..Default::default()
        };
        let cmd = Modifiers {
            logo: true,
            ..Default::default()
        };
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(modifiers_on(command_k, true), cmd);
        assert_eq!(modifiers_on(control_k, true), ctrl);
        // What a Linux keyboard sends for Control+K: `cmd` rides along and means Control.
        let linux_control_k = Mods {
            ctrl: true,
            cmd: true,
            ..Default::default()
        };
        assert_eq!(modifiers_on(linux_control_k, false), ctrl);
        // Tests never switch to Command, so they see Control on every platform.
        assert!(!keymap::command_key());
        assert_eq!(modifiers_on(linux_control_k, keymap::command_key()), ctrl);
        assert!(primary(linux_control_k) && !primary(Mods::default()));
        // On a Mac, Command is the primary modifier and Control is not.
        assert!(primary_on(command_k, true) && !primary_on(control_k, true));
        assert!(primary_on(linux_control_k, false));
    }

    #[test]
    fn the_systems_close_and_minimize_keys_are_only_plain_command_w_and_m() {
        use rune::Key;
        let command = Mods {
            cmd: true,
            logo: true,
            ..Default::default()
        };
        let w = Key::Command('w');
        assert_eq!(
            window_menu_key_on(&w, command, true),
            Some(WindowMenuKey::Close)
        );
        assert_eq!(
            window_menu_key_on(&Key::Command('m'), command, true),
            Some(WindowMenuKey::Minimize)
        );
        // Not on Control platforms, not with another modifier, not another key.
        assert_eq!(window_menu_key_on(&w, command, false), None);
        let shifted = Mods {
            shift: true,
            ..command
        };
        assert_eq!(window_menu_key_on(&w, shifted, true), None);
        assert_eq!(window_menu_key_on(&Key::Command('k'), command, true), None);
    }
}
