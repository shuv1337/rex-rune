//! Built-in chrome and terminal palettes share one catalogue. Saved chrome IDs and
//! terminal IDs are separate: the original Rosé Pine and Gruvbox IDs differ.
use crate::terminal_theme::{TerminalTheme, ThemeDirs, valid_name};
use serde::{Deserialize, Deserializer, Serialize};
use std::{borrow::Cow, path::Path, sync::LazyLock};

mod catalog;
mod custom;
pub mod derive;
#[cfg(test)]
mod tests;
pub use catalog::BUILTINS;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ChromeTheme(Cow<'static, str>);

#[allow(non_upper_case_globals)]
impl ChromeTheme {
    const fn builtin(id: &'static str) -> Self {
        Self(Cow::Borrowed(id))
    }
    pub const Midnight: Self = Self::builtin("midnight");
    pub const RosePine: Self = Self::builtin("rose_pine");
    pub const RosePineMoon: Self = Self::builtin("rose_pine_moon");
    pub const GruvboxDark: Self = Self::builtin("gruvbox_dark");
    pub fn all() -> impl Iterator<Item = Self> {
        BUILTINS.iter().map(|b| b.chrome.clone())
    }
    pub fn id(&self) -> &str {
        &self.0
    }
    pub fn from_id(id: &str) -> Self {
        if ["custom:", "ghostty:"]
            .iter()
            .any(|prefix| id.strip_prefix(prefix).is_some_and(valid_name))
        {
            return Self(Cow::Owned(id.to_owned()));
        }
        BUILTINS
            .iter()
            .find(|b| b.chrome.id() == id)
            .map(|b| b.chrome.clone())
            .unwrap_or_default()
    }
    pub fn builtin_preset(&self) -> Option<&'static Builtin> {
        BUILTINS.iter().find(|b| b.chrome == *self)
    }
    pub fn preset(&self) -> &'static Builtin {
        self.builtin_preset().unwrap_or_else(|| {
            Self::Midnight
                .builtin_preset()
                .expect("Midnight is built in")
        })
    }
    pub fn light_default() -> Self {
        Self::builtin("merino-light")
    }
    pub fn is_default(&self) -> bool {
        *self == Self::Midnight
    }
    pub fn is_light_default(&self) -> bool {
        *self == Self::light_default()
    }
    pub fn name(&self) -> &str {
        self.builtin_preset().map_or_else(
            || {
                self.id()
                    .split_once(':')
                    .map_or(self.id(), |(_, name)| name)
            },
            |b| b.name,
        )
    }
}
impl Default for ChromeTheme {
    fn default() -> Self {
        Self::Midnight
    }
}
impl<'de> Deserialize<'de> for ChromeTheme {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|id| Self::from_id(&id))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaletteSpec {
    pub dark: bool,
    pub ground: u32,
    pub card: u32,
    pub sidebar: u32,
    pub popover: u32,
    pub ink: u32,
    pub muted: u32,
    pub accent: u32,
    pub accent2: u32,
    pub live: u32,
    pub warning: u32,
    pub danger: u32,
    pub on_accent: u32,
    pub terminal_selection: u32,
    pub border_subtle: Option<u32>,
    pub border_strong: Option<u32>,
    pub surface_raised: Option<u32>,
}

impl PaletteSpec {
    pub fn scheme(&self) -> &'static str {
        if self.dark { "dark" } else { "light" }
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedTheme {
    pub palette: PaletteSpec,
    pub terminal: TerminalTheme,
    pub name: String,
}

static RESOLVED_BUILTINS: LazyLock<Vec<ResolvedTheme>> = LazyLock::new(|| {
    BUILTINS
        .iter()
        .map(|b| ResolvedTheme {
            palette: b.palette,
            terminal: b.theme(),
            name: b.name.to_owned(),
        })
        .collect()
});

/// Snapshot of imported themes. At startup only configured choices are read; opening the
/// picker replaces the snapshot. Switching a system slot never requires file I/O.
#[derive(Default)]
pub struct ThemeLibrary(Vec<(ChromeTheme, ResolvedTheme)>);

impl ThemeLibrary {
    pub fn selected(dir: &Path, ghostty: &ThemeDirs, ids: &[&ChromeTheme]) -> Self {
        let mut themes = Self::default();
        for &id in ids {
            if themes.0.iter().any(|(existing, _)| existing == id) {
                continue;
            }
            let imported = if let Some(stem) = id.id().strip_prefix("custom:") {
                custom::scan(dir, Some(stem)).next().map(|(_, theme)| theme)
            } else if let Some(name) = id.id().strip_prefix("ghostty:") {
                ghostty
                    .load(name)
                    .ok()
                    .map(|term| ghostty_theme(name, term))
            } else {
                None
            };
            if let Some(theme) = imported {
                themes.0.push((id.clone(), theme));
            }
        }
        themes
    }

    pub fn scan(dir: &Path, ghostty: &ThemeDirs) -> Self {
        let mut custom: Vec<_> = custom::scan(dir, None).collect();
        custom.sort_by_cached_key(|(id, theme)| (theme.name.to_lowercase(), id.id().to_owned()));
        custom.extend(ghostty.read_all().into_iter().map(|(info, term)| {
            (
                ChromeTheme::from_id(&format!("ghostty:{}", info.name)),
                ghostty_theme(&info.name, term),
            )
        }));
        Self(custom)
    }

    /// Missing imports use Midnight without writing over the saved choice.
    pub fn resolve(&self, id: &ChromeTheme) -> (&ChromeTheme, &ResolvedTheme) {
        if let Some((index, builtin)) = BUILTINS.iter().enumerate().find(|(_, b)| &b.chrome == id) {
            return (&builtin.chrome, &RESOLVED_BUILTINS[index]);
        }
        self.0
            .iter()
            .find(|(theme_id, _)| theme_id == id)
            .map_or_else(
                || self.resolve(&ChromeTheme::Midnight),
                |(id, theme)| (id, theme),
            )
    }

    pub fn entries(&self) -> impl Iterator<Item = (&'static str, &ChromeTheme, &ResolvedTheme)> {
        self.0
            .iter()
            .filter(|(id, _)| id.id().starts_with("custom:"))
            .map(|(id, theme)| ("Custom", id, theme))
            .chain(
                BUILTINS
                    .iter()
                    .zip(RESOLVED_BUILTINS.iter())
                    .map(|(b, theme)| {
                        (
                            if b.palette.dark { "Dark" } else { "Light" },
                            &b.chrome,
                            theme,
                        )
                    }),
            )
            .chain(
                self.0
                    .iter()
                    .filter(|(id, _)| id.id().starts_with("ghostty:"))
                    .map(|(id, theme)| ("Ghostty", id, theme)),
            )
    }
}

fn ghostty_theme(name: &str, terminal: TerminalTheme) -> ResolvedTheme {
    ResolvedTheme {
        palette: derive::derive_from_terminal(&terminal, &Default::default()),
        terminal,
        name: name.to_owned(),
    }
}
pub struct Builtin {
    pub chrome: ChromeTheme,
    pub id: &'static str,
    pub name: &'static str,
    pub palette: PaletteSpec,
    // Trailing roles left out of a preset retain Rex's defaults.
    terminal: &'static [u32],
    ansi: &'static [u32],
}
impl Builtin {
    pub fn scheme(&self) -> &'static str {
        self.palette.scheme()
    }
    pub fn theme(&self) -> TerminalTheme {
        TerminalTheme {
            background: Some(self.palette.ground),
            foreground: self.terminal.first().copied(),
            cursor: self.terminal.get(1).copied(),
            cursor_text: self.terminal.get(2).copied(),
            selection_background: self.terminal.get(3).copied(),
            selection_foreground: self.terminal.get(4).copied(),
            palette: (0..).zip(self.ansi.iter().copied()).collect(),
            scheme: Some(self.scheme()),
        }
    }
}
