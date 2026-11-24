use winit::dpi::{LogicalPosition, LogicalSize};
use winit::window::Window;

use super::{ImeSync, Win, ime_sync::ImeStep};
use crate::runtime::Runtime;

/// Update native IME state from either the frame or the focus handler.
pub(super) fn sync_ime<S: 'static>(
    window: &Window,
    rt: &mut Runtime<S>,
    ime: &mut ImeSync,
    focused: bool,
) {
    let step = ime_sync_step(rt, ime, focused, window.scale_factor() as f32);
    if step.cancel || step.set_allowed.is_some() {
        window.set_ime_allowed(false);
    }
    if step.set_allowed == Some(true) {
        window.set_ime_allowed(true);
    }
    if let Some(a) = step.cursor_area {
        window.set_ime_cursor_area(
            LogicalPosition::new(a[0] as f64, a[1] as f64),
            LogicalSize::new(a[2] as f64, a[3] as f64),
        );
    }
}

pub(super) fn ime_sync_step<S: 'static>(
    rt: &mut Runtime<S>,
    ime: &mut ImeSync,
    focused: bool,
    scale: f32,
) -> ImeStep {
    ime.step(
        std::mem::take(&mut rt.ime_reset),
        rt.ime_area.map(|a| rt.to_window(a)),
        rt.has_secure_focus(),
        focused,
        scale,
    )
}

/// What a built frame hands the window besides its pixels: secure input, the IME, the cursor,
/// help tags and the clipboard.
pub(super) fn sync_platform<S: 'static>(
    l: &mut Win,
    rt: &mut Runtime<S>,
    secure_input: &mut crate::input::secure_input::SecureInput,
    ime: &mut ImeSync,
    clipboard: &crate::clipboard::Client,
) {
    let focused = l.window.has_focus();
    secure_input.set(rt.has_secure_focus() && focused);
    sync_ime(&l.window, rt, ime, focused);
    l.apply_cursor(rt, focused);
    l.sync_option(rt);
    #[cfg(target_os = "macos")]
    if rt.zoom() == 1.0 {
        l.native.help.sync(&l.window, rt.help_regions());
    } else {
        // Window points: the view's times its zoom.
        let regions: Vec<([f32; 4], String)> = rt.help_regions().iter().map(|(r, t)| (rt.to_window(*r), t.clone())).collect();
        l.native.help.sync(&l.window, &regions);
    }
    // The process has one menu theme: the active window's, so its window menu matches it.
    #[cfg(target_os = "windows")]
    if focused { crate::platform::win32::dark_mode::menus(rt.backdrop_dark().unwrap_or(rt.env.system_dark)); }
    clipboard.flush(rt);
}

impl Win {
    /// Option as the runtime last asked: Alt, or the layout's characters while a field has
    /// focus. Set after a frame's input, so a field focused by a key has Option's characters
    /// from the next key on. It needs no drawable, so frames that run input without drawing
    /// (covered or occluded windows) set it too.
    pub(super) fn sync_option<S>(&mut self, rt: &Runtime<S>) {
        #[cfg(target_os = "macos")]
        if rt.option_alt() != self.option_as_alt {
            self.option_as_alt = rt.option_alt();
            super::chrome::option_as_alt(&self.window, self.option_as_alt);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = rt;
    }

    /// The runtime's cursor while the pointer is in the window, the arrow once it leaves.
    /// `Hidden` needs focus, so it can't outlive the window being active.
    pub(super) fn apply_cursor<S>(&mut self, rt: &Runtime<S>, focused: bool) {
        let want = match rt.cursor() { crate::el::Cursor::Hidden if !focused => crate::el::Cursor::Default, c => c };
        self.cursor.apply(&self.window, want, rt.pointer().is_some());
    }
}
