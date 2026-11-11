//! Settings → Interface's rows for the toolbar, the animation style and the window, and its
//! Customize panes page: padding, the active pane's cues and Focus follows mouse.
use super::{
    settings_kit::{
        SEGMENT, Segment, card, footnote, page, section, segmented_fixed, select_row, setting,
        stepper,
    },
    theme::*,
    toolbar_reveal,
    workspace::Workspace,
};
use rex_linux::settings::interface::{AnimationStyle, InterfaceConfig, ToolbarBehavior};
use rune::{El, Tx};
use rune_kit::Switch;
use serde_json::Value;

/// The Toolbar choices, in the order the control lists them.
const TOOLBARS: [ToolbarBehavior; 3] = [
    ToolbarBehavior::Always,
    ToolbarBehavior::OnHover,
    ToolbarBehavior::Hidden,
];

/// The Animation style choices, in the order the control lists them.
const ANIMATIONS: [AnimationStyle; 3] = [
    AnimationStyle::Snappy,
    AnimationStyle::Natural,
    AnimationStyle::Off,
];

/// A switch for one of the interface settings, saved as soon as it flips.
fn toggle(key: &str, label: &str, on: bool, field: fn(&mut InterfaceConfig) -> &mut bool) -> El {
    Switch::new(on)
        .label(label.to_owned())
        .hide_label()
        .on_toggle(move |s: &mut Workspace| {
            let value = field(&mut s.settings.interface);
            *value = !*value;
            s.save();
        })
        .build(kit())
        .key(key.to_owned())
}

/// When the toolbar shows, and how to bring it out where it has no row.
pub fn toolbar(s: &Workspace) -> El {
    let keys = s.shortcut_detail(toolbar_reveal::TOGGLE, &Value::Null);
    let reveal = if keys.is_empty() {
        "Reveal toolbar, in Commands,".to_owned()
    } else {
        format!("Reveal toolbar ({keys}, and in Commands)")
    };
    let note = format!(
        "Without its row, {reveal} floats the toolbar over the terminals until it is run again, so the window can still be moved and closed."
    );
    section(
        "Toolbar",
        [
            card([select_row(
                "toolbar-behavior",
                "Toolbar",
                "On hover and Hidden give its row to the terminals.",
                &["Always show", "On hover", "Hidden"],
                TOOLBARS
                    .iter()
                    .position(|t| *t == s.settings.interface.toolbar)
                    .unwrap_or_default(),
                |s, i, tx| s.set_toolbar(TOOLBARS[i.min(TOOLBARS.len() - 1)], tx),
            )]),
            footnote(note).key("toolbar-reveal-note"),
        ],
    )
}

/// How the chrome moves.
pub fn animation(s: &Workspace) -> El {
    section(
        "Animation",
        [
            card([setting(
                "Animation style",
                "How the chrome moves. Terminals never animate.",
                // Three segments in the width of the page's two-segment controls, so the
                // rows' controls still line up on both edges.
                segmented_fixed(
                    "animation-style",
                    2. * SEGMENT,
                    &[
                        Segment::new("Snappy"),
                        Segment::new("Natural"),
                        Segment::new("Off"),
                    ],
                    ANIMATIONS
                        .iter()
                        .position(|a| *a == s.settings.interface.animation),
                    |s, i, _| {
                        s.settings.interface.animation = ANIMATIONS[i.min(ANIMATIONS.len() - 1)];
                        s.save();
                    },
                ),
            )]),
            footnote(
                "Natural moves more slowly and settles softly. Off stops movement and keeps a brief fade, as your desktop's reduce-motion setting does whatever is chosen here.",
            ),
        ],
    )
}

/// What a launch does with the window.
pub fn window(s: &Workspace) -> El {
    section(
        "Window",
        [card([setting(
            "Show window on launch",
            "Off starts Rex minimized, for a client that starts with your desktop.",
            toggle(
                "show-window-on-launch",
                "Show window on launch",
                s.settings.interface.show_window_on_launch,
                |i| &mut i.show_window_on_launch,
            ),
        )])],
    )
}

/// One of the two padding steppers.
fn padding_row(key: &str, label: &str, value: f32, vertical: bool) -> El {
    let step = move |by: f32| {
        move |s: &mut Workspace, _: &mut Tx| {
            s.settings.step_padding(vertical, by);
            s.save();
        }
    };
    setting(
        label,
        "",
        stepper(key, format!("{value} pt"), step(-1.), step(1.)),
    )
}

/// Customize panes: a page of Interface's.
pub fn panes(s: &Workspace) -> El {
    let interface = &s.settings.interface;
    let (sides, ends) = s.settings.padding();
    page([
        section(
            "Padding",
            [
                card([
                    padding_row("pane-padding-x", "Horizontal padding", sides, false),
                    padding_row("pane-padding-y", "Vertical padding", ends, true),
                ]),
                footnote(
                    "The space between a pane's edges and its terminal's content, from 0 to 32 pt each way. A terminal's own background fills it.",
                ),
            ],
        ),
        section(
            "Active pane",
            [
                card([setting(
                    "Highlight active pane",
                    "An accent outline on the pane with the keyboard, in a tab with several.",
                    Switch::new(s.settings.highlights_active_pane())
                        .label("Highlight active pane")
                        .hide_label()
                        .on_toggle(|s: &mut Workspace| {
                            // From here on it is a choice, and no longer follows the density.
                            let on = s.settings.highlights_active_pane();
                            s.settings.interface.highlight_active_pane = Some(!on);
                            s.save();
                        })
                        .build(kit())
                        .key("highlight-active-pane"),
                )]),
                card([
                    setting(
                        "Fade inactive pane headers",
                        "Dims the name and buttons of panes without the keyboard.",
                        toggle(
                            "fade-inactive-headers",
                            "Fade inactive pane headers",
                            interface.fade_inactive_pane_headers,
                            |i| &mut i.fade_inactive_pane_headers,
                        ),
                    ),
                    setting(
                        "Fade headers in inactive windows",
                        "Dims every pane's header while this window is not the active one.",
                        toggle(
                            "fade-headers-inactive-window",
                            "Fade headers in inactive windows",
                            interface.fade_pane_headers_in_inactive_window,
                            |i| &mut i.fade_pane_headers_in_inactive_window,
                        ),
                    ),
                ]),
                footnote(
                    "Only headers fade, where Pane headers shows one. Terminal content is never dimmed, and a pane's activity word stays at full strength.",
                ),
            ],
        ),
        section(
            "Focus",
            [
                card([setting(
                    "Focus follows mouse",
                    "Resting the pointer on a pane focuses it.",
                    toggle(
                        "focus-follows-mouse",
                        "Focus follows mouse",
                        interface.focus_follows_mouse,
                        |i| &mut i.focus_follows_mouse,
                    ),
                )]),
                footnote(
                    "It moves focus only. A pane still takes this window's size when you type or paste in it, not when the pointer passes.",
                ),
            ],
        ),
    ])
}
