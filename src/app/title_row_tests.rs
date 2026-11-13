//! The toolbar as the window's title bar (macOS): one row beside the traffic lights, which a
//! lone pane's header joins. Headless runs draw it wherever a test turns it on.
use super::pane_verb_tests::rect;
use super::*;
use rex_rune::{connection::titles::Place, settings::interface::ToolbarBehavior};
use rune::{ChromeInsets, TitlebarAction};

/// Where the traffic lights end in a window at rest.
const LIGHTS: f32 = 76.;
const ROW: f32 = crate::app::platform::chrome::TITLE_ROW;

/// What a window reports with its traffic lights ending `leading` points in: 0 in full
/// screen.
fn lights(leading: f32) -> ChromeInsets {
    let mut insets = ChromeInsets::default();
    insets.leading = leading;
    insets.height = ROW;
    insets
}

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..3 {
        rt.update(|s, tx| {
            crate::app::poll(s, 0., tx);
        });
        for _ in 0..30 {
            rt.frame(1. / 60.);
        }
    }
}

/// `tabbed_workspace` with the title row on, keeping only the tabs named in `tabs`; the
/// first is active.
fn unified(tabs: &[&str]) -> (Runtime<Workspace>, flume::Receiver<HostCommand>) {
    unified_at(tabs, (1024., 700.))
}

fn unified_at(
    tabs: &[&str],
    size: (f32, f32),
) -> (Runtime<Workspace>, flume::Receiver<HostCommand>) {
    let (mut ws, rx) = tabbed_workspace(tabs[0]);
    ws.unified_title = true;
    let view = ws.hosts[0].model.views.get_mut("s").unwrap();
    view.windows
        .retain(|w| w.window_id.as_deref().is_some_and(|id| tabs.contains(&id)));
    ws.hosts[0].model.places.insert(
        "block:a".into(),
        Place {
            cwd: Some("/home/u/rex".into()),
            home: Some("/home/u".into()),
            shell: None,
        },
    );
    let mut rt = runtime_at(ws, size);
    rt.set_chrome_insets(lights(LIGHTS));
    settle(&mut rt);
    (rt, rx)
}

fn text_of(rt: &Runtime<Workspace>, key: &str) -> Option<String> {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .and_then(|e| e.text)
}

fn within_row(r: [f32; 4]) -> bool {
    r[1] >= 0. && r[1] + r[3] <= ROW
}

#[test]
fn a_lone_panes_header_is_the_title_row() {
    let (rt, _rx) = unified(&["single"]);
    let bar = rect(&rt, "toolbar");
    assert_eq!((bar[1], bar[3]), (0., ROW), "one row at the top: {bar:?}");
    assert!(
        rect(&rt, "toggle-sidebar")[0] >= LIGHTS,
        "the row starts past the traffic lights"
    );
    assert!(!has(&rt, "pane-header:block:a"), "no header row of its own");
    let pane = rect(&rt, "pane:block:a");
    assert_eq!(
        (pane[0], pane[1], pane[2]),
        (0., ROW, 1024.),
        "the terminal starts right under the row, edge to edge: {pane:?}"
    );
    assert_eq!(text_of(&rt, "title-directory").as_deref(), Some("~/rex"));
    assert!(within_row(rect(&rt, "title-name")));
    assert!(!has(&rt, "tab-track"), "one tab needs no pills");
    // The pane's own buttons end the row, after the window's.
    let settings = rect(&rt, "settings");
    let split = rect(&rt, "pane-split-right:block:a");
    let close = rect(&rt, "pane-close:block:a");
    assert!(within_row(split) && within_row(close));
    assert!(settings[0] < split[0] && split[0] < close[0]);
    assert!(
        close[0] + close[2] <= 1024. - 6.,
        "a margin at the end: {close:?}"
    );
    assert!(has(&rt, "identity"), "the session stays one press away");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn several_tabs_show_their_pills_and_split_panes_keep_their_headers() {
    let (rt, _rx) = unified(&["single", "split"]);
    assert!(has(&rt, "tab:single") && has(&rt, "tab:split"));
    assert!(within_row(rect(&rt, "tab:split")));
    assert!(!has(&rt, "title-name"), "the active pill names the tab");
    assert!(
        has(&rt, "pane-split-right:block:a") && !has(&rt, "pane-header:block:a"),
        "the active tab's lone pane still has its buttons in the row"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();

    let (rt, _rx) = unified(&["split"]);
    assert!(
        has(&rt, "pane-header:block:b") && has(&rt, "pane-header:block:c"),
        "each split pane names itself and shows whether it is focused"
    );
    assert!(!has(&rt, "title-divider"), "no pane's buttons in the row");
    let title = text_of(&rt, "title-name");
    assert!(title.is_some());
    assert_eq!(
        title,
        text_of(&rt, "pane-name:block:b"),
        "the title is the focused pane's"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();

    // In the smallest window the session keeps a few letters and the row still ends inside
    // the window.
    let (rt, _rx) = unified_at(&["single", "split"], (640., 480.));
    let session = rect(&rt, "identity");
    assert!(session[2] >= 84., "{session:?}");
    let close = rect(&rt, "pane-close:block:a");
    assert!(close[0] + close[2] <= 640., "{close:?}");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn empty_title_space_moves_and_zooms_the_window_and_controls_do_not() {
    let (mut rt, _rx) = unified(&["single"]);
    let press = |rt: &mut Runtime<Workspace>, key: &str| {
        let r = rect(rt, key);
        let (x, y) = (r[0] + r[2] / 2., r[1] + r[3] / 2.);
        rt.pointer_down(x, y);
        let action = rt.take_titlebar_action();
        rt.pointer_up(x, y);
        action
    };
    assert_eq!(press(&mut rt, "title-drag"), Some(TitlebarAction::Drag));
    assert_eq!(
        press(&mut rt, "title-drag"),
        Some(TitlebarAction::DoubleClick),
        "the system's double-click action, zoom by default"
    );
    rt.frame(1.);
    assert_eq!(
        press(&mut rt, "title-name"),
        Some(TitlebarAction::Drag),
        "the title moves the window too"
    );
    rt.frame(1.);
    assert_eq!(press(&mut rt, "settings"), None);
    rt.frame(1. / 60.);
    assert!(rt.state.panel.is_some(), "the control did its own thing");
    std::fs::remove_file(&rt.state.settings_path).ok();

    for key in ["identity", "pane-split-right:block:a"] {
        let (mut rt, _rx) = unified(&["single"]);
        assert_eq!(press(&mut rt, key), None, "{key}");
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
    let (mut rt, _rx) = unified(&["single", "split"]);
    assert_eq!(press(&mut rt, "tab:split"), None, "a tab pill");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn without_a_toolbar_row_a_title_strip_keeps_the_traffic_lights_clear_until_full_screen() {
    let (mut rt, _rx) = unified(&["single"]);
    rt.update(|s, tx| s.set_toolbar(ToolbarBehavior::Hidden, tx));
    settle(&mut rt);
    assert!(has(&rt, "title-strip") && !has(&rt, "toolbar"));
    assert!(
        !has(&rt, "pane-header:block:a") && has(&rt, "pane-split-right:block:a"),
        "the lone pane's header is the strip"
    );
    assert_eq!(rect(&rt, "pane:block:a")[1], ROW);

    // Revealing the toolbar over the strip moves nothing.
    rt.update(|s, _| s.toolbar_reveal.pinned = true);
    settle(&mut rt);
    assert!(has(&rt, "toolbar") && !has(&rt, "pane-header:block:a"));
    assert_eq!(rect(&rt, "pane:block:a")[1], ROW);
    rt.update(|s, _| s.toolbar_reveal.pinned = false);

    // Full screen hides the traffic lights, and the panes take the window's top under
    // their own header. The system says so with no resize.
    rt.set_chrome_insets(lights(0.));
    settle(&mut rt);
    assert!(!has(&rt, "title-strip") && has(&rt, "pane-header:block:a"));
    assert!(rect(&rt, "pane:block:a")[1] < ROW);

    // Leaving it brings them back, and the strip with them.
    rt.set_chrome_insets(lights(LIGHTS));
    settle(&mut rt);
    assert!(has(&rt, "title-strip") && rect(&rt, "toggle-sidebar")[0] >= LIGHTS);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_title_hides_only_a_directory_it_ends_in() {
    use super::super::title_row::ends_in_path;
    assert!(ends_in_path("zsh ~/rex", "~/rex"));
    assert!(ends_in_path("u@host:/tmp/home", "/tmp/home"));
    assert!(ends_in_path("~", "~"));
    assert!(!ends_in_path("vim mysrc", "src"));
    assert!(!ends_in_path("/var/tmp/home", "/tmp/home"));
    assert!(!ends_in_path("vim ~/rex/main.rs", "~/rex"));
}

#[test]
fn a_window_in_full_screen_keeps_the_size_it_had() {
    let (mut rt, _rx) = unified(&["single"]);
    rt.update(|s, _| s.settings.window_size = Some([1024., 700.]));
    // The resize into full screen can come before the traffic lights go.
    rt.set_viewport((1512., 982.), 1.);
    settle(&mut rt);
    assert!(rt.state.resized.is_some());
    rt.set_chrome_insets(lights(0.));
    settle(&mut rt);
    assert!(rt.state.full_screen);
    assert!(rt.state.resized.is_none(), "nothing left to save");
    assert_eq!(rt.state.settings.window_size, Some([1024., 700.]));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn switch_session_opens_under_the_session_button() {
    let (mut rt, _rx) = unified(&["single"]);
    rt.update(|s, _| s.picker(true));
    settle(&mut rt);
    let button = rect(&rt, "identity");
    let list = rect(&rt, "menu-surface");
    assert!(list[1] >= ROW - 4. && list[1] < ROW + 4., "{list:?}");
    assert!(
        (list[0] + list[2] - (button[0] + button[2])).abs() < 1.,
        "{list:?} ends where {button:?} does"
    );
    press(&mut rt, Key::Escape);

    // With no row drawn there is no button, and the list keeps to the window's end.
    rt.update(|s, tx| s.set_toolbar(ToolbarBehavior::Hidden, tx));
    rt.update(|s, _| s.picker(true));
    settle(&mut rt);
    let list = rect(&rt, "menu-surface");
    assert!(
        list[0] + list[2] <= 1024. - crate::app::theme::MARGIN + 1.,
        "{list:?}"
    );
    assert!(list[0] > 512., "{list:?}");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_zoomed_pane_and_sidebar_tabs_keep_what_they_show() {
    // A zoomed pane of several keeps its header, which says it is zoomed.
    let (mut ws, _rx) = tabbed_workspace("split");
    ws.unified_title = true;
    let view = ws.hosts[0].model.views.get_mut("s").unwrap();
    for w in &mut view.windows {
        if w.window_id.as_deref() == Some("split") {
            w.zoomed_block_id = Some("block:b".into());
        }
    }
    let mut rt = runtime_at(ws, (1024., 700.));
    rt.set_chrome_insets(lights(LIGHTS));
    settle(&mut rt);
    assert!(has(&rt, "pane-header:block:b") && has(&rt, "pane-zoomed:block:b"));
    assert!(!has(&rt, "title-divider"));
    std::fs::remove_file(&rt.state.settings_path).ok();

    // Tabs in the sidebar leave the row to the focused pane's title.
    let (mut rt, _rx) = unified(&["single", "split"]);
    rt.update(|s, _| s.set_vertical_tabs(true));
    settle(&mut rt);
    assert!(has(&rt, "title-name") && !has(&rt, "tab-track"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_separate_toolbar_is_unchanged_without_a_unified_title() {
    let (ws, _rx) = tabbed_workspace("single");
    let mut rt = runtime_at(ws, (1024., 700.));
    settle(&mut rt);
    assert_eq!(rect(&rt, "toolbar")[3], crate::app::theme::TOOLBAR);
    assert!(has(&rt, "pane-header:block:a") && !has(&rt, "title-name"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}
