//! The toolbar as the window's title bar (`platform::chrome::UNIFIED`, macOS), laid out as
//! Rex for macOS lays out its title row: the traffic lights, the sidebar toggle, then what is
//! on screen, drag space, the session, Commands, New tab and Settings, and at the far end a
//! lone pane's own buttons.
//!
//! What is on screen is the session's tabs while there are several and they are in the
//! toolbar, and otherwise the focused pane's program, name and directory (`sq ~`). A tab with
//! one pane has no header row of its own: its header is this row (`pane_header::in_title`).
//! Split panes keep theirs, which name each pane and mark the focused one.
//!
//! The row moves the window, except on its controls, and a double-click on its empty space
//! does what the system's setting for title bars says.
use super::{
    activity::{block_program, pane_activity, tab_progress},
    pane_header,
    panel::Intent,
    panes, tab_icons,
    theme::*,
    toolbar::{
        activity_chip, chip_press, download_chip, identity_detail, sequence_chip, tab_track,
    },
    toolbar_reveal::bar_height,
    widgets::{chrome_button, glyph},
    workspace::Workspace,
};
use rune::{El, Tx, el, row, text};

/// A control's square in the row.
const CONTROL: f32 = 28.;

/// Where the row's own content starts: past the traffic lights, or at the outer margin in
/// full screen, where the system hides them.
fn leading(s: &Workspace) -> f32 {
    if s.title_inset > 0. {
        s.title_inset / s.ui_scale() + 8.
    } else {
        MARGIN
    }
}

/// Whether the row shows the session's tabs: several, kept in the toolbar.
fn shows_tabs(s: &Workspace) -> bool {
    !s.sidebar_tabs()
        && s.model()
            .view()
            .is_some_and(|v| v.windows.iter().filter(|t| t.window_id.is_some()).count() > 1)
}

/// The focused pane as the window's title: its program's glyph, its name, and the directory
/// its program is in where the name doesn't already say. `merged` is the lone pane whose
/// header this row is, which also shows its activity word and progress here.
fn title(s: &Workspace, merged: bool) -> El {
    let model = s.model();
    let Some(block) = s.focused_block().filter(|_| model.window().is_some()) else {
        return el();
    };
    let name = s.pane_name(&block);
    // A shell's title usually ends in the directory already, perhaps more of it than the
    // shortened one.
    let dir = model
        .directory_of(&block)
        .filter(|dir| !name.ends_with(dir.trim_start_matches("…/")));
    let icons = tab_icons::icons_of(
        &[block_program(model, &block)],
        s.settings.interface.tab_icon_count.min(1),
    );
    let lit = s.window_focused;
    let menu_id = block.clone();
    let mut t = row()
        .key("title")
        .min_w(0.)
        .shrink()
        .h(CONTROL)
        .gap(8.)
        .center_y()
        .on_secondary_press(move |s: &mut Workspace, p, _| {
            s.pane_menu(menu_id.clone(), Some((p.x, p.y)))
        });
    t = tab_icons::append(
        t,
        "title-icon",
        &icons,
        lit,
        s.settings.interface.tab_icon_color,
    );
    let mut words = row().min_w(0.).shrink().gap(6.).center_y().child(
        text(name.clone())
            .key("title-name")
            .font(medium(12.5))
            .color(rgb(if lit { ink() } else { muted() }))
            .truncate()
            .help_when_truncated(name)
            .shrink(),
    );
    if let Some(dir) = dir {
        words = words.child(
            text(dir)
                .key("title-directory")
                .font(ui(12.5))
                .color(rgb(muted()))
                .truncate()
                .shrink(),
        );
    }
    t = t.child(words);
    if merged {
        t = t.children(panes::pane_word(s, &block, 11.5));
        if let Some(progress) = tab_progress(&pane_activity(model, &block))
            .filter(|_| s.settings.notifications.show_in_tabs)
        {
            t = t.child(super::activity::progress_bar(
                format!("pane-progress:{block}"),
                progress,
                14.,
            ));
        }
    }
    t
}

/// The session on screen, compact at the row's trailing end: a press opens the session list
/// under it, as the toolbar's identity chip does.
fn session_button(s: &Workspace) -> El {
    let host = &s.settings.hosts[s.active].label;
    let session = s
        .model()
        .view()
        .and_then(|v| v.label.clone())
        .unwrap_or_else(|| "Choose session".into());
    let detail = identity_detail(s);
    chip_press(
        row()
            .key("identity")
            .role("button")
            .label(format!("{session} · {host} · Switch session"))
            .max_w(220.)
            .shrink()
            .min_w(84.)
            .h(CONTROL)
            .pl(7.)
            .pr(5.)
            .gap(6.)
            .center_y()
            .radius(8.)
            .continuous_corners()
            .hover(white(0.08))
            .press(white(0.12))
            .help(format!(
                "{session} · {host} · {}",
                s.shortcut_help("Switch session", "session.switch")
            ))
            .on_layout(|s: &mut Workspace, r| s.title_session = r)
            .on_press_tx(|s: &mut Workspace, tx: &mut Tx| {
                s.picker(true);
                tx.blur();
            })
            .on_secondary_press(|s: &mut Workspace, p, _| {
                if let Some(session) = s.model().selected.clone() {
                    s.session_menu(s.active, session, (p.x, p.y));
                }
            })
            .child(glyph(s.host_icon(s.active), 14., rgb(muted())))
            .child(
                text(session)
                    .font(medium(12.))
                    .color(rgb(muted()))
                    .truncate()
                    .shrink(),
            )
            .children(detail.map(|(detail, tone)| {
                text(detail)
                    .key("identity-host")
                    .font(ui(11.))
                    .color(rgb(tone))
                    .truncate()
                    .shrink()
            }))
            .child(glyph(&CHEVRON, 12., rgb(muted()))),
    )
}

/// The row's start: past the traffic lights, the sidebar toggle.
fn start(s: &Workspace) -> El {
    row()
        .key("toolbar")
        .w_full()
        .h(bar_height(s))
        .pl(leading(s))
        .pr(8.)
        .gap(6.)
        .center_y()
        .window_drag()
        .child(chrome_button(
            "toggle-sidebar",
            &SIDEBAR,
            &s.shortcut_help("Toggle sidebar", "client.sidebar.toggle"),
            CONTROL,
            |s, tx| s.perform(Intent::Sidebar, tx),
        ))
}

/// The title row where the toolbar has its row (or floats it over the title strip).
pub fn title_row(s: &Workspace) -> El {
    let merged = s
        .model()
        .window()
        .is_some_and(|w| pane_header::in_title(s, w));
    let tabs = shows_tabs(s);
    let fill = tabs && s.settings.interface.tabs_fill_available_space;
    let bar = start(s)
        .child(if tabs {
            tab_track(s, 30., 24.)
        } else {
            title(s, merged)
        })
        .child(sequence_chip(s).unwrap_or_else(el))
        .children(activity_chip(s))
        .child(download_chip(s))
        // Always some window to drag, outside the tabs.
        .child(
            el().key("title-drag")
                .grow(if fill { 0. } else { 1. })
                .min_w(48.)
                .h_full(),
        )
        .child(session_button(s))
        .child(chrome_button(
            "commands",
            &COMMAND,
            &s.shortcut_help("Commands, sessions and tabs", "client.palette.toggle"),
            CONTROL,
            |s, _| s.picker(false),
        ))
        .child(chrome_button(
            "new-tab",
            &PLUS,
            &s.shortcut_help("New tab", "client.tab.new"),
            CONTROL,
            |s, tx| s.perform(Intent::NewWindow, tx),
        ))
        .child(chrome_button(
            "settings",
            &SETTINGS,
            &s.shortcut_help("Settings", "client.settings"),
            CONTROL,
            |s, tx| s.perform(Intent::Settings, tx),
        ));
    pane_end(s, bar, true)
}

/// The lone pane's own buttons at the row's far end, when its header is this row, set off
/// from the window's controls by a `divider`.
fn pane_end(s: &Workspace, bar: El, divider: bool) -> El {
    let model = s.model();
    let (Some(window), Some(block)) = (
        model.window().filter(|w| pane_header::in_title(s, w)),
        s.focused_block(),
    ) else {
        return bar;
    };
    let width = s.viewport[0] / s.ui_scale();
    bar.children(panes::strip(s, &block, width, &s.metrics()))
        .children(divider.then(|| {
            el().key("title-divider")
                .w(1.)
                .h(16.)
                .mx(4.)
                .fill(white(0.1))
        }))
        .child(pane_header::actions(s, window, &block, width, true))
}

/// The title strip while the toolbar has no row of its own: the traffic lights, the sidebar
/// toggle and the focused pane's title, so the window can still be moved and named, the live
/// status chips that float elsewhere, and a lone pane's buttons, as in the title row.
pub fn title_strip(s: &Workspace) -> El {
    let merged = s
        .model()
        .window()
        .is_some_and(|w| pane_header::in_title(s, w));
    let bar = start(s)
        .key("title-strip")
        .child(title(s, merged))
        .children(super::toolbar::floating_status(s))
        .child(el().key("title-drag").grow(1.).h_full());
    pane_end(s, bar, false)
}
