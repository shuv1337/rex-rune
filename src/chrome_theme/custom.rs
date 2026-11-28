//! User-owned theme files. The two small text readers accept only the documented theme
//! subset; neither TOML nor plist is used as a general-purpose configuration language.
use super::{ChromeTheme, ResolvedTheme, derive::ChromeOverrides, derive::derive_from_terminal};
use crate::terminal_theme::{TerminalTheme, parse_hex, read_theme_file, valid_name};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::{collections::BTreeMap, path::Path};

mod plist;
mod toml;

pub(super) fn scan(
    dir: &Path,
    only: Option<&str>,
) -> impl Iterator<Item = (ChromeTheme, ResolvedTheme)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let file = entry.file_name();
            let path = Path::new(&file);
            let name = file.to_str()?;
            let stem = path.file_stem()?.to_str()?;
            (valid_name(name) && valid_name(stem) && only.is_none_or(|id| id == stem))
                .then(|| (file.clone(), stem.to_owned(), entry.path()))
        })
        .collect();
    files.sort_unstable();
    let mut themes = BTreeMap::new();
    for (file, stem, path) in files {
        if themes.contains_key(&stem) {
            continue;
        }
        let ext = Path::new(&file).extension().and_then(|ext| ext.to_str());
        if let Some(theme) = read_theme_file(&path).and_then(|text| parse(&stem, ext, &text)) {
            themes.insert(stem, theme);
        }
    }
    themes
        .into_iter()
        .map(|(stem, theme)| (ChromeTheme::from_id(&format!("custom:{stem}")), theme))
}

fn parse(stem: &str, ext: Option<&str>, text: &str) -> Option<ResolvedTheme> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (name, chrome, mut terminal) = match ext.map(str::to_ascii_lowercase).as_deref() {
        Some("json") => serde_json::from_str::<ThemeFile>(text).ok()?.parts(),
        Some("toml") => serde_json::from_value::<ThemeFile>(toml::parse(text)?)
            .ok()?
            .parts(),
        Some("itermcolors") => (None, Default::default(), plist::parse(text)?),
        _ => (
            None,
            Default::default(),
            TerminalTheme::parse(stem, text).ok()?,
        ),
    };
    if chrome == ChromeOverrides::default() && terminal == TerminalTheme::default() {
        return None;
    }
    let palette = derive_from_terminal(&terminal, &chrome);
    terminal.background.get_or_insert(palette.ground);
    terminal.foreground.get_or_insert(palette.ink);
    terminal.scheme = Some(palette.scheme());
    let name = name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| crate::connection::perform::sanitize_text(name, 120))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| stem.to_owned());
    Some(ResolvedTheme {
        name,
        palette,
        terminal,
    })
}

#[derive(Clone, Copy)]
struct Hex(u32);

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        parse_hex(&String::deserialize(d)?)
            .map(Self)
            .ok_or_else(|| serde::de::Error::custom("expected a hex RGB colour"))
    }
}

macro_rules! theme_file {
    ($($role:ident),+ $(,)?) => {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct ThemeFile {
            name: Option<String>,
            appearance: Option<super::derive::Appearance>,
            terminal: TerminalFile,
            $($role: Option<Hex>,)+
        }

        impl ThemeFile {
            fn parts(self) -> (Option<String>, ChromeOverrides, TerminalTheme) {
                (self.name, ChromeOverrides {
                    appearance: self.appearance,
                    $($role: self.$role.map(|h| h.0),)+
                }, self.terminal.theme())
            }
        }
    };
}

theme_file!(
    ground,
    card,
    sidebar,
    popover,
    ink,
    muted,
    accent,
    accent2,
    live,
    warning,
    danger,
    on_accent,
    terminal_selection,
    border_subtle,
    border_strong,
    surface_raised,
);

#[derive(Default, Deserialize)]
#[serde(default)]
struct TerminalFile {
    background: Option<Hex>,
    foreground: Option<Hex>,
    cursor: Option<Hex>,
    cursor_text: Option<Hex>,
    selection_background: Option<Hex>,
    selection_foreground: Option<Hex>,
    ansi: Vec<Hex>,
    palette: BTreeMap<u8, Hex>,
}

impl TerminalFile {
    fn theme(self) -> TerminalTheme {
        let mut palette: BTreeMap<_, _> =
            (0..=255).zip(self.ansi.into_iter().map(|h| h.0)).collect();
        palette.extend(self.palette.into_iter().map(|(i, h)| (i, h.0)));
        TerminalTheme {
            background: self.background.map(|h| h.0),
            foreground: self.foreground.map(|h| h.0),
            cursor: self.cursor.map(|h| h.0),
            cursor_text: self.cursor_text.map(|h| h.0),
            selection_background: self.selection_background.map(|h| h.0),
            selection_foreground: self.selection_foreground.map(|h| h.0),
            palette: palette.into_iter().collect(),
            scheme: None,
        }
    }
}
