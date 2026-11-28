//! Derive chrome in OKLab, without changing the terminal's original colours.
use super::PaletteSpec;
use crate::{
    color::{blend, contrast, is_light, luminance, oklab, on, readable, soften},
    terminal_theme::TerminalTheme,
};

#[derive(Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    Dark,
    Light,
}

#[derive(Default, PartialEq, Eq)]
pub struct ChromeOverrides {
    pub appearance: Option<Appearance>,
    pub ground: Option<u32>,
    pub card: Option<u32>,
    pub sidebar: Option<u32>,
    pub popover: Option<u32>,
    pub ink: Option<u32>,
    pub muted: Option<u32>,
    pub accent: Option<u32>,
    pub accent2: Option<u32>,
    pub live: Option<u32>,
    pub warning: Option<u32>,
    pub danger: Option<u32>,
    pub on_accent: Option<u32>,
    pub terminal_selection: Option<u32>,
    pub border_subtle: Option<u32>,
    pub border_strong: Option<u32>,
    pub surface_raised: Option<u32>,
}

pub fn derive_from_seed(seed: &ChromeOverrides) -> PaletteSpec {
    derive_from_terminal(&TerminalTheme::default(), seed)
}

pub fn derive_from_terminal(term: &TerminalTheme, seed: &ChromeOverrides) -> PaletteSpec {
    let ground = seed.ground.or(term.background);
    let dark = match seed.appearance {
        Some(Appearance::Dark) => true,
        Some(Appearance::Light) => false,
        None => ground.map_or(term.scheme != Some("light"), |bg| !is_light(bg)),
    };
    let default = |dark_color, light_color| if dark { dark_color } else { light_color };
    let ground = ground.unwrap_or(default(0x0c1119, 0xf6f7f9));

    // Try AAA for primary ink. A mid-tone ground may allow only AA, even with black or white.
    let extreme = on(ground);
    let ink = readable(
        seed.ink
            .or(term.foreground)
            .unwrap_or(default(0xe6ebf2, 0x1f2328)),
        ground,
        extreme,
        7.0_f32.min(contrast(extreme, ground)),
    );
    // Back off a surface that crowds the ink or makes shared contrasting accents impossible.
    // This also handles explicit overrides on the opposite side of the luminance scale.
    let text_floor = 5.5_f32.min(contrast(ink, ground));
    let surface = |color| {
        (0..=32)
            .map(|i| blend(color, ground, i as f32 / 32.))
            .find(|&bg| {
                contrast(ink, bg) >= text_floor
                    && contrast(extreme, bg) >= 3.0
                    && (luminance(bg) > luminance(ink)) == (luminance(ground) > luminance(ink))
            })
            .unwrap_or(ground)
    };
    let card = surface(seed.card.unwrap_or(ground));
    let sidebar = surface(seed.sidebar.unwrap_or_else(|| blend(ground, ink, 0.045)));
    let popover = surface(seed.popover.unwrap_or_else(|| {
        if dark {
            blend(ground, ink, 0.08)
        } else {
            blend(ground, 0xffffff, 0.45)
        }
    }));
    let surface_raised = surface(
        seed.surface_raised
            .unwrap_or_else(|| blend(sidebar, ink, 0.07)),
    );
    let surfaces = [ground, card, sidebar, popover, surface_raised];
    let muted = seed.muted.map_or_else(
        || soften(ink, &surfaces, 0.40, 4.6),
        |muted| fit(muted, ink, &surfaces, 4.6_f32.min(text_floor)),
    );
    let ansi = |index| {
        term.palette
            .iter()
            .find(|&&(i, _)| i == index)
            .map(|&(_, color)| color)
    };
    let tone = |color| fit(color, extreme, &surfaces, 3.0);
    let accent = tone(
        seed.accent
            .or(term
                .cursor
                .filter(|&cursor| useful_cursor(cursor, ground, ink)))
            .or_else(|| ansi(4))
            .unwrap_or(default(0x7cc4ff, 0x285ec8)),
    );
    let accent2 = tone(
        seed.accent2
            .or_else(|| ansi(5).or_else(|| ansi(6)).or_else(|| ansi(4)))
            .unwrap_or(default(0xa78bfa, 0x6842ad)),
    );
    PaletteSpec {
        dark,
        ground,
        card,
        sidebar,
        popover,
        ink,
        muted,
        accent,
        accent2,
        live: tone(
            seed.live
                .or_else(|| ansi(2))
                .unwrap_or(default(0x5eead4, 0x187b63)),
        ),
        warning: tone(
            seed.warning
                .or_else(|| ansi(3))
                .unwrap_or(default(0xf5d48a, 0x895b09)),
        ),
        danger: tone(
            seed.danger
                .or_else(|| ansi(1))
                .unwrap_or(default(0xff8a8a, 0xb53346)),
        ),
        on_accent: readable(
            seed.on_accent.unwrap_or_else(|| on(accent)),
            accent,
            on(accent),
            4.5,
        ),
        terminal_selection: seed
            .terminal_selection
            .or(term.selection_background)
            .unwrap_or_else(|| blend(ground, accent, if dark { 0.36 } else { 0.22 })),
        border_subtle: Some(seed.border_subtle.unwrap_or_else(|| blend(card, ink, 0.13))),
        border_strong: Some(seed.border_strong.unwrap_or_else(|| blend(card, ink, 0.24))),
        surface_raised: Some(surface_raised),
    }
}

fn fit(color: u32, toward: u32, surfaces: &[u32], floor: f32) -> u32 {
    (0..=32)
        .map(|i| blend(color, toward, i as f32 / 32.))
        .find(|&c| surfaces.iter().all(|&bg| contrast(c, bg) >= floor))
        .unwrap_or(toward)
}

fn useful_cursor(cursor: u32, ground: u32, ink: u32) -> bool {
    let [l, a, b] = oklab(cursor);
    a.hypot(b) >= 0.025
        && [ground, ink].into_iter().all(|other| {
            let [ol, oa, ob] = oklab(other);
            (l - ol).hypot((a - oa).hypot(b - ob)) >= 0.065
        })
}
