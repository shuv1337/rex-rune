//! The chrome palette's tokens, the kit pinned to them, shared surfaces (chips, glass,
//! cards) and the app's icons.
use rex_rune::chrome_theme::{BUILTINS, PaletteSpec};
use rex_rune::settings::ChromeTheme;
use rune::{Color, El, Icon, Shadow, Spring, SpringPair, TextStyle, Transition};
use rune_kit::{Kit, PaletteFocus, PaletteSeed};
use std::{
    cell::RefCell,
    sync::{Arc, LazyLock, OnceLock},
};

/// sRGB `0xRRGGBB` → linear `Color`.
pub fn rgb(value: u32) -> Color {
    let channel = |shift: u32| rune::color::srgb_decode(((value >> shift) & 0xff) as f32 / 255.);
    Color {
        r: channel(16),
        g: channel(8),
        b: channel(0),
        a: 1.,
    }
}

/// `a` over `b` at opacity `t`, mixed in sRGB.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    rex_rune::color::over(a, b, t)
}

pub fn tint(a: f32) -> Color {
    palette().tint(a)
}
pub fn scrim() -> Color {
    palette().scrim()
}
pub fn drop_shadow(a: f32) -> Color {
    Color::BLACK.alpha(if palette().dark { a } else { a * 0.235 })
}
pub fn shadow(x: f32, y: f32, blur: f32, color: Color) -> Shadow {
    Shadow {
        x,
        y,
        blur,
        spread: 0.,
        color,
        inset: false,
    }
}
pub fn inset(y: f32, color: Color) -> Shadow {
    Shadow {
        x: 0.,
        y,
        blur: 0.,
        spread: 0.,
        color,
        inset: true,
    }
}

/// One chrome palette: the colour tokens, and the kit pinned to them, built once.
pub struct Palette {
    spec: PaletteSpec,
    kit: OnceLock<Kit>,
    selection_strength: f32,
    pub button_end: u32,
}
impl std::ops::Deref for Palette {
    type Target = PaletteSpec;
    fn deref(&self) -> &PaletteSpec {
        &self.spec
    }
}
static PALETTES: LazyLock<Vec<Arc<Palette>>> = LazyLock::new(|| {
    BUILTINS
        .iter()
        .map(|b| {
            Arc::new(Palette {
                selection_strength: selection_strength(&b.palette),
                button_end: rex_rune::color::readable(
                    mix(b.palette.accent2, b.palette.accent, 0.55),
                    b.palette.on_accent,
                    b.palette.accent,
                    4.5,
                ),
                spec: b.palette,
                kit: OnceLock::new(),
            })
        })
        .collect()
});
pub static MIDNIGHT: LazyLock<Arc<Palette>> = LazyLock::new(|| palette_of(&ChromeTheme::Midnight));
pub fn palette_of(chrome: &ChromeTheme) -> Arc<Palette> {
    let index = BUILTINS
        .iter()
        .position(|b| &b.chrome == chrome)
        .expect("validated built-in chrome ID");
    PALETTES[index].clone()
}
thread_local! {
    static ACTIVE: RefCell<Arc<Palette>> = RefCell::new(MIDNIGHT.clone());
}
pub fn palette() -> Arc<Palette> {
    ACTIVE.with_borrow(Arc::clone)
}
pub fn set_palette(p: Arc<Palette>) {
    ACTIVE.replace(p);
}
pub fn native_scheme(chrome: &ChromeTheme) -> rune::ColorScheme {
    if chrome.preset().palette.dark {
        rune::ColorScheme::Dark
    } else {
        rune::ColorScheme::Light
    }
}
pub fn ground() -> u32 {
    palette().ground
}
pub fn card() -> u32 {
    palette().card
}
pub fn popover() -> u32 {
    palette().popover
}
pub fn ink() -> u32 {
    palette().ink
}
pub fn muted() -> u32 {
    palette().muted
}
pub fn accent() -> u32 {
    palette().accent
}
pub fn accent2() -> u32 {
    palette().accent2
}
pub fn warning() -> u32 {
    palette().warning
}
pub fn danger() -> u32 {
    palette().danger
}
pub fn on_accent() -> u32 {
    palette().on_accent
}

/// How much of its tone the `SELECTION` fill is.
pub const SELECTION_FILL: f32 = 0.18;

/// Keep small text readable even on a keycap stacked over the selected row.
/// Compute this once per palette, keeping the normal wash wherever it fits.
fn selection_strength(p: &PaletteSpec) -> f32 {
    use rex_rune::color::{contrast, over};
    let overlay = if p.dark { 0xffffff } else { p.ink };
    (0..=20)
        .rev()
        .map(|n| n as f32 / 20.)
        .find(|&a| {
            [p.accent, p.danger].into_iter().all(|tone| {
                contrast(
                    p.ink,
                    over(
                        overlay,
                        over(tone, p.popover, SELECTION_FILL * a),
                        KEYCAP_LIT * a,
                    ),
                ) >= 4.55
            })
        })
        .unwrap_or(0.)
}

/// `SELECTION`, the keyboard-selected row of a list, as (fill, outline): a tint of `ACCENT`
/// under a 1 pt outline in it, so selection is never fill alone and never reads as a hover. A
/// destructive row tints in `DANGER` instead. Text on it is `INK`.
pub fn selection(destructive: bool) -> (Color, Color) {
    let tone = rgb(if destructive { danger() } else { accent() });
    (
        tone.alpha(SELECTION_FILL * palette().selection_strength),
        tone,
    )
}

/// Terminal search marks, drawn over the cells as the selection is: every match is a tint of
/// `ACCENT` at `SEARCH_MATCH` under a 1 pt outline in it at `SEARCH_OUTLINE`, and the selected
/// one is `ACCENT` at `SEARCH_CURRENT` with its text in `ON_ACCENT`. The tint is light because
/// a match keeps the terminal's own text colour, dim ones included: the outline is what marks
/// it.
pub const SEARCH_MATCH: f32 = 0.18;
pub const SEARCH_OUTLINE: f32 = 1.;
pub const SEARCH_CURRENT: f32 = 1.;
pub const SEARCH_RADIUS: f32 = 2.5;

/// Resting keycap opacity and the maximum opacity on a selected row.
pub const KEYCAP_FILL: f32 = 0.07;
const KEYCAP_LIT: f32 = 0.12;
pub fn keycap_alpha(lit: bool) -> f32 {
    if lit {
        KEYCAP_LIT * palette().selection_strength
    } else {
        KEYCAP_FILL
    }
}

/// A keycap's text: halfway from `MUTED` to `INK`, so that 10.5 pt keys meet 4.5:1 on their
/// chip in every palette. `MUTED` alone does not on the lighter popovers.
pub fn keycap_ink() -> u32 {
    mix(ink(), muted(), 0.5)
}

pub const TOOLBAR: f32 = 52.;
pub const GUTTER: f32 = 8.;
pub const MARGIN: f32 = 10.;
pub const CARD_RADIUS: f32 = 12.;
pub const CARD_HEADER: f32 = 30.;

/// Springs for chrome: quick and calm.
pub const SNAPPY: Spring = Spring {
    duration: 0.28,
    bounce: 0.0,
};
pub const POP: Spring = Spring {
    duration: 0.32,
    bounce: 0.12,
};
/// For things that must not move (terminal content).
pub const STILL: Spring = Spring {
    duration: 0.001,
    bounce: 0.,
};

/// A sliding highlight's own entrance and exit: none, so one highlight shows even when its
/// matched partner isn't mounted (Reduce Motion, a row scrolled in).
pub fn no_presence() -> Transition {
    Transition::identity().springs(SpringPair::new(STILL, STILL))
}

/// A popover's entrance from its anchor: a little scale and a fade on a gentle spring.
pub fn pop_in() -> Transition {
    Transition::scale(0.96).springs(SpringPair::new(POP, SNAPPY))
}

impl Palette {
    fn tint(&self, a: f32) -> Color {
        (if self.dark {
            Color::WHITE
        } else {
            rgb(self.ink)
        })
        .alpha(a)
    }
    fn scrim(&self) -> Color {
        Color::BLACK.alpha(if self.dark { 0.5 } else { 0.32 })
    }
    /// This palette's kit, pinned once and shared by every view.
    pub fn kit(&self) -> &Kit {
        let p = self;
        p.kit.get_or_init(|| {
            let mut k = Kit::from_palette_focus(
                PaletteSeed {
                    bg: rgb(p.ground),
                    fg: rgb(p.ink),
                    accent: rgb(p.accent),
                },
                PaletteFocus::Custom(rgb(p.accent)),
            );
            k.dark = p.dark;
            let c = &mut k.c;
            (c.page, c.bg_base, c.surface_base) = (rgb(p.ground), rgb(p.ground), rgb(p.ground));
            c.bg_deep = rgb(p.card);
            for slot in [
                &mut c.layer_01,
                &mut c.layer_02,
                &mut c.surface_raised,
                &mut c.surface_raised_base,
                &mut c.surface_float,
            ] {
                *slot = rgb(p.popover);
            }
            (c.hover, c.surface_hover, c.surface_raised_hover) =
                (p.tint(0.07), p.tint(0.07), p.tint(0.07));
            c.pressed = p.tint(0.11);
            (c.border_base, c.border_muted, c.border_weak) =
                (p.tint(0.12), p.tint(0.08), p.tint(0.06));
            (c.border_strong, c.border_hover) = (p.tint(0.2), p.tint(0.2));
            c.border_focus = rgb(p.accent);
            (c.text_base, c.text_strong, c.icon_base) = (rgb(p.ink), rgb(p.ink), rgb(p.ink));
            for slot in [
                &mut c.text_muted,
                &mut c.text_faint,
                &mut c.text_weak,
                &mut c.icon_muted,
                &mut c.icon_faint,
                &mut c.icon_weaker,
            ] {
                *slot = rgb(p.muted);
            }
            (c.text_accent, c.icon_accent) = (rgb(p.accent), rgb(p.accent));
            c.success.fg = rgb(p.live);
            c.warning.fg = rgb(p.warning);
            c.danger.fg = rgb(p.danger);
            c.scrim = p.scrim();
            c.submit_fg = rgb(p.on_accent);
            c.submit_icon = rgb(p.on_accent);
            k.radius.sm = 8.;
            k.radius.md = 10.;
            k
        })
    }
}

pub fn ui(size: f32) -> TextStyle {
    TextStyle::new(size, 450.)
}
pub fn medium(size: f32) -> TextStyle {
    TextStyle::new(size, 560.)
}
pub fn strong(size: f32) -> TextStyle {
    TextStyle::new(size, 620.)
}
/// Small capitals for group labels.
pub fn label_caps() -> TextStyle {
    TextStyle::new(10.5, 650.).tracking(0.6)
}
/// Code-like values in chrome (paths, commits, endpoints, ids, commands): the terminal's font
/// stack, which ends in the bundled IBM Plex Mono.
pub fn mono(size: f32) -> TextStyle {
    TextStyle::new(size, 450.).mono()
}

/// A glassy pill every control in the bar sits on.
pub fn chip(e: El) -> El {
    let p = palette();
    e.fill(p.surface_raised.map_or_else(|| tint(0.055), rgb))
        .border(1., p.border_strong.map_or_else(|| tint(0.13), rgb))
        .radius(8.)
        .continuous_corners()
}

/// The canvas and accent preview shared by the picker and its Settings control.
pub fn swatch(p: &PaletteSpec) -> El {
    rune::row()
        .size(24., 14.)
        .clip_rounded(4.)
        .child(rune::el().size(12., 14.).fill(rgb(p.ground)))
        .child(rune::el().size(12., 14.).fill(rgb(p.accent)))
        .child(
            rune::el()
                .pin(Some(0.), Some(0.), Some(0.), Some(0.))
                .radius(4.)
                .border(1., tint(0.26)),
        )
}

pub fn glass(e: El, radius: f32, blur: bool) -> El {
    let e = e
        .fill(rgb(popover()).alpha(if palette().dark { 0.95 } else { 0.98 }))
        .border_gradient(1., tint(0.16), tint(0.05))
        .radius(radius)
        .continuous_corners()
        .box_shadow(shadow(0., 24., 60., drop_shadow(0.6)))
        .box_shadow(shadow(0., 2., 6., drop_shadow(0.35)));
    if blur { e.backdrop_blur(22.) } else { e }
}

macro_rules! icon {
    ($name:ident, $id:literal, $($el:literal),+) => {
        pub static $name: Icon = Icon {
            name: $id,
            view: 20.0,
            body: concat!($("<", $el, r#" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#),+),
        };
    };
}
icon!(
    TERMINAL,
    "rex-terminal",
    r#"path d="m4 6 4 4-4 4""#,
    r#"path d="M10.5 14.5h5.5""#
);
icon!(CHEVRON, "rex-chevron", r#"path d="m7 8.5 3 3 3-3""#);
// The find bar's steps: wider than the disclosure chevron, to sit beside the close cross.
icon!(STEP_UP, "rex-step-up", r#"path d="m6 12 4-4 4 4""#);
icon!(STEP_DOWN, "rex-step-down", r#"path d="m6 8 4 4 4-4""#);
icon!(RIGHT, "rex-right", r#"path d="m8.5 6.5 3.5 3.5-3.5 3.5""#);
icon!(LEFT, "rex-left", r#"path d="m11.5 6.5-3.5 3.5 3.5 3.5""#);
icon!(CHECK, "rex-check", r#"path d="m5.5 10.5 3 3 6-7""#);
icon!(
    PLUS,
    "rex-plus",
    r#"path d="M10 4.5v11""#,
    r#"path d="M4.5 10h11""#
);
icon!(
    CLOSE,
    "rex-close",
    r#"path d="m6.5 6.5 7 7""#,
    r#"path d="m13.5 6.5-7 7""#
);
icon!(
    PENCIL,
    "rex-pencil",
    r#"path d="M12.5 5.5 14.5 7.5 8 14l-2.75.75L6 12z""#
);
icon!(
    SEARCH,
    "rex-search",
    r#"circle cx="9" cy="9" r="5""#,
    r#"path d="m13 13 3.5 3.5""#
);
// Insert path's field: a path on the terminal's machine, not a search.
icon!(
    FOLDER,
    "rex-folder",
    r#"path d="M3.5 6.5A1.5 1.5 0 0 1 5 5h3l1.75 2H15a1.5 1.5 0 0 1 1.5 1.5V14a1.5 1.5 0 0 1-1.5 1.5H5A1.5 1.5 0 0 1 3.5 14z""#
);
icon!(
    COMMAND,
    "rex-command",
    r#"path d="M12.5 5v10a2.5 2.5 0 1 0 2.5-2.5H5a2.5 2.5 0 1 0 2.5 2.5V5a2.5 2.5 0 1 0-2.5 2.5h10a2.5 2.5 0 1 0-2.5-2.5""#
);
// A gear: eight teeth around a hub. Sliders read as view options, not Settings.
icon!(
    SETTINGS,
    "rex-settings",
    r#"path d="M8.69 4.55L9.02 2.56A7.5 7.5 0 0 1 10.98 2.56L11.31 4.55A5.6 5.6 0 0 1 12.93 5.23L14.57 4.05A7.5 7.5 0 0 1 15.95 5.43L14.77 7.07A5.6 5.6 0 0 1 15.45 8.69L17.44 9.02A7.5 7.5 0 0 1 17.44 10.98L15.45 11.31A5.6 5.6 0 0 1 14.77 12.93L15.95 14.57A7.5 7.5 0 0 1 14.57 15.95L12.93 14.77A5.6 5.6 0 0 1 11.31 15.45L10.98 17.44A7.5 7.5 0 0 1 9.02 17.44L8.69 15.45A5.6 5.6 0 0 1 7.07 14.77L5.43 15.95A7.5 7.5 0 0 1 4.05 14.57L5.23 12.93A5.6 5.6 0 0 1 4.55 11.31L2.56 10.98A7.5 7.5 0 0 1 2.56 9.02L4.55 8.69A5.6 5.6 0 0 1 5.23 7.07L4.05 5.43A7.5 7.5 0 0 1 5.43 4.05L7.07 5.23A5.6 5.6 0 0 1 8.69 4.55Z""#,
    r#"circle cx="10" cy="10" r="2.4""#
);
icon!(
    MONITOR,
    "rex-monitor",
    r#"rect x="3" y="4" width="14" height="9.5" rx="2""#,
    r#"path d="M10 13.5v3""#,
    r#"path d="M7 16.5h6""#
);
icon!(
    SERVER,
    "rex-server",
    r#"rect x="3.5" y="4" width="13" height="5" rx="1.5""#,
    r#"rect x="3.5" y="11" width="13" height="5" rx="1.5""#,
    r#"path d="M6.5 6.5h.01""#,
    r#"path d="M6.5 13.5h.01""#
);
// Add host's groups: the machines of a tailnet, and the ones that are away.
icon!(
    NETWORK,
    "rex-network",
    r#"circle cx="10" cy="5" r="2""#,
    r#"circle cx="4.75" cy="14.5" r="2""#,
    r#"circle cx="15.25" cy="14.5" r="2""#,
    r#"path d="m9 6.75-3.25 6""#,
    r#"path d="m11 6.75 3.25 6""#,
    r#"path d="M6.75 14.5h6.5""#
);
icon!(
    MOON,
    "rex-moon",
    r#"path d="M16 11.75A6.5 6.5 0 1 1 8.25 4 5.25 5.25 0 0 0 16 11.75Z""#
);
icon!(
    SIDEBAR,
    "rex-sidebar",
    r#"rect x="3" y="4" width="14" height="12" rx="3""#,
    r#"path d="M8 4v12""#
);
// The pane header's actions: a pane cut in two side by side, cut in two one over the other,
// grown to its tab's corners, and brought back from them.
icon!(
    SPLIT_RIGHT,
    "rex-split-right",
    r#"rect x="3" y="4.5" width="14" height="11" rx="2""#,
    r#"path d="M10 4.5v11""#
);
icon!(
    SPLIT_DOWN,
    "rex-split-down",
    r#"rect x="3" y="4.5" width="14" height="11" rx="2""#,
    r#"path d="M3 10h14""#
);
icon!(
    ZOOM,
    "rex-zoom",
    r#"path d="M11.5 4.5h4v4""#,
    r#"path d="m15.5 4.5-4.25 4.25""#,
    r#"path d="M8.5 15.5h-4v-4""#,
    r#"path d="m4.5 15.5 4.25-4.25""#
);
icon!(
    RESTORE,
    "rex-restore",
    r#"path d="M11.5 4.5v4h4""#,
    r#"path d="m11.5 8.5 4-4""#,
    r#"path d="M8.5 15.5v-4h-4""#,
    r#"path d="m8.5 11.5-4 4""#
);
icon!(MINUS, "rex-minus", r#"path d="M6 10h8""#);
// A popup button's badge: the value opens a list.
icon!(
    UPDOWN,
    "rex-updown",
    r#"path d="m7 8 3-3 3 3""#,
    r#"path d="m7 12 3 3 3-3""#
);
// Pane density: cards with gaps, or panes that share their borders.
icon!(
    PANES_APART,
    "rex-panes-apart",
    r#"rect x="3.5" y="4.5" width="5.5" height="11" rx="1.5""#,
    r#"rect x="11" y="4.5" width="5.5" height="11" rx="1.5""#
);
icon!(
    PANES_JOINED,
    "rex-panes-joined",
    r#"rect x="3.5" y="4.5" width="13" height="11" rx="1.5""#,
    r#"path d="M10 4.5v11""#
);
// Tab position: a strip along the top (the sidebar position uses `SIDEBAR`).
icon!(
    TABS_TOP,
    "rex-tabs-top",
    r#"rect x="3" y="4" width="14" height="12" rx="3""#,
    r#"path d="M3 8h14""#
);
// A muted tab: a bell with a stroke through it.
icon!(
    BELL_OFF,
    "rex-bell-off",
    r#"path d="M7.6 5.2A4 4 0 0 1 14 8.5c0 1.6.3 2.8.8 3.7""#,
    r#"path d="M6.2 7.2A4 4 0 0 0 6 8.5c0 2.6-.9 3.9-1.75 4.75h9.25""#,
    r#"path d="M8.75 16h2.5""#,
    r#"path d="m4 4 12 12""#
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midnight_keeps_its_token_values() {
        let p = &MIDNIGHT;
        assert_eq!(
            [
                p.ground,
                p.card,
                p.popover,
                p.ink,
                p.muted,
                p.accent,
                p.accent2,
                p.live,
                p.warning,
                p.danger,
                p.on_accent,
                p.terminal_selection,
            ],
            [
                0x001528, 0x001528, 0x0d1524, 0xe8eef6, 0x93a3b8, 0x7cc4ff, 0xa78bfa, 0x5eead4,
                0xf5d48a, 0xff8a8a, 0x061220, 0x2f4a6e,
            ]
        );
        assert_eq!(palette().kit().c.page, rgb(0x001528));
        assert_eq!(palette().kit().c.text_muted, rgb(0x93a3b8));
    }

    use rex_rune::color::{contrast, over};

    /// Hue in degrees.
    fn hue(c: u32) -> f64 {
        let [r, g, b] = [16, 8, 0].map(|shift| ((c >> shift) & 0xff) as f64 / 255.);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let d = max - min;
        let h = if d == 0. {
            0.
        } else if max == r {
            ((g - b) / d).rem_euclid(6.)
        } else if max == g {
            (b - r) / d + 2.
        } else {
            (r - g) / d + 4.
        };
        h * 60.
    }

    #[test]
    fn every_palette_is_legible_and_pairs_with_its_terminal_theme() {
        assert!((contrast(0xffffff, 0x000000) - 21.).abs() < 1e-5);
        assert_eq!(BUILTINS.len(), 50);
        assert_eq!(BUILTINS.iter().filter(|b| b.palette.dark).count(), 35);
        for chrome in ChromeTheme::all() {
            let p = palette_of(&chrome);
            set_palette(p.clone());
            let name = chrome.name();
            assert_eq!(p.kit().dark, p.dark);
            assert!(
                contrast(p.button_end, p.on_accent) >= 4.5,
                "{name}: button gradient ink"
            );
            assert!(
                contrast(p.on_accent, over(p.accent, p.ground, SEARCH_CURRENT)) >= 4.5,
                "{name}: current search match"
            );
            assert!(contrast(p.on_accent, p.accent) >= 4.5, "{name}: button ink");
            let overlay = if p.dark { 0xffffff } else { p.ink };
            assert!(contrast(p.muted, p.card) >= 4.5, "{name}: muted on card");
            assert!(contrast(p.ink, p.popover) >= 4.5, "{name}: ink on popover");
            let keys = keycap_ink();
            let rail = if p.sidebar == p.ground {
                over(overlay, p.sidebar, 0.025)
            } else {
                p.sidebar
            };
            let field = p
                .surface_raised
                .unwrap_or_else(|| over(overlay, rail, 0.055));
            for (surface, under) in [("popover", p.popover), ("sidebar field", field)] {
                let chip = over(overlay, under, KEYCAP_FILL);
                assert!(
                    contrast(keys, chip) >= 4.5,
                    "{name}: keycaps on the {surface}"
                );
            }
            // A selected row keeps its label and its keycaps in ink on either tint, and its
            // outline stands clear of the popover around it.
            for (kind, tone) in [("selection", p.accent), ("destructive selection", p.danger)] {
                let fill = over(tone, p.popover, selection(tone == p.danger).0.a);
                assert!(contrast(p.ink, fill) >= 4.5, "{name}: ink on the {kind}");
                let chip = over(overlay, fill, keycap_alpha(true));
                assert!(
                    contrast(p.ink, chip) >= 4.5,
                    "{name}: keycaps on the {kind}"
                );
                assert!(contrast(tone, p.popover) >= 3., "{name}: {kind} outline");
            }
            // Trouble always carries a word; Gruvbox's brightest red is 4.3:1 on its ground.
            for (word, color) in [("warning", p.warning), ("danger", p.danger)] {
                for surface in [p.card, p.popover] {
                    assert!(contrast(color, surface) >= 3., "{name}: {word}");
                }
            }
            let gap = (hue(p.warning) - hue(p.danger)).abs();
            assert!(
                matches!(chrome.id(), "silver-point-dark" | "silver-point-light")
                    || gap.min(360. - gap) >= 30.,
                "{name}: warning and danger differ in hue"
            );
            assert_eq!(p.card, p.ground, "{name}: one surface");
            let paired = rex_rune::terminal_theme::paired(&chrome);
            assert_eq!(
                paired.theme().background,
                Some(p.ground),
                "{name}: the paired theme sits on the ground"
            );
            set_palette(p.clone());
            assert_eq!(palette().kit().c.page, rgb(p.ground), "{name}: its own kit");
            assert_eq!(palette().kit().c.text_base, rgb(p.ink));
        }
        set_palette(MIDNIGHT.clone());
        assert_eq!(
            palette().kit().c.page,
            rgb(0x001528),
            "Midnight's kit is unchanged"
        );
    }
}
