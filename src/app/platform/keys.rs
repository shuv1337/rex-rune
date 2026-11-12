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

/// A key event's modifiers as a shortcut spells them.
pub fn modifiers(m: Mods) -> Modifiers {
    modifiers_on(m, keymap::command_key())
}

/// [`modifiers`] where shortcuts are on Command (`cmd` is its own modifier, spelled `cmd`)
/// or on Control (`cmd` is only Rune's other name for Control).
pub fn modifiers_on(m: Mods, command: bool) -> Modifiers {
    Modifiers {
        ctrl: m.ctrl || (m.cmd && !command),
        alt: m.alt,
        shift: m.shift,
        logo: m.logo || (m.cmd && command),
    }
}

/// Whether the platform's shortcut modifier is held, for the few keys that are not
/// configurable (Enter in Commands, the pane menu).
pub fn primary(m: Mods) -> bool {
    let modifiers = modifiers(m);
    if keymap::command_key() {
        modifiers.logo
    } else {
        modifiers.ctrl
    }
}

/// Whether Control is held, for a key that goes to the program in the terminal. On macOS
/// Command is not Control and is sent as Super; elsewhere `cmd` is set along with Control.
pub fn control(m: Mods) -> bool {
    m.ctrl || (m.cmd && !cfg!(target_os = "macos"))
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
        assert_eq!(modifiers(linux_control_k), ctrl);
        assert!(primary(linux_control_k) && !primary(Mods::default()));
    }
}
