//! The pane area: server-placed terminals as floating cards in 8pt gutters, each with a header
//! (its program's glyph, label, activity, exception pill and the pane's buttons), drag handles
//! in the gutters, and empty states. A tab with a single pane draws it edge to edge instead.
//! Compact density removes card decorations and gaps from every placement. Where a header is,
//! and whether it is a row or floats over the terminal, is `pane_header`'s to say.
//! Cards spring when the layout changes; the terminal grid inside never animates.
use super::{
    activity::{PaneProgram, pane_activity, progress_bar, tab_progress, tab_word},
    drag, find, floating_drag,
    pane_header::{self, Placement, full_bleed},
    panel::{Form, Intent, PanelKind},
    tab_icons,
    term::{self, CursorLook, GridLook, Metrics, MouseGesture},
    theme::*,
    widgets::{Pill, pill},
    workspace::{DividerDrag, Workspace},
};
use rex_rune::{
    connection::Command,
    layers, layout,
    settings::{PaneDensity, PaneHeader},
    terminal::selection::Phase as SelectionPhase,
};
use rexrs::api::session::Window;
use rune::{Cursor, El, Phase, PointerButton, PointerEvent, Tx, WheelEvent, col, el, row, text};
use std::borrow::Cow;

/// The area cards are laid out in, in the pane area's own points.
pub(super) fn inner(s: &Workspace) -> [f32; 4] {
    let [_, _, aw, ah] = s.area;
    if s.settings.pane_density == PaneDensity::Compact {
        return [0., 0., aw, ah];
    }
    [
        MARGIN,
        super::toolbar_reveal::top_margin(s),
        (aw - 2. * MARGIN).max(0.),
        (ah - MARGIN - super::toolbar_reveal::top_margin(s)).max(0.),
    ]
}

/// A placement's card rect: its share of the inner area, inset by half a gutter wherever it
/// meets another card.
pub fn card_rect(s: &Workspace, rect: [f64; 4]) -> [f32; 4] {
    let [ix, iy, iw, ih] = inner(s);
    let [x, y, w, h] = rect;
    let half = if s.settings.pane_density == PaneDensity::Compact {
        0.
    } else {
        GUTTER / 2.
    };
    let (l, t) = (x > 1e-6, y > 1e-6);
    let (r, b) = (x + w < 1. - 1e-6, y + h < 1. - 1e-6);
    let mut c = [
        ix + x as f32 * iw,
        iy + y as f32 * ih,
        w as f32 * iw,
        h as f32 * ih,
    ];
    if l {
        c[0] += half;
        c[2] -= half;
    }
    if t {
        c[1] += half;
        c[3] -= half;
    }
    if r {
        c[2] -= half;
    }
    if b {
        c[3] -= half;
    }
    c[2] = c[2].max(0.);
    c[3] = c[3].max(0.);
    c
}

enum CardState {
    Focused,
    Target,
    Plain,
}

fn header(
    s: &Workspace,
    window: &Window,
    block: &str,
    width: f32,
    focused: bool,
    m: &Metrics,
    lead: f32,
) -> El {
    let label = s.pane_name(block);
    let activity = pane_activity(s.model(), block);
    let word = pane_word_of(s, block);
    let chip = pane_header::zoomed_chip(window, block);
    let id = block.to_owned();
    let menu_id = block.to_owned();
    let fade = pane_header::header_opacity(s, block);
    let h = row()
        .key(format!("pane-header:{block}"))
        .hover_group()
        .w_full()
        .h(CARD_HEADER)
        // On a card the glyph clears the corner; edge to edge it lines up with the
        // terminal's text under it, and the name follows it.
        .pl(lead)
        // The trailing buttons stay clear of the card's 12 pt corner, where a hover fill
        // 4 pt from the edge all but touched the curve of the border.
        .pr(pane_header::HEADER_TAIL)
        .gap(pane_header::HEADER_GAP)
        .center_y()
        .preserve_focus_on_press(true);
    let h = if let Some((host_id, layer_id)) = floating_drag::draggable_header(s, window, block) {
        // Clicks under the drag slop still focus the pane on release; drags send only
        // `ResizeLayer` and never fire an accidental focus press on a horizontal move.
        floating_drag::attach_drag(h, host_id, layer_id, id, layers::Grip::Move)
    } else {
        let h = if drag::tiled_split(window)
            && layers::layer_of(window, block).is_some_and(|layer| layer.kind == "tiled")
            && let Some(window) = &window.window_id
        {
            let item = drag::Item::Pane(s.hosts[s.active].id, window.clone(), id.clone());
            let h = if s.rearrange.dimmed(&item) {
                h.opacity(0.85)
            } else {
                h
            };
            drag::attach(h, item)
        } else {
            h
        };
        h.on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
            s.clear_focus_history_walk();
            if s.focused_block().as_deref() != Some(id.as_str()) {
                s.exit_copy_mode();
                if let Some(t) = s.terminals.get(&s.term_key(&id)) {
                    let _ = t.connection.commands.focus();
                }
            }
            tx.blur();
        })
    }
    .on_secondary_press(move |s: &mut Workspace, p, _| {
        s.pane_menu(menu_id.clone(), Some((p.x, p.y)))
    });
    // The pane's own program before its name, as the title row draws the lone pane's: the
    // tab glyph rule, lit with its pane, and faded with the name where headers fade. The
    // glyph is decided from the room the row will have: its pads and gaps, the buttons it
    // draws, and the word, chip and reconnecting strip beside the name. A header too narrow
    // for all of them keeps its name and buttons instead, where the glyph would push Close out
    // of the pane or Retry over the buttons.
    let strip = strip(s, block, width, m);
    let extras = pane_header::extras(
        word.as_ref().map(|(word, _)| word.as_ref()),
        chip.is_some(),
        strip.as_ref().map(|_| label.as_str()),
    );
    let h = if pane_header::glyph_fits(width, lead, pane_header::buttons(width), extras) {
        let icons = tab_icons::icons_of(
            &[PaneProgram::of(s.model(), block)],
            s.settings.interface.tab_icon_count.min(1),
        );
        tab_icons::append_restoring(
            h,
            &format!("pane-icon:{block}"),
            &icons,
            focused,
            s.settings.interface.tab_icon_color,
            fade,
        )
    } else {
        h
    }
    .child(
        text(label.clone())
            .key(format!("pane-name:{block}"))
            .font(medium(11.5))
            .color(rgb(if focused { ink() } else { muted() }).alpha(fade))
            .group_color(rgb(if focused { ink() } else { muted() }))
            .truncate()
            .help_when_truncated(label)
            .shrink(),
    );
    let h = match chip {
        Some(chip) => h.child(chip),
        None => h,
    };
    // The label truncates first; the word always stays whole.
    let h = match &word {
        Some(word) => h.child(pane_word_el(s, block, word, 11.)),
        None => h,
    }
    .child(el().grow(1.));
    let h = match strip {
        Some(strip) => h.child(strip),
        None => h,
    };
    let h = match tab_progress(&activity).filter(|_| s.settings.notifications.show_in_tabs) {
        Some(progress) => h.child(progress_bar(
            format!("pane-progress:{block}"),
            progress,
            14.,
        )),
        None => h,
    };
    h.child(pane_header::actions(s, window, block, width, focused))
}

/// The tab's words for this pane alone, at `size`; Done and Bell stay with the tab, which
/// they latch.
pub(super) fn pane_word(s: &Workspace, block: &str, size: f32) -> Option<El> {
    let word = pane_word_of(s, block)?;
    Some(pane_word_el(s, block, &word, size))
}

/// The word this pane's header draws beside its name, with its colour, when **Show activity in
/// tabs** shows words at all.
fn pane_word_of(s: &Workspace, block: &str) -> Option<(Cow<'static, str>, u32)> {
    tab_word(&pane_activity(s.model(), block), false, false)
        .filter(|_| s.settings.notifications.show_in_tabs)
}

/// That word as an element, with the program's message as its tooltip.
fn pane_word_el(
    s: &Workspace,
    block: &str,
    (word, colour): &(Cow<'static, str>, u32),
    size: f32,
) -> El {
    let w = text(word.clone())
        .key(format!("pane-word:{block}"))
        .font(medium(size))
        .color(rgb(*colour));
    match pane_activity(s.model(), block).msg.clone() {
        Some(msg) => w.help(msg),
        None => w,
    }
}

/// The exception strip for a pane that is reconnecting, with its Retry action.
pub(super) fn strip(s: &Workspace, block: &str, width: f32, m: &Metrics) -> Option<El> {
    let key = s.term_key(block);
    let t = s.terminals.get(&key);
    let status = t.map(|t| t.status());
    let pill_label = status.as_ref().and_then(|st| {
        let hidden = t.map_or(0, |t| t.hidden_rows(m.cell));
        let rows = if hidden > 0 {
            format!(
                " · {hidden} row{} hidden",
                if hidden == 1 { "" } else { "s" }
            )
        } else {
            String::new()
        };
        (!st.live).then(|| (format!("Reconnecting · input paused{rows}"), "Retry"))
    });
    pill_label.map(|(msg, action)| {
        let target = key.clone();
        let status_text = status.map(|s| s.status).unwrap_or_default();
        let mut p = row()
            .key("strip")
            .h(22.)
            .pl(10.)
            .pr(3.)
            .gap(8.)
            .center_y()
            .radius(11.)
            .fill(rgb(mix(warning(), card(), 0.1)))
            .border(1., rgb(warning()).alpha(0.35))
            .shrink()
            .min_w(0.);
        if width >= 360. {
            p = p.child(
                text(msg)
                    .font(medium(11.))
                    .color(rgb(warning()))
                    .truncate()
                    .shrink()
                    .help(status_text),
            );
        }
        p.child(
            pill(
                &format!("strip-action:{block}"),
                action,
                Pill::Warning,
                18.,
                move |s: &mut Workspace, tx: &mut Tx| {
                    s.terminal_command(&target, Command::Reconnect);
                    tx.blur();
                },
            )
            .focusable(false)
            .preserve_focus_on_press(true)
            .tab_index(-1),
        )
    })
}

pub(super) fn outlines(s: &Workspace, window: &Window, flat: bool) -> bool {
    s.settings.highlights_active_pane() && (!flat || layout::placements(window, None).0.len() > 1)
}

/// `card_bleed` fills the pane area. Compact density also draws flat, at its own bounds.
#[allow(clippy::too_many_arguments)]
fn pane_card(
    s: &Workspace,
    window: &Window,
    block: &str,
    rect: [f32; 4],
    m: &Metrics,
    state: CardState,
    input: bool,
    card_bleed: bool,
) -> El {
    let bleed = card_bleed || s.settings.pane_density == PaneDensity::Compact;
    let lead = pane_header::lead(s, card_bleed);
    let key = s.term_key(block);
    // The search field has the terminal's top-right corner, so nothing floats over it.
    let searching = s.terminals.get(&key).is_some_and(|t| t.search.is_some());
    let placement = match pane_header::placement(s, window, block) {
        Placement::Floating if searching && s.grabbed_header() != Some(block) => Placement::None,
        placement => placement,
    };
    let headed = placement == Placement::Row;
    let id = block.to_owned();
    let (drop_id, hover_id) = (block.to_owned(), block.to_owned());
    let focused = matches!(state, CardState::Focused);
    let mut c = col()
        .key(format!("pane:{block}"))
        .absolute(rect[0], rect[1])
        .size(rect[2], rect[3])
        .fill(rgb(card()))
        .motion(SNAPPY)
        // Cards appear and leave at once (a tab or session switch never crossfades terminals);
        // only a card that stays springs to its new place.
        .transition(no_presence())
        .on_secondary_press(move |s: &mut Workspace, p, _| {
            s.pane_menu(id.clone(), Some((p.x, p.y)))
        })
        .on_drop_hover(move |s: &mut Workspace, inside, _| {
            if inside && !s.in_copy_mode() {
                s.drop_hover = Some(hover_id.clone());
            } else if s.drop_hover.as_ref() == Some(&hover_id) {
                s.drop_hover = None;
            }
        })
        .on_drop_files(move |s: &mut Workspace, paths, _| {
            s.drop_hover = None;
            s.drop_files(&drop_id, paths);
        });
    // A flat single pane has no neighbour to distinguish; cards (including zoomed
    // cards) retain the existing density-following cue unless explicitly overridden.
    let outlined = focused && outlines(s, window, bleed);
    let ring = bleed && (outlined || s.drop_hover.as_deref() == Some(block));
    if !bleed {
        c = c
            .radius(CARD_RADIUS)
            .continuous_corners()
            .clip_rounded(CARD_RADIUS);
        c = match state {
            CardState::Focused if outlined => c
                .border_gradient(1., rgb(accent()).alpha(0.85), rgb(accent()).alpha(0.5))
                .box_shadow(shadow(0., 0., 24., rgb(accent()).alpha(0.2)))
                .box_shadow(shadow(0., 14., 40., drop_shadow(0.55))),
            CardState::Target => c.border(1., rgb(accent()).alpha(0.85)).box_shadow(shadow(
                0.,
                10.,
                30.,
                drop_shadow(0.45),
            )),
            CardState::Focused | CardState::Plain => c
                .border_gradient(1., tint(0.10), tint(0.035))
                .box_shadow(shadow(0., 10., 30., drop_shadow(0.45))),
        };
    }
    let clear = if headed {
        pane_header::clearance(s, rect)
    } else {
        0.
    };
    if headed {
        if clear > 0. {
            c = c.child(el().key("pane-header-clear").w_full().h(clear));
        }
        c = c.child(header(s, window, block, rect[2], focused, m, lead));
    }
    let c = match s.terminals.get(&key) {
        Some(t) => {
            let keyboard =
                input && s.window_focused && !t.search.as_ref().is_some_and(|edit| edit.focused);
            let cursor = if keyboard {
                CursorLook::Block
            } else {
                CursorLook::Hollow
            };
            let colors = s.term_colors();
            let look = GridLook {
                cursor: Some(cursor),
                cursor_force: s
                    .settings
                    .terminal
                    .cursor_force
                    .then(|| s.settings.terminal.cursor_style.shape()),
                ime: (keyboard && !s.copy_mode_on(&key)).then(|| s.terminal_option_as_alt()),
                colors,
            };
            let (k1, k2, k3) = (key.clone(), key.clone(), key.clone());
            let (k4, k5, k6) = (key.clone(), key.clone(), key.clone());
            let menu_block = block.to_owned();
            let mut grid = el()
                .key(format!("grid:{block}"))
                .label({
                    // An untitled pane is already called "Terminal".
                    let name = s.pane_name(block);
                    if name == "Terminal" {
                        name
                    } else {
                        format!("Terminal {name}")
                    }
                })
                .pin(Some(0.), Some(0.), Some(0.), Some(0.))
                // A program that tracks the mouse owns the pointer, as in Ghostty; a link under
                // Ctrl is the exception.
                .cursor(if t.link.is_some() {
                    Cursor::Pointer
                } else if t.tracking() {
                    Cursor::Default
                } else {
                    Cursor::Text
                });
            // Middle presses go to mouse-reporting programs, or opt-in PRIMARY paste.
            if t.tracking() || s.primary_selection() {
                grid = grid.pointer_buttons([PointerButton::Middle]);
            }
            let grid = grid
                // The terminal never animates: not its cells, colours or place, even while the
                // card around it springs. The card clips it.
                .instant_changes()
                .motion(STILL)
                .on_layout_tx(move |s: &mut Workspace, r, tx| {
                    let cell = s.metrics().cell;
                    let Some(t) = s.terminals.get_mut(&k1) else {
                        return;
                    };
                    // Moving the origin or changing unused cell slack needs no second
                    // view pass. Repeated padding steps otherwise consume Rune's layout
                    // feedback budget before the next whole row/column changes.
                    let old_grid = ((t.rect[2] / cell.0).floor(), t.visible_rows(cell));
                    let old_corner = t.find.corner;
                    t.rect = r;
                    t.resize_if_owner(cell);
                    let same_grid = old_grid == ((r[2] / cell.0).floor(), t.visible_rows(cell));
                    s.place_find(&k1);
                    if same_grid && s.terminals[&k1].find.corner == old_corner {
                        tx.live_only();
                    }
                })
                .on_drag(move |s: &mut Workspace, d, tx| {
                    let cell = s.metrics().cell;
                    let focused = s.focused_block().as_deref() == Some(k2.1.as_str());
                    let copies = s.settings.terminal.copy_on_selection || s.primary_selection();
                    let button = match tx.button() {
                        PointerButton::Primary => "left",
                        PointerButton::Middle => "middle",
                        _ => return,
                    };
                    if d.phase == Phase::Down {
                        s.clear_focus_history_walk();
                    }
                    if s.in_copy_mode() {
                        if button == "middle" {
                            if let Some(t) = s.terminals.get_mut(&k2) {
                                t.primary_press = None;
                                t.mouse = None;
                            }
                            return;
                        }
                        if d.phase == Phase::Down {
                            let same_pane = s.copy_mode_on(&k2);
                            s.exit_copy_mode();
                            if let Some(t) = s.terminals.get_mut(&k2) {
                                if !same_pane && !focused {
                                    let _ = t.connection.commands.focus();
                                }
                                t.mouse = None;
                                t.mouse_cell = Some(t.cell_at(d.x, d.y, cell));
                                t.pressed_link = None;
                                t.primary_press = None;
                                t.selecting = false;
                            }
                        }
                        return;
                    }
                    let Some(t) = s.terminals.get_mut(&k2) else {
                        return;
                    };
                    let mods = tx.mods();
                    let at = t.cell_at(d.x, d.y, cell);
                    if d.phase == Phase::Down {
                        t.primary_press =
                            (button == "middle" && !t.tracking()).then_some((d.x, d.y));
                        if !focused {
                            let _ = t.connection.commands.focus();
                        }
                        // Ctrl+click opens a link, even over a program that tracks the mouse.
                        t.pressed_link = None;
                        if button == "left"
                            && super::platform::keys::primary(mods)
                            && let Some((_, link)) = t.link_at(d.x, d.y, cell)
                        {
                            t.clear_selection();
                            t.mouse = None;
                            t.pressed_link = Some(link.uri);
                            return;
                        }
                        // Shift keeps local selection, as Ghostty and xterm do.
                        if t.tracking() && !mods.shift && t.status().live {
                            t.clear_selection();
                            t.mouse = Some(MouseGesture { button });
                            t.send_mouse("press", Some(button), at, mods);
                            return;
                        }
                        t.mouse = None;
                    }
                    // A Ctrl+press on a link opens it if released over the same link.
                    if let Some(uri) = &t.pressed_link {
                        if d.phase != Phase::Up {
                            tx.live_only();
                            return;
                        }
                        let same = t
                            .link_at(d.x, d.y, cell)
                            .is_some_and(|(_, l)| &l.uri == uri);
                        let uri = t.pressed_link.take().filter(|_| same);
                        if let Some(uri) = uri {
                            s.open_link(uri);
                        }
                        return;
                    }
                    if let Some(gesture) = t.mouse {
                        match d.phase {
                            Phase::Move => {
                                // The view reads nothing a forwarded drag changes.
                                tx.live_only();
                                if t.mouse_cell != Some(at) {
                                    t.send_mouse("motion", Some(gesture.button), at, mods)
                                }
                            }
                            Phase::Up => {
                                t.mouse = None;
                                t.send_mouse("release", Some(gesture.button), at, mods);
                            }
                            _ => {}
                        }
                        return;
                    }
                    if button != "left" {
                        if let Some((x, y)) = t.primary_press
                            && (d.x - x).hypot(d.y - y) > 3.
                        {
                            t.primary_press = None;
                        }
                        if d.phase == Phase::Up {
                            let paste = t.primary_press.take().is_some() && !t.tracking();
                            if paste {
                                s.paste_primary(&k2.1);
                            }
                        }
                        return;
                    }
                    // Focus loss cancels the gesture, including later motion and release.
                    if d.phase != Phase::Down && !t.selecting {
                        return;
                    }
                    let i = t.index(d.x, d.y, cell);
                    let phase = match d.phase {
                        Phase::Down => {
                            t.begin_selection(i, d.clicks);
                            SelectionPhase::Down(d.clicks)
                        }
                        Phase::Move => {
                            t.extend_selection(i);
                            SelectionPhase::Move
                        }
                        Phase::Up => SelectionPhase::Up,
                    };
                    t.selection_event(phase, d.x, d.y, cell);
                    if copies && d.phase == Phase::Up {
                        t.want_selected_text();
                    }
                })
                // Pointer motion with no button held: Ctrl shows the link under it, and programs
                // that track every move (1003) hear it (the server drops it for the others).
                // Only the focused pane reports it.
                .on_pointer_move_local(move |s: &mut Workspace, p: PointerEvent, tx: &mut Tx| {
                    let cell = s.metrics().cell;
                    let focused = s.focused_block().as_deref() == Some(k4.1.as_str());
                    let in_copy = s.copy_mode_on(&k4);
                    let Some(t) = s.terminals.get_mut(&k4) else {
                        tx.live_only();
                        return;
                    };
                    if t.selecting {
                        tx.live_only();
                        return;
                    }
                    // Holding Ctrl shows the link a click would open.
                    let link = if super::platform::keys::primary(tx.mods()) {
                        t.link_at(p.x, p.y, cell)
                    } else {
                        None
                    };
                    let same = link.as_ref().map(|(_, l)| l) == t.link.as_ref().map(|(_, l)| l);
                    t.link = link;
                    if same {
                        tx.live_only();
                    }
                    let at = t.cell_at(p.x, p.y, cell);
                    if in_copy
                        || tx.mods().shift
                        || !focused
                        || t.mouse.is_some()
                        || !t.tracking()
                        || !t.status().live
                    {
                        if in_copy {
                            t.mouse_cell = Some(at);
                        }
                        return;
                    }
                    if t.mouse_cell != Some(at) {
                        t.send_mouse("motion", None, at, tx.mods());
                    }
                })
                .on_hover_change(move |s: &mut Workspace, inside: bool, _: &mut Tx| {
                    if !inside && let Some(t) = s.terminals.get_mut(&k6) {
                        t.link = None;
                    }
                })
                // Right-click opens the pane menu, even over a program that tracks the mouse;
                // Shift+right-click goes to such a program (sent on release, when Rune reports
                // it).
                .on_secondary_press_local(move |s: &mut Workspace, p: PointerEvent, tx: &mut Tx| {
                    s.clear_focus_history_walk();
                    let cell = s.metrics().cell;
                    let mods = tx.mods();
                    let focused = s.focused_block().as_deref() == Some(k5.1.as_str());
                    if s.copy_mode_on(&k5) && mods.shift {
                        return;
                    }
                    if let Some(t) = s.terminals.get_mut(&k5)
                        && t.tracking()
                        && mods.shift
                        && t.status().live
                    {
                        if !focused {
                            s.exit_copy_mode();
                            if let Some(t) = s.terminals.get_mut(&k5) {
                                let _ = t.connection.commands.focus();
                            }
                        }
                        if let Some(t) = s.terminals.get_mut(&k5) {
                            let at = t.cell_at(p.x, p.y, cell);
                            t.clear_selection();
                            t.send_mouse("press", Some("right"), at, mods);
                            t.send_mouse("release", Some("right"), at, mods);
                        }
                        return;
                    }
                    let w = p.window();
                    s.pane_menu(menu_block.clone(), Some((w.x, w.y)));
                })
                .on_wheel(move |s: &mut Workspace, e: WheelEvent, _| {
                    let scale = if e.precise { s.ui_scale() } else { 1. };
                    let cell = s.metrics().cell;
                    let in_copy = s.copy_mode_on(&k3);
                    let lines = {
                        let Some(t) = s.terminals.get_mut(&k3) else {
                            return false;
                        };
                        t.scroll -= e.dy / (cell.1 * scale);
                        let lines = t.scroll.trunc();
                        if lines != 0. {
                            t.scroll -= lines;
                        }
                        lines
                    };
                    if lines != 0. {
                        s.clear_focus_history_walk();
                        if in_copy {
                            s.copy_mode_scroll(lines as i32);
                        } else if let Some(t) = s.terminals.get_mut(&k3) {
                            // Shift + wheel keeps scrolling the local scrollback.
                            if t.tracking() && !e.mods.shift && t.status().live {
                                let at = t.cell_at(e.x, e.y, cell);
                                t.send_wheel(at, lines as i32);
                            } else {
                                t.send(Command::Scroll(lines as isize));
                            }
                        }
                    }
                    true
                })
                .child(term::grid(&format!("g{block}"), t, m, look));
            let (pad_x, pad_y) = s.settings.padding();
            // The card draws a background of its own colour, edge included; any other sits
            // inside the card's border, never over it.
            let fill = t.body(&colors);
            // Keep each inset distinct: the find bar and its apron share the grid's
            // origin and available size, including a differently coloured body's border.
            let (body, top, side, lead, foot) = if fill == card() {
                (col().w_full().px(pad_x).pb(pad_y), pad_y, pad_x, 0., pad_y)
            } else {
                let (edge, radius) = if bleed {
                    (0., 0.)
                } else {
                    (1., CARD_RADIUS - 1.)
                };
                // Under a header row the fill starts square, at the row; with none it has the
                // card's top corners too and stays inside the top border.
                let (above, crown) = if headed { (0., 0.) } else { (edge, radius) };
                (
                    col()
                        .self_stretch()
                        .mx(edge)
                        .mb(edge)
                        .mt(above)
                        .px((pad_x - edge).max(0.))
                        .pb((pad_y - edge).max(0.))
                        .corners([crown, crown, radius, radius])
                        .continuous_corners()
                        .fill(rgb(fill)),
                    (pad_y - above).max(0.),
                    edge + (pad_x - edge).max(0.),
                    above,
                    edge + (pad_y - edge).max(0.),
                )
            };
            // The grid and Find share one layout box, but the bar is not a child
            // of the grid's pointer target and cannot send mouse input to a program.
            let content = el().grow(1.).w_full().min_h(0.).child(grid);
            let content = match &t.search {
                Some(edit) => {
                    let above = if headed { CARD_HEADER + clear } else { 0. } + lead + top;
                    let pane = find::Pane {
                        size: (
                            (rect[2] - 2. * side).max(0.),
                            (rect[3] - above - foot).max(0.),
                        ),
                        cell: m.cell,
                        lowered: find::lowered(s, window, block, t),
                    };
                    content
                        .child(find::apron(block, t, pane, fill))
                        .child(find::bar(s, block, &key, edit, t, pane))
                }
                None => content,
            };
            c.child(
                body.grow(1.)
                    .min_h(0.)
                    .pt(top)
                    .instant_changes()
                    .motion(STILL)
                    .child(content),
            )
        }
        None => c.child(
            col().grow(1.).center().child(
                text(if s.model().connected {
                    "Opening terminal…"
                } else {
                    "Waiting for host…"
                })
                .font(ui(13.))
                .color(rgb(muted())),
            ),
        ),
    };
    // Flat terminal backgrounds cover the card's own paint. Draw the cue over
    // them without adding an inset or changing the grid when focus moves.
    let c = if ring {
        c.child(
            el().key(format!("pane-outline:{block}"))
                .absolute(0., 0.)
                .size(rect[2], rect[3])
                .border(1., rgb(accent()).alpha(0.85))
                .inert()
                .motion(STILL),
        )
    } else {
        c
    };
    // With no header to hold it, the strip floats over the pane's top-right corner.
    let c = match strip(s, block, rect[2], m).filter(|_| placement == Placement::None) {
        Some(strip) => c.child(strip.pin(None, Some(8.), Some(12.), None)),
        None => c,
    };
    let top = pane_header::clearance(s, rect);
    let floating = (placement == Placement::Floating).then(|| {
        pane_header::floating(
            header(s, window, block, rect[2], focused, m, lead),
            !bleed,
            top,
        )
    });
    if !headed && !searching && s.settings.pane_header == PaneHeader::OnHover {
        // A hover band only sees its own target or descendants. The floating
        // header stays inside it so showing it does not report a hover leave.
        c.child(pane_header::reveal_band(block, top).children(floating))
    } else {
        c.children(floating)
    }
}

fn empty(s: &Workspace) -> El {
    let model = s.model();
    let (title, detail, label, intent) = if !model.connected {
        (
            "Host unavailable",
            model.status.clone(),
            "Retry connection",
            Intent::Retry,
        )
    } else if model.selected.is_some() {
        (
            "This session has no tabs",
            "Create a tab to start a terminal.".into(),
            "New tab",
            Intent::NewWindow,
        )
    } else if model.sessions.is_empty() {
        (
            "Your workspace starts here",
            "Create a session on this host to begin.".into(),
            "New session",
            Intent::NewSession,
        )
    } else {
        (
            "Choose a session",
            match s.shortcut_detail("session.switch", &serde_json::Value::Null) {
                keys if keys.is_empty() => "Select a session in the sidebar.".into(),
                keys => format!("Select a session in the sidebar or press {keys}."),
            },
            "New session",
            Intent::NewSession,
        )
    };
    let setup = if s.can_start_ssh(s.active) {
        let host = s.active;
        pill(
            "offer-ssh-start",
            "Start personal server…",
            Pill::Neutral,
            32.,
            move |s, tx| s.perform(Intent::StartSsh(host), tx),
        )
    } else if s.can_install_ssh(s.active) {
        let host = s.active;
        pill(
            "offer-ssh-install",
            "Install Rex on this host…",
            Pill::Neutral,
            32.,
            move |s, tx| s.perform(Intent::InstallSsh(host), tx),
        )
    } else if s.can_check_key(s.active).is_some() {
        let host = s.active;
        pill(
            "offer-ssh-key",
            "Check host key…",
            Pill::Neutral,
            32.,
            move |s, tx| s.perform(Intent::HostKey(host), tx),
        )
    } else {
        el()
    };
    col().key("empty").w_full().h_full().center().child(
        glass(
            col()
                .w_frac(0.94)
                .max_w(420.)
                .padding(28.)
                .gap(12.)
                .items_center(),
            16.,
            false,
        )
        .child(super::widgets::app_logo(s, 44.))
        .child(text(title).font(strong(18.)).color(rgb(ink())))
        .child(
            text(detail)
                .font(ui(13.))
                .color(rgb(muted()))
                .wrap()
                .max_w(360.)
                .align(rune::Align::Center),
        )
        .child(el().h(4.))
        .child(
            row()
                .gap(8.)
                .child(
                    pill(
                        "empty-action",
                        label,
                        Pill::Primary,
                        32.,
                        move |s: &mut Workspace, tx: &mut Tx| {
                            s.perform(intent.clone(), tx);
                            tx.blur();
                        },
                    )
                    .preserve_focus_on_press(true),
                )
                .children((model.connected && model.selected.is_some()).then(|| {
                    pill(
                        "empty-close-session",
                        "Close session",
                        Pill::Neutral,
                        32.,
                        Workspace::close_session_on_screen,
                    )
                    .preserve_focus_on_press(true)
                })),
        )
        .child(setup),
    )
}

/// The block a pane-specific panel targets, if one is open.
fn target_block(s: &Workspace) -> Option<String> {
    let panel = s.panel.as_ref()?;
    matches!(
        panel.kind,
        PanelKind::Pane
            | PanelKind::Layer
            | PanelKind::Form(Form::RenamePane(_))
            | PanelKind::Confirm { .. }
    )
    .then(|| panel.target.as_ref().map(|t| t.block.clone()))
    .flatten()
}

/// `px` is device pixels per point, for drawing terminal box drawing on whole pixels.
pub fn area(s: &Workspace, px: f32) -> El {
    let m = Metrics { px, ..s.metrics() };
    let mut area = el()
        .key("panes")
        .grow(1.)
        .h_full()
        .min_w(0.)
        .on_layout(|s: &mut Workspace, r| s.area = r);
    let Some(current) = s.model().window() else {
        return area.child(empty(s));
    };
    let focused = s.focused_block();
    let target = target_block(s);
    let preview = s.drag.as_ref().map(|d| (d.divider.key(), d.ratio));
    let (panes, dividers) =
        layout::placements(current, preview.as_ref().map(|(k, r)| (k.as_str(), *r)));
    let bleed = full_bleed(s, current);
    let [ix, iy, iw, ih] = inner(s);

    let emit_card = |placement: &layout::Placement| {
        let card_bleed = pane_header::placement_full_bleed(s, current, placement);
        let rect = if card_bleed {
            [0., 0., s.area[2], s.area[3]]
        } else {
            card_rect(s, placement.rect)
        };
        let is_focus = focused.as_deref() == Some(placement.block.as_str());
        let state = if target.as_deref() == Some(placement.block.as_str())
            || s.drop_hover.as_deref() == Some(placement.block.as_str())
        {
            CardState::Target
        } else if is_focus && s.panel.is_none() {
            CardState::Focused
        } else {
            CardState::Plain
        };
        let card = pane_card(
            s,
            current,
            &placement.block,
            rect,
            &m,
            state,
            is_focus && s.panel.is_none(),
            card_bleed,
        );
        let card = if drag::tiled_split(current)
            && let Some(window) = &current.window_id
        {
            s.rearrange.measure_item(
                card,
                drag::Item::Pane(
                    s.hosts[s.active].id,
                    window.clone(),
                    placement.block.clone(),
                ),
            )
        } else {
            card
        };
        if matches!(&s.rearrange.cue, Some(drag::Cue::Swap(block)) if block == &placement.block) {
            let radius = if card_bleed || s.settings.pane_density == PaneDensity::Compact {
                0.
            } else {
                CARD_RADIUS
            };
            card.child(
                el().key(format!("pane-drop-target:{}", placement.block))
                    .pin(Some(0.), Some(0.), Some(0.), Some(0.))
                    .radius(radius)
                    .continuous_corners()
                    .border(2., rgb(accent()))
                    .fill(rgb(accent()).alpha(0.10))
                    .inert()
                    .motion(STILL),
            )
        } else {
            card
        }
    };

    let emit_divider = |divider: &layout::Divider| {
        let [x, y, w, h] = divider.rect;
        let horizontal = divider.horizontal;
        let dragging = s
            .drag
            .as_ref()
            .is_some_and(|d| d.divider.key() == divider.key());
        let handle = if horizontal {
            el().absolute(
                ix + (x + w * divider.ratio) as f32 * iw - 5.,
                iy + y as f32 * ih,
            )
            .w(10.)
            .h(h as f32 * ih)
        } else {
            el().absolute(
                ix + x as f32 * iw,
                iy + (y + h * divider.ratio) as f32 * ih - 5.,
            )
            .h(10.)
            .w(w as f32 * iw)
        };
        let d = divider.clone();
        divider_handle(
            handle.key(format!("divider:{}", divider.key())),
            horizontal,
            dragging,
            s.settings.pane_density == PaneDensity::Compact,
        )
        .on_drag(move |s: &mut Workspace, g, tx| {
            let Some(session) = s.model().selected.clone() else {
                return;
            };
            if g.phase == Phase::Down || s.drag.is_none() {
                s.clear_focus_history_walk();
                s.cancel_rearrange();
                s.layer_drag = None;
                s.drag = Some(DividerDrag {
                    host: s.active,
                    session,
                    divider: d.clone(),
                    start: d.ratio,
                    ratio: d.ratio,
                });
            }
            let [_, _, iw, ih] = inner(s);
            let scale = s.ui_scale() as f64;
            if let Some(drag) = &mut s.drag {
                let [_, _, w, h] = drag.divider.rect;
                let moved = if drag.divider.horizontal {
                    g.dx as f64 / (iw as f64 * w * scale)
                } else {
                    g.dy as f64 / (ih as f64 * h * scale)
                };
                drag.ratio = (drag.start + moved).clamp(0.1, 0.9);
            }
            tx.instant();
            if g.phase == Phase::Up {
                if g.cancelled() {
                    s.drag = None;
                } else {
                    s.finish_drag();
                }
            }
        })
    };

    if layout::full_window_zoom(current).is_some() {
        for placement in &panes {
            area = area.child(emit_card(placement));
        }
    } else {
        // Emit layer by layer back to front: that layer's cards, then its own split dividers,
        // then (for floating layers on API >= 13 drawn at their bounds) its outer resize grips.
        // A tiled divider behind a floating card never steals pointer input through the card.
        for (layer_index, layer) in current.layers.iter().enumerate() {
            if layers::minimized(layer) {
                continue;
            }
            for placement in panes.iter().filter(|p| p.layer_index == layer_index) {
                area = area.child(emit_card(placement));
            }
            for divider in dividers.iter().filter(|d| d.layer_index == layer_index) {
                area = area.child(emit_divider(divider));
            }
            if !bleed && layers::floating(layer) {
                for grip in floating_drag::grips(s, layer) {
                    area = area.child(grip);
                }
            }
        }
    }
    if let Some(ghost) = floating_drag::ghost(s) {
        area = area.child(ghost);
    }
    area
}

/// A drag target with a permanent straight separator in Compact density, otherwise a hover line.
pub fn divider_handle(e: El, vertical_line: bool, dragging: bool, compact: bool) -> El {
    let (offset, inset, thickness) = if compact { (5., 0., 1.) } else { (4., 8., 2.) };
    let line = if vertical_line {
        el().pin(Some(offset), Some(inset), None, Some(inset))
            .w(thickness)
    } else {
        el().pin(Some(inset), Some(offset), Some(inset), None)
            .h(thickness)
    };
    let line = line.key("divider-line");
    let line = if compact {
        line.fill(if dragging { rgb(accent()) } else { tint(0.18) })
    } else if dragging {
        line.radius(1.).fill(rgb(accent())).box_shadow(shadow(
            0.,
            0.,
            10.,
            rgb(accent()).alpha(0.5),
        ))
    } else {
        line.radius(1.).fill(tint(0.18)).group_opacity(0., 1.)
    };
    e.cursor(if vertical_line {
        Cursor::ResizeColumn
    } else {
        Cursor::ResizeRow
    })
    .hover_group()
    .preserve_focus_on_press(true)
    .child(line)
}
