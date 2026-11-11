//! Settings: the sidebar on the left and the chosen page on the right, under its header
//! (`settings_nav` draws both and keeps the history between pages). Pages are built from
//! `settings_kit` (sections, cards, rows and their controls).
use super::{
    dialogs::modal_layer,
    keycaps::{Chords, keycaps, spell},
    panel::{Intent, SettingsSection},
    settings_kit::{
        NAV_WIDTH, PANEL_HEIGHT, PANEL_WIDTH, Segment, button, card, disclosure, footnote,
        mono_footnote, page, row_frame, section, segmented, select_row, setting, stepper,
        unavailable, value, words,
    },
    settings_nav::{HEADER, PAGE_INSET, Page, Status, SubPage},
    theme::*,
    widgets::{Pill, glyph},
    workspace::Workspace,
};
use rex_linux::settings::{NoticePosition, NotificationsConfig, PaneDensity, PaneHeader};
use rune::{El, KeyEvent, PointerButton, ScrollPos, ScrollSpec, Tx, col, el, row, text};
use rune_kit::Switch;

/// The least space between the dialog and the toolbar above it, and the window's bottom edge.
const CLEARANCE: f32 = 12.;

/// The Pane headers choices, in the order the control lists them.
const PANE_HEADERS: [PaneHeader; 4] = [
    PaneHeader::Auto,
    PaneHeader::Always,
    PaneHeader::OnHover,
    PaneHeader::Hidden,
];

fn interface(s: &Workspace) -> El {
    let k = kit();
    let percent = (s.settings.ui_scale * 100.).round() as i32;
    let corner = match s.settings.notice_position {
        NoticePosition::TopRight => 0,
        NoticePosition::BottomRight => 1,
    };
    page([
        section(
            "Appearance",
            [card([setting(
                "Interface zoom",
                "Chrome and terminals together, from 70% to 150%.",
                row()
                    .gap(8.)
                    .center_y()
                    .child(stepper(
                        "zoom",
                        format!("{percent}%"),
                        |s, tx| s.change_ui_scale(-1, tx),
                        |s, tx| s.change_ui_scale(1, tx),
                    ))
                    .child(button("zoom-reset", "Reset", Pill::Neutral, |s, tx| {
                        s.change_ui_scale(0, tx)
                    })),
            )])],
        ),
        section(
            "Panes",
            [card([
                setting(
                    "Pane density",
                    "Compact joins panes edge to edge, with no gaps.",
                    segmented(
                        "pane-density",
                        &[
                            Segment::new("Compact").icon(&PANES_JOINED),
                            Segment::new("Comfortable").icon(&PANES_APART),
                        ],
                        Some(usize::from(
                            s.settings.pane_density == PaneDensity::Comfortable,
                        )),
                        |s, i, tx| {
                            s.settings.pane_density = if i == 0 {
                                PaneDensity::Compact
                            } else {
                                PaneDensity::Comfortable
                            };
                            s.save();
                            tx.instant();
                        },
                    ),
                ),
                select_row(
                    "pane-header",
                    "Pane headers",
                    "Auto shows them on split panes in Comfortable density.",
                    &["Auto", "Always", "On hover", "Hidden"],
                    PANE_HEADERS
                        .iter()
                        .position(|h| *h == s.settings.pane_header)
                        .unwrap_or_default(),
                    |s, i, tx| {
                        s.settings.pane_header = PANE_HEADERS[i.min(PANE_HEADERS.len() - 1)];
                        // A header that was brought out by hand goes back with the old choice.
                        s.header_reveal = Default::default();
                        s.save();
                        tx.instant();
                    },
                ),
                disclosure(
                    "customize-panes",
                    "Customize panes",
                    "The space around each terminal's content.",
                    "",
                    |s, _| s.navigate(Page::Sub(SubPage::Panes)),
                ),
            ])],
        ),
        section(
            "Sidebar and tabs",
            [card([
                setting(
                    "Session sidebar",
                    "Hosts and their sessions beside the terminals.",
                    Switch::new(s.sidebar.is_open())
                        .label("Show session sidebar")
                        .hide_label()
                        .on_toggle_tx(|s: &mut Workspace, tx: &mut Tx| {
                            s.perform(Intent::Sidebar, tx)
                        })
                        .build(k)
                        .key("sidebar-switch"),
                ),
                setting(
                    "Tab position",
                    "Sidebar tabs show while the sidebar is open.",
                    segmented(
                        "tab-position",
                        &[
                            Segment::new("Top").icon(&TABS_TOP),
                            Segment::new("Sidebar").icon(&SIDEBAR),
                        ],
                        Some(usize::from(s.settings.vertical_tabs)),
                        |s, i, _| s.set_vertical_tabs(i == 1),
                    ),
                ),
                disclosure(
                    "customize-tabs",
                    "Customize tabs",
                    "Their width and their programs' icons.",
                    "",
                    |s, _| s.navigate(Page::Sub(SubPage::Tabs)),
                ),
            ])],
        ),
        section(
            "Notices",
            [card([select_row(
                "notice-position",
                "Position",
                "Where in-app notices appear.",
                &["Top right", "Bottom right"],
                corner,
                |s, i, _| {
                    s.set_notice_position(if i == 0 {
                        NoticePosition::TopRight
                    } else {
                        NoticePosition::BottomRight
                    })
                },
            )])],
        ),
    ])
}

/// Where a notification switch keeps its value.
type Field = fn(&mut NotificationsConfig) -> &mut bool;

/// A switch for one notification preference, saved as soon as it flips.
pub(super) fn notify_switch(key: &str, label: &str, on: bool, disabled: bool, field: Field) -> El {
    Switch::new(on)
        .label(label.to_owned())
        .hide_label()
        .disabled(disabled)
        .on_toggle(move |s: &mut Workspace| {
            let value = field(&mut s.settings.notifications);
            *value = !*value;
            s.save();
        })
        .build(kit())
        .key(key.to_owned())
}

fn notifications(s: &Workspace) -> El {
    let n = &s.settings.notifications;
    let off = !n.system_notifications;
    page([
        section(
            "",
            [card([
                setting(
                    "Activity in tabs",
                    "A word on a tab that needs you, failed, or finished while you were away.",
                    notify_switch(
                        "notify-tabs",
                        "Show activity in tabs",
                        n.show_in_tabs,
                        false,
                        |n| &mut n.show_in_tabs,
                    ),
                ),
                setting(
                    "Desktop notifications",
                    "For tabs you aren't looking at. Clicking one shows its tab.",
                    notify_switch(
                        "notify-system",
                        "Desktop notifications",
                        n.system_notifications,
                        false,
                        |n| &mut n.system_notifications,
                    ),
                ),
            ])],
        ),
        // Each state has a page of its own: how it shows in tabs and whether it notifies.
        section(
            "When a program",
            [card(Status::ALL.map(|status| {
                disclosure(
                    &format!("notify-status:{}", status.key()),
                    status.title(),
                    status.detail(),
                    &status.summary(n),
                    move |s, _| s.navigate(Page::Sub(SubPage::Status(status))),
                )
            }))],
        ),
        section(
            "Programs",
            [card([setting(
                "Messages from programs",
                "Notifications programs send themselves with OSC 9 or OSC 777.",
                notify_switch(
                    "notify-messages",
                    "Messages from programs",
                    n.program_messages,
                    off,
                    |n| &mut n.program_messages,
                ),
            )])],
        ),
    ])
}

fn terminal(s: &Workspace) -> El {
    let saved = &s.settings.font_family;
    let active = s.terminal_family.unwrap_or("IBM Plex Mono (bundled)");
    // The row's helper is one sentence. What terminals are drawn in now goes under the card,
    // and only while that isn't the family the row names.
    let drawn = if *saved != s.launch_saved_font {
        Some(format!(
            "{saved} is saved and applies when you reopen the client. Until then terminals use {active}."
        ))
    } else if s.terminal_family.is_none() {
        Some(format!(
            "{} isn't installed, so terminals use {active}.",
            s.launch_font
        ))
    } else {
        None
    };
    let fallbacks = if s.settings.font_fallbacks.is_empty() {
        "Automatic".to_owned()
    } else {
        s.settings.font_fallbacks.join(", ")
    };
    let shells = s.settings.hosts.iter().enumerate().map(|(i, host)| {
        disclosure(
            &format!("host-shell:{i}"),
            &host.label,
            "",
            host.shell.as_deref().unwrap_or("Server default"),
            move |s, tx| s.open_anchored(&format!("host-shell:{i}"), Intent::HostShell(i), tx),
        )
    });
    page([
        section(
            "Typography",
            std::iter::once(card([
                disclosure(
                    "font-change",
                    "Font family",
                    "Applies when you reopen the client.",
                    saved,
                    |s, tx| s.open_anchored("font-change", Intent::Fonts, tx),
                ),
                setting(
                    "Font size",
                    "From 8 to 48 pt, for every terminal.",
                    stepper(
                        "font",
                        format!("{} pt", s.settings.font_size),
                        |s, _| s.change_font(-1.),
                        |s, _| s.change_font(1.),
                    ),
                ),
                setting(
                    "Fallback fonts",
                    "For glyphs the font lacks, before the system's own fallbacks.",
                    value(fallbacks).truncate().max_w(260.),
                ),
            ]))
            .chain(drawn.map(|words| footnote(words).key("font-drawn"))),
        ),
        section(
            "Default shell",
            [
                card(shells),
                footnote(
                    "What new sessions, tabs and splits start on each host. Running terminals keep theirs.",
                ),
            ],
        ),
        section(
            "",
            [
                footnote("These settings are saved in"),
                mono_footnote(s.settings_path.display().to_string()).mt(-6.),
            ],
        ),
    ])
}

fn host_card(s: &Workspace, i: usize, last: bool) -> El {
    let host = &s.settings.hosts[i];
    let connected = s.hosts.get(i).is_some_and(|h| h.model.connected);
    let mut actions = row().gap(6.).center_y();
    // Quiet when healthy: only trouble gets words, beside the action that fixes it.
    if !connected {
        actions = actions
            .child(text("Offline").font(medium(12.)).color(rgb(warning())))
            .child(button(
                &format!("host-retry:{i}"),
                "Retry",
                Pill::Warning,
                move |s, tx| s.perform(Intent::RetryHost(i), tx),
            ));
    }
    actions = actions.child(button(
        &format!("host-edit:{i}"),
        "Edit…",
        Pill::Neutral,
        move |s, tx| s.perform(Intent::EditHost(i), tx),
    ));
    actions = actions.child(if last {
        unavailable(
            &format!("host-remove:{i}"),
            "Remove…",
            "The workspace needs at least one host",
        )
    } else {
        button(
            &format!("host-remove:{i}"),
            "Remove…",
            Pill::Danger,
            move |s, tx| s.perform(Intent::RemoveHost(i), tx),
        )
    });
    let mut address = row().gap(6.).center_y().min_w(0.).child(
        text(host.endpoint.clone())
            .font(mono(11.))
            .color(rgb(muted()))
            .truncate()
            .selectable(true),
    );
    if let Some(shell) = &host.shell {
        address = address.child(
            text(format!("· {shell}"))
                .font(mono(11.))
                .color(rgb(muted())),
        );
    }
    card([
        row_frame(true)
            .key(format!("host-row:{i}"))
            .gap(12.)
            .child(glyph(s.host_icon(i), 16., rgb(muted())))
            .child(
                col()
                    .grow(1.)
                    .min_w(0.)
                    .gap(2.)
                    .child(
                        text(host.label.clone())
                            .font(medium(13.))
                            .color(rgb(ink()))
                            .truncate(),
                    )
                    .child(address),
            )
            .child(actions),
        setting(
            "Remote control",
            "Let rex do and other clients on this host change the view in this window.",
            Switch::new(host.remote_control)
                .label(format!("Remote control for {}", host.label))
                .hide_label()
                .on_toggle(move |s: &mut Workspace| s.toggle_remote_control(i))
                .build(kit())
                .key(format!("remote-control:{i}")),
        ),
    ])
}

fn hosts(s: &Workspace) -> El {
    let last = s.settings.hosts.len() == 1;
    let cards = (0..s.settings.hosts.len()).map(|i| host_card(s, i, last));
    page([
        section(
            "This client",
            [card([disclosure(
                "client-label",
                "Client label",
                "Its name on every host. Takes effect when you reopen the app.",
                &s.settings.client_label(),
                |s, tx| s.perform(Intent::ClientLabel, tx),
            )])],
        ),
        section(
            "Hosts",
            cards.chain([
                row().px(2.).child(button(
                    "settings-add-host",
                    "Add host…",
                    Pill::Neutral,
                    |s, tx| s.perform(Intent::AddHost, tx),
                )),
                footnote(
                    "Removing a host only stops this app connecting to it; its sessions keep running.",
                ),
            ]),
        ),
    ])
}

/// Presentation only: preserve dispatch priority within each action/argument group.
struct ShortcutRow<'a> {
    binding: &'a rex_linux::keymap::ResolvedBinding,
    also_numpad: bool,
    arrows: bool,
}

fn shortcut_rows<'a>(
    effective: &[&'a rex_linux::keymap::ResolvedBinding],
    category: &str,
) -> Vec<ShortcutRow<'a>> {
    use super::actions::{SPECS, find};
    use rex_linux::keymap::Key;

    let mut list: Vec<_> = effective
        .iter()
        .copied()
        .filter(|b| {
            find(&b.binding.action)
                .map(|a| a.category())
                .unwrap_or("Server")
                == category
        })
        .collect();
    list.sort_by_cached_key(|b| {
        (
            SPECS
                .iter()
                .position(|s| s.name == b.binding.action)
                .unwrap_or(SPECS.len()),
            b.binding.action.clone(),
            b.binding
                .args
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
        )
    });
    let default_focus = |b: &&rex_linux::keymap::ResolvedBinding, direction: &str| {
        b.binding.action == "pane.focus"
            && b.binding.source.as_deref() == Some("linux")
            && b.binding.keys == [format!("ctrl+alt+{direction}")]
            && b.binding.args == Some(serde_json::json!({"direction": direction}))
    };
    let arrows = ["left", "right", "up", "down"]
        .iter()
        .all(|d| list.iter().any(|b| default_focus(b, d)));
    let mut rows: Vec<_> = list
        .iter()
        .map(|b| ShortcutRow {
            binding: b,
            also_numpad: false,
            arrows: false,
        })
        .collect();
    let mut hidden = std::collections::HashSet::new();
    for (i, b) in list.iter().enumerate() {
        if arrows
            && ["left", "right", "up", "down"]
                .iter()
                .any(|d| default_focus(b, d))
        {
            if default_focus(b, "down") {
                rows[i].arrows = true;
            } else {
                hidden.insert(i);
            }
        }
        let Some([chord]) = b.chords.as_deref() else {
            continue;
        };
        let Key::Physical(code) = &chord.key else {
            continue;
        };
        let equivalent = match *code {
            "NumpadAdd" => '+',
            "NumpadSubtract" => '-',
            "Numpad0" => '0',
            _ => continue,
        };
        if let Some(j) = list.iter().position(|other| {
            other.binding.action == b.binding.action
                && other.binding.args == b.binding.args
                && other.binding.source == b.binding.source
                && other.chords.as_deref().is_some_and(|chords| matches!(chords,
                    [other] if other.modifiers == chord.modifiers && other.key == Key::Character(equivalent)))
        }) {
            rows[j].also_numpad = true;
            hidden.insert(i);
        }
    }
    rows.into_iter()
        .enumerate()
        .filter_map(|(i, row)| (!hidden.contains(&i)).then_some(row))
        .collect()
}

/// The headings of the Shortcuts page, in order. Every action's `category()` is one of them
/// (a test checks), and an action this client does not know is listed under Server.
pub(super) const SHORTCUT_CATEGORIES: [&str; 6] = [
    "Navigate",
    "Create and close",
    "Panes",
    "Text and view",
    "Interface",
    "Server",
];

fn shortcuts(s: &Workspace) -> El {
    let mut sections = Vec::new();
    let effective = s.effective_bindings();
    for name in SHORTCUT_CATEGORIES {
        let list = shortcut_rows(&effective, name);
        if list.is_empty() {
            continue;
        }
        // A row reads as a menu row does: what it runs on the left, in the name Commands and
        // the menus use, and its keys on the right as the same keycaps.
        let rows = list.iter().enumerate().map(|(index, display)| {
            let binding = display.binding;
            let chords: Chords = if display.arrows {
                vec![vec!["Ctrl".into(), "Alt".into(), "Arrows".into()]]
            } else {
                let chords = binding.chords.as_ref().unwrap();
                chords.iter().map(|chord| chord.keycaps()).collect()
            };
            let source = match binding.binding.source.as_deref() {
                Some("linux") => None,
                Some("settings") => Some("shortcuts.json"),
                Some(source) => Some(source),
                None => Some("Server"),
            };
            let title = if display.arrows {
                "Focus adjacent pane".into()
            } else {
                super::actions::binding_title(&binding.binding)
            };
            let numpad = display.also_numpad.then_some("Also on the numpad");
            let detail: Vec<&str> = source.into_iter().chain(numpad).collect();
            let detail = detail.join(" · ");
            row_frame(!detail.is_empty())
                .key(format!("shortcut-{name}-{index}"))
                .child(words(&title, &detail))
                .child(
                    keycaps(&chords, false)
                        .key(format!("shortcut-keys-{name}-{index}"))
                        .label(spell(&chords)),
                )
        });
        sections.push(section(name, [card(rows)]));
    }
    // The pointer's buttons are not keys: plain words, as in Commands.
    sections.push(section(
        "Pointer",
        [card([setting(
            "Back and forward in history",
            "",
            value("Mouse Back / Forward"),
        )])],
    ));
    let diagnostics = &s.effective_keymap().diagnostics;
    if !diagnostics.is_empty() || s.model().keymap_error.is_some() {
        let problem = |words: String| {
            text(words)
                .font(ui(11.5))
                .color(rgb(warning()))
                .wrap()
                .grow(1.)
                .shrink()
        };
        let mut rows = Vec::new();
        if let Some(error) = &s.model().keymap_error {
            rows.push(row_frame(false).child(problem(error.clone())));
        }
        for (index, found) in diagnostics.iter().enumerate() {
            rows.push(
                row_frame(false).child(
                    problem(format!(
                        "{}: {}",
                        found.source.as_deref().unwrap_or("keymap"),
                        found.message
                    ))
                    .key(format!("shortcut-problem-{index}")),
                ),
            );
        }
        sections.push(section("Problems", [card(rows)]));
    }
    let path = |key: &str, words: String| {
        row_frame(false).child(
            text(words)
                .key(key.to_owned())
                .font(mono(11.))
                .color(rgb(muted()))
                .selectable(true)
                .wrap()
                .grow(1.)
                .shrink(),
        )
    };
    let mut files = Vec::new();
    if let Some(snapshot) = s.keymap_snapshot() {
        files.push(path(
            "server-shortcuts-path",
            format!(
                "Server: {}{}",
                snapshot.reply.config.path,
                if snapshot.reply.config.exists {
                    ""
                } else {
                    " (not created)"
                }
            ),
        ));
    }
    files.push(path(
        "shortcuts-path",
        rex_linux::settings::shortcuts::Shortcuts::path(&s.settings_path)
            .display()
            .to_string(),
    ));
    sections.push(section(
        "Files",
        [
            card(files),
            footnote(
                "Edit the overrides file, then return to Rex to reload. Escape cancels a sequence; unbound keys reach the terminal.",
            ),
        ],
    ));
    page(sections)
}

/// A page's rows. `settings_nav` builds it to draw it, and to read its rows for Search
/// Settings.
pub(super) fn body(s: &Workspace, page: Page) -> El {
    match page {
        Page::Section(section) => match section {
            SettingsSection::Interface => interface(s),
            SettingsSection::Themes => super::settings_pages::themes(s),
            SettingsSection::Terminal => terminal(s),
            SettingsSection::Notifications => notifications(s),
            SettingsSection::Integrations => super::integrations::section(s),
            SettingsSection::Hosts => hosts(s),
            SettingsSection::Shortcuts => shortcuts(s),
            SettingsSection::Updates => super::updates::section(s),
            SettingsSection::About => super::about::section(s),
        },
        Page::Sub(SubPage::Panes) => super::settings_pages::panes(s),
        Page::Sub(SubPage::Tabs) => super::tab_settings::page(s),
        Page::Sub(SubPage::Status(status)) => super::settings_pages::status(s, status),
    }
}

/// How wide the page beside the sidebar is, its insets included, in a window `viewport` wide.
pub(super) fn page_width(viewport: f32) -> f32 {
    let width = PANEL_WIDTH.min(viewport * 0.92);
    width - NAV_WIDTH.min(width * 0.3) - 1.
}

/// The dialog's surface: the sidebar, and the page under its header. The page scrolls with a
/// fade at whichever edge hides content and a thumb while there is more than fits.
fn surface(s: &Workspace, viewport: [f32; 2]) -> El {
    let page = s.settings_page();
    let body = super::settings_nav::draw(s, page);
    let width = PANEL_WIDTH.min(viewport[0] * 0.92);
    // Under the toolbar, which stays whole above it, with a margin at both ends.
    let height = PANEL_HEIGHT.min((viewport[1] - TOOLBAR - 2. * CLEARANCE).max(0.));
    let nav_width = NAV_WIDTH.min(width * 0.3);
    let (page_width, page_height) = (page_width(viewport[0]), height - HEADER);
    let thumb = s.settings_fx.thumb.clone();
    // Where the page is scrolled to, for Back to return to it there.
    let offset = s.settings_nav.offset();
    let scroller = col()
        .key(format!("settings-scroll:{}", page.key()))
        .grow(1.)
        .min_h(0.)
        .w_full()
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_edge_fade(16.)
        .on_scroll_offset(move |at: ScrollPos| {
            offset.set(at.offset[1]);
            let hidden = at.max[1];
            if hidden < 1. {
                thumb.set([0.; 4]);
                return;
            }
            let length = (page_height * page_height / (page_height + hidden)).max(24.);
            let top = at.offset[1] / hidden * (page_height - length - 8.);
            thumb.set([page_width - 7., top + 4., 4., length]);
        })
        .child(body.px(PAGE_INSET).pt(2.).pb(24.));
    let content = col()
        .grow(1.)
        .min_w(0.)
        .h_full()
        .child(super::settings_shell::header(s, page))
        .child(
            col()
                .grow(1.)
                .min_h(0.)
                .w_full()
                .child(scroller)
                .child(s.settings_fx.thumb_el()),
        );
    // The pointer's Back and Forward buttons, anywhere over the dialog. The window's own
    // handler for them is behind the modal, so the dialog takes them itself; the primary
    // button's press ends here as it does on the dialog, leaving focus where it was.
    let frame = row()
        .key("settings-frame")
        .w_full()
        .h_full()
        .pointer_buttons([PointerButton::Back, PointerButton::Forward])
        .preserve_focus_on_press(true)
        .on_press_tx(|s: &mut Workspace, tx: &mut Tx| {
            let direction = match tx.button() {
                PointerButton::Back => -1,
                PointerButton::Forward => 1,
                _ => return,
            };
            s.settings_history(direction, tx);
        })
        .child(super::settings_shell::sidebar(s, nav_width))
        .child(el().w(1.).h_full().fill(white(0.06)))
        .child(content);
    row()
        .w(width)
        .h(height)
        // Alt Left and Alt Right, whichever of its controls has the keyboard.
        .on_key(|s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| s.settings_key(ev, tx))
        .child(frame)
}

pub fn settings(s: &Workspace, viewport: [f32; 2]) -> El {
    modal_layer(s, surface(s, viewport), false, TOOLBAR)
}

/// Settings under a list opened from one of its controls: drawn where it was, taking no input.
pub fn settings_under(s: &Workspace, viewport: [f32; 2]) -> El {
    modal_layer(s, surface(s, viewport), true, TOOLBAR)
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;
    use crate::app::actions;
    use rex_linux::keymap::Overrides;
    use serde_json::json;

    #[test]
    fn defaults_are_compact_and_in_registry_order() {
        let map = actions::resolve(&Overrides::default());
        let effective = actions::effective(&map, &[]);
        let panes = shortcut_rows(&effective, "Panes");
        let focus: Vec<_> = panes
            .iter()
            .filter(|r| r.binding.binding.action == "pane.focus")
            .collect();
        assert_eq!(focus.len(), 1);
        assert!(focus[0].arrows);
        let interface = shortcut_rows(&effective, "Interface");
        assert_eq!(interface.iter().filter(|r| r.also_numpad).count(), 3);
        assert!(
            !interface
                .iter()
                .any(|r| r.binding.binding.keys[0].contains("Numpad"))
        );
        for category in [
            "Navigate",
            "Create and close",
            "Panes",
            "Text and view",
            "Interface",
        ] {
            let rows = shortcut_rows(&effective, category);
            let positions: Vec<_> = rows
                .iter()
                .map(|r| {
                    actions::SPECS
                        .iter()
                        .position(|s| s.name == r.binding.binding.action)
                        .unwrap()
                })
                .collect();
            assert!(positions.windows(2).all(|w| w[0] <= w[1]));
        }
        let scroll: Vec<_> = effective
            .iter()
            .filter(|b| b.binding.action == "pane.scroll")
            .map(|b| actions::binding_title(&b.binding))
            .collect();
        assert!(scroll.contains(&"Scroll up · 20 lines".into()));
        assert!(scroll.contains(&"Scroll down · 20 lines".into()));
    }

    #[test]
    fn custom_focus_is_not_folded_and_try_order_is_retained() {
        let map = actions::resolve(&Overrides::parse(&json!([
            {"keys":"ctrl+alt+left","action":"pane.focus","args":{"direction":"right"}},
            {"keys":"ctrl+j","action":"client.tab.new"},
            {"keys":"ctrl+y","action":"client.tab.new"},
            {"keys":"ctrl+[Numpad0]","action":"client.zoom.in"}
        ])));
        let effective = actions::effective(&map, &[]);
        assert!(shortcut_rows(&effective, "Panes").iter().all(|r| !r.arrows));
        let expected: Vec<_> = effective
            .iter()
            .filter(|b| b.binding.action == "client.tab.new")
            .map(|b| &b.binding.keys)
            .collect();
        let rows = shortcut_rows(&effective, "Create and close");
        let actual: Vec<_> = rows
            .iter()
            .filter(|r| r.binding.binding.action == "client.tab.new")
            .map(|r| &r.binding.binding.keys)
            .collect();
        assert_eq!(actual, expected);
        assert!(
            shortcut_rows(&effective, "Interface")
                .iter()
                .any(|r| r.binding.binding.keys == ["ctrl+[Numpad0]"] && !r.also_numpad)
        );
    }
}
