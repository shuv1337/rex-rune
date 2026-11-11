//! Settings → Interface's switches that need nothing from a server: how the chrome moves,
//! when the toolbar shows, the space and the cues around panes, and whether a launch shows
//! the window. Every default is what the client did before the switch existed.
use serde::{Deserialize, Serialize};

/// The most space a pane keeps around its terminal's content, in points.
pub const MAX_PADDING: f32 = 32.;

/// When the toolbar shows. A value this version doesn't know reads as Always.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolbarBehavior {
    /// Over the top of the window while the pointer rests on its top edge; the row it had
    /// goes to the panes.
    OnHover,
    /// Only while Reveal toolbar has brought it out.
    Hidden,
    /// In its own row above the panes.
    #[default]
    #[serde(other)]
    Always,
}

/// How the chrome moves; terminal content never does. A value this version doesn't know reads
/// as Snappy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationStyle {
    /// Slower springs that settle softly.
    Natural,
    /// Nothing travels or scales; a change of colour or opacity takes a brief fade. The same
    /// as the desktop's own reduce-motion preference, which applies whatever is chosen here.
    Off,
    /// Quick springs with no overshoot.
    #[default]
    #[serde(other)]
    Snappy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InterfaceConfig {
    pub toolbar: ToolbarBehavior,
    pub animation: AnimationStyle,
    /// The space above and below a terminal's content. None follows `pane_padding`, which is
    /// then the space on all four sides, as it was before the two could differ; with a value
    /// here `pane_padding` is the space at the sides only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane_padding_vertical: Option<f32>,
    /// An accent outline on the pane that has the keyboard, in a tab with several. None
    /// follows the density: on for Comfortable's cards, off for Compact's flush panes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlight_active_pane: Option<bool>,
    /// Resting the pointer on a pane focuses it. It moves the session's focus and nothing
    /// else: size ownership still follows typing, pasting and the window coming forward.
    pub focus_follows_mouse: bool,
    /// Dim the name and buttons in the headers of panes without the keyboard. Chrome only:
    /// terminal content is never dimmed.
    pub fade_inactive_pane_headers: bool,
    /// Dim them in every pane's header while this window is not the active one.
    pub fade_pane_headers_in_inactive_window: bool,
    /// Off starts the window minimized, for a client started with the desktop.
    pub show_window_on_launch: bool,
}

impl Default for InterfaceConfig {
    fn default() -> Self {
        Self {
            toolbar: ToolbarBehavior::default(),
            animation: AnimationStyle::default(),
            pane_padding_vertical: None,
            highlight_active_pane: None,
            focus_follows_mouse: false,
            fade_inactive_pane_headers: false,
            fade_pane_headers_in_inactive_window: false,
            show_window_on_launch: true,
        }
    }
}

impl InterfaceConfig {
    /// Bring a loaded file's values into range.
    pub(super) fn normalize(&mut self) {
        self.pane_padding_vertical = self
            .pane_padding_vertical
            .filter(|padding| padding.is_finite())
            .map(|padding| padding.clamp(0., MAX_PADDING));
    }
}

impl super::Settings {
    /// The space between a pane's edges and its terminal's content, as (sides, top and
    /// bottom).
    pub fn padding(&self) -> (f32, f32) {
        (
            self.pane_padding,
            self.interface
                .pane_padding_vertical
                .unwrap_or(self.pane_padding),
        )
    }

    /// Step the space at a pane's sides (`vertical` false) or above and below its content by
    /// whole points, within 0 to [`MAX_PADDING`]. The other keeps the value it shows.
    pub fn step_padding(&mut self, vertical: bool, step: f32) {
        let (sides, ends) = self.padding();
        let stepped = |value: f32| (value + step).round().clamp(0., MAX_PADDING);
        if vertical {
            self.interface.pane_padding_vertical = Some(stepped(ends));
        } else {
            // Until now the one value was both: keep the ends where they are.
            self.interface.pane_padding_vertical = Some(ends);
            self.pane_padding = stepped(sides);
        }
    }

    /// Whether the pane with the keyboard is outlined: as chosen, or as the density has it.
    pub fn highlights_active_pane(&self) -> bool {
        self.interface
            .highlight_active_pane
            .unwrap_or(self.pane_density == super::PaneDensity::Comfortable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{PaneDensity, Settings};
    use serde_json::json;

    fn saved(settings: &Settings) -> serde_json::Value {
        serde_json::to_value(settings).unwrap()
    }

    /// A save and a load of `settings` through a file, as a relaunch does.
    fn reloaded(settings: &Settings, name: &str) -> Settings {
        let path = std::env::temp_dir().join(format!(
            "rex-interface-settings-{name}-{}.json",
            std::process::id()
        ));
        settings.save(&path).unwrap();
        let back = Settings::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        back
    }

    #[test]
    fn a_file_from_before_these_switches_loads_as_the_client_behaved_then() {
        for old in [
            json!({}),
            json!({"pane_padding": 12, "pane_density": "compact"}),
        ] {
            let settings: Settings = serde_json::from_value(old).unwrap();
            assert_eq!(settings.interface, InterfaceConfig::default());
            let i = &settings.interface;
            assert_eq!(i.toolbar, ToolbarBehavior::Always);
            assert_eq!(i.animation, AnimationStyle::Snappy);
            assert!(!i.focus_follows_mouse);
            assert!(!i.fade_inactive_pane_headers && !i.fade_pane_headers_in_inactive_window);
            assert!(i.show_window_on_launch);
            // One padding on all four sides, and the outline only on Comfortable's cards.
            assert_eq!(
                settings.padding(),
                (settings.pane_padding, settings.pane_padding)
            );
            assert_eq!(
                settings.highlights_active_pane(),
                settings.pane_density == PaneDensity::Comfortable
            );
            // Nothing is written for the two that follow another setting.
            let interface = &saved(&settings)["interface"];
            assert!(interface.get("pane_padding_vertical").is_none());
            assert!(interface.get("highlight_active_pane").is_none());
        }
    }

    #[test]
    fn toolbar_behaviour_round_trips_and_reads_an_unknown_value_as_always() {
        for (toolbar, word) in [
            (ToolbarBehavior::Always, "always"),
            (ToolbarBehavior::OnHover, "on_hover"),
            (ToolbarBehavior::Hidden, "hidden"),
        ] {
            let mut settings = Settings::default();
            settings.interface.toolbar = toolbar;
            assert_eq!(saved(&settings)["interface"]["toolbar"], word);
            assert_eq!(reloaded(&settings, "toolbar").interface.toolbar, toolbar);
        }
        let newer: Settings =
            serde_json::from_value(json!({"interface": {"toolbar": "left_side"}})).unwrap();
        assert_eq!(newer.interface.toolbar, ToolbarBehavior::Always);
    }

    #[test]
    fn animation_style_round_trips_and_reads_an_unknown_value_as_snappy() {
        for (animation, word) in [
            (AnimationStyle::Snappy, "snappy"),
            (AnimationStyle::Natural, "natural"),
            (AnimationStyle::Off, "off"),
        ] {
            let mut settings = Settings::default();
            settings.interface.animation = animation;
            assert_eq!(saved(&settings)["interface"]["animation"], word);
            assert_eq!(
                reloaded(&settings, "animation").interface.animation,
                animation
            );
        }
        let newer: Settings =
            serde_json::from_value(json!({"interface": {"animation": "bouncy"}})).unwrap();
        assert_eq!(newer.interface.animation, AnimationStyle::Snappy);
    }

    #[test]
    fn the_switches_round_trip_each_on_its_own_key() {
        type Field = fn(&mut InterfaceConfig) -> &mut bool;
        let switches: [(&str, bool, Field); 4] = [
            ("focus_follows_mouse", false, |i| &mut i.focus_follows_mouse),
            ("fade_inactive_pane_headers", false, |i| {
                &mut i.fade_inactive_pane_headers
            }),
            ("fade_pane_headers_in_inactive_window", false, |i| {
                &mut i.fade_pane_headers_in_inactive_window
            }),
            ("show_window_on_launch", true, |i| {
                &mut i.show_window_on_launch
            }),
        ];
        for (key, default, field) in switches {
            let mut settings = Settings::default();
            assert_eq!(*field(&mut settings.interface), default, "{key}");
            *field(&mut settings.interface) = !default;
            assert_eq!(saved(&settings)["interface"][key], !default, "{key}");
            let mut back = reloaded(&settings, key);
            assert_eq!(*field(&mut back.interface), !default, "{key}");
            // Only that one changed.
            *field(&mut back.interface) = default;
            assert_eq!(back.interface, InterfaceConfig::default(), "{key}");
        }
    }

    #[test]
    fn the_active_pane_outline_follows_the_density_until_it_is_chosen() {
        let mut settings = Settings::default();
        for (density, follows) in [
            (PaneDensity::Comfortable, true),
            (PaneDensity::Compact, false),
        ] {
            settings.pane_density = density;
            settings.interface.highlight_active_pane = None;
            assert_eq!(settings.highlights_active_pane(), follows);
            for chosen in [true, false] {
                settings.interface.highlight_active_pane = Some(chosen);
                assert_eq!(settings.highlights_active_pane(), chosen);
                assert_eq!(
                    saved(&settings)["interface"]["highlight_active_pane"],
                    chosen
                );
                let back = reloaded(&settings, "highlight");
                assert_eq!(back.interface.highlight_active_pane, Some(chosen));
            }
        }
    }

    #[test]
    fn padding_steps_each_way_on_its_own_and_a_loaded_value_is_brought_into_range() {
        let mut settings = Settings::default();
        assert_eq!(settings.padding(), (8., 8.));
        // The sides move alone: the ends keep the 8 pt they showed.
        settings.step_padding(false, 1.);
        settings.step_padding(false, 1.);
        assert_eq!(settings.padding(), (10., 8.));
        settings.step_padding(true, -1.);
        settings.step_padding(true, -1.);
        assert_eq!(settings.padding(), (10., 6.));
        let json = saved(&settings);
        assert_eq!(json["pane_padding"], 10.);
        assert_eq!(json["interface"]["pane_padding_vertical"], 6.);
        assert_eq!(reloaded(&settings, "padding").padding(), (10., 6.));

        for _ in 0..40 {
            settings.step_padding(true, -1.);
            settings.step_padding(false, 1.);
        }
        assert_eq!(settings.padding(), (MAX_PADDING, 0.));

        for (written, read) in [(-3., 0.), (0., 0.), (6., 6.), (99., MAX_PADDING)] {
            let mut settings = Settings::default();
            settings.interface.pane_padding_vertical = Some(written);
            assert_eq!(reloaded(&settings, "range").padding(), (8., read));
        }
    }
}
