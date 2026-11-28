//! `App::float_on_open`: tiling window managers float a toplevel that maps at a fixed size, so
//! the window maps with its minimum equal to its maximum and takes its own limits once the
//! compositor has mapped it.
//!
//! Releasing those limits as soon as the first frame finishes (`Host::first_frame_ready`, inside
//! `AboutToWait`) races with explicit-sync Wayland compositors such as Hyprland: the compositor
//! holds the mapping commit until its acquire timeline point signals, yet applies
//! `xdg_toplevel.set_min_size` / `set_max_size` to pending state on arrival, so a busy compositor
//! can map the window with `min != max` and tile it.
//!
//! Instead, `first_frame_ready` marks the release as owed and requests a redraw, and `Host::paint`
//! releases the limits on that follow-up paint. winit's Wayland backend withholds
//! `WindowEvent::RedrawRequested` until the first frame's `wl_surface.frame` callback arrives
//! (armed by `pre_present_notify`), which happens only after the compositor applies the mapping
//! commit.
#![cfg_attr(
    not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))),
    allow(dead_code)
)]

#[cfg(not(target_arch = "wasm32"))]
use winit::dpi::{LogicalSize, PhysicalSize, Size};
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
use winit::window::Window;
#[cfg(not(target_arch = "wasm32"))]
use winit::window::WindowAttributes;

use crate::machine::Effects;
#[cfg(not(target_arch = "wasm32"))]
use crate::windows::WindowOptions;

#[cfg(test)]
#[path = "float_on_open_tests.rs"]
mod tests;

/// Lifecycle of the fixed size limits held by `App::float_on_open` for one native window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::window) enum State {
    /// Window opened with `min == max` so a tiling window manager floats it on map; waiting for
    /// the first frame to be presented (`Event::FirstFrameDrawn`).
    Fixed,
    /// First frame was presented (arming the compositor's frame callback on Wayland) and a
    /// follow-up redraw has been requested; waiting for that post-callback `Event::Paint` before
    /// restoring the window's configured size limits.
    Owed,
    /// Limits are not held (either `float_on_open` is disabled, or the limits have already been
    /// released).
    #[default]
    Done,
}

impl State {
    /// Initial state for a newly created native window.
    pub(in crate::window) const fn new(float_on_open: bool) -> Self {
        if float_on_open { Self::Fixed } else { Self::Done }
    }

    /// Whether the window's min/max size limits are still locked at its opening size.
    #[cfg(test)]
    pub(in crate::window) const fn is_locked(self) -> bool {
        !matches!(self, Self::Done)
    }
}

/// Events driving the `float_on_open` release state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::window) enum Event {
    /// The window finished presenting its first frame (`Host::first_frame_ready`).
    FirstFrameDrawn,
    /// `Host::paint` entered for `WindowEvent::RedrawRequested` (on Wayland, withheld by winit
    /// until the prior frame's `wl_callback.done` arrives).
    Paint,
}

/// Side effect emitted by [`step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::window) enum Effect {
    /// Ask winit for a redraw so the next `Host::paint` runs after the first frame's callback.
    RequestRedraw,
    /// Restore the window's configured min/max size limits ([`release`]).
    Release,
}

/// Advances the `float_on_open` release state machine.
pub(in crate::window) fn step(state: &mut State, event: Event) -> Effects<Effect> {
    match (*state, event) {
        (State::Fixed, Event::FirstFrameDrawn) => {
            *state = State::Owed;
            Effects::one(Effect::RequestRedraw)
        }
        (State::Owed, Event::Paint) => {
            *state = State::Done;
            Effects::one(Effect::Release)
        }
        (State::Fixed, Event::Paint)
        | (State::Owed, Event::FirstFrameDrawn)
        | (State::Done, Event::FirstFrameDrawn | Event::Paint) => Effects::none(),
    }
}

/// `attrs` with the window's size as both its minimum and its maximum.
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) fn fixed(attrs: WindowAttributes, options: &WindowOptions, size: (f32, f32)) -> WindowAttributes {
    let fixed: Size = attrs.inner_size.unwrap_or_else(|| match options.physical_size {
        Some((w, h)) => PhysicalSize::new(w, h).into(),
        None => LogicalSize::new(size.0, size.1).into(),
    });
    attrs.with_min_inner_size(fixed).with_max_inner_size(fixed)
}

/// Mapped at a fixed size, so a tiling window manager floated it: now the user can resize it.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(in crate::window) fn release(window: &Window, options: &WindowOptions, app_min: Option<(f32, f32)>) {
    let (min, max) = options.size_limits(app_min);
    window.set_min_inner_size(Some(LogicalSize::new(min.0, min.1)));
    window.set_max_inner_size(max.map(|(w, h)| LogicalSize::new(w, h)));
}
