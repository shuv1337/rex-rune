//! Cached program glyphs and local colour policy. No fetching or shell themes.
use super::{
    activity::PaneProgram,
    program_icons::{ProgramIcon, shell},
    theme::*,
    widgets::program_glyph,
    workspace::Workspace,
};
use rex_rune::settings::interface::{InterfaceConfig, TabIconColor};
use rune::{El, col, el, row, text};

pub struct TabIcon {
    pub icon: &'static ProgramIcon,
    pub program: Option<String>,
}

/// Deduplicate resolved icons (vim/vi, shells/unknowns), not process names.
pub fn icons_of(programs: &[PaneProgram], count: u8) -> Vec<TabIcon> {
    let mut icons: Vec<TabIcon> = Vec::new();
    let count = usize::from(count.min(InterfaceConfig::MAX_TAB_ICONS));
    for program in programs {
        if icons.len() == count {
            break;
        }
        let icon = program.icon();
        if icons.iter().all(|other| other.icon.slug != icon.slug) {
            icons.push(TabIcon {
                icon,
                program: program.name.clone(),
            });
        }
    }
    if count > 0 && icons.is_empty() {
        icons.push(TabIcon {
            icon: shell(),
            program: None,
        });
    }
    icons
}

/// Includes the gap before the label; zero icons leave no child and no gap.
pub fn leading_width(count: usize) -> f32 {
    count as f32 * (15. + 8.)
}

/// Append directly to the existing 8pt-gap row: one keeps the original toolbar tree
/// and geometry, and zero has no spacer. Preview and both tab surfaces share this, and a
/// pane header too, which passes its own fading as `fade`: it multiplies the glyph's
/// resting colour and nothing else.
pub fn append(
    mut parent: El,
    key: &str,
    icons: &[TabIcon],
    active: bool,
    mode: TabIconColor,
    fade: f32,
) -> El {
    for (index, icon) in icons.iter().enumerate() {
        let mut cell = el()
            .key(if index == 0 {
                key.to_owned()
            } else {
                format!("{key}:{index}")
            })
            .child(program_glyph(
                icon.icon,
                15.,
                rgb(if in_color(mode, active, index == 0) {
                    icon.icon.tint
                } else if active {
                    ink()
                } else {
                    muted()
                })
                .alpha(fade),
            ));
        if let Some(program) = &icon.program {
            cell = cell.help(program.clone());
        }
        parent = parent.child(cell);
    }
    parent
}

/// The first distinct glyph represents the focused pane, including empty/fallback
/// tabs. Dedup never adds a second representative of the same glyph.
pub fn in_color(mode: TabIconColor, on_screen: bool, focused: bool) -> bool {
    match mode {
        TabIconColor::Always => true,
        TabIconColor::ActivePane => on_screen && focused,
        TabIconColor::ActiveTab => on_screen,
        TabIconColor::Never => false,
    }
}

impl Workspace {
    pub fn step_tab_icons(&mut self, step: i8) {
        self.settings.interface.tab_icon_count = self
            .settings
            .interface
            .tab_icon_count
            .saturating_add_signed(step)
            .min(InterfaceConfig::MAX_TAB_ICONS);
        self.reveal_tab_again = true;
        self.save();
    }
}

/// A deliberately inert sample, not a fabricated server tab. Uses the real glyph
/// resolver and row layout, including the zero-icon case.
pub fn preview(s: &Workspace) -> El {
    let programs = ["zsh", "claude", "nvim", "git", "cargo"].map(|p| PaneProgram {
        name: Some(p.to_owned()),
        executable: Some(p.to_owned()),
        script: None,
    });
    let icons = icons_of(&programs, s.settings.interface.tab_icon_count);
    let mut samples = col()
        .key("tab-icons-preview")
        .role("img")
        .label("Active and inactive sample tabs with the chosen icon count and colour")
        .padding(12.)
        .gap(4.)
        .inert();
    for (active, key, label) in [
        (true, "tab-icons-preview-icon", "Active sample"),
        (false, "tab-icons-preview-inactive", "Inactive sample"),
    ] {
        let pill = append(
            row()
                .h(28.)
                .px(14.)
                .gap(8.)
                .center_y()
                .radius(8.)
                .continuous_corners()
                .fill(white(if active { 0.11 } else { 0.05 })),
            key,
            &icons,
            active,
            s.settings.interface.tab_icon_color,
            1.,
        )
        .child(text(label).font(medium(12.5)).color(rgb(if active {
            ink()
        } else {
            muted()
        })));
        samples = samples.child(pill);
    }
    samples
}
