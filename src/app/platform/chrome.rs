//! Whose frame the window has.

/// Whether Rex draws the window's frame itself: no system title bar, and its own close
/// button, drag region and resize bands.
///
/// It does on Linux, where an app-drawn frame is the norm and a tiling compositor gives
/// none. macOS keeps the system's title bar. A borderless window there has no close,
/// minimize or zoom button and cannot be resized by its edges (winit cannot start an edge
/// resize on macOS), and the system's own frame brings all of that, with full screen and
/// window tiling, behaving as it does in every Mac app. Drawing Rex's toolbar in the title
/// bar beside the traffic lights (Rune's `Titlebar`) is the better look and needs checking
/// by eye.
pub const OWN_FRAME: bool = !cfg!(target_os = "macos");

/// The frame to ask Rune for. Its default on macOS is a transparent title bar with the
/// content, and so Rex's toolbar, running under the traffic lights. This asks for the
/// standard one: an opaque title bar that shows the title, with the content below it.
/// Other platforms keep their frame either way.
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
