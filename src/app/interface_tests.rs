//! Settings → Interface's client-only switches: when the toolbar shows and Reveal toolbar,
//! the animation style, padding each way, the active pane's cues, Focus follows mouse and
//! Show window on launch.
use super::pane_verb_tests::{Camera, framed_terminal, rect, sent_verbs};
use super::*;
use crate::app::{
    actions, pane_cues,
    panel::SettingsSection,
    theme,
    toolbar_reveal::{self, Placement},
};
use rex_linux::settings::{
    PaneDensity, PaneHeader,
    interface::{AnimationStyle, InterfaceConfig, ToolbarBehavior},
};
use serde_json::Value;

const TOOLBAR: f32 = 52.;
const MARGIN: f32 = 10.;

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..90 {
        rt.frame(1. / 60.);
    }
}

fn tap(rt: &mut Runtime<Workspace>, key: &str) {
    click(rt, key);
    settle(rt);
}

/// The tab `active` of `tabbed_workspace` under `toolbar`, its panes live terminals that
/// send to the second channel.
fn drawn(
    active: &str,
    toolbar: ToolbarBehavior,
) -> (
    Runtime<Workspace>,
    flume::Receiver<HostCommand>,
    flume::Receiver<HostCommand>,
) {
    let (mut ws, host) = tabbed_workspace(active);
    ws.settings.interface.toolbar = toolbar;
    let (sender, sent) = flume::unbounded();
    for block in ["block:a", "block:b", "block:c"] {
        framed_terminal(&mut ws, block, &sender, b"$ make\r\nok");
    }
    let mut rt = runtime(ws);
    settle(&mut rt);
    (rt, host, sent)
}

/// Rest the pointer at (x, y) long enough for a delayed hover to be heard.
fn rest_at(rt: &mut Runtime<Workspace>, x: f32, y: f32) {
    rt.pointer_move(x, y);
    for _ in 0..30 {
        rt.frame(1. / 60.);
    }
}

/// Ctrl Alt Shift T, the default key of Reveal toolbar.
fn reveal_key(rt: &mut Runtime<Workspace>) {
    rt.key(KeyEvent {
        key: Key::Text("T".into()),
        mods: Mods {
            ctrl: true,
            shift: true,
            alt: true,
            ..Mods::default()
        },
    });
    settle(rt);
}

/// What the app's Commands row for the toolbar reads, and whether it can run.
fn toolbar_command(rt: &mut Runtime<Workspace>) -> (String, bool) {
    rt.update(|s, _| s.picker(false));
    rt.frame(1. / 60.);
    let row = rt
        .state
        .entries()
        .into_iter()
        .find(|e| e.label.ends_with(" toolbar"))
        .map(|e| (e.label, e.enabled))
        .expect("a Commands row");
    press(rt, Key::Escape);
    row
}

/// Presses the row named `label` of the list a popup button opened.
fn choose(rt: &mut Runtime<Workspace>, control: &str, label: &str) {
    tap(rt, control);
    let r = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.role == "menuitem" && e.label.as_deref() == Some(label))
        .unwrap_or_else(|| panic!("no {label} row"))
        .rect;
    rt.pointer_down(r[0] + r[2] / 2., r[1] + r[3] / 2.);
    rt.pointer_up(r[0] + r[2] / 2., r[1] + r[3] / 2.);
    settle(rt);
}

/// Settings open on Interface in a window tall enough for the dialog's full height.
fn interface_settings() -> Runtime<Workspace> {
    let (mut ws, _host) = tabbed_workspace("split");
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    let mut rt = runtime_at(ws, (1440., 1000.));
    settle(&mut rt);
    rt
}

/// Scrolls the Settings page to its foot.
fn to_the_foot(rt: &mut Runtime<Workspace>) {
    rt.pointer_move(900., 500.);
    rt.wheel(900., 500., -4000.);
    for _ in 0..30 {
        rt.frame(1. / 60.);
    }
}

fn saved(rt: &Runtime<Workspace>) -> InterfaceConfig {
    relaunch(&rt.state).settings.interface
}

#[test]
fn always_keeps_the_toolbar_in_its_row_above_the_panes() {
    let (rt, _host, _sent) = drawn("single", ToolbarBehavior::Always);
    assert_eq!(toolbar_reveal::placement(&rt.state), Placement::Row);
    assert_eq!(rect(&rt, "toolbar"), [0., 0., 1024., TOOLBAR]);
    assert_eq!(rt.state.area, [0., TOOLBAR, 1024., 700. - TOOLBAR]);
    assert!(!has(&rt, "toolbar-band") && !has(&rt, "toolbar-float"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn hidden_gives_the_toolbars_row_to_the_panes() {
    let (mut rt, _host, _sent) = drawn("single", ToolbarBehavior::Hidden);
    assert_eq!(toolbar_reveal::placement(&rt.state), Placement::None);
    assert!(!has(&rt, "toolbar") && !has(&rt, "toolbar-band"));
    assert_eq!(rt.state.area, [0., 0., 1024., 700.], "all 52 pt of it");
    assert_eq!(rect(&rt, "pane:block:a"), [0., 0., 1024., 700.]);
    assert_eq!(rect(&rt, "grid:block:a")[1], 8., "under its own padding");
    // Resting on the top edge brings nothing out.
    rest_at(&mut rt, 300., 3.);
    assert!(!has(&rt, "toolbar"));
    std::fs::remove_file(&rt.state.settings_path).ok();

    // Cards and the sidebar keep the outer margin from the window's top edge.
    let (mut ws, _host) = tabbed_workspace("split");
    ws.settings.interface.toolbar = ToolbarBehavior::Hidden;
    ws.sidebar.toggle();
    let mut rt = runtime(ws);
    settle(&mut rt);
    let (left, right) = (rect(&rt, "pane:block:b"), rect(&rt, "pane:block:c"));
    assert_eq!((left[1], right[1]), (MARGIN, MARGIN));
    assert_eq!(left[1] + left[3], 700. - MARGIN, "and from its bottom edge");
    assert_eq!(rect(&rt, "sidebar")[1], 0.);
    let first = rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting && e.text.is_some())
        .map(|e| e.rect[1])
        .fold(f32::MAX, f32::min);
    assert!(
        first >= MARGIN,
        "nothing is drawn against the edge: {first}"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();

    // Compact's flush panes go edge to edge.
    let (mut ws, _host) = tabbed_workspace("split");
    ws.settings.interface.toolbar = ToolbarBehavior::Hidden;
    ws.settings.pane_density = PaneDensity::Compact;
    let mut rt = runtime(ws);
    settle(&mut rt);
    assert_eq!(rect(&rt, "pane:block:b")[1], 0.);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn on_hover_floats_the_toolbar_over_the_top_edge_without_moving_a_terminal() {
    let (mut rt, host, sent) = drawn("single", ToolbarBehavior::OnHover);
    assert!(!has(&rt, "toolbar"));
    assert_eq!(rt.state.area, [0., 0., 1024., 700.]);
    assert_eq!(rect(&rt, "toolbar-band"), [0., 0., 1024., 8.]);
    let (pane, grid) = (rect(&rt, "pane:block:a"), rect(&rt, "grid:block:a"));
    let x = 300.;

    // Below the edge: nothing.
    rest_at(&mut rt, x, 40.);
    assert!(!has(&rt, "toolbar"));
    // A pass over the edge on the way elsewhere: nothing either.
    rt.pointer_move(x, 4.);
    rt.frame(1. / 60.);
    rt.pointer_move(x, 90.);
    settle(&mut rt);
    assert!(!has(&rt, "toolbar"), "too quick to mean it");

    // Resting on the top 8 pt brings it out, within 150 ms.
    rt.pointer_move(x, 4.);
    for _ in 0..9 {
        rt.frame(1. / 60.);
    }
    assert!(has(&rt, "toolbar"), "revealed");
    assert_eq!(toolbar_reveal::placement(&rt.state), Placement::Floating);
    settle(&mut rt);
    assert_eq!(rect(&rt, "toolbar-float"), [0., 0., 1024., TOOLBAR]);
    assert_eq!(rect(&rt, "toolbar"), [0., 0., 1024., TOOLBAR]);
    assert_eq!(rect(&rt, "pane:block:a"), pane, "the pane keeps its place");
    assert_eq!(rect(&rt, "grid:block:a"), grid, "and the terminal its rows");
    assert_eq!(rt.state.area, [0., 0., 1024., 700.]);
    assert!(sent_verbs(&sent).is_empty() && requests(&host).is_empty());

    // On the toolbar itself it stays, and its controls work.
    assert_eq!(rect(&rt, "toolbar-band")[3], TOOLBAR);
    let settings = rect(&rt, "settings");
    rest_at(&mut rt, settings[0] + 10., settings[1] + 26.);
    assert!(has(&rt, "toolbar"));
    click(&mut rt, "settings");
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));
    press(&mut rt, Key::Escape);

    // Leaving it puts it away.
    rt.pointer_move(x, 200.);
    settle(&mut rt);
    assert!(!has(&rt, "toolbar"), "gone again");
    assert!(!rt.state.toolbar_reveal.hover);
    assert_eq!(rect(&rt, "toolbar-band")[3], 8.);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_hover_edge_leaves_presses_to_the_terminal() {
    let (mut ws, _host) = tabbed_workspace("single");
    ws.settings.interface.toolbar = ToolbarBehavior::OnHover;
    // No padding: the terminal's first row is under the edge.
    ws.settings.pane_padding = 0.;
    let (sender, sent) = flume::unbounded();
    framed_terminal(&mut ws, "block:a", &sender, b"first line\r\nsecond");
    let mut rt = runtime(ws);
    settle(&mut rt);
    assert_eq!(rect(&rt, "grid:block:a")[1], 0.);
    // Inside the edge, under the 6 pt that resize the window.
    rt.pointer_move(40., 7.);
    rt.pointer_down(40., 7.);
    rt.pointer_up(40., 7.);
    rt.frame(1. / 60.);
    assert!(
        sent_verbs(&sent)
            .iter()
            .any(|(block, verb)| block == "block:a" && verb.starts_with("selection Down")),
        "the terminal took the press"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn reveal_toolbar_floats_it_from_the_keyboard_to_move_or_close_the_window() {
    let spec = actions::find("client.toolbar.toggle").expect("registered");
    assert_eq!(spec.title(), "Reveal toolbar");
    assert_eq!(spec.category(), "Interface");
    assert!(matches!(spec.remote(), actions::Remote::Never(_)));
    assert!(spec.accepts(&Value::Null) && !spec.accepts(&serde_json::json!({"x": 1})));
    assert!(!spec.repeats() && !spec.global());
    let keys: Vec<_> = rex_linux::keymap::defaults()
        .bindings
        .into_iter()
        .filter(|b| b.action == "client.toolbar.toggle")
        .map(|b| b.keys)
        .collect();
    assert_eq!(keys, [["ctrl+alt+shift+t"]], "one default key");

    for toolbar in [ToolbarBehavior::Hidden, ToolbarBehavior::OnHover] {
        let (mut rt, host, sent) = drawn("single", toolbar);
        assert_eq!(
            rt.state
                .shortcut_detail("client.toolbar.toggle", &Value::Null),
            "Ctrl Alt Shift T"
        );
        let (area, grid) = (rt.state.area, rect(&rt, "grid:block:a"));
        let before = std::fs::read(&rt.state.settings_path).ok();
        assert_eq!(
            toolbar_command(&mut rt),
            ("Reveal toolbar".into(), true),
            "{toolbar:?}"
        );

        reveal_key(&mut rt);
        assert!(rt.state.toolbar_reveal.pinned, "{toolbar:?}");
        assert_eq!(rect(&rt, "toolbar-float"), [0., 0., 1024., TOOLBAR]);
        assert_eq!(rt.state.area, area, "{toolbar:?}: the panes keep the row");
        assert_eq!(rect(&rt, "grid:block:a"), grid, "{toolbar:?}: same rows");
        assert_eq!(toolbar_command(&mut rt), ("Hide toolbar".into(), true));
        // It stays with the pointer anywhere, until it is run again.
        rest_at(&mut rt, 300., 400.);
        assert!(has(&rt, "toolbar"));
        // The window's drag space and Close are in it.
        let drag = rect(&rt, "title-drag");
        assert!(drag[2] >= 48. && drag[3] == TOOLBAR, "{drag:?}");
        assert!(rt.shell_request.is_none());
        click(&mut rt, "close-app");
        assert!(rt.shell_request.take().is_some(), "Close closes the window");

        reveal_key(&mut rt);
        assert!(!has(&rt, "toolbar"), "{toolbar:?}: put back");
        assert!(!rt.state.toolbar_reveal.pinned);
        assert!(sent_verbs(&sent).is_empty() && requests(&host).is_empty());
        assert_eq!(
            std::fs::read(&rt.state.settings_path).ok(),
            before,
            "nothing was saved"
        );
        std::fs::remove_file(&rt.state.settings_path).ok();
    }

    // Always has nothing to bring out; its key is still the app's, so it reaches no program.
    let (mut rt, _host, sent) = drawn("single", ToolbarBehavior::Always);
    reveal_key(&mut rt);
    assert!(!rt.state.toolbar_reveal.pinned);
    assert_eq!(toolbar_reveal::placement(&rt.state), Placement::Row);
    assert_eq!(toolbar_command(&mut rt), ("Reveal toolbar".into(), false));
    assert!(sent.try_iter().next().is_none());
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_waiting_key_sequence_still_shows_with_no_toolbar_to_hold_it() {
    use rune::{PhysicalInput, PhysicalKeyCode as Code};
    let (mut ws, _host) = tabbed_workspace("single");
    ws.settings.interface.toolbar = ToolbarBehavior::Hidden;
    ws.settings.shortcut_file.overrides = rex_linux::keymap::Overrides::parse(
        &serde_json::json!([{"keys":"ctrl+b>c","action":"client.tab.new"}]),
    );
    ws.local_keymap = actions::resolve(&ws.settings.shortcut_file.overrides);
    ws.keymap_generation += 1;
    let mut rt = runtime(ws);
    assert!(!has(&rt, "pending-sequence"));
    let mods = Mods {
        ctrl: true,
        ..Mods::default()
    };
    rt.physical_key_input(
        PhysicalInput::Key {
            key: Code::KeyB,
            mods,
            pressed: true,
            repeat: false,
        },
        KeyEvent {
            key: Key::Command('b'),
            mods,
        },
        None,
    );
    settle(&mut rt);
    assert_eq!(rt.state.bindings.sequence.depth, 1);
    assert!(!has(&rt, "toolbar"));
    let chip = rect(&rt, "pending-sequence-float");
    assert_eq!((chip[0], chip[1]), (MARGIN, 8.));
    assert!(has(&rt, "pending-sequence"));
    press(&mut rt, Key::Escape);
    settle(&mut rt);
    assert!(!has(&rt, "pending-sequence"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_toolbar_setting_saves_each_choice_and_a_relaunch_keeps_it() {
    let mut rt = interface_settings();
    // The window's settings are the page's last group.
    to_the_foot(&mut rt);
    let label = |rt: &Runtime<Workspace>| {
        rt.elements()
            .into_iter()
            .find(|e| !e.exiting && e.key.as_deref() == Some("toolbar-behavior"))
            .and_then(|e| e.label)
    };
    assert_eq!(label(&rt).as_deref(), Some("Always show"));
    let note = element_text(&rt, "toolbar-reveal-note").join(" ");
    assert!(
        note.contains("Reveal toolbar (Ctrl Alt Shift T, and in Commands)"),
        "the way back is named from the keymap: {note}"
    );
    for (choice, toolbar) in [
        ("Hidden", ToolbarBehavior::Hidden),
        ("On hover", ToolbarBehavior::OnHover),
        ("Always show", ToolbarBehavior::Always),
    ] {
        choose(&mut rt, "toolbar-behavior", choice);
        assert_eq!(rt.state.settings.interface.toolbar, toolbar);
        assert_eq!(saved(&rt).toolbar, toolbar);
        assert_eq!(label(&rt).as_deref(), Some(choice));
        assert!(
            matches!(
                rt.state.panel.as_ref().map(|p| &p.kind),
                Some(PanelKind::Settings)
            ),
            "Settings stays open"
        );
        assert_eq!(has(&rt, "toolbar"), toolbar == ToolbarBehavior::Always);
        let dialog = rect(&rt, "dialog");
        assert!(
            dialog[1] >= toolbar_reveal::row_height(&rt.state) && dialog[1] + dialog[3] <= 1000.,
            "{choice}: the dialog is whole, clear of the toolbar's row: {dialog:?}"
        );
    }
    // A choice puts away a toolbar that was revealed by hand.
    choose(&mut rt, "toolbar-behavior", "Hidden");
    rt.update(|s, _| s.toggle_toolbar());
    assert!(rt.state.toolbar_reveal.pinned);
    choose(&mut rt, "toolbar-behavior", "On hover");
    assert!(!rt.state.toolbar_reveal.pinned);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn notices_keep_to_the_corner_whether_or_not_the_toolbar_is_there() {
    for (toolbar, top) in [
        (ToolbarBehavior::Always, TOOLBAR + 4.),
        (ToolbarBehavior::Hidden, MARGIN),
    ] {
        let (mut rt, _host, _sent) = drawn("single", toolbar);
        rt.update(|s, _| s.inform("Copied"));
        settle(&mut rt);
        assert_eq!(rect(&rt, "toasts")[1], top, "{toolbar:?}");
        if toolbar == ToolbarBehavior::Hidden {
            rt.update(|s, _| s.toggle_toolbar());
            settle(&mut rt);
            assert_eq!(rect(&rt, "toasts")[1], TOOLBAR + 4., "under a floating one");
        }
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}

#[test]
fn animation_style_sets_the_springs_and_off_reduces_motion() {
    let mut rt = interface_settings();
    assert!(!rt.reduced_motion);
    assert_eq!(theme::snappy().duration, theme::SNAPPY.duration);
    let segment = |rt: &Runtime<Workspace>, label: &str| {
        let group = rt
            .elements()
            .into_iter()
            .find(|e| !e.exiting && e.key.as_deref() == Some("animation-style"))
            .expect("the control");
        rt.elements()
            .into_iter()
            .find(|e| {
                !e.exiting
                    && e.role == "radio"
                    && e.label.as_deref() == Some(label)
                    && e.path.starts_with(&group.path)
            })
            .unwrap_or_else(|| panic!("no {label} segment"))
    };
    assert!(segment(&rt, "Snappy").focusable);
    let pick = |rt: &mut Runtime<Workspace>, label: &str| {
        let r = segment(rt, label).rect;
        rt.pointer_down(r[0] + r[2] / 2., r[1] + r[3] / 2.);
        rt.pointer_up(r[0] + r[2] / 2., r[1] + r[3] / 2.);
        settle(rt);
    };

    pick(&mut rt, "Natural");
    assert_eq!(
        rt.state.settings.interface.animation,
        AnimationStyle::Natural
    );
    assert_eq!(saved(&rt).animation, AnimationStyle::Natural);
    assert_eq!(theme::snappy().duration, theme::NATURAL.duration);
    assert_eq!(theme::pop().bounce, theme::NATURAL_POP.bounce);
    assert!(!rt.reduced_motion, "it still moves");

    pick(&mut rt, "Off");
    assert_eq!(saved(&rt).animation, AnimationStyle::Off);
    assert!(rt.reduced_motion, "Rune stops travel for the whole window");

    pick(&mut rt, "Snappy");
    assert_eq!(saved(&rt).animation, AnimationStyle::Snappy);
    assert!(!rt.reduced_motion);
    assert_eq!(theme::snappy().duration, theme::SNAPPY.duration);
    assert_eq!(theme::pop().bounce, theme::POP.bounce);
    assert!(
        matches!(
            rt.state.panel.as_ref().map(|p| &p.kind),
            Some(PanelKind::Settings)
        ),
        "Settings stays open"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn off_lands_a_moved_card_at_once_and_the_other_styles_spring_it() {
    // A divider preview moves the cards: where the left one's edge is a frame later.
    let after_a_frame = |animation: AnimationStyle| {
        let (mut ws, _host) = tabbed_workspace("split");
        ws.settings.interface.animation = animation;
        let mut rt = runtime(ws);
        settle(&mut rt);
        let before = rect(&rt, "pane:block:b")[2];
        rt.update(|s, _| {
            let view = s.hosts[0].model.views.get_mut("s").unwrap();
            let layout = serde_json::json!({"split": {"direction": "horizontal", "ratio": 0.3,
                "before": {"block_id": "block:b"}, "after": {"block_id": "block:c"}}});
            view.windows[1].layers[0].layout = serde_json::from_value(layout).unwrap();
        });
        rt.frame(1. / 60.);
        let now = rect(&rt, "pane:block:b")[2];
        settle(&mut rt);
        let end = rect(&rt, "pane:block:b")[2];
        std::fs::remove_file(&rt.state.settings_path).ok();
        assert!(end < before - 100., "the card got narrower");
        ((now - end).abs(), before - end)
    };
    let (left, travel) = after_a_frame(AnimationStyle::Off);
    assert!(
        left < 0.01,
        "Off: there at once, {left} of {travel} pt to go"
    );
    let (snappy, _) = after_a_frame(AnimationStyle::Snappy);
    let (natural, _) = after_a_frame(AnimationStyle::Natural);
    assert!(snappy > 1., "Snappy is still on its way: {snappy}");
    assert!(
        natural > snappy,
        "Natural takes longer: {natural} against {snappy}"
    );
}

#[test]
fn padding_is_set_each_way_on_the_panes_page_and_saved() {
    let mut rt = interface_settings();
    assert!(!has(&rt, "pane-padding-x-smaller"));
    tap(&mut rt, "customize-panes");
    assert_eq!(
        rt.state.settings_page(),
        crate::app::settings_nav::Page::Sub(crate::app::settings_nav::SubPage::Panes)
    );
    assert!(
        focused(&rt).is_some_and(|path| path.contains("settings-back")),
        "the keyboard is on the way back"
    );
    assert!(
        rt.elements()
            .iter()
            .any(|e| !e.exiting && e.text.as_deref() == Some("Panes")),
        "the page is titled"
    );
    assert_eq!(rt.state.settings.padding(), (8., 8.));
    for _ in 0..2 {
        click(&mut rt, "pane-padding-x-larger");
        click(&mut rt, "pane-padding-y-smaller");
    }
    assert_eq!(rt.state.settings.padding(), (10., 6.));
    assert_eq!(relaunch(&rt.state).settings.padding(), (10., 6.));
    let shown = |rt: &Runtime<Workspace>, value: &str| {
        rt.elements()
            .iter()
            .any(|e| !e.exiting && e.text.as_deref() == Some(value))
    };
    assert!(shown(&rt, "10 pt") && shown(&rt, "6 pt"));
    for _ in 0..9 {
        click(&mut rt, "pane-padding-y-smaller");
    }
    assert_eq!(relaunch(&rt.state).settings.padding(), (10., 0.));

    // Back, and Escape, return to Interface with the keyboard on the row that opened it.
    tap(&mut rt, "settings-back");
    assert_eq!(rt.state.settings_section, SettingsSection::Interface);
    assert!(focused(&rt).is_some_and(|path| path.contains("customize-panes")));
    tap(&mut rt, "customize-panes");
    press(&mut rt, Key::Escape);
    settle(&mut rt);
    assert_eq!(rt.state.settings_section, SettingsSection::Interface);
    assert!(
        rt.state.panel.is_some(),
        "Escape left the page, not Settings"
    );

    // Terminal no longer has it.
    tap(&mut rt, "settings-section:terminal");
    to_the_foot(&mut rt);
    assert!(!has(&rt, "pane-padding-smaller") && !has(&rt, "pane-padding-x-smaller"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn padding_each_way_places_every_grid_and_one_value_is_still_all_four_sides() {
    for (tab, density) in [
        ("single", PaneDensity::Comfortable),
        ("split", PaneDensity::Comfortable),
        ("split", PaneDensity::Compact),
    ] {
        let (mut ws, _host) = tabbed_workspace(tab);
        ws.settings.pane_density = density;
        ws.settings.pane_header = PaneHeader::Hidden;
        ws.settings.pane_padding = 0.;
        let (sender, _sent) = flume::unbounded();
        let blocks: &[&str] = if tab == "single" {
            &["block:a"]
        } else {
            &["block:b", "block:c"]
        };
        for block in blocks {
            live_terminal(&mut ws, block, &sender);
        }
        let mut rt = runtime(ws);
        settle(&mut rt);
        let grids = |rt: &Runtime<Workspace>| -> Vec<[f32; 4]> {
            blocks
                .iter()
                .map(|block| rect(rt, &format!("grid:{block}")))
                .collect()
        };
        let bare = grids(&rt);
        // A file from before the two could differ: one value, all four sides.
        rt.update(|s, _| s.settings.pane_padding = 8.);
        settle(&mut rt);
        for (padded, bare) in grids(&rt).iter().zip(&bare) {
            assert_eq!(
                *padded,
                [bare[0] + 8., bare[1] + 8., bare[2] - 16., bare[3] - 16.],
                "{tab} {density:?}"
            );
        }
        rt.update(|s, _| {
            s.settings.pane_padding = 12.;
            s.settings.interface.pane_padding_vertical = Some(4.);
        });
        settle(&mut rt);
        for ((padded, bare), block) in grids(&rt).iter().zip(&bare).zip(blocks) {
            assert_eq!(
                *padded,
                [bare[0] + 12., bare[1] + 4., bare[2] - 24., bare[3] - 8.],
                "{tab} {density:?}"
            );
            assert_eq!(rt.state.terminals[&rt.state.term_key(block)].rect, *padded);
        }
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}

#[test]
fn the_active_pane_outline_follows_the_density_until_its_switch_is_pressed() {
    let outlined = |tab: &str, density: PaneDensity, chosen: Option<bool>| {
        let (mut ws, _host) = tabbed_workspace(tab);
        ws.settings.pane_density = density;
        ws.settings.interface.highlight_active_pane = chosen;
        let flat = density == PaneDensity::Compact || tab == "single";
        let on = pane_cues::outlines(&ws, ws.model().window().unwrap(), flat);
        std::fs::remove_file(&ws.settings_path).ok();
        on
    };
    use PaneDensity::{Comfortable, Compact};
    // As before the switch: cards are outlined, flush panes are not.
    assert!(outlined("split", Comfortable, None));
    assert!(!outlined("split", Compact, None));
    assert!(!outlined("single", Comfortable, None));
    // Chosen, it is the same in either density.
    assert!(!outlined("split", Comfortable, Some(false)));
    assert!(outlined("split", Compact, Some(true)));
    assert!(outlined("split", Comfortable, Some(true)));
    // A tab's only pane, edge to edge, has no other to be told from.
    assert!(!outlined("single", Compact, Some(true)));
    assert!(!outlined("single", Comfortable, Some(true)));

    let mut rt = interface_settings();
    tap(&mut rt, "customize-panes");
    assert!(rt.state.settings.highlights_active_pane(), "Comfortable");
    tap(&mut rt, "highlight-active-pane");
    assert_eq!(
        rt.state.settings.interface.highlight_active_pane,
        Some(false)
    );
    assert_eq!(saved(&rt).highlight_active_pane, Some(false));
    tap(&mut rt, "highlight-active-pane");
    assert_eq!(saved(&rt).highlight_active_pane, Some(true));
    // Now a choice, it stays on in Compact.
    rt.update(|s, _| s.settings.pane_density = Compact);
    assert!(rt.state.settings.highlights_active_pane());
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// The opacity the element keyed `key` is drawn at.
fn opacity(rt: &Runtime<Workspace>, key: &str) -> f32 {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing {key}"))
        .opacity
}

/// The opacity of the name in `block`'s header.
fn name_opacity(rt: &Runtime<Workspace>, block: &str) -> f32 {
    let header = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(&format!("pane-header:{block}")))
        .unwrap_or_else(|| panic!("no header on {block}"));
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.text.is_some() && e.path.starts_with(&header.path))
        .expect("its name")
        .opacity
}

#[test]
fn headers_fade_on_panes_without_the_keyboard_and_content_never_does() {
    use rex_linux::connection::activity::{BlockedKind, ProgramState::*};
    let scene = |inactive: bool, window: bool| {
        let (mut ws, _host) = tabbed_workspace("split");
        ws.settings.interface.fade_inactive_pane_headers = inactive;
        ws.settings.interface.fade_pane_headers_in_inactive_window = window;
        let (sender, _sent) = flume::unbounded();
        framed_terminal(&mut ws, "block:b", &sender, b"$ cargo test");
        framed_terminal(&mut ws, "block:c", &sender, b"$ claude");
        let mut blocked = record(Blocked, true);
        blocked.kind = Some(BlockedKind::Question);
        ws.hosts[0]
            .model
            .activity
            .insert("block:c".into(), [("".into(), blocked)].into());
        let mut rt = runtime(ws);
        settle(&mut rt);
        rt
    };
    let whole = |rt: &Runtime<Workspace>, block: &str| {
        (name_opacity(rt, block) - 1.).abs() < 0.01
            && (opacity(rt, &format!("pane-actions:{block}")) - 1.).abs() < 0.01
    };
    let faded = |rt: &Runtime<Workspace>, block: &str| {
        (name_opacity(rt, block) - pane_cues::FADE).abs() < 0.01
            && (opacity(rt, &format!("pane-actions:{block}")) - pane_cues::FADE).abs() < 0.01
    };
    let content_whole = |rt: &Runtime<Workspace>| {
        ["block:b", "block:c"].iter().all(|block| {
            opacity(rt, &format!("grid:{block}")) == 1.
                && opacity(rt, &format!("pane:{block}")) == 1.
        }) && opacity(rt, "pane-word:block:c") == 1.
    };

    // Off, as before: nothing fades, focused or not, active window or not.
    let mut rt = scene(false, false);
    assert!(whole(&rt, "block:b") && whole(&rt, "block:c"));
    rt.update(|s, _| s.window_focus(false));
    settle(&mut rt);
    assert!(whole(&rt, "block:b") && whole(&rt, "block:c") && content_whole(&rt));
    std::fs::remove_file(&rt.state.settings_path).ok();

    // Inactive panes: the pane without the keyboard, and it follows the focus.
    let mut rt = scene(true, false);
    assert_eq!(rt.state.focused_block().as_deref(), Some("block:b"));
    assert!(whole(&rt, "block:b") && faded(&rt, "block:c"));
    assert!(
        content_whole(&rt),
        "its terminal and its activity word stay"
    );
    rt.update(|s, _| {
        s.hosts[0].model.views.get_mut("s").unwrap().windows[1].focused_block_id =
            Some("block:c".into());
    });
    settle(&mut rt);
    assert!(faded(&rt, "block:b") && whole(&rt, "block:c"));
    // The window going behind another changes nothing with only this switch.
    rt.update(|s, _| s.window_focus(false));
    settle(&mut rt);
    assert!(faded(&rt, "block:b") && whole(&rt, "block:c"));
    std::fs::remove_file(&rt.state.settings_path).ok();

    // Inactive windows: every header, only while the window is not the active one.
    let mut rt = scene(false, true);
    assert!(whole(&rt, "block:b") && whole(&rt, "block:c"));
    rt.update(|s, _| s.window_focus(false));
    settle(&mut rt);
    assert!(faded(&rt, "block:b") && faded(&rt, "block:c") && content_whole(&rt));
    rt.update(|s, _| s.window_focus(true));
    settle(&mut rt);
    assert!(whole(&rt, "block:b") && whole(&rt, "block:c"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// What a channel was sent, as words: `session.focus_block block:c`, `claim`, `terminal`.
fn sent_words(rx: &flume::Receiver<HostCommand>) -> Vec<String> {
    rx.try_iter()
        .map(|command| match command {
            HostCommand::Request { method, payload } => {
                format!("{method} {}", payload["block_id"].as_str().unwrap_or("-"))
            }
            HostCommand::Claim { .. } => "claim".into(),
            HostCommand::Terminal { .. } => "terminal".into(),
            _ => "other".into(),
        })
        .collect()
}

#[test]
fn focus_follows_mouse_asks_for_focus_only_and_never_claims_a_pane() {
    let scene = |on: bool| {
        let (mut ws, host) = tabbed_workspace("split");
        ws.settings.interface.focus_follows_mouse = on;
        let (sender, sent) = flume::unbounded();
        live_terminal(&mut ws, "block:b", &sender);
        live_terminal(&mut ws, "block:c", &sender);
        let mut rt = runtime(ws);
        settle(&mut rt);
        let _ = (sent_words(&host), sent_words(&sent));
        (rt, host, sent)
    };
    let centre = |rt: &Runtime<Workspace>, block: &str| {
        let r = rect(rt, &format!("grid:{block}"));
        (r[0] + r[2] / 2., r[1] + r[3] / 2.)
    };

    // Off, as before: the pointer focuses nothing.
    let (mut rt, host, sent) = scene(false);
    let (x, y) = centre(&rt, "block:c");
    rest_at(&mut rt, x, y);
    assert!(sent_words(&sent).is_empty() && sent_words(&host).is_empty());
    std::fs::remove_file(&rt.state.settings_path).ok();

    let (mut rt, host, sent) = scene(true);
    assert_eq!(rt.state.focused_block().as_deref(), Some("block:b"));
    let (bx, by) = centre(&rt, "block:b");
    let (cx, cy) = centre(&rt, "block:c");
    // Resting on the pane that has the keyboard asks for nothing.
    rest_at(&mut rt, bx, by);
    assert!(sent_words(&sent).is_empty());
    // A pass over the other pane on the way out leaves it alone.
    rt.pointer_move(cx, cy);
    rt.frame(1. / 60.);
    rt.pointer_move(bx, by);
    settle(&mut rt);
    assert!(sent_words(&sent).is_empty(), "too quick to mean it");

    // Resting there focuses it: one request, the one a press on its header sends.
    rest_at(&mut rt, cx, cy);
    assert_eq!(sent_words(&sent), ["session.focus_block block:c"]);
    assert!(
        sent_words(&host).is_empty(),
        "no session.own_block (a Claim), no resize and no key"
    );
    assert!(rt.state.claimed.is_empty(), "no pane was claimed");
    // The server moves the focus; the pointer still resting there asks for nothing more.
    rt.update(|s, _| {
        s.hosts[0].model.views.get_mut("s").unwrap().windows[1].focused_block_id =
            Some("block:c".into());
    });
    rest_at(&mut rt, cx + 3., cy);
    assert!(sent_words(&sent).is_empty());
    // Back again.
    rest_at(&mut rt, bx, by);
    assert_eq!(sent_words(&sent), ["session.focus_block block:b"]);

    // Not while a menu is open, a divider is dragged or the window is behind another.
    rt.update(|s, _| s.picker(false));
    settle(&mut rt);
    rt.update(|s, _| s.hover_focus("block:b"));
    assert!(sent_words(&sent).is_empty(), "a panel is open");
    press(&mut rt, Key::Escape);
    rt.update(|s, _| {
        s.window_focus(false);
        s.hover_focus("block:b");
    });
    assert!(sent_words(&sent).is_empty(), "the window is not active");
    rt.update(|s, _| {
        s.window_focused = true;
        s.hover_focus("block:b");
    });
    assert_eq!(sent_words(&sent), ["session.focus_block block:b"]);
    assert!(
        sent_words(&host).iter().all(|word| word != "claim"),
        "hover never claimed"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_panes_page_switches_and_show_window_on_launch_save() {
    let mut rt = interface_settings();
    to_the_foot(&mut rt);
    assert!(rt.state.settings.interface.show_window_on_launch);
    tap(&mut rt, "show-window-on-launch");
    assert!(!saved(&rt).show_window_on_launch);
    tap(&mut rt, "show-window-on-launch");
    assert!(saved(&rt).show_window_on_launch);

    rt.pointer_move(900., 500.);
    rt.wheel(900., 500., 4000.);
    settle(&mut rt);
    tap(&mut rt, "customize-panes");
    type Field = fn(&InterfaceConfig) -> bool;
    let switches: [(&str, Field); 3] = [
        ("fade-inactive-headers", |i| i.fade_inactive_pane_headers),
        ("fade-headers-inactive-window", |i| {
            i.fade_pane_headers_in_inactive_window
        }),
        ("focus-follows-mouse", |i| i.focus_follows_mouse),
    ];
    for (key, field) in switches {
        assert!(!field(&rt.state.settings.interface), "{key} starts off");
        tap(&mut rt, key);
        assert!(field(&saved(&rt)), "{key}");
        tap(&mut rt, key);
        assert!(!field(&saved(&rt)), "{key}");
    }
    assert_eq!(saved(&rt), InterfaceConfig::default());
    assert!(
        matches!(
            rt.state.panel.as_ref().map(|p| &p.kind),
            Some(PanelKind::Settings)
        ),
        "Settings stays open"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// The switches as drawn, for a look (see `Camera`).
#[test]
fn interface_switches_draw() {
    use rex_linux::connection::activity::{BlockedKind, ProgramState::*};
    let Some(mut camera) = Camera::new() else {
        return;
    };
    let scene = |active: &str, edit: &dyn Fn(&mut Workspace)| {
        let (mut ws, _host) = tabbed_workspace(active);
        let (sender, _sent) = flume::unbounded();
        framed_terminal(
            &mut ws,
            "block:a",
            &sender,
            b"$ tail -f server.log\r\nlistening on :8080\r\nGET /health 200",
        );
        framed_terminal(
            &mut ws,
            "block:b",
            &sender,
            b"$ cargo test\r\nrunning 12 tests\r\ntest result: ok",
        );
        framed_terminal(&mut ws, "block:c", &sender, b"$ claude\r\n> waiting");
        let mut blocked = record(Blocked, true);
        blocked.kind = Some(BlockedKind::Question);
        ws.hosts[0]
            .model
            .activity
            .insert("block:c".into(), [("".into(), blocked)].into());
        edit(&mut ws);
        let mut rt = Runtime::new(ws, view, (1024., 700.), Env::new(true, 2.));
        rt.on_key = Some(crate::app::input::on_key);
        settle(&mut rt);
        rt
    };
    let hidden = |s: &mut Workspace| s.settings.interface.toolbar = ToolbarBehavior::Hidden;

    let mut rt = scene("split", &|_| {});
    rt.update(|s, tx| s.perform(Intent::Settings, tx));
    settle(&mut rt);
    camera.snap(&mut rt, "interface-page");
    to_the_foot(&mut rt);
    settle(&mut rt);
    camera.snap(&mut rt, "interface-page-foot");
    tap(&mut rt, "toolbar-behavior");
    camera.snap(&mut rt, "interface-toolbar-list");
    press(&mut rt, Key::Escape);
    rt.pointer_move(900., 400.);
    rt.wheel(900., 400., 4000.);
    settle(&mut rt);
    tap(&mut rt, "customize-panes");
    camera.snap(&mut rt, "interface-panes-page");
    std::fs::remove_file(&rt.state.settings_path).ok();

    let mut rt = scene("single", &hidden);
    camera.snap(&mut rt, "toolbar-hidden-single");
    rt.update(|s, _| s.toggle_toolbar());
    settle(&mut rt);
    camera.snap(&mut rt, "toolbar-revealed-single");
    std::fs::remove_file(&rt.state.settings_path).ok();

    let mut rt = scene("split", &hidden);
    camera.snap(&mut rt, "toolbar-hidden-split");
    rt.update(|s, _| s.toggle_toolbar());
    settle(&mut rt);
    camera.snap(&mut rt, "toolbar-revealed-split");
    rt.update(|s, _| {
        s.toggle_toolbar();
        s.inform("Copied the pane ID");
    });
    settle(&mut rt);
    camera.snap(&mut rt, "toolbar-hidden-notice");
    std::fs::remove_file(&rt.state.settings_path).ok();

    let mut rt = scene("split", &|s| {
        hidden(s);
        s.sidebar.toggle();
    });
    camera.snap(&mut rt, "toolbar-hidden-sidebar");
    std::fs::remove_file(&rt.state.settings_path).ok();

    let mut rt = scene("split", &|s| {
        s.settings.pane_density = PaneDensity::Compact;
        s.settings.interface.highlight_active_pane = Some(true);
    });
    camera.snap(&mut rt, "outline-compact");
    std::fs::remove_file(&rt.state.settings_path).ok();

    let mut rt = scene("split", &|s| {
        s.settings.interface.highlight_active_pane = Some(false);
        s.settings.interface.fade_inactive_pane_headers = true;
    });
    camera.snap(&mut rt, "outline-off-headers-faded");
    std::fs::remove_file(&rt.state.settings_path).ok();

    let mut rt = scene("split", &|s| {
        s.settings.pane_padding = 24.;
        s.settings.interface.pane_padding_vertical = Some(2.);
    });
    camera.snap(&mut rt, "padding-24-by-2");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_tab_switch_never_fades_the_old_panes_out_over_the_new_one() {
    for animation in [
        AnimationStyle::Snappy,
        AnimationStyle::Natural,
        AnimationStyle::Off,
    ] {
        let (mut ws, _host) = tabbed_workspace("split");
        ws.settings.interface.animation = animation;
        let (sender, _sent) = flume::unbounded();
        for block in ["block:a", "block:b", "block:c"] {
            framed_terminal(&mut ws, block, &sender, b"$ make\r\nok");
        }
        let mut rt = runtime(ws);
        settle(&mut rt);
        rt.update(|s, _| {
            s.hosts[0]
                .model
                .views
                .get_mut("s")
                .unwrap()
                .active_window_id = Some("single".into());
        });
        rt.frame(1. / 60.);
        // Reduced motion turns a very short spring into a fade; the cards must not take it.
        let lingering: Vec<_> = rt
            .elements()
            .into_iter()
            .filter_map(|e| e.key)
            .filter(|key| key.starts_with("pane:") || key.starts_with("grid:"))
            .filter(|key| !key.ends_with("block:a"))
            .collect();
        assert!(lingering.is_empty(), "{animation:?}: {lingering:?}");
        assert!(has(&rt, "pane:block:a"));
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}
