//! The icons a tab shows for its panes' programs: how many, which of them are drawn in
//! colour, and what a shell's icon looks like in colour (Settings → Interface → Tabs).
//! Toolbar pills, sidebar tab rows and the Settings preview all draw them with [`tab_icons`].
use super::{activity::program_icon, theme::*, widgets::glyph, workspace::Workspace};
use rex_linux::settings::{ShellIconTheme, TabIconColor, TabsConfig};
use rune::{Color, El, Icon, el, row};

/// An icon's box, and the room between two of a tab's icons.
const SIZE: f32 = 15.;
const GAP: f32 = 6.;
/// The badge a tab's first icon sits on: the focused pane's program, which is what the tab
/// is doing. A rounded square, so it is never taken for a button.
const BADGE: f32 = 20.;
const BADGE_RADIUS: f32 = 6.;
/// How far a shell's tile reaches past its icon's box on every side, where the shell isn't
/// the first icon: as large as the badge. The tile is an overlay, so an icon takes the same
/// room in colour and out of it and a tab keeps its width.
const TILE_OUTSET: f32 = (BADGE - SIZE) / 2.;
/// The green of a shell's prompt, on its dark tile.
const PROMPT: u32 = 0x7fdc8c;

/// How wide a row of `count` icons is.
pub fn row_width(count: usize) -> f32 {
    match count {
        0 => 0.,
        more => BADGE + (more - 1) as f32 * (GAP + SIZE),
    }
}

/// One of a tab's icons.
#[derive(Clone, Debug)]
pub struct TabIcon {
    pub icon: &'static Icon,
    /// What the pane runs, for the tooltip.
    pub program: Option<String>,
    /// Its pane is the tab's focused one.
    pub focused: bool,
}

impl TabIcon {
    fn shell(&self) -> bool {
        std::ptr::eq(self.icon, &TERMINAL)
    }
}

/// A tab's icons from its panes' programs (`activity::tab_programs`: the focused pane's
/// first). Panes that would show the same icon share one, so two shells are one prompt. A
/// tab with no pane yet is a shell.
pub fn icons_of(programs: &[Option<String>]) -> Vec<TabIcon> {
    let mut icons: Vec<TabIcon> = Vec::new();
    for (index, program) in programs.iter().enumerate() {
        let icon = program_icon(program.as_deref());
        if !icons.iter().any(|i| std::ptr::eq(i.icon, icon)) {
            icons.push(TabIcon {
                icon,
                program: program.clone(),
                focused: index == 0,
            });
        }
    }
    if icons.is_empty() {
        icons.push(TabIcon {
            icon: &TERMINAL,
            program: None,
            focused: true,
        });
    }
    icons
}

/// The colour a program's icon takes when icons are in colour: its own where it is known by
/// one, and otherwise a hue no other icon has. Shells have a tile instead ([`ShellTile`]).
pub fn tint(icon: &Icon) -> u32 {
    match icon.name {
        "rex-claude" => 0xe08a62,
        "rex-codex" => 0x4cc2a0,
        "rex-opencode" => 0xfab283,
        "rex-pi" => 0xb79cf5,
        "rex-vim" => 0x7cc45a,
        "rex-git" => 0xf2684a,
        "rex-crate" => 0xdea584,
        "rex-key" => 0xe6c35c,
        _ => PROMPT,
    }
}

/// A shell's icon in colour: a tile (the badge itself, when the shell is the tab's first
/// icon) and the prompt on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShellTile {
    pub fill: Color,
    pub edge: Color,
    pub ink: u32,
}

/// How a window draws its tabs' icons.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconLook {
    pub count: usize,
    pub color: TabIconColor,
    pub shell: ShellTile,
}

impl IconLook {
    /// Whether an icon is in colour: `on_screen` says its tab is the one on screen.
    pub fn in_color(&self, icon: &TabIcon, on_screen: bool) -> bool {
        match self.color {
            TabIconColor::Always => true,
            TabIconColor::ActiveTab => on_screen,
            TabIconColor::ActivePane => on_screen && icon.focused,
            TabIconColor::Never => false,
        }
    }
}

/// WCAG contrast between two sRGB colours, from 1 to 21.
pub fn contrast(a: u32, b: u32) -> f32 {
    fn luminance(color: u32) -> f32 {
        let channel = |shift: u32| {
            let c = ((color >> shift) & 0xff) as f32 / 255.;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The prompt's colour on a tile in `background`: the theme's green where it reads there
/// (3:1, as for any graphic), and its text colour where it doesn't.
pub fn prompt_on(background: u32, green: u32, foreground: u32) -> u32 {
    if contrast(green, background) >= 3. {
        green
    } else {
        foreground
    }
}

impl Workspace {
    /// How tabs draw their icons now, from the Tabs settings and, for a shell's tile, the
    /// terminal theme in use or the window colours.
    pub fn tab_icon_look(&self) -> IconLook {
        let TabsConfig {
            icons,
            icon_color,
            shell_icon,
            ..
        } = self.settings.tabs;
        let dark = ShellTile {
            fill: black(0.42),
            edge: white(0.1),
            ink: PROMPT,
        };
        let shell = match (shell_icon, &self.term_theme) {
            (ShellIconTheme::Accent, _) => ShellTile {
                ink: accent(),
                ..dark
            },
            // Follow server declares no colours, so there are none to take.
            (ShellIconTheme::Theme, Some(theme)) => {
                let c = self.swatch(theme);
                ShellTile {
                    fill: rgb(c.background),
                    edge: rgb(mix(c.foreground, c.background, 0.16)),
                    ink: prompt_on(c.background, c.ansi[2], c.foreground),
                }
            }
            _ => dark,
        };
        IconLook {
            count: usize::from(icons.min(TabsConfig::MAX_ICONS)),
            color: icon_color,
            shell,
        }
    }
}

/// A tab's icons in a row: as many as `look` shows, each in colour or in the tab's own ink
/// (`INK` on the tab on screen, `MUTED` on the others). The first sits on a badge. It is
/// keyed `key`, the rest `key:1`, `key:2` and so on; each names its program in a tooltip.
/// None when tabs show no icons.
pub fn tab_icons(key: &str, icons: &[TabIcon], on_screen: bool, look: &IconLook) -> Option<El> {
    if look.count == 0 {
        return None;
    }
    let plain = rgb(if on_screen { ink() } else { muted() });
    let mut icons_row = row().gap(GAP).center_y();
    for (index, icon) in icons.iter().take(look.count).enumerate() {
        let first = index == 0;
        let colored = look.in_color(icon, on_screen);
        let tile = colored && icon.shell();
        let mut cell = el().key(if first {
            key.to_owned()
        } else {
            format!("{key}:{index}")
        });
        if first {
            cell = cell
                .size(BADGE, BADGE)
                .center()
                .radius(BADGE_RADIUS)
                .continuous_corners();
            cell = if tile {
                cell.fill(look.shell.fill).border(1., look.shell.edge)
            } else {
                cell.fill(white(0.06))
            };
        } else if tile {
            cell = cell.child(
                el().key("tile")
                    .pin(
                        Some(-TILE_OUTSET),
                        Some(-TILE_OUTSET),
                        Some(-TILE_OUTSET),
                        Some(-TILE_OUTSET),
                    )
                    .radius(BADGE_RADIUS)
                    .continuous_corners()
                    .fill(look.shell.fill)
                    .border(1., look.shell.edge),
            );
        }
        let color = if tile {
            rgb(look.shell.ink)
        } else if colored {
            rgb(tint(icon.icon))
        } else {
            plain
        };
        cell = cell.child(glyph(icon.icon, SIZE, color));
        if let Some(program) = &icon.program {
            cell = cell.help(program.clone());
        }
        icons_row = icons_row.child(cell);
    }
    Some(icons_row)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn programs(names: &[Option<&str>]) -> Vec<Option<String>> {
        names.iter().map(|n| n.map(str::to_owned)).collect()
    }

    #[test]
    fn panes_with_the_same_icon_share_one_and_the_focused_pane_is_first() {
        let icons = icons_of(&programs(&[
            Some("nvim"),
            Some("zsh"),
            None,
            Some("vim"),
            Some("claude"),
        ]));
        let names: Vec<&str> = icons.iter().map(|i| i.icon.name).collect();
        assert_eq!(names, ["rex-vim", "rex-terminal", "rex-claude"]);
        assert_eq!(
            icons.iter().map(|i| i.focused).collect::<Vec<_>>(),
            [true, false, false]
        );
        // The tooltip names the first pane that shows the icon.
        assert_eq!(icons[0].program.as_deref(), Some("nvim"));
        assert_eq!(icons[1].program.as_deref(), Some("zsh"));
        // A tab whose panes haven't arrived is a shell.
        let empty = icons_of(&[]);
        assert_eq!(empty.len(), 1);
        assert!(empty[0].shell() && empty[0].focused && empty[0].program.is_none());
    }

    #[test]
    fn each_colour_mode_colours_the_icons_it_names() {
        let icons = icons_of(&programs(&[Some("claude"), Some("bash")]));
        let look = |color| IconLook {
            count: 2,
            color,
            shell: ShellTile {
                fill: black(0.4),
                edge: white(0.1),
                ink: PROMPT,
            },
        };
        let modes = |color| {
            let look = look(color);
            [
                look.in_color(&icons[0], true),
                look.in_color(&icons[1], true),
                look.in_color(&icons[0], false),
                look.in_color(&icons[1], false),
            ]
        };
        assert_eq!(modes(TabIconColor::Always), [true, true, true, true]);
        assert_eq!(modes(TabIconColor::ActiveTab), [true, true, false, false]);
        assert_eq!(modes(TabIconColor::ActivePane), [true, false, false, false]);
        assert_eq!(modes(TabIconColor::Never), [false; 4]);
    }

    /// A tab's fill at its lightest, over `ground`: the pill of the tab on screen.
    fn pill(ground: u32) -> u32 {
        mix(0xffffff, ground, 0.11)
    }

    #[test]
    fn every_tint_reads_on_every_palette_and_no_two_are_alike() {
        let icons = [
            &CLAUDE, &CODEX, &OPENCODE, &PI, &VIM, &GIT, &CRATE, &KEY, &TERMINAL,
        ];
        for chrome in rex_linux::settings::ChromeTheme::ALL {
            let palette = palette_of(chrome);
            for icon in icons {
                for surface in [palette.ground, pill(palette.ground)] {
                    let ratio = contrast(tint(icon), surface);
                    assert!(
                        ratio >= 3.,
                        "{} on {} is {ratio:.2}:1",
                        icon.name,
                        chrome.name()
                    );
                }
            }
        }
        // Programs are told apart by their icon's shape first; the tints still differ.
        let mut tints: Vec<u32> = icons[..8].iter().map(|i| tint(i)).collect();
        tints.sort_unstable();
        tints.dedup();
        assert_eq!(tints.len(), 8);
    }

    #[test]
    fn a_themes_prompt_is_its_green_unless_that_is_lost_on_its_background() {
        // Merino Dark-like: green reads on the background.
        assert_eq!(prompt_on(0x101418, 0x7fdc8c, 0xe6e6e6), 0x7fdc8c);
        // A pale green on a white background doesn't; the text colour does.
        assert_eq!(prompt_on(0xffffff, 0xb8f0c0, 0x1a1a1a), 0x1a1a1a);
        assert!(contrast(0x000000, 0xffffff) > 20.9);
        assert!((contrast(0x777777, 0x777777) - 1.).abs() < 1e-6);
    }
}
