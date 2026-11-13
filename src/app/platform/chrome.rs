//! Whose frame the window has.

/// Whether Rex draws the window's frame itself: no system title bar, and its own close
/// button, drag region and resize bands.
///
/// It does on Linux, where an app-drawn frame is the norm and a tiling compositor gives
/// none. macOS keeps the system's frame: its traffic lights, edge resizing, full screen and
/// window tiling, behaving as they do in every Mac app. A borderless window there has no
/// close, minimize or zoom button and cannot be resized by its edges (winit cannot start an
/// edge resize on macOS).
pub const OWN_FRAME: bool = !cfg!(target_os = "macos");

/// Whether Rex's toolbar is the window's title bar ([`TITLEBAR`]): one row with the traffic
/// lights at its leading end and no native title, as in Rex for macOS.
pub const UNIFIED: bool = cfg!(target_os = "macos");

/// The unified title row's height in points, before the interface zoom. Rex for macOS draws
/// about 36 pt; two more keep a tab pill's track clear of the row's edges.
pub const TITLE_ROW: f32 = 38.;

/// The title bar Rex draws itself, where it does ([`UNIFIED`]): Rune hides the native one,
/// runs the content under a transparent title bar and centres the traffic lights in the row.
pub const TITLEBAR: Option<rune::Titlebar> = if UNIFIED {
    Some(rune::Titlebar::new(TITLE_ROW))
} else {
    None
};

/// The frame to ask Rune for: on macOS a standard titled window, which [`TITLEBAR`] then
/// replaces with a transparent title bar over the content. Other platforms keep their frame
/// either way.
pub const FRAME: rune::Chrome = rune::Chrome {
    full_size_content: false,
    ..rune::Chrome::DEFAULT
};

/// The platform's one window system, where it has one, for the debug report. `None` where
/// the session decides (X11 or Wayland).
pub const WINDOW_SYSTEM: Option<&str> = if cfg!(target_os = "macos") {
    Some("Quartz")
} else {
    None
};
