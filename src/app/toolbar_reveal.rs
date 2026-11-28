//! When the toolbar shows and where it is drawn.
//!
//! Settings → Interface → Toolbar decides: Always keeps its row above the panes, On hover
//! gives that row to the panes and floats the toolbar over the top of the window while the
//! pointer rests on the window's top edge, and Hidden draws none. Where the toolbar is the
//! title bar, both leave a title strip beside the traffic lights, except in full screen.
//! Reveal toolbar floats it from the keyboard in either, until it is run again, so a window
//! without a toolbar row can still be moved, closed and steered. A toolbar that floats never
//! moves the panes under it: no terminal is resized.
use super::{theme::*, workspace::Workspace};
use rex_rune::settings::interface::ToolbarBehavior;
use rune::{Edge, El, PointerEvent, SpringPair, Transition, Tx, el};
use std::time::Duration;

/// Floats the toolbar where it has no row, until it is run again.
pub const TOGGLE: &str = "client.toolbar.toggle";

/// The band along the window's top edge where the pointer brings an On hover toolbar out.
/// No taller than the default padding above a terminal's first row, so working on that row
/// never brings it out.
pub const EDGE: f32 = 8.;
/// How long the pointer rests on the edge before the toolbar shows, and how long after it
/// left the toolbar stays. A pass over the edge on the way out of the window shows nothing.
const REVEAL_AFTER: Duration = Duration::from_millis(80);
const HIDE_AFTER: Duration = Duration::from_millis(320);

/// The toolbar brought out without a setting change. Never saved.
#[derive(Default)]
pub struct Reveal {
    /// The pointer rests on the window's top edge, or on the toolbar it brought out.
    pub hover: bool,
    /// Reveal toolbar is on.
    pub pinned: bool,
    /// A pointer enter or leave waiting for its delay to elapse, in Rune clock seconds.
    deadline: Option<f64>,
}

impl Reveal {
    pub fn clear_hover(&mut self) {
        self.hover = false;
        self.deadline = None;
    }
}

/// How the toolbar is drawn now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    None,
    /// In its own row above the panes.
    Row,
    /// Over the top of the window.
    Floating,
}

pub fn placement(s: &Workspace) -> Placement {
    let reveal = &s.toolbar_reveal;
    match s.settings.interface.toolbar {
        ToolbarBehavior::Always => Placement::Row,
        ToolbarBehavior::OnHover
            if reveal.hover || reveal.pinned || s.rearrange.holding_toolbar() =>
        {
            Placement::Floating
        }
        ToolbarBehavior::Hidden if reveal.pinned => Placement::Floating,
        ToolbarBehavior::OnHover | ToolbarBehavior::Hidden => Placement::None,
    }
}

/// Whether Reveal toolbar has anything to bring out.
pub fn can_reveal(s: &Workspace) -> bool {
    s.settings.interface.toolbar != ToolbarBehavior::Always
}

/// The toolbar's height: its own 52 pt row, or the shorter title row where it is the
/// window's title bar.
pub fn bar_height(s: &Workspace) -> f32 {
    if s.unified_title {
        super::platform::chrome::TITLE_ROW
    } else {
        TOOLBAR
    }
}

/// Whether the window's top keeps a bare title strip while the toolbar has no row: where the
/// toolbar is the title bar, the traffic lights need a row of their own, except in full
/// screen, where the system hides them.
pub fn shows_title_strip(s: &Workspace) -> bool {
    s.unified_title && s.title_inset > 0. && placement(s) != Placement::Row
}

/// The height the toolbar's row takes from the panes: nothing unless it is always shown, or
/// the title strip that stands in for it.
pub fn row_height(s: &Workspace) -> f32 {
    if placement(s) == Placement::Row || shows_title_strip(s) {
        bar_height(s)
    } else {
        0.
    }
}

/// The height the toolbar covers at the window's top now, in its row, floating, or as the
/// title strip.
pub fn shown_height(s: &Workspace) -> f32 {
    if placement(s) == Placement::None && !shows_title_strip(s) {
        0.
    } else {
        bar_height(s)
    }
}

/// The space pane cards and the sidebar keep from the window's top edge: none under the
/// toolbar's row or the title strip, which is their margin, and the outer margin where they
/// start at the edge.
pub fn top_margin(s: &Workspace) -> f32 {
    if placement(s) == Placement::Row || shows_title_strip(s) {
        0.
    } else {
        MARGIN
    }
}

impl Workspace {
    /// Reveal toolbar, and the same again to put it back.
    pub fn toggle_toolbar(&mut self) {
        self.toolbar_reveal.pinned = !self.toolbar_reveal.pinned && can_reveal(self);
        self.reveal_tab_again = true;
    }

    /// Settings → Interface → Toolbar: saved, and a toolbar that was brought out by hand goes
    /// back with the old choice.
    pub fn set_toolbar(&mut self, toolbar: ToolbarBehavior, tx: &mut Tx) {
        self.settings.interface.toolbar = toolbar;
        self.toolbar_reveal = Reveal::default();
        self.reveal_tab_again = true;
        self.save();
        // The panes take or give the row at once; terminals never animate.
        tx.instant();
    }
}

/// A floating toolbar's surface: the popover colour over the panes with a hairline under it,
/// sliding in from the window's top edge.
pub fn floating(s: &Workspace, toolbar: El) -> El {
    el().key("toolbar-float")
        .pin(Some(0.), Some(0.), Some(0.), None)
        .h(bar_height(s))
        .fill(rgb(popover()))
        .box_shadow(shadow(0., 8., 24., drop_shadow(0.4)))
        .transition(
            Transition::move_edge(Edge::Top)
                .combined(Transition::opacity())
                .springs(SpringPair::new(SNAPPY, SNAPPY)),
        )
        .child(toolbar)
        .child(
            el().pin(Some(0.), None, Some(0.), Some(0.))
                .h(1.)
                .fill(tint(0.09)),
        )
}

/// Observe the window's edge without inserting a hit target above the pane. The pane
/// must still receive selection, context menus and mouse reports before the toolbar opens.
pub fn pointer(s: &mut Workspace, point: PointerEvent, tx: &mut Tx) {
    // A keyboard-pinned toolbar does not acquire a hover hold just because the
    // pointer is on a control. Unpinning it should put it away.
    let height = if s.toolbar_reveal.hover || shows_title_strip(s) {
        bar_height(s)
    } else {
        EDGE
    };
    let y = point.y / s.ui_scale();
    track(&mut s.toolbar_reveal, (0. ..height).contains(&y), tx);
}

pub fn window_hover(s: &mut Workspace, inside: bool, tx: &mut Tx) {
    if !inside {
        track(&mut s.toolbar_reveal, false, tx);
    }
}

pub fn pointer_press(s: &mut Workspace, _: rune::PointerButton, _: PointerEvent, tx: &mut Tx) {
    if !s.toolbar_reveal.hover {
        s.toolbar_reveal.deadline = None;
    }
    if s.settings.terminal.local_echo != rex_rune::settings::terminal::LocalEcho::Off
        && s.active_terminal_mut()
            .is_some_and(|t| t.clear_local_echo())
    {
        return;
    }
    tx.live_only();
}

fn track(reveal: &mut Reveal, inside: bool, tx: &mut Tx) {
    if reveal.hover == inside {
        reveal.deadline = None;
    } else if reveal.deadline.is_none() {
        let delay = if inside { REVEAL_AFTER } else { HIDE_AFTER };
        reveal.deadline = Some(tx.clock() + delay.as_secs_f64());
        tx.wake_after(delay);
    }
    tx.live_only();
}

pub fn poll(s: &mut Workspace, clock: f64) -> bool {
    let reveal = &mut s.toolbar_reveal;
    if reveal.deadline.is_some_and(|deadline| deadline <= clock) {
        reveal.deadline = None;
        reveal.hover = !reveal.hover;
        s.reveal_tab_again |= reveal.hover;
        return true;
    }
    false
}

/// With no toolbar row, pending-sequence and receiving-download chips float at the top left.
/// This surface exists only while it has visible status; it reserves no terminal space.
pub fn sequence_float(chip: El) -> El {
    el().key("pending-sequence-float")
        .pin(Some(MARGIN), Some(GUTTER), None, None)
        .radius(10.)
        .continuous_corners()
        .fill(rgb(popover()))
        .box_shadow(shadow(0., 4., 16., drop_shadow(0.45)))
        .transition(Transition::opacity().springs(SpringPair::new(SNAPPY, SNAPPY)))
        .child(chip)
}
