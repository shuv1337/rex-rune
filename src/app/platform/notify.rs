//! Desktop notifications: which service, if any, can show them on this platform.
//! Linux and the BSDs use the freedesktop notification service; macOS the system's
//! notification centre; other platforms (Windows) have none yet, and say so.
//!
//! Choosing here keeps every call site free of an operating-system `cfg`. A platform or a
//! copy of the app that cannot post says why through its backend, and Settings →
//! Notifications shows that reason in the service row beside the unchanged switches.

#[cfg(all(unix, not(target_os = "macos")))]
mod dbus;
#[cfg(all(unix, not(target_os = "macos")))]
pub use dbus::{Desktop, restart_failure};

#[cfg(target_os = "macos")]
mod user_notifications;
#[cfg(target_os = "macos")]
pub use user_notifications::{Desktop, restart_failure};

#[cfg(not(unix))]
mod none;
#[cfg(not(unix))]
pub use none::{Desktop, restart_failure};

/// The freedesktop service and its bus, on Linux and the BSDs.
#[cfg(all(unix, not(target_os = "macos")))]
pub const SERVICE_SECTION: &str = "Desktop service";
#[cfg(all(unix, not(target_os = "macos")))]
pub const SERVICE_DETAIL: &str = "Last check, not a delivery receipt.";
#[cfg(all(unix, not(target_os = "macos")))]
pub const TEST_FOOTNOTE: &str = "Tests work even when desktop notifications are off.";

/// The notification centre, which macOS may withhold from this copy of the app.
#[cfg(target_os = "macos")]
pub const SERVICE_SECTION: &str = "macOS notifications";
#[cfg(target_os = "macos")]
pub const SERVICE_DETAIL: &str = "What macOS lets this app do.";
#[cfg(target_os = "macos")]
pub const TEST_FOOTNOTE: &str =
    "Tests work even when desktop notifications are off, and ask macOS for permission.";

/// No desktop channel at all.
#[cfg(not(unix))]
pub const SERVICE_SECTION: &str = "Desktop service";
#[cfg(not(unix))]
pub const SERVICE_DETAIL: &str = "Not available on this platform.";
#[cfg(not(unix))]
pub const TEST_FOOTNOTE: &str = "Tests work even when desktop notifications are off.";
