//! Optional top-level window override for Rune's automatic presentation mode.

#[cfg(not(target_arch = "wasm32"))]
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
    let modes: Vec<_> = offered.iter().map(|choice| choice.to_wgpu()).collect();
    let default = super::surface::present_mode(&modes, prefer_mailbox);
    choose_override(
        PresentChoice::from_wgpu(default).unwrap_or(PresentChoice::Vsync),
        offered,
        asked,
    )
}

fn choose_override(
    default: PresentChoice,
    offered: &[PresentChoice],
    asked: Option<&str>,
) -> (PresentChoice, Option<String>) {
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

/// Uses the capabilities already queried by `surface_config_with`.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn override_mode(
    default: wgpu::PresentMode,
    modes: &[wgpu::PresentMode],
    wayland: bool,
) -> wgpu::PresentMode {
    use std::env::VarError;
    static OVERRIDE: OnceLock<Result<String, VarError>> = OnceLock::new();
    let fallback = PresentChoice::from_wgpu(default).unwrap_or(PresentChoice::Vsync);
    let (selected, warning) = match OVERRIDE.get_or_init(|| std::env::var("RUNE_PRESENT_MODE")) {
        Err(VarError::NotPresent) => (default, None),
        Ok(value) => {
            let offered: Vec<_> = modes
                .iter()
                .copied()
                .filter_map(PresentChoice::from_wgpu)
                .collect();
            let (choice, warning) = choose_override(fallback, &offered, Some(value));
            (choice.to_wgpu(), warning)
        }
        Err(VarError::NotUnicode(_)) => (
            default,
            Some(format!(
                "rune: ignoring non-UTF-8 RUNE_PRESENT_MODE (expected {EXPECTED_MODES}); using {}",
                fallback.label()
            )),
        ),
    };
    if let Some(warning) = warning {
        static WARN_ONCE: Once = Once::new();
        WARN_ONCE.call_once(|| eprintln!("{warning}"));
    }
    if std::env::var_os("RUNE_GPU_LOG").is_some_and(|v| v != "0") {
        static LOG_ONCE: Once = Once::new();
        LOG_ONCE.call_once(|| {
            eprintln!("rune: present mode {selected:?} (wayland={wayland}, offered={modes:?})")
        });
    }
    selected
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
