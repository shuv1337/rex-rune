//! Settings: the sidebar on the left and the chosen page on the right, under its header
//! (`settings_nav` draws both and keeps the history between pages). Pages are built from
//! `settings_kit` (sections, cards, rows and their controls).
use super::{
    dialogs::modal_layer,
    panel::{Intent, SettingsSection},
    settings_kit::{
        NAV_WIDTH, PANEL_HEIGHT, PANEL_WIDTH, Segment, button, card, disclosure, footnote, page,
        row_frame, section, segmented, select_row, setting, stepper, unavailable,
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
        super::interface_page::toolbar(s),
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
                    &SubPage::Panes.opener(),
                    "Customize panes",
                    "Padding, the active pane's cues and focus.",
                    "",
                    |s, tx| s.open_sub_page(SubPage::Panes, tx),
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
                    &SubPage::Tabs.opener(),
                    "Customize tabs",
                    "Their width and their programs' icons.",
                    "",
                    |s, tx| s.open_sub_page(SubPage::Tabs, tx),
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
        super::interface_page::animation(s),
        super::interface_page::window(s),
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
                let sub = SubPage::Status(status);
                disclosure(
                    &sub.opener(),
                    status.title(),
                    status.detail(),
                    &status.summary(n),
                    move |s, tx| s.open_sub_page(sub, tx),
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

/// A page's rows. `settings_nav` builds it to draw it, and to read its rows for Search
/// Settings.
pub(super) fn body(s: &Workspace, page: Page) -> El {
    match page {
        Page::Section(section) => match section {
            SettingsSection::Interface => interface(s),
            SettingsSection::Themes => super::settings_pages::themes(s),
            SettingsSection::Terminal => super::terminal_page::terminal(s),
            SettingsSection::Notifications => notifications(s),
            SettingsSection::Integrations => super::integrations::section(s),
            SettingsSection::Hosts => hosts(s),
            SettingsSection::Shortcuts => super::shortcuts_page::page(s),
            SettingsSection::Updates => super::updates::section(s),
            SettingsSection::About => super::about::section(s),
        },
        Page::Sub(SubPage::Panes) => super::interface_page::panes(s),
        Page::Sub(SubPage::AdvancedFont) => super::font_page::advanced(s),
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
    // Under the toolbar's row, which stays whole above it, with a margin at both ends.
    let toolbar = super::toolbar_reveal::row_height(s);
    let height = PANEL_HEIGHT.min((viewport[1] - toolbar - 2. * CLEARANCE).max(0.));
    let nav_width = NAV_WIDTH.min(width * 0.3);
    // What stays above the page while it scrolls: the Shortcuts filter.
    let pinned = (page == Page::Section(SettingsSection::Shortcuts))
        .then(|| super::shortcuts_page::filter_field(s, PAGE_INSET));
    let above = HEADER
        + pinned
            .as_ref()
            .map_or(0., |_| super::shortcuts_page::FILTER);
    let (page_width, page_height) = (page_width(viewport[0]), height - above);
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
        .children(pinned)
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
    let mut dialog = row()
        .w(width)
        .h(height)
        // Alt Left and Alt Right, whichever of its controls has the keyboard.
        .on_key(|s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| s.settings_key(ev, tx))
        .child(frame);
    // Leaving a page opened from a section's row hands the keyboard back to that row: this
    // empty marker goes with the page, and Rune returns focus that was in the dialog.
    if let Page::Sub(sub) = page {
        dialog = dialog.child(
            el().key("page-focus-return")
                .absolute(0., 0.)
                .size(0., 0.)
                .focus_return(sub.opener()),
        );
    }
    dialog
}

pub fn settings(s: &Workspace, viewport: [f32; 2]) -> El {
    modal_layer(
        s,
        surface(s, viewport),
        false,
        super::toolbar_reveal::row_height(s),
    )
}

/// Settings under a list opened from one of its controls: drawn where it was, taking no input.
pub fn settings_under(s: &Workspace, viewport: [f32; 2]) -> El {
    modal_layer(
        s,
        surface(s, viewport),
        true,
        super::toolbar_reveal::row_height(s),
    )
}
