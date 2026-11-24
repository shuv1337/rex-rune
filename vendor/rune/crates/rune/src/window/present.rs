//! Present-mode selection for native top-level windows.
//!
//! Top-level Wayland windows are already paced by winit's `wl_surface.frame` callback through
//! `Window::pre_present_notify()`. Pairing that wait with Vulkan FIFO can double-block on some
//! compositors/drivers (for example KWin + Mesa RADV), dropping throughput to ~19 fps on a 60 Hz
//! display. Preferring `Mailbox` when the surface advertises it keeps frames tear-free without
//! blocking acquire/present on a second vertical blank, while falling back to `AutoVsync` when
//! `Mailbox` is unavailable.
//!
//! X11, macOS, Windows, embedded surfaces, and Wayland layer-shell overlays keep `AutoVsync` by
//! default (`Overlay::attach` relies on FIFO vsync under `ControlFlow::Poll`).
//!
//! `RUNE_PRESENT_MODE` overrides the choice for top-level windows (`auto`, `fifo`,
//! `fifo-relaxed` / `fifo_relaxed`, `mailbox`, `immediate`), validated against the surface's
//! reported capabilities.

use std::sync::{Once, OnceLock};

/// Present modes exposed as a Rune-owned enum so callers and tests need not depend on `wgpu`.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentChoice {
    Vsync,
    Fifo,
    FifoRelaxed,
    Mailbox,
    Immediate,
}

impl PresentChoice {
    fn label(self) -> &'static str {
        match self {
            Self::Vsync => "auto-vsync",
            Self::Fifo => "fifo",
            Self::FifoRelaxed => "fifo-relaxed",
            Self::Mailbox => "mailbox",
            Self::Immediate => "immediate",
        }
    }

    fn from_wgpu(mode: wgpu::PresentMode) -> Option<Self> {
        match mode {
            wgpu::PresentMode::Fifo => Some(Self::Fifo),
            wgpu::PresentMode::FifoRelaxed => Some(Self::FifoRelaxed),
            wgpu::PresentMode::Mailbox => Some(Self::Mailbox),
            wgpu::PresentMode::Immediate => Some(Self::Immediate),
            wgpu::PresentMode::AutoVsync | wgpu::PresentMode::AutoNoVsync => None,
        }
    }

    fn to_wgpu(self) -> wgpu::PresentMode {
        match self {
            Self::Vsync => wgpu::PresentMode::AutoVsync,
            Self::Fifo => wgpu::PresentMode::Fifo,
            Self::FifoRelaxed => wgpu::PresentMode::FifoRelaxed,
            Self::Mailbox => wgpu::PresentMode::Mailbox,
            Self::Immediate => wgpu::PresentMode::Immediate,
        }
    }
}

const EXPECTED_MODES: &str = "auto, fifo, fifo-relaxed, mailbox, immediate";
const MAX_ASKED_CHARS: usize = 64;

fn preview_asked(raw: &str) -> String {
    let mut chars = raw.chars();
    let prefix: String = chars.by_ref().take(MAX_ASKED_CHARS).collect();
    if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}

fn format_offered(offered: &[PresentChoice]) -> String {
    if offered.is_empty() {
        return "none".into();
    }
    let mut out = String::new();
    for (i, choice) in offered.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(choice.label());
    }
    out
}

/// Picks a surface present mode from `offered` capabilities and an optional `RUNE_PRESENT_MODE`
/// string. Returns the chosen mode and an optional warning when the requested override was
/// invalid or unsupported by `offered`.
///
/// The returned `PresentChoice` is always either `PresentChoice::Vsync` or present in `offered`,
/// and the automatic path (`asked` is `None`, empty/whitespace, or `"auto"`) never returns
/// `PresentChoice::Immediate`.
#[doc(hidden)]
pub fn choose_present(
    prefer_mailbox: bool,
    offered: &[PresentChoice],
    asked: Option<&str>,
) -> (PresentChoice, Option<String>) {
    let default = if prefer_mailbox && offered.contains(&PresentChoice::Mailbox) {
        PresentChoice::Mailbox
    } else {
        PresentChoice::Vsync
    };
    let Some(raw) = asked else {
        return (default, None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
        return (default, None);
    }
    let requested = if trimmed.eq_ignore_ascii_case("fifo") {
        PresentChoice::Fifo
    } else if trimmed.eq_ignore_ascii_case("fifo-relaxed")
        || trimmed.eq_ignore_ascii_case("fifo_relaxed")
    {
        PresentChoice::FifoRelaxed
    } else if trimmed.eq_ignore_ascii_case("mailbox") {
        PresentChoice::Mailbox
    } else if trimmed.eq_ignore_ascii_case("immediate") {
        PresentChoice::Immediate
    } else {
        let shown = preview_asked(raw);
        return (
            default,
            Some(format!(
                "rune: ignoring invalid RUNE_PRESENT_MODE={shown:?} (expected {EXPECTED_MODES}); using {}",
                default.label()
            )),
        );
    };
    if offered.contains(&requested) {
        (requested, None)
    } else {
        let offered_list = format_offered(offered);
        (
            default,
            Some(format!(
                "rune: RUNE_PRESENT_MODE={} is not supported by this surface (offered: {offered_list}); using {}",
                requested.label(),
                default.label()
            )),
        )
    }
}

enum EnvOverride {
    Unset,
    Utf8(String),
    NonUtf8,
}

fn env_override() -> &'static EnvOverride {
    static OVERRIDE: OnceLock<EnvOverride> = OnceLock::new();
    OVERRIDE.get_or_init(|| match std::env::var_os("RUNE_PRESENT_MODE") {
        None => EnvOverride::Unset,
        Some(value) => match value.into_string() {
            Ok(text) => EnvOverride::Utf8(text),
            Err(_) => EnvOverride::NonUtf8,
        },
    })
}

/// Applies the platform default or `RUNE_PRESENT_MODE` override to a top-level window's
/// `SurfaceConfiguration`, validated against `surface.get_capabilities(adapter).present_modes`.
pub(super) fn apply(
    config: &mut wgpu::SurfaceConfiguration,
    surface: &wgpu::Surface,
    adapter: &wgpu::Adapter,
    wayland: bool,
) {
    let caps = surface.get_capabilities(adapter);
    let offered: Vec<PresentChoice> = caps
        .present_modes
        .iter()
        .copied()
        .filter_map(PresentChoice::from_wgpu)
        .collect();
    let (choice, warning) = match env_override() {
        EnvOverride::Unset => choose_present(wayland, &offered, None),
        EnvOverride::Utf8(value) => choose_present(wayland, &offered, Some(value.as_str())),
        EnvOverride::NonUtf8 => {
            let (default, _) = choose_present(wayland, &offered, None);
            (
                default,
                Some(format!(
                    "rune: ignoring non-UTF-8 RUNE_PRESENT_MODE (expected {EXPECTED_MODES}); using {}",
                    default.label()
                )),
            )
        }
    };
    if let Some(warning) = warning {
        static WARN_ONCE: Once = Once::new();
        WARN_ONCE.call_once(|| eprintln!("{warning}"));
    }
    config.present_mode = choice.to_wgpu();
    if std::env::var_os("RUNE_GPU_LOG").is_some_and(|v| v != "0") {
        static LOG_ONCE: Once = Once::new();
        LOG_ONCE.call_once(|| {
            eprintln!(
                "rune: present mode {:?} (wayland={wayland}, offered={:?})",
                config.present_mode, caps.present_modes
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wgpu_roundtrip_covers_concrete_modes() {
        let pairs = [
            (wgpu::PresentMode::Fifo, PresentChoice::Fifo),
            (wgpu::PresentMode::FifoRelaxed, PresentChoice::FifoRelaxed),
            (wgpu::PresentMode::Mailbox, PresentChoice::Mailbox),
            (wgpu::PresentMode::Immediate, PresentChoice::Immediate),
        ];
        for (mode, choice) in pairs {
            assert_eq!(PresentChoice::from_wgpu(mode), Some(choice));
            assert_eq!(choice.to_wgpu(), mode);
        }
        assert_eq!(PresentChoice::from_wgpu(wgpu::PresentMode::AutoVsync), None);
        assert_eq!(
            PresentChoice::from_wgpu(wgpu::PresentMode::AutoNoVsync),
            None
        );
        assert_eq!(PresentChoice::Vsync.to_wgpu(), wgpu::PresentMode::AutoVsync);
    }

    #[test]
    fn wayland_prefers_mailbox_when_offered_and_falls_back_to_vsync() {
        assert_eq!(
            choose_present(true, &[PresentChoice::Fifo, PresentChoice::Mailbox], None),
            (PresentChoice::Mailbox, None)
        );
        assert_eq!(
            choose_present(true, &[PresentChoice::Fifo, PresentChoice::Immediate], None),
            (PresentChoice::Vsync, None)
        );
        assert_eq!(
            choose_present(false, &[PresentChoice::Fifo, PresentChoice::Mailbox], None),
            (PresentChoice::Vsync, None)
        );
    }

    #[test]
    fn invalid_and_unsupported_overrides_warn_and_fall_back() {
        let (choice, warn) = choose_present(true, &[PresentChoice::Fifo], Some("mailbox"));
        assert_eq!(choice, PresentChoice::Vsync);
        let warn = warn.expect("unsupported override should warn");
        assert!(warn.contains("mailbox") && warn.contains("fifo") && warn.contains("auto-vsync"));

        let long = "é".repeat(120);
        let (choice, warn) = choose_present(
            true,
            &[PresentChoice::Fifo, PresentChoice::Mailbox],
            Some(&long),
        );
        assert_eq!(choice, PresentChoice::Mailbox);
        let warn = warn.expect("invalid override should warn");
        assert!(warn.contains("...") && warn.contains(EXPECTED_MODES));
        assert!(warn.len() < 300);
    }
}
