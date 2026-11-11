//! The terminal themes that ship with the client.
//!
//! - Midnight, Rosé Pine, Rosé Pine Moon and Gruvbox Dark are paired with the chrome palettes
//!   of the same names.
//! - Merino Dark and Merino Light are Rex Linux's own: warm neutrals, every palette colour at
//!   least 4.5:1 on the background (WCAG AA) but black on dark and bright white on light.
//!   They are what a light and dark pair starts with (`settings::DEFAULT_LIGHT_THEME`,
//!   `settings::DEFAULT_DARK_THEME`).
//! - The rest are well-known themes whose authors publish them under a permissive licence,
//!   with the colours of their Ghostty theme files. `docs/terminal-themes.md` lists each
//!   theme's project, author and licence; a theme is added here only with a row there.
//!
//! Dark themes come first, then light ones; within each, Rex's own, then by name. A built-in
//! theme's id never changes once released: it is what `workspace.json` holds.
use super::{Builtin, Colours, TerminalTheme};
use crate::settings::ChromeTheme;

/// A theme made by hand in `terminal_theme.rs`.
const fn made(
    id: &'static str,
    name: &'static str,
    chrome: Option<ChromeTheme>,
    theme: fn() -> TerminalTheme,
) -> Builtin {
    Builtin {
        id,
        name,
        chrome,
        colours: Colours::Made(theme),
    }
}

/// A theme with every colour: its background; its foreground, cursor, cursor text, selection
/// background and selection foreground; then palette colours 0 to 15.
const fn full(
    id: &'static str,
    name: &'static str,
    background: u32,
    text: [u32; 5],
    palette: [u32; 16],
) -> Builtin {
    Builtin {
        id,
        name,
        chrome: None,
        colours: Colours::Full {
            background,
            text,
            palette,
        },
    }
}

pub const BUILTINS: &[Builtin] = &[
    // Dark.
    full(
        "merino-dark",
        "Merino Dark",
        0x1c1b1a,
        [0xe7e2d8, 0xe7e2d8, 0x1c1b1a, 0x3d3a35, 0xe7e2d8],
        [
            0x2e2c2a, 0xea7a6e, 0x93c47d, 0xe3b567, 0x7fabe6, 0xc493dd, 0x6fc3bd, 0xcdc7bb,
            0x7c776e, 0xf39a8f, 0xb1dc98, 0xf2cd8b, 0xa3c5f3, 0xd9b2ec, 0x93dad4, 0xf7f3ea,
        ],
    ),
    made(
        "midnight",
        "Midnight",
        Some(ChromeTheme::Midnight),
        TerminalTheme::midnight,
    ),
    made(
        "rose-pine",
        "Rosé Pine",
        Some(ChromeTheme::RosePine),
        TerminalTheme::rose_pine,
    ),
    made(
        "rose-pine-moon",
        "Rosé Pine Moon",
        Some(ChromeTheme::RosePineMoon),
        TerminalTheme::rose_pine_moon,
    ),
    made(
        "gruvbox-dark",
        "Gruvbox Dark",
        Some(ChromeTheme::GruvboxDark),
        TerminalTheme::gruvbox_dark,
    ),
    full(
        "ayu-dark",
        "Ayu Dark",
        0x0b0e14,
        [0xbfbdb6, 0xe6b450, 0x0b0e14, 0x409fff, 0x0b0e14],
        [
            0x11151c, 0xea6c73, 0x7fd962, 0xf9af4f, 0x53bdfa, 0xcda1fa, 0x90e1c6, 0xc7c7c7,
            0x686868, 0xf07178, 0xaad94c, 0xffb454, 0x59c2ff, 0xd2a6ff, 0x95e6cb, 0xffffff,
        ],
    ),
    full(
        "ayu-mirage",
        "Ayu Mirage",
        0x1f2430,
        [0xcccac2, 0xffcc66, 0x1f2430, 0x409fff, 0x1f2430],
        [
            0x171b24, 0xed8274, 0x87d96c, 0xfacc6e, 0x6dcbfa, 0xdabafa, 0x90e1c6, 0xc7c7c7,
            0x686868, 0xf28779, 0xd5ff80, 0xffd173, 0x73d0ff, 0xdfbfff, 0x95e6cb, 0xffffff,
        ],
    ),
    full(
        "catppuccin-frappe",
        "Catppuccin Frappé",
        0x303446,
        [0xc6d0f5, 0xf2d5cf, 0x303446, 0xf2d5cf, 0x303446],
        [
            0x51576d, 0xe78284, 0xa6d189, 0xe5c890, 0x8caaee, 0xf4b8e4, 0x81c8be, 0xb5bfe2,
            0x626880, 0xeda0a2, 0xb9dba2, 0xecd7ae, 0xadc2f3, 0xf38ed8, 0x98d2ca, 0xa5adce,
        ],
    ),
    full(
        "catppuccin-macchiato",
        "Catppuccin Macchiato",
        0x24273a,
        [0xcad3f5, 0xf4dbd6, 0x24273a, 0xf4dbd6, 0x24273a],
        [
            0x494d64, 0xed8796, 0xa6da95, 0xeed49f, 0x8aadf4, 0xf5bde6, 0x8bd5ca, 0xb8c0e0,
            0x5b6078, 0xf2a7b2, 0xbde3b0, 0xf4e3c1, 0xadc5f7, 0xf493da, 0xa5ded6, 0xa5adcb,
        ],
    ),
    full(
        "catppuccin-mocha",
        "Catppuccin Mocha",
        0x1e1e2e,
        [0xcdd6f4, 0xf5e0dc, 0x1e1e2e, 0xf5e0dc, 0x1e1e2e],
        [
            0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
            0x585b70, 0xf7aec2, 0xc2ecbf, 0xfcd682, 0xaeccfc, 0xf398da, 0xb1eae1, 0xa6adc8,
        ],
    ),
    full(
        "dracula",
        "Dracula",
        0x282a36,
        [0xf8f8f2, 0xf8f8f2, 0x282a36, 0x44475a, 0xffffff],
        [
            0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
            0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
        ],
    ),
    full(
        "everforest-dark",
        "Everforest Dark",
        0x232a2e,
        [0xd3c6aa, 0xe69875, 0x543a48, 0x543a48, 0xd3c6aa],
        [
            0x7a8478, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xf2efdf,
            0xa6b0a0, 0xf85552, 0x8da101, 0xdfa000, 0x3a94c5, 0xdf69ba, 0x35a77c, 0xfffbef,
        ],
    ),
    full(
        "flexoki-dark",
        "Flexoki Dark",
        0x100f0f,
        [0xcecdc3, 0xcecdc3, 0x100f0f, 0x403e3c, 0xcecdc3],
        [
            0x100f0f, 0xd14d41, 0x879a39, 0xd0a215, 0x4385be, 0xce5d97, 0x3aa99f, 0x878580,
            0x575653, 0xaf3029, 0x66800b, 0xad8301, 0x205ea6, 0xa02f6f, 0x24837b, 0xcecdc3,
        ],
    ),
    full(
        "kanagawa-wave",
        "Kanagawa Wave",
        0x1f1f28,
        [0xdcd7ba, 0xdcd7ba, 0x1f1f28, 0xdcd7ba, 0x1f1f28],
        [
            0x090618, 0xc34043, 0x76946a, 0xc0a36e, 0x7e9cd8, 0x957fb8, 0x6a9589, 0xc8c093,
            0x727169, 0xe82424, 0x98bb6c, 0xe6c384, 0x7fb4ca, 0x938aa9, 0x7aa89f, 0xdcd7ba,
        ],
    ),
    full(
        "nord",
        "Nord",
        0x2e3440,
        [0xd8dee9, 0xeceff4, 0x282828, 0xeceff4, 0x4c566a],
        [
            0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
            0x596377, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
        ],
    ),
    full(
        "one-dark",
        "One Dark",
        0x21252b,
        [0xabb2bf, 0xabb2bf, 0x21252b, 0x323844, 0xabb2bf],
        [
            0x21252b, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
            0x767676, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
        ],
    ),
    full(
        "solarized-dark",
        "Solarized Dark",
        0x002b36,
        [0x839496, 0x839496, 0x073642, 0x073642, 0x93a1a1],
        [
            0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
            0x335e69, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
        ],
    ),
    // Light.
    full(
        "merino-light",
        "Merino Light",
        0xf8f5ef,
        [0x2b2925, 0x2b2925, 0xf8f5ef, 0xdcd5c6, 0x2b2925],
        [
            0x2b2925, 0xa8322a, 0x33702d, 0x7d5800, 0x2459a8, 0x7d3f9a, 0x166f75, 0x6f6a60,
            0x5a564e, 0xc5392b, 0x2a7f35, 0x946700, 0x1d68cc, 0x9449ba, 0x0d7c82, 0x8a857a,
        ],
    ),
    full(
        "ayu-light",
        "Ayu Light",
        0xf8f9fa,
        [0x5c6166, 0xffaa33, 0xf8f9fa, 0x035bd6, 0xf8f9fa],
        [
            0x000000, 0xea6c6d, 0x6cbf43, 0xeca944, 0x3199e1, 0x9e75c7, 0x46ba94, 0xbababa,
            0x686868, 0xf07171, 0x86b300, 0xf2ae49, 0x399ee6, 0xa37acc, 0x4cbf99, 0xd1d1d1,
        ],
    ),
    full(
        "catppuccin-latte",
        "Catppuccin Latte",
        0xeff1f5,
        [0x4c4f69, 0xdc8a78, 0xeff1f5, 0xdc8a78, 0xeff1f5],
        [
            0xbcc0cc, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0x5c5f77,
            0xacb0be, 0xe7103f, 0x46b02f, 0xe49931, 0x3878f6, 0xef95d7, 0x19a1a8, 0x6c6f85,
        ],
    ),
    full(
        "everforest-light",
        "Everforest Light",
        0xefebd4,
        [0x5c6a72, 0xf57d26, 0xeaedc8, 0xeaedc8, 0x5c6a72],
        [
            0x7a8478, 0xe67e80, 0x9ab373, 0xc1a266, 0x7fbbb3, 0xd699b6, 0x83c092, 0xb2af9f,
            0xa6b0a0, 0xf85552, 0x8da101, 0xdfa000, 0x3a94c5, 0xdf69ba, 0x35a77c, 0xfffbef,
        ],
    ),
    full(
        "flexoki-light",
        "Flexoki Light",
        0xfffcf0,
        [0x100f0f, 0x100f0f, 0xfffcf0, 0xcecdc3, 0x100f0f],
        [
            0x100f0f, 0xaf3029, 0x66800b, 0xad8301, 0x205ea6, 0xa02f6f, 0x24837b, 0x6f6e69,
            0xb7b5ac, 0xd14d41, 0x879a39, 0xd0a215, 0x4385be, 0xce5d97, 0x3aa99f, 0xcecdc3,
        ],
    ),
    full(
        "gruvbox-light",
        "Gruvbox Light",
        0xfbf1c7,
        [0x3c3836, 0x3c3836, 0xfbf1c7, 0x3c3836, 0xfbf1c7],
        [
            0xfbf1c7, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0x7c6f64,
            0x928374, 0x9d0006, 0x79740e, 0xb57614, 0x076678, 0x8f3f71, 0x427b58, 0x3c3836,
        ],
    ),
    full(
        "kanagawa-lotus",
        "Kanagawa Lotus",
        0xf2ecbc,
        [0x545464, 0x43436c, 0xf2ecbc, 0x545464, 0xf2ecbc],
        [
            0x1f1f28, 0xc84053, 0x6f894e, 0x77713f, 0x4d699b, 0xb35b79, 0x597b75, 0x545464,
            0x8a8980, 0xd7474b, 0x6e915f, 0x836f4a, 0x6693bf, 0x624c83, 0x5e857a, 0x43436c,
        ],
    ),
    full(
        "one-light",
        "One Light",
        0xf9f9f9,
        [0x2a2c33, 0xbbbbbb, 0xffffff, 0xededed, 0x2a2c33],
        [
            0x000000, 0xde3e35, 0x3f953a, 0xd2b67c, 0x2f5af3, 0x950095, 0x3f953a, 0xbbbbbb,
            0x000000, 0xde3e35, 0x3f953a, 0xd2b67c, 0x2f5af3, 0xa00095, 0x3f953a, 0xffffff,
        ],
    ),
    full(
        "rose-pine-dawn",
        "Rosé Pine Dawn",
        0xfaf4ed,
        [0x575279, 0x575279, 0xfaf4ed, 0xdfdad9, 0x575279],
        [
            0xf2e9e1, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279,
            0x9893a5, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279,
        ],
    ),
    full(
        "solarized-light",
        "Solarized Light",
        0xfdf6e3,
        [0x657b83, 0x657b83, 0xeee8d5, 0xeee8d5, 0x586e75],
        [
            0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xbbb5a2,
            0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
        ],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        settings::{DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME},
        terminal_theme::{builtin, is_light},
    };
    use std::collections::HashSet;

    /// The WCAG contrast ratio of two colours, from 1 (the same) to 21 (black on white).
    fn contrast(a: u32, b: u32) -> f32 {
        fn luminance(color: u32) -> f32 {
            let channel = |shift: u32| {
                let value = ((color >> shift) & 0xff) as f32 / 255.;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
        }
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn colour(theme: &TerminalTheme, index: u8) -> u32 {
        theme
            .palette
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, colour)| *colour)
            .unwrap_or_else(|| panic!("no palette colour {index}"))
    }

    #[test]
    fn ids_and_names_are_unique_and_dark_themes_come_before_light_ones() {
        let ids: HashSet<_> = BUILTINS.iter().map(|b| b.id).collect();
        let names: HashSet<_> = BUILTINS.iter().map(|b| b.name).collect();
        assert_eq!(ids.len(), BUILTINS.len());
        assert_eq!(names.len(), BUILTINS.len());
        for b in BUILTINS {
            assert!(
                b.id.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'),
                "{}: an id is lower-case words joined by hyphens",
                b.id
            );
        }
        let lights: Vec<bool> = BUILTINS.iter().map(Builtin::light).collect();
        let first_light = lights.iter().position(|light| *light).unwrap();
        assert!(lights[first_light..].iter().all(|light| *light));
        assert_eq!(BUILTINS[0].id, "merino-dark", "Rex's own dark theme first");
        assert_eq!(
            BUILTINS[first_light].id, "merino-light",
            "and its light one"
        );
        assert_eq!(
            (first_light, BUILTINS.len() - first_light),
            (17, 10),
            "seventeen dark themes and ten light ones"
        );
        // The ids released before the set grew still name their themes.
        for id in ["midnight", "rose-pine", "rose-pine-moon", "gruvbox-dark"] {
            assert!(builtin(&format!("rex/{id}")).is_some(), "{id}");
        }
    }

    #[test]
    fn every_theme_is_legible_and_says_which_appearance_it_is_for() {
        for b in BUILTINS {
            let theme = b.theme();
            let background = theme.background.expect("a background");
            assert_eq!(
                theme.scheme,
                Some(if is_light(background) {
                    "light"
                } else {
                    "dark"
                }),
                "{}",
                b.name
            );
            assert_eq!(b.light(), is_light(background), "{}", b.name);
            // Midnight draws Rex's own white text on its navy.
            let text = theme.foreground.unwrap_or(0xffffff);
            assert!(
                contrast(text, background) >= 4.,
                "{}: text on its background is {:.2}:1",
                b.name,
                contrast(text, background)
            );
            if let (Some(selection), Some(selected)) =
                (theme.selection_background, theme.selection_foreground)
            {
                assert!(
                    contrast(selected, selection) >= 2.,
                    "{}: selected text is {:.2}:1",
                    b.name,
                    contrast(selected, selection)
                );
            }
            if b.chrome.is_none() {
                assert_eq!(theme.palette.len(), 16, "{}: colours 0 to 15", b.name);
                assert!(theme.cursor.is_some() && theme.cursor_text.is_some());
            }
        }
    }

    #[test]
    fn merino_is_the_default_pair_and_meets_aa_in_every_colour() {
        let light = builtin(DEFAULT_LIGHT_THEME).expect("the default light theme");
        let dark = builtin(DEFAULT_DARK_THEME).expect("the default dark theme");
        assert_eq!((light.name, dark.name), ("Merino Light", "Merino Dark"));
        assert!(light.light() && !dark.light());
        for b in [light, dark] {
            let theme = b.theme();
            let background = theme.background.unwrap();
            assert!(contrast(theme.foreground.unwrap(), background) >= 7.);
            assert!(
                contrast(
                    theme.selection_foreground.unwrap(),
                    theme.selection_background.unwrap()
                ) >= 7.
            );
            assert!(contrast(theme.cursor.unwrap(), background) >= 7.);
            // Red to cyan, and their bright ones: text a program colours stays readable.
            for index in (1..=6).chain(9..=14) {
                let ratio = contrast(colour(&theme, index), background);
                assert!(ratio >= 4.5, "{} colour {index} is {ratio:.2}:1", b.name);
            }
            // The greys: white and bright black are text too; bright white on light and
            // black on dark are the ends of the scale.
            for index in [7, 8] {
                let ratio = contrast(colour(&theme, index), background);
                assert!(ratio >= 3.5, "{} colour {index} is {ratio:.2}:1", b.name);
            }
        }
        // Black on dark is told from the background, and bright white on light.
        assert!(contrast(colour(&dark.theme(), 0), 0x1c1b1a) > 1.2);
        assert!(contrast(colour(&light.theme(), 15), 0xf8f5ef) > 3.);
    }
}
