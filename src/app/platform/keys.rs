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

/// Whether the Return key presses the button that has keyboard focus, as Linux and Windows
/// dialogs do. There, a dialog's default button takes Return only while no button has focus,
/// and Space presses a focused button at any time (Rune does both already). macOS keeps
/// Return for the default button and Space for the focused one, so buttons add no Return
/// handler there and Tab order stays as AppKit leaves it.
pub const RETURN_PRESSES_FOCUSED: bool = !cfg!(target_os = "macos");

/// Whether `ev` is the plain Return that presses a focused button where
/// [`RETURN_PRESSES_FOCUSED`] holds: no modifier, as Rune's default button needs.
pub fn return_presses_focused(ev: &rune::KeyEvent) -> bool {
    RETURN_PRESSES_FOCUSED && ev.key == rune::Key::Enter && ev.mods == Mods::default()
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

/// Whether Alt is a Mac's Option key, which composes characters unless it is told to act as
/// Alt (`Tx::set_option_as_alt`). Elsewhere Alt is always Alt.
pub const OPTION_COMPOSES: bool = cfg!(target_os = "macos");

/// Whether the keyboard layout in use is U.S. English or ABC. Main thread only. True where
/// [`OPTION_COMPOSES`] is false.
pub fn us_layout() -> bool {
    #[cfg(target_os = "macos")]
    return layout::current().is_some_and(|id| layout::is_us(&id));
    #[cfg(not(target_os = "macos"))]
    true
}

/// Calls `changed` whenever the person picks another keyboard layout, for as long as the
/// process runs. Call it once, from the main thread. Nothing where [`OPTION_COMPOSES`] is
/// false.
pub fn watch_layout(changed: impl Fn() + Send + Sync + 'static) {
    #[cfg(target_os = "macos")]
    layout::watch(Box::new(changed));
    #[cfg(not(target_os = "macos"))]
    let _ = changed;
}

#[cfg(target_os = "macos")]
mod layout {
    use std::ffi::{c_char, c_void};

    type CFTypeRef = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    const DELIVER_IMMEDIATELY: isize = 4;

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        static kTISPropertyInputSourceID: CFTypeRef;
        static kTISNotifySelectedKeyboardInputSourceChanged: CFTypeRef;
        fn TISCopyCurrentKeyboardLayoutInputSource() -> CFTypeRef;
        fn TISGetInputSourceProperty(source: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: CFTypeRef);
        fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, len: isize, encoding: u32) -> u8;
        fn CFNotificationCenterGetDistributedCenter() -> CFTypeRef;
        fn CFNotificationCenterAddObserver(
            center: CFTypeRef,
            observer: *const c_void,
            callback: extern "C" fn(CFTypeRef, *mut c_void, CFTypeRef, CFTypeRef, CFTypeRef),
            name: CFTypeRef,
            object: CFTypeRef,
            suspension: isize,
        );
    }

    /// Layouts whose Option characters few people type in a terminal, as Rex for macOS and
    /// Ghostty decide.
    pub(super) fn is_us(id: &str) -> bool {
        matches!(id, "com.apple.keylayout.US" | "com.apple.keylayout.ABC")
    }

    /// The input source id of the layout in use, such as `com.apple.keylayout.German`.
    pub(super) fn current() -> Option<String> {
        // SAFETY: the copied source is released once its id, which it owns, is read.
        unsafe {
            let source = TISCopyCurrentKeyboardLayoutInputSource();
            if source.is_null() {
                return None;
            }
            let id = TISGetInputSourceProperty(source, kTISPropertyInputSourceID);
            let mut buf = [0 as c_char; 128];
            let read = !id.is_null()
                && CFStringGetCString(id, buf.as_mut_ptr(), buf.len() as isize, UTF8) != 0;
            CFRelease(source);
            read.then(|| {
                std::ffi::CStr::from_ptr(buf.as_ptr())
                    .to_string_lossy()
                    .into_owned()
            })
        }
    }

    type Changed = Box<dyn Fn() + Send + Sync>;

    extern "C" fn notified(
        _: CFTypeRef,
        observer: *mut c_void,
        _: CFTypeRef,
        _: CFTypeRef,
        _: CFTypeRef,
    ) {
        // SAFETY: `observer` is the `Changed` that `watch` leaked, which is never freed.
        let changed = unsafe { &*(observer as *const Changed) };
        // A panic must not unwind into CoreFoundation.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(changed));
    }

    pub(super) fn watch(changed: Changed) {
        let observer = Box::into_raw(Box::new(changed));
        // SAFETY: the observer lives for the process and may be called on any thread.
        unsafe {
            CFNotificationCenterAddObserver(
                CFNotificationCenterGetDistributedCenter(),
                observer as *const c_void,
                notified,
                kTISNotifySelectedKeyboardInputSourceChanged,
                std::ptr::null(),
                DELIVER_IMMEDIATELY,
            );
        }
    }
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
    fn return_presses_a_focused_button_only_where_the_platform_does() {
        let enter = rune::KeyEvent {
            key: rune::Key::Enter,
            mods: Mods::default(),
        };
        assert_eq!(return_presses_focused(&enter), !cfg!(target_os = "macos"));
        // Space and modified Return are not the plain Return that presses a focused button.
        assert!(!return_presses_focused(&rune::KeyEvent {
            key: rune::Key::Text(" ".into()),
            mods: Mods::default(),
        }));
        assert!(!return_presses_focused(&rune::KeyEvent {
            key: rune::Key::Enter,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
        }));
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
