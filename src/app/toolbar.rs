//! The 52pt bar on the window ground: sidebar toggle, identity chip, tab track, drag space,
//! Commands, New tab and Settings, a divider, and Close. Tooltips name each control's shortcut
//! from the effective keymap (`keycaps.rs`), never from a literal.
use super::{
    activity::{progress_bar, tab_activity, tab_name, tab_programs, tab_progress, tab_word},
    panel::Intent,
    tab_icons::{IconLook, TabIcon, icons_of, tab_icons},
    tabs::{muted_mark, target_ring},
    theme::*,
    widgets::{chrome_button, glyph},
    workspace::Workspace,
};
use rune::{Cursor, El, PointerButton, ScrollPos, Tx, col, el, row, text};

/// The identity chip's width limits.
const IDENTITY_MIN: f32 = 120.;
const IDENTITY_MAX: f32 = 210.;

/// A chip's press: consume it (chips never drag the window) and keep the terminal focused.
fn chip_press(e: El) -> El {
    e.cursor(Cursor::Pointer)
        .preserve_focus_on_press(true)
        .tab_index(-1)
}

/// The host line under the session's name: trouble always, and otherwise the host only while
/// a second host is connected, when the session's name alone could be either's.
fn identity_detail(s: &Workspace) -> Option<(String, u32)> {
    let host = &s.settings.hosts[s.active].label;
    if !s.model().connected {
        Some((format!("{host} · Reconnecting"), warning()))
    } else if s.hosts.iter().filter(|h| h.model.connected).count() > 1 {
        Some((host.clone(), muted()))
    } else {
        None
    }
}

fn identity(s: &Workspace) -> El {
    let model = s.model();
    let host = &s.settings.hosts[s.active].label;
    let session = model
        .view()
        .and_then(|v| v.label.clone())
        .unwrap_or_else(|| "Choose session".into());
    let mut words = col().grow(1.).shrink().min_w(0.).gap(1.).child(
        text(session.clone())
            .font(strong(12.5))
            .color(rgb(ink()))
            .truncate(),
    );
    if let Some((detail, tone)) = identity_detail(s) {
        words = words.child(
            text(detail)
                .key("identity-host")
                .font(ui(10.5))
                .color(rgb(tone))
                .truncate(),
        );
    }
    chip_press(chip(
        row()
            .key("identity")
            .role("button")
            .label(format!("{session} · {host} · Switch session"))
            // As wide as its words, between a floor that keeps it a comfortable target and
            // the width a long name truncates at.
            .min_w(IDENTITY_MIN)
            .max_w(IDENTITY_MAX)
            .h(34.)
            .pl(6.)
            .pr(8.)
            .gap(9.)
            .center_y()
            .hover(white(0.08))
            .help(format!(
                "{session} · {host} · {}",
                s.shortcut_help("Switch session", "session.switch")
            ))
            .on_press_tx(|s: &mut Workspace, tx: &mut Tx| {
                s.picker(true);
                tx.blur();
            })
            .on_secondary_press(|s: &mut Workspace, p, _| {
                if let Some(session) = s.model().selected.clone() {
                    s.session_menu(s.active, session, (p.x, p.y));
                }
            })
            .child(el().size(22., 22.).center().child(glyph(
                s.host_icon(s.active),
                16.,
                rgb(muted()),
            )))
            // The words take any room the floor leaves, so the chevron keeps to the edge.
            .child(words)
            .child(glyph(&CHEVRON, 14., rgb(muted()))),
    ))
}

/// A tab pill and the track it sits on. Both are capsules, and the pill's ends are concentric
/// with the track's: the track's inset is half the difference.
pub const PILL: f32 = 30.;
pub const TRACK: f32 = 34.;
const TRACK_INSET: f32 = (TRACK - PILL) / 2.;
/// The narrowest and the widest a tab sized to its content is.
const PILL_MIN: f32 = 96.;
const PILL_MAX: f32 = 240.;
/// The × at a pill's end, and its distance from the pill's edges.
const CLOSE_SIZE: f32 = 18.;
const CLOSE_INSET: f32 = (PILL - CLOSE_SIZE) / 2.;
/// A pill's padding: before its icon's badge, before its name when it shows no icon, and
/// after its content, where the × is.
const PAD_BADGE: f32 = 6.;
const PAD_NAME: f32 = 14.;
const PAD_CLOSE: f32 = CLOSE_INSET + CLOSE_SIZE + 4.;

/// A glassy capsule `height` tall: the tab track.
pub fn capsule(e: El, height: f32) -> El {
    chip(e.h(height)).radius(height / 2.)
}

/// The narrowest a tab that shares the strip gets before the strip scrolls instead: any
/// tab's floor, or the room for its `icons`, 40 pt of its name and what follows the name
/// (the activity `word`, which is never cut, and the muted mark). The word's width is an
/// estimate on the generous side.
pub fn fill_floor(icons: usize, word: Option<&str>, quiet: bool) -> f32 {
    let mut floor = 2. * PAD_CLOSE + 40.;
    if icons > 0 {
        floor += super::tab_icons::row_width(icons) + 8.;
    }
    if let Some(word) = word {
        floor += 8. + word.chars().count() as f32 * 7.;
    }
    if quiet {
        floor += 8. + 14.;
    }
    floor.max(PILL_MIN)
}

/// A tab pill's frame: as wide as its content, between 96 and 240 pt; `icons` says whether
/// it starts with an icon's badge. With `fill` it takes an equal share of the strip instead,
/// never less than that floor, with its content centred. Either way the trailing end keeps
/// room for the × ([`close_mark`]).
pub fn pill_frame(fill: Option<f32>, icons: bool) -> El {
    let frame = row().h_full().gap(8.).center_y().hover_group();
    match fill {
        Some(floor) => frame.px(PAD_CLOSE).grow(1.).min_w(floor).justify_center(),
        None => frame
            .pl(if icons { PAD_BADGE } else { PAD_NAME })
            .pr(PAD_CLOSE)
            .min_w(PILL_MIN)
            .max_w(PILL_MAX),
    }
}

/// The fill and rim under the tab on screen.
pub fn pill_highlight() -> El {
    el().pin(Some(0.), Some(0.), Some(0.), Some(0.))
        .radius(PILL / 2.)
        .fill(white(0.11))
        .box_shadow(shadow(0., 1., 3., black(0.35)))
        .box_shadow(inset(1., white(0.09)))
}

/// A tab's name on its pill. It is cut short before anything else on the pill gives way.
pub fn pill_label(label: String, active: bool) -> El {
    text(label.clone())
        .font(if active { medium(12.5) } else { ui(12.5) })
        .color(rgb(if active { ink() } else { muted() }))
        .truncate()
        .help_when_truncated(label)
        .shrink()
}

/// The × at a pill's trailing end, as drawn. It shows on the tab on screen and, on any
/// other tab, only while the pointer is over that tab. An overlay in room the pill always
/// keeps, so nothing moves when it appears.
fn close_mark(key: String, active: bool) -> El {
    let mark = row()
        .key(key)
        .pin(None, Some(CLOSE_INSET), Some(CLOSE_INSET), None)
        .size(CLOSE_SIZE, CLOSE_SIZE)
        .center()
        .radius(CLOSE_SIZE / 2.)
        .child(glyph(&CLOSE, 12., rgb(muted())));
    if active {
        mark
    } else {
        mark.group_opacity(0., 1.)
    }
}

/// [`close_mark`] as the button it is on a tab. The caller gives it its press.
fn pill_close(key: String, active: bool) -> El {
    close_mark(key, active)
        .role("button")
        .label("Close tab")
        .cursor(Cursor::Pointer)
        .hover(white(0.13))
        .press(white(0.2))
        .preserve_focus_on_press(true)
        .tab_index(-1)
        .help("Close tab")
}

/// A tab as the toolbar draws it, for a picture of one in Settings: its icons, its name and,
/// on the one on screen, its ×. The caller adds what follows the name.
pub fn sample_pill(
    key: &str,
    icons: &[TabIcon],
    name: &str,
    active: bool,
    fill: Option<f32>,
    look: &IconLook,
) -> El {
    let mut pill = pill_frame(fill, look.count > 0).key(key.to_owned());
    if active {
        pill = pill
            .child(pill_highlight())
            .child(close_mark(format!("{key}-close"), true));
    }
    pill.children(tab_icons(&format!("{key}-icon"), icons, active, look))
        .child(pill_label(name.to_owned(), active))
}

/// The session's tabs on their track. With no tab to show (no session on screen, or one whose
/// view hasn't arrived) there is no track: an empty one would be a sliver of chip.
fn tab_track(s: &Workspace) -> El {
    let model = s.model();
    let tabs = model
        .view()
        .is_some_and(|view| view.windows.iter().any(|tab| tab.window_id.is_some()));
    if !tabs {
        return el();
    }
    let session = model.selected.clone().unwrap_or_default();
    let fill = s.settings.tabs.fill;
    let look = s.tab_icon_look();
    let mut names = row()
        .key("tab-names")
        .h(PILL)
        .gap(2.)
        .on_layout(|s: &mut Workspace, r| s.tab_strip.content = r[2]);
    // With `fill`: the least the tabs need together, where the strip starts to scroll.
    let mut floors = -2.;
    if let Some(view) = model.view() {
        for tab in &view.windows {
            let Some(id) = tab.window_id.clone() else {
                continue;
            };
            let active = view.active_window_id.as_ref() == Some(&id);
            let label = tab_name(model, tab);
            let activity = tab_activity(model, tab);
            let shown = s.tab_activity_shown(s.active, &id);
            let quiet = s.tab_muted(s.active, &id);
            let word =
                tab_word(&activity, s.unseen(&id), s.bell_on(s.active, &id)).filter(|_| shown);
            let (press_id, rename_id, menu_id) = (id.clone(), id.clone(), id.clone());
            let close_id = id.clone();
            let menu_session = session.clone();
            let icons = icons_of(&tab_programs(model, tab));
            let floor = fill.then(|| {
                fill_floor(
                    icons.len().min(look.count),
                    word.as_ref().map(|(word, _)| word.as_ref()),
                    quiet,
                )
            });
            floors += floor.unwrap_or_default() + 2.;
            let mut tab_el = pill_frame(floor, look.count > 0)
                .key(format!("tab:{id}"))
                .role("tab")
                .focus_selected(active)
                .label(match &word {
                    Some((word, _)) => format!("{label} · {word}"),
                    None if quiet => format!("{label} · Muted"),
                    None => label.clone(),
                })
                .cursor(Cursor::Pointer)
                .preserve_focus_on_press(true)
                .pointer_buttons([PointerButton::Middle])
                .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                    // Middle-click closes, behind the same confirmation as Close tab.
                    if tx.button() == PointerButton::Middle {
                        s.perform(Intent::CloseWindow(press_id.clone()), tx);
                    } else {
                        s.perform(Intent::Tab(press_id.clone()), tx);
                        tx.blur();
                    }
                })
                .on_double_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                    s.perform(Intent::RenameWindow(rename_id.clone()), tx);
                })
                .on_secondary_press(move |s: &mut Workspace, p, _| {
                    s.tab_menu(s.active, &menu_session, &menu_id, (p.x, p.y))
                });
            if active {
                tab_el = tab_el.child(
                    pill_highlight()
                        .matched(format!("tab-pill:{session}"))
                        .transition(no_presence())
                        .motion(SNAPPY),
                );
            } else {
                tab_el = tab_el.radius(PILL / 2.).hover(white(0.05));
            }
            tab_el = tab_el
                .children(tab_icons(&format!("tab-icon:{id}"), &icons, active, &look))
                .child(pill_label(label, active));
            // The label truncates first; the word always stays whole.
            if let Some((word, tone)) = word {
                let done = word == "Done";
                let mut mark = text(word)
                    .key(format!("tab-word:{id}"))
                    .font(medium(11.5))
                    .color(rgb(tone));
                if let Some(msg) = activity.msg.clone().filter(|_| !done) {
                    mark = mark.help(msg);
                }
                tab_el = tab_el.child(mark);
            }
            if quiet {
                tab_el = tab_el.child(muted_mark(&id));
            }
            if shown && let Some(progress) = tab_progress(&activity) {
                tab_el = tab_el.child(progress_bar(format!("tab-progress:{id}"), progress, 14.));
            }
            if s.tab_targeted(s.active, &id) {
                tab_el = tab_el.child(target_ring(&id, PILL / 2.));
            }
            // Behind the same confirmation as Close tab in the tab's menu.
            tab_el = tab_el.child(pill_close(format!("tab-close:{id}"), active).on_press_tx(
                move |s: &mut Workspace, tx: &mut Tx| {
                    s.perform(Intent::CloseWindow(close_id.clone()), tx);
                    tx.blur();
                },
            ));
            names = names.child(tab_el);
        }
    }
    if fill {
        // As wide as the strip, and as its tabs at their narrowest once those need more.
        names = names.grow(1.).min_w(floors.max(0.));
    }
    let overflow = s.tab_strip.overflows();
    let (fade, pos) = (s.tab_fade.clone(), s.tab_pos.clone());
    let strip = row()
        .key("tabs")
        .min_w(0.)
        .shrink()
        .h(PILL)
        .scroll_x()
        .scroll_edge_fade(16.)
        .on_layout(|s: &mut Workspace, r| s.tab_strip.viewport = r[2])
        .on_scroll_offset(move |p: ScrollPos| {
            pos.set([p.offset[0], p.max[0]]);
            fade[0].set(if p.offset[0] > 0.5 { 1. } else { 0.35 });
            fade[1].set(if p.offset[0] < p.max[0] - 0.5 {
                1.
            } else {
                0.35
            });
        })
        .child(names);
    let strip = if fill { strip.grow(1.) } else { strip };
    let mut track = row()
        .key("tab-track")
        .px(TRACK_INSET)
        .gap(2.)
        .center_y()
        .min_w(0.)
        .shrink();
    if fill {
        track = track.grow(1.);
    }
    if overflow {
        track = track.child(
            chrome_button(
                "previous-tabs",
                &LEFT,
                "Scroll previous tabs",
                26.,
                |s, tx| s.scroll_tabs(-1, tx),
            )
            .live_opacity(&s.tab_fade[0]),
        );
    }
    track = track.child(strip);
    if overflow {
        track = track.child(
            chrome_button("next-tabs", &RIGHT, "Scroll next tabs", 26., |s, tx| {
                s.scroll_tabs(1, tx)
            })
            .live_opacity(&s.tab_fade[1]),
        );
    }
    chip_press(capsule(track, TRACK).on_press(|_: &mut Workspace| {}))
}

/// A toolbar button at the right end: no fill at rest and a round hover inside, but the hit
/// area takes the bar's whole height (and the last one its right margin), so the corner above
/// Close closes.
fn bar_button(
    key: &str,
    i: &'static rune::Icon,
    label: &str,
    help: &str,
    width: f32,
    destructive: bool,
    f: impl Fn(&mut Workspace, &mut Tx) + 'static,
) -> El {
    let hover = if destructive {
        rgb(danger()).alpha(0.22)
    } else {
        white(0.09)
    };
    row()
        .key(key.to_owned())
        .role("button")
        .label(label.to_owned())
        .w(width)
        .h(TOOLBAR)
        .pr(width - 34.)
        .center_y()
        .justify_start()
        .cursor(Cursor::Pointer)
        .hover_group()
        .preserve_focus_on_press(true)
        .tab_index(-1)
        .help(help.to_owned())
        .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
            f(s, tx);
            tx.blur();
        })
        .child(
            el().size(30., 30.)
                .ml(2.)
                .radius(15.)
                .center()
                .child(
                    el().pin(Some(0.), Some(0.), Some(0.), Some(0.))
                        .radius(15.)
                        .fill(hover)
                        .group_opacity(0., 1.),
                )
                .child(glyph(i, 15., rgb(muted()))),
        )
}

pub fn toolbar(s: &Workspace) -> El {
    row()
        .key("toolbar")
        .w_full()
        .h(TOOLBAR)
        .pl(MARGIN)
        .gap(8.)
        .center_y()
        .window_drag()
        .on_secondary_press(|_: &mut Workspace, p, tx| tx.show_window_menu(p.x, p.y))
        .child(chrome_button(
            "toggle-sidebar",
            &SIDEBAR,
            &s.shortcut_help("Toggle sidebar", "client.sidebar.toggle"),
            30.,
            |s, tx| s.perform(Intent::Sidebar, tx),
        ))
        .child(identity(s))
        .child(if s.sidebar_tabs() { el() } else { tab_track(s) })
        .child(match s.bindings.sequence.label(s.effective_keymap()) {
            Some(label) => chip(
                el().key("pending-sequence")
                    .h(28.)
                    .max_w(170.)
                    .px(8.)
                    .center_y()
                    .help(format!(
                        "{label} — waiting for the next key; Escape cancels"
                    ))
                    .child(
                        text(format!("{label} …"))
                            .font(medium(11.5))
                            .color(rgb(ink()))
                            .truncate(),
                    ),
            ),
            None => el(),
        })
        // Always at least 48pt of window to drag, outside the tabs. Tabs that fill the bar
        // take the rest.
        .child(if s.tabs_fill_toolbar() {
            el().key("title-drag").w(48.).h_full()
        } else {
            el().key("title-drag").grow(1.).min_w(48.).h_full()
        })
        .child(
            row()
                .center_y()
                .child(bar_button(
                    "commands",
                    &COMMAND,
                    "Commands",
                    &s.shortcut_help("Commands, sessions and tabs", "client.palette.toggle"),
                    36.,
                    false,
                    |s, _| s.picker(false),
                ))
                .child(bar_button(
                    "new-tab",
                    &PLUS,
                    "New tab",
                    &s.shortcut_help("New tab", "client.tab.new"),
                    36.,
                    false,
                    |s, tx| s.perform(Intent::NewWindow, tx),
                ))
                .child(bar_button(
                    "settings",
                    &SETTINGS,
                    "Settings",
                    &s.shortcut_help("Settings", "client.settings"),
                    36.,
                    false,
                    |s, tx| s.perform(Intent::Settings, tx),
                ))
                .child(
                    el().key("toolbar-divider")
                        .w(1.)
                        .h(16.)
                        .mx(6.)
                        .fill(white(0.1)),
                )
                .child(bar_button(
                    "close-app",
                    &CLOSE,
                    "Close app",
                    "Close app · server programs keep running",
                    36. + MARGIN,
                    true,
                    |_, tx| tx.close_window(),
                )),
        )
}

impl Workspace {
    /// The toolbar's tabs are there and share its width.
    fn tabs_fill_toolbar(&self) -> bool {
        self.settings.tabs.fill
            && !self.sidebar_tabs()
            && self
                .model()
                .view()
                .is_some_and(|view| view.windows.iter().any(|tab| tab.window_id.is_some()))
    }

    pub fn scroll_tabs(&mut self, direction: i32, tx: &mut Tx) {
        let step = (self.tab_strip.viewport * 0.6).max(80.);
        let [offset, max] = self.tab_pos.get();
        tx.scroll_to_x("tabs", (offset + step * direction as f32).clamp(0., max));
    }
}
