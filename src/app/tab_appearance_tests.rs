//! Tab appearance: tabs that fill the toolbar, how many program icons a tab shows, which of
//! them are in colour, a shell's tile, and the Tabs page of Settings with its live preview.
use super::*;
use crate::app::{
    settings_nav::{Page, SubPage},
    tab_icons::tint,
    theme::{CLAUDE, VIM, accent, ink, muted, rgb},
};
use rex_linux::settings::{ShellIconTheme, TabIconColor, TabsConfig};
use rune::Color;
use serde_json::{Value, json};

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..90 {
        rt.frame(1. / 60.);
    }
}

fn rect(rt: &Runtime<Workspace>, key: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing {key}"))
        .rect
}

/// The tab "split" of [`tabbed_workspace`] with a third pane: claude in the focused pane
/// (block:b), nvim beside it, and a pane whose program isn't known. The tab "single" runs
/// a shell.
fn programs(active: &str) -> (Workspace, flume::Receiver<HostCommand>) {
    let (mut ws, rx) = tabbed_workspace(active);
    let model = &mut ws.hosts[0].model;
    let view = model.views.get_mut("s").unwrap();
    view.windows[1].layers[0]
        .blocks
        .push(serde_json::from_value(json!({"block_id": "block:d"})).unwrap());
    model.programs.insert("block:a".into(), "zsh".into());
    model.programs.insert("block:b".into(), "claude".into());
    model.programs.insert("block:c".into(), "nvim".into());
    (ws, rx)
}

fn with_tabs(ws: &mut Workspace, tabs: TabsConfig) {
    ws.settings.tabs = tabs;
}

/// The colour of the glyph in the icon cell keyed `key`, and whether it is on a shell's
/// tile: one drawn under it, or for a tab's first icon its badge in other than the plain
/// 6% white.
fn icon(rt: &Runtime<Workspace>, key: &str) -> (Color, bool) {
    let elements = rt.elements();
    let cell = elements
        .iter()
        .position(|e| !e.exiting && e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing {key}"));
    let inside: Vec<_> = elements
        .iter()
        .filter(|e| !e.exiting && e.parent == Some(cell))
        .collect();
    let badge = elements[cell].fill;
    let plain = crate::app::theme::white(0.06);
    let tile = inside.iter().any(|e| e.key.as_deref() == Some("tile"))
        || (badge.a > 0. && !(same(badge, plain) && (badge.a - plain.a).abs() < 1e-4));
    let color = inside
        .iter()
        .find_map(|e| e.text_color)
        .unwrap_or_else(|| panic!("{key} has no glyph"));
    (color, tile)
}

fn same(a: Color, b: Color) -> bool {
    (a.r - b.r).abs() < 1e-4 && (a.g - b.g).abs() < 1e-4 && (a.b - b.b).abs() < 1e-4
}

#[test]
fn by_default_a_tab_shows_its_focused_panes_icon_alone_in_the_tabs_ink() {
    let (ws, _rx) = programs("split");
    assert_eq!(ws.settings.tabs, TabsConfig::default());
    let rt = runtime(ws);
    // One icon each: the focused pane's.
    for gone in ["tab-icon:split:1", "tab-icon:single:1"] {
        assert!(!has(&rt, gone), "{gone}");
    }
    let (on_screen, tile) = icon(&rt, "tab-icon:split");
    assert!(same(on_screen, rgb(ink())) && !tile);
    let (other, tile) = icon(&rt, "tab-icon:single");
    assert!(same(other, rgb(muted())) && !tile);
    // The pill: a 30 pt capsule on the 34 pt track, its icon on a 20 pt badge 6 pt in.
    let (pill, badge) = (rect(&rt, "tab:split"), rect(&rt, "tab-icon:split"));
    assert_eq!(pill[3], 30.);
    assert_eq!(rect(&rt, "tab-track")[3], 34.);
    assert_eq!(badge[0] - pill[0], 6.);
    assert_eq!([badge[2], badge[3]], [20., 20.]);
    assert_eq!(pill[2], 96., "a short name keeps 96 pt");
    assert!((96. ..=240.).contains(&rect(&rt, "tab:single")[2]));
    // The window's drag space takes what the tabs leave.
    let drag = rect(&rt, "title-drag");
    assert!(drag[2] > 300., "{drag:?}");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_tab_shows_as_many_icons_as_the_setting_allows_one_for_each_program() {
    let widths = |icons: u8| {
        let (mut ws, _rx) = programs("split");
        with_tabs(
            &mut ws,
            TabsConfig {
                icons,
                ..TabsConfig::default()
            },
        );
        let rt = runtime(ws);
        let shown: Vec<bool> = ["tab-icon:split", "tab-icon:split:1", "tab-icon:split:2"]
            .iter()
            .map(|key| has(&rt, key))
            .collect();
        let width = rect(&rt, "tab:split")[2];
        std::fs::remove_file(&rt.state.settings_path).ok();
        (shown, width)
    };
    let (none, bare) = widths(0);
    assert_eq!(none, [false, false, false]);
    let (one, single) = widths(1);
    assert_eq!(one, [true, false, false]);
    let (two, double) = widths(2);
    assert_eq!(two, [true, true, false]);
    // Three panes, three different icons; more room than that shows no more.
    let (three, triple) = widths(3);
    assert_eq!(three, [true, true, true]);
    assert_eq!(widths(5), (three, triple));
    // "dev" is short, so the pill is at its 96 pt floor until icons need more; then each
    // icon adds its 15 pt and the 6 pt before it.
    assert_eq!([bare, single], [96., 96.]);
    assert!(double > single);
    assert_eq!(triple - double, 21.);

    // The order is the focused pane's program, then the others in the server's order, and
    // each names its program.
    let (mut ws, _rx) = programs("split");
    ws.settings.tabs.icons = 5;
    let rt = runtime(ws);
    let xs: Vec<f32> = ["tab-icon:split", "tab-icon:split:1", "tab-icon:split:2"]
        .iter()
        .map(|key| rect(&rt, key)[0])
        .collect();
    assert!(xs[0] < xs[1] && xs[1] < xs[2], "{xs:?}");
    assert_eq!(xs[1] - xs[0], 26., "6 pt after the 20 pt badge");
    assert_eq!(xs[2] - xs[1], 21., "15 pt icons 6 pt apart");
    let name = rect(&rt, "tab:split");
    assert!(xs[2] + 15. < name[0] + name[2]);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn each_colour_mode_colours_the_icons_it_names_and_a_shell_gets_its_tile() {
    // (mode, [claude on screen, nvim on screen, the other tab's shell]) in colour or not.
    let mut widths = Vec::new();
    for (mode, expect) in [
        (TabIconColor::Always, [true, true, true]),
        (TabIconColor::ActiveTab, [true, true, false]),
        (TabIconColor::ActivePane, [true, false, false]),
        (TabIconColor::Never, [false, false, false]),
    ] {
        let (mut ws, _rx) = programs("split");
        with_tabs(
            &mut ws,
            TabsConfig {
                icons: 3,
                icon_color: mode,
                ..TabsConfig::default()
            },
        );
        let rt = runtime(ws);
        let (claude, _) = icon(&rt, "tab-icon:split");
        let (nvim, _) = icon(&rt, "tab-icon:split:1");
        let (shell, tile) = icon(&rt, "tab-icon:single");
        assert_eq!(
            [
                same(claude, rgb(tint(&CLAUDE))),
                same(nvim, rgb(tint(&VIM))),
                tile
            ],
            expect,
            "{mode:?}"
        );
        // Out of colour an icon is in its tab's ink, as the name is.
        if !expect[0] {
            assert!(same(claude, rgb(ink())));
        }
        if !expect[1] {
            assert!(same(nvim, rgb(ink())));
        }
        if !expect[2] {
            assert!(same(shell, rgb(muted())));
        }
        widths.push([rect(&rt, "tab:single")[2], rect(&rt, "tab:split")[2]]);
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
    // Colour never moves anything: every pill is as wide in each mode.
    assert!(widths.iter().all(|w| *w == widths[0]), "{widths:?}");
}

#[test]
fn a_shells_tile_takes_the_terminal_themes_colours_or_the_accent() {
    let shell = |theme: ShellIconTheme, named: Option<&str>| {
        let (mut ws, _rx) = programs("split");
        with_tabs(
            &mut ws,
            TabsConfig {
                icon_color: TabIconColor::Always,
                shell_icon: theme,
                ..TabsConfig::default()
            },
        );
        if let Some(id) = named {
            ws.apply_theme(TerminalThemeChoice::Named(id.into()))
                .unwrap();
        }
        let look = ws.tab_icon_look();
        let rt = runtime(ws);
        let (prompt, tile) = icon(&rt, "tab-icon:single");
        assert!(tile);
        assert!(same(prompt, rgb(look.shell.ink)));
        std::fs::remove_file(&rt.state.settings_path).ok();
        look.shell
    };
    let default = shell(ShellIconTheme::Default, None);
    let accented = shell(ShellIconTheme::Accent, None);
    assert_eq!(accented.ink, accent());
    assert_eq!(
        accented.fill, default.fill,
        "the accent is on the same tile"
    );
    assert_ne!(default.ink, accent());
    // Follow server has no colours to take, so the tile stays the default one.
    assert_eq!(shell(ShellIconTheme::Theme, None), default);
    // A named theme gives its background and a prompt that reads on it.
    let light = shell(ShellIconTheme::Theme, Some("rex/merino-light"));
    let theme = rex_linux::terminal_theme::builtin("rex/merino-light")
        .unwrap()
        .theme();
    let background = theme.background.unwrap();
    assert!(same(light.fill, rgb(background)));
    assert!(crate::app::tab_icons::contrast(light.ink, background) >= 3.);
    assert_ne!(light, default);
}

#[test]
fn tabs_that_fill_the_toolbar_share_its_width_and_leave_the_drag_space() {
    let (mut ws, _rx) = programs("split");
    ws.settings.tabs.fill = true;
    let mut rt = runtime(ws);
    settle(&mut rt);
    let (first, second) = (rect(&rt, "tab:single"), rect(&rt, "tab:split"));
    assert!((first[2] - second[2]).abs() < 0.5, "{first:?} {second:?}");
    assert!(first[2] > 240., "wider than a tab sized to its name can be");
    let strip = rect(&rt, "tabs");
    assert!((first[2] + second[2] + 2. - strip[2]).abs() < 0.5);
    assert!(!rt.state.tab_strip.overflows());
    // What is left of the bar to drag the window by, between the tabs and the buttons.
    let (drag, track) = (rect(&rt, "title-drag"), rect(&rt, "tab-track"));
    assert_eq!(drag[2], 48.);
    assert!(drag[0] >= track[0] + track[2]);
    assert!(drag[0] + drag[2] <= rect(&rt, "commands")[0] + 0.5);
    // A tab's content is centred on it.
    let name = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.text.as_deref() == Some("server"))
        .expect("the tab's name")
        .rect;
    let glyph = rect(&rt, "tab-icon:single");
    let (left, right) = (
        glyph[0] - first[0],
        first[0] + first[2] - (name[0] + name[2]),
    );
    assert!((left - right).abs() < 1., "{left} {right}");

    // Listed in the sidebar there is no track to fill, and the bar is all drag space again.
    rt.update(|s, _| s.set_vertical_tabs(true));
    settle(&mut rt);
    assert!(!has(&rt, "tab-track"));
    assert!(rect(&rt, "title-drag")[2] > 300.);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_tab_on_screen_and_a_tab_under_the_pointer_show_a_close_button() {
    let (ws, rx) = programs("single");
    let mut rt = runtime(ws);
    settle(&mut rt);
    let opacity = |rt: &Runtime<Workspace>, key: &str| {
        rt.elements()
            .into_iter()
            .find(|e| !e.exiting && e.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("missing {key}"))
            .opacity
    };
    // On the tab on screen it is always there; on another only under the pointer.
    assert_eq!(opacity(&rt, "tab-close:single"), 1.);
    assert_eq!(opacity(&rt, "tab-close:split"), 0.);
    // In room the pill keeps for it: 18 pt, 6 pt from the pill's end, after the name.
    for tab in ["single", "split"] {
        let (pill, close) = (
            rect(&rt, &format!("tab:{tab}")),
            rect(&rt, &format!("tab-close:{tab}")),
        );
        assert_eq!([close[2], close[3]], [18., 18.]);
        assert_eq!(pill[0] + pill[2] - (close[0] + close[2]), 6.);
        assert_eq!(close[1] - pill[1], 6.);
    }
    let name = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.text.as_deref() == Some("server"))
        .expect("the tab's name")
        .rect;
    assert!(name[0] + name[2] <= rect(&rt, "tab-close:single")[0]);
    let before = rect(&rt, "tab:split");
    let over = (before[0] + 20., before[1] + before[3] / 2.);
    rt.pointer_move(over.0, over.1);
    settle(&mut rt);
    assert_eq!(opacity(&rt, "tab-close:split"), 1.);
    assert_eq!(
        rect(&rt, "tab:split"),
        before,
        "nothing moves when it shows"
    );

    // It asks before closing, as the tab's menu and a middle-click do, and names its own
    // tab; the tab on screen stays.
    click(&mut rt, "tab-close:split");
    let panel = rt.state.panel.as_ref().expect("a close confirmation");
    assert!(
        matches!(&panel.kind, PanelKind::Confirm { method, payload, .. }
            if method == "session.close_window" && payload["window_id"] == "split")
    );
    assert!(
        requests(&rx).iter().all(|r| r.0 != "session.focus_window"),
        "closing from the × doesn't switch tabs"
    );
    press(&mut rt, Key::Escape);
    assert!(rt.state.panel.is_none());
    click(&mut rt, "tab-close:single");
    let panel = rt.state.panel.as_ref().expect("a close confirmation");
    assert!(matches!(&panel.kind, PanelKind::Confirm { payload, .. }
            if payload["window_id"] == "single"));
    // A button by name, and the pointer's: the Tab key never lands on it.
    assert!(
        rt.elements()
            .iter()
            .filter(|e| e
                .key
                .as_deref()
                .is_some_and(|k| k.starts_with("tab-close:")))
            .all(|e| e.role == "button" && e.label.as_deref() == Some("Close tab"))
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

fn many_tabs(count: usize) -> (Workspace, flume::Receiver<HostCommand>) {
    let (mut ws, rx) = tabbed_workspace("t0");
    let windows: Vec<Value> = (0..count)
        .map(|i| {
            json!({"window_id": format!("t{i}"), "label": format!("project number {i}"),
                "focused_block_id": format!("block:{i}"),
                "layers": [{"kind": "tiled", "layout": {"block_id": format!("block:{i}")},
                    "blocks": [{"block_id": format!("block:{i}")}]}]})
        })
        .collect();
    ws.hosts[0].model.views.insert(
        "s".into(),
        serde_json::from_value(json!({
            "revision": 1, "session_id": "s", "active_window_id": "t0", "windows": windows
        }))
        .unwrap(),
    );
    (ws, rx)
}

#[test]
fn a_crowded_strip_of_filling_tabs_scrolls_before_a_name_is_cut_to_nothing() {
    let (mut ws, _rx) = many_tabs(12);
    ws.settings.tabs.fill = true;
    let mut rt = runtime(ws);
    settle(&mut rt);
    assert!(rt.state.tab_strip.overflows());
    assert!(has(&rt, "next-tabs") && has(&rt, "previous-tabs"));
    let tab = rect(&rt, "tab:t0");
    // Every tab at its floor: room for the ×, the same again before the badge so the
    // content stays centred, the badge, and 40 pt of the name.
    assert_eq!(tab[2], 2. * 28. + 20. + 8. + 40.);
    assert!((tab[2] - rect(&rt, "tab:t5")[2]).abs() < 0.5);
    // A tab in trouble keeps its word whole, however crowded.
    use rex_linux::connection::activity::{BlockedKind, ProgramState::Blocked};
    let mut blocked = record(Blocked, true);
    blocked.kind = Some(BlockedKind::Permission);
    rt.update(|s, _| set_activity(s, "block:1", &[("", blocked.clone())]));
    settle(&mut rt);
    let (pill, word) = (rect(&rt, "tab:t1"), rect(&rt, "tab-word:t1"));
    assert!(
        word[0] + word[2] <= pill[0] + pill[2] - 27.5,
        "{pill:?} {word:?}"
    );
    assert!(pill[2] > tab[2]);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn sidebar_tab_rows_show_the_same_icons() {
    let (mut ws, _rx) = programs("split");
    with_tabs(
        &mut ws,
        TabsConfig {
            icons: 2,
            icon_color: TabIconColor::ActiveTab,
            ..TabsConfig::default()
        },
    );
    ws.set_vertical_tabs(true);
    let mut rt = runtime(ws);
    settle(&mut rt);
    let (claude, _) = icon(&rt, "sidebar-tab-icon:0:s:split");
    assert!(same(claude, rgb(tint(&CLAUDE))));
    let (nvim, _) = icon(&rt, "sidebar-tab-icon:0:s:split:1");
    assert!(same(nvim, rgb(tint(&VIM))));
    assert!(!has(&rt, "sidebar-tab-icon:0:s:split:2"));
    let (shell, tile) = icon(&rt, "sidebar-tab-icon:0:s:single");
    assert!(same(shell, rgb(muted())) && !tile);
    rt.update(|s, _| s.settings.tabs.icons = 0);
    settle(&mut rt);
    assert!(!has(&rt, "sidebar-tab-icon:0:s:split"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// Settings on the Tabs page, reached as a user does: Interface, then Customize tabs.
fn tabs_page() -> Runtime<Workspace> {
    let mut ws = workspace();
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    let mut rt = runtime_at(ws, (1440., 1000.));
    settle(&mut rt);
    click(&mut rt, "settings-section:interface");
    settle(&mut rt);
    click(&mut rt, "customize-tabs");
    settle(&mut rt);
    assert_eq!(rt.state.settings_page(), Page::Sub(SubPage::Tabs));
    rt
}

fn saved(rt: &Runtime<Workspace>) -> TabsConfig {
    Settings::load(&rt.state.settings_path).unwrap().tabs
}

/// Take `option` from the list a popup button opened.
fn choose(rt: &mut Runtime<Workspace>, control: &str, option: &str) {
    click(rt, control);
    settle(rt);
    let index = rt
        .state
        .entries()
        .iter()
        .position(|e| e.label == option)
        .unwrap_or_else(|| panic!("{control} has no {option}"));
    rt.update(|s, tx| s.choose(index, tx));
    settle(rt);
}

#[test]
fn the_tabs_page_changes_and_saves_each_setting_and_its_preview_follows() {
    let mut rt = tabs_page();
    assert_eq!(element_text(&rt, "settings-title").join(""), "Tabs");
    assert!(has(&rt, "tabs-preview"));
    let preview = rect(&rt, "tabs-preview");
    // The preview is a picture: nothing in it takes the pointer or the Tab key.
    assert!(
        rt.elements()
            .iter()
            .filter(|e| e
                .key
                .as_deref()
                .is_some_and(|k| k.starts_with("tabs-preview")))
            .all(|e| e.inert && !e.focusable)
    );
    // By default: one icon each, out of colour, tabs as wide as their names.
    assert!(has(&rt, "tabs-preview-tab:0-icon") && !has(&rt, "tabs-preview-tab:0-icon:1"));
    let (prompt, tile) = icon(&rt, "tabs-preview-tab:0-icon");
    assert!(same(prompt, rgb(ink())) && !tile);
    let narrow = rect(&rt, "tabs-preview-tab:0")[2];
    assert!(narrow < 240.);

    // Width.
    click(&mut rt, "tabs-fill");
    settle(&mut rt);
    assert!(rt.state.settings.tabs.fill && saved(&rt).fill);
    let (first, second) = (
        rect(&rt, "tabs-preview-tab:0"),
        rect(&rt, "tabs-preview-tab:1"),
    );
    assert!((first[2] - second[2]).abs() < 0.5 && first[2] > narrow);
    assert!(second[0] + second[2] > preview[0] + preview[2] - 20.);
    click(&mut rt, "tabs-fill");
    settle(&mut rt);
    assert!(!saved(&rt).fill);

    // Icons per tab, from 0 to 5 and no further.
    for expect in 2..=5 {
        click(&mut rt, "tab-icons-larger");
        assert_eq!(saved(&rt).icons, expect);
    }
    click(&mut rt, "tab-icons-larger");
    assert_eq!(saved(&rt).icons, 5);
    settle(&mut rt);
    assert!(has(&rt, "tabs-preview-tab:0-icon:4") && has(&rt, "tabs-preview-tab:1-icon:4"));

    // Colour.
    choose(&mut rt, "tab-icon-color", "Active pane");
    assert_eq!(saved(&rt).icon_color, TabIconColor::ActivePane);
    assert!(
        rt.state
            .panel
            .as_ref()
            .is_some_and(|p| matches!(p.kind, PanelKind::Settings))
    );
    let (prompt, tile) = icon(&rt, "tabs-preview-tab:0-icon");
    assert!(tile && !same(prompt, rgb(ink())));
    let (claude, _) = icon(&rt, "tabs-preview-tab:0-icon:1");
    assert!(same(claude, rgb(ink())), "only the focused pane's");
    choose(&mut rt, "tab-icon-color", "Always");
    assert_eq!(saved(&rt).icon_color, TabIconColor::Always);
    let (claude, _) = icon(&rt, "tabs-preview-tab:0-icon:1");
    assert!(same(claude, rgb(tint(&CLAUDE))));
    let (_, tile) = icon(&rt, "tabs-preview-tab:1-icon:1");
    assert!(tile, "the other tab's shell too");

    // The shell's tile.
    choose(&mut rt, "shell-icon", "Accent");
    assert_eq!(saved(&rt).shell_icon, ShellIconTheme::Accent);
    let (prompt, _) = icon(&rt, "tabs-preview-tab:0-icon");
    assert!(same(prompt, rgb(accent())));
    choose(&mut rt, "shell-icon", "Terminal theme");
    assert_eq!(saved(&rt).shell_icon, ShellIconTheme::Theme);

    // Down to none: the preview has names only.
    for _ in 0..6 {
        click(&mut rt, "tab-icons-smaller");
    }
    settle(&mut rt);
    assert_eq!(saved(&rt).icons, 0);
    assert!(!has(&rt, "tabs-preview-tab:0-icon"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn choices_with_nothing_to_act_on_are_dimmed_and_out_of_reach() {
    let mut rt = tabs_page();
    let control = |rt: &Runtime<Workspace>, key: &str| {
        let e = rt
            .elements()
            .into_iter()
            .find(|e| !e.exiting && e.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("missing {key}"));
        (e.inert, e.opacity < 0.5)
    };
    // Out of colour (the default) there is no tile to choose.
    assert_eq!(control(&rt, "tab-icon-color"), (false, false));
    assert_eq!(control(&rt, "shell-icon"), (true, true));
    assert!(shown(&rt, "Icons are not in colour."));
    click(&mut rt, "shell-icon");
    settle(&mut rt);
    assert!(
        rt.state
            .panel
            .as_ref()
            .is_some_and(|p| matches!(p.kind, PanelKind::Settings))
    );
    choose(&mut rt, "tab-icon-color", "Active tab");
    assert_eq!(control(&rt, "shell-icon"), (false, false));
    // With no icons neither choice has anything to act on; both keep their values.
    click(&mut rt, "tab-icons-smaller");
    settle(&mut rt);
    assert_eq!(control(&rt, "tab-icon-color"), (true, true));
    assert_eq!(control(&rt, "shell-icon"), (true, true));
    assert!(shown(&rt, "Tabs show no icons."));
    assert_eq!(saved(&rt).icon_color, TabIconColor::ActiveTab);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn search_settings_finds_the_tabs_page_and_its_rows() {
    let mut ws = workspace();
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    let mut rt = runtime_at(ws, (1440., 1000.));
    settle(&mut rt);
    click(&mut rt, "settings-search");
    for c in "icons per".chars() {
        press(&mut rt, Key::Text(c.to_string()));
    }
    settle(&mut rt);
    let hits: Vec<(String, String)> = rt
        .state
        .settings_nav
        .hits()
        .iter()
        .map(|hit| (hit.label.clone(), hit.place.clone()))
        .collect();
    assert_eq!(
        hits,
        [("Icons per tab".to_owned(), "Interface › Tabs".to_owned())]
    );
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert_eq!(rt.state.settings_page(), Page::Sub(SubPage::Tabs));
    assert!(has(&rt, "tabs-preview"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// The toolbar's tabs and the Tabs page as drawn, for a look (see `Camera`).
#[test]
fn the_tab_track_and_the_tabs_page_draw_each_setting() {
    let Some(mut camera) = pane_verb_tests::Camera::new() else {
        return;
    };
    let shots: [(&str, TabsConfig, bool); 7] = [
        ("tabs-default", TabsConfig::default(), false),
        (
            "tabs-icons-3",
            TabsConfig {
                icons: 3,
                ..TabsConfig::default()
            },
            false,
        ),
        (
            "tabs-colour-always",
            TabsConfig {
                icons: 3,
                icon_color: TabIconColor::Always,
                ..TabsConfig::default()
            },
            false,
        ),
        (
            "tabs-colour-active-pane",
            TabsConfig {
                icons: 3,
                icon_color: TabIconColor::ActivePane,
                shell_icon: ShellIconTheme::Accent,
                ..TabsConfig::default()
            },
            false,
        ),
        (
            "tabs-fill",
            TabsConfig {
                fill: true,
                icons: 2,
                icon_color: TabIconColor::ActiveTab,
                ..TabsConfig::default()
            },
            false,
        ),
        (
            "tabs-sidebar",
            TabsConfig {
                icons: 3,
                icon_color: TabIconColor::Always,
                shell_icon: ShellIconTheme::Theme,
                ..TabsConfig::default()
            },
            true,
        ),
        (
            "tabs-none",
            TabsConfig {
                icons: 0,
                ..TabsConfig::default()
            },
            false,
        ),
    ];
    for (name, tabs, sidebar) in shots {
        let (mut ws, _rx) = programs("split");
        let model = &mut ws.hosts[0].model;
        let windows = &mut model.views.get_mut("s").unwrap().windows;
        let mut third = windows[0].clone();
        third.window_id = Some("third".into());
        third.label = Some("release notes".into());
        third.focused_block_id = Some("block:e".into());
        third.layers[0].blocks[0].block_id = Some("block:e".into());
        windows.push(third);
        model.programs.insert("block:e".into(), "lazygit".into());
        with_tabs(&mut ws, tabs);
        if tabs.shell_icon == ShellIconTheme::Theme {
            ws.apply_theme(TerminalThemeChoice::Named("rex/merino-light".into()))
                .unwrap();
        }
        if sidebar {
            ws.set_vertical_tabs(true);
        }
        let mut rt = Runtime::new(ws, view, (1024., 700.), Env::new(true, 2.));
        settle(&mut rt);
        camera.snap(&mut rt, name);
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}
