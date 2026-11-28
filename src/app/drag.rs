//! Pointer-only rearrangement. Cues are local; order and geometry are always Rex's.
use super::{
    theme::{STILL, accent, rgb},
    workspace::{HostId, Workspace},
};
use rex_rune::{
    connection::{HostCommand, LayoutDrop, TRANSFER_WINDOW_MIN_API_REVISION},
    layout,
};
use rexrs::api::session;
use rune::{El, Live, Phase, PointerButton, el};
use std::{cell::RefCell, collections::HashMap};

const SLOP: f32 = 4.;
pub(super) const DIM: f32 = 0.55;
type Rect = [f32; 4];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Rail {
    Toolbar,
    Sidebar,
}

impl Rail {
    fn axis(self) -> usize {
        usize::from(self == Self::Sidebar)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Item {
    Session(HostId, String),
    Tab(Rail, HostId, String, String),
    Pane(HostId, String, String),
}

impl Item {
    fn host(&self) -> HostId {
        match *self {
            Self::Session(host, _) | Self::Tab(_, host, ..) | Self::Pane(host, ..) => host,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Measure {
    Item(Item),
    Rail(Rail),
}

#[derive(Default)]
pub(super) struct State {
    // Rune writes these in window points as drawn, including scroll, springs and UI zoom.
    // Keep only elements mounted by the latest view, without committing on every scroll.
    rects: RefCell<HashMap<Measure, (Live<Rect>, bool)>>,
    pub(super) gesture: Option<Gesture>,
    pub(super) cue: Option<Cue>,
}

pub(super) struct Gesture {
    item: Item,
    epoch: u64,
    // The foreground context is independent of the source: sidebar rows may be off-session.
    foreground: (HostId, Option<String>, Option<String>),
    point: Option<[f32; 2]>,
}

#[derive(Debug, PartialEq)]
pub(super) enum Cue {
    Insert { before: Option<String>, bar: Rect },
    Transfer { host: HostId, session: String },
    Swap(String),
}

impl State {
    fn measure(&self, e: El, key: Measure) -> El {
        let mut rects = self.rects.borrow_mut();
        let (rect, mounted) = rects
            .entry(key)
            .or_insert_with(|| (Live::new([0.; 4]), false));
        *mounted = true;
        e.drawn_rect_into(rect)
    }

    pub(super) fn measure_rail(&self, e: El, rail: Rail) -> El {
        self.measure(e, Measure::Rail(rail))
    }

    pub(super) fn measure_item(&self, e: El, item: Item) -> El {
        self.measure(e, Measure::Item(item))
    }

    pub(super) fn finish_view(&self) {
        self.rects
            .borrow_mut()
            .retain(|_, (_, mounted)| std::mem::take(mounted));
    }

    fn rect(&self, key: Measure) -> Option<Rect> {
        let r = self.rects.borrow().get(&key)?.0.get();
        (r[2] > 0. && r[3] > 0.).then_some(r)
    }

    fn item_rect(&self, item: Item) -> Option<Rect> {
        self.rect(Measure::Item(item))
    }

    pub(super) fn dimmed(&self, item: &Item) -> bool {
        self.gesture
            .as_ref()
            .is_some_and(|g| &g.item == item && g.point.is_some())
    }

    pub(super) fn held_pane(&self) -> Option<&str> {
        match &self.gesture.as_ref()?.item {
            Item::Pane(_, _, block) => Some(block),
            _ => None,
        }
    }

    pub(super) fn holding_toolbar(&self) -> bool {
        self.gesture
            .as_ref()
            .is_some_and(|g| matches!(g.item, Item::Tab(Rail::Toolbar, ..)))
    }

    pub(super) fn bar(&self, scale: f32) -> Option<El> {
        let Some(Cue::Insert { bar, .. }) = self.cue else {
            return None;
        };
        let [x, y, w, h] = bar.map(|v| v / scale);
        Some(
            el().key("drag-insertion")
                .absolute(x, y)
                .size(w, h)
                .fill(rgb(accent()))
                .inert()
                .motion(STILL),
        )
    }
}

fn contains([x, y, w, h]: Rect, [px, py]: [f32; 2]) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x = a[0].max(b[0]);
    let y = a[1].max(b[1]);
    let w = (a[0] + a[2]).min(b[0] + b[2]) - x;
    let h = (a[1] + a[3]).min(b[1] + b[3]) - y;
    (w > 0. && h > 0.).then_some([x, y, w, h])
}

/// In model order, each midpoint separates the gap before that row from the gap after it.
/// Before the source and immediately after it are the same slot, including at the end.
pub(super) fn insertion(
    rows: &[(&str, Rect)],
    source: &str,
    rail: Rail,
    clip: Rect,
    point: [f32; 2],
    scale: f32,
    end: Option<f32>,
) -> Option<Cue> {
    let axis = rail.axis();
    let across = 1 - axis;
    let source_slot = rows.iter().position(|(id, _)| *id == source)?;
    let first = rows.first()?.1;
    let last = rows.last()?.1;
    let end = end.unwrap_or(last[axis] + last[axis + 2]);
    // One point of space for the bar just outside the first and last row.
    let mut strip = first;
    strip[axis] -= scale;
    strip[axis + 2] = end + scale - strip[axis];
    let visible = intersect(strip, clip)?;
    if !contains(visible, point) {
        return None;
    }
    let slot = rows.partition_point(|(_, r)| point[axis] >= r[axis] + r[axis + 2] / 2.);
    if slot == source_slot || slot == source_slot + 1 {
        return None;
    }
    let next = rows.get(slot);
    // Rows use a 2 pt gap. Sessions can also have tabs between rows, so draw
    // just above the next session, never halfway through its predecessor's tabs.
    let edge = next.map_or(end, |(_, next)| {
        next[axis] - if slot == 0 { 0. } else { scale }
    });
    let mut bar = visible;
    let thickness = (2. * scale).min(visible[axis + 2]);
    bar[axis] = (edge - thickness / 2.).clamp(
        visible[axis],
        visible[axis] + (visible[axis + 2] - thickness),
    );
    bar[axis + 2] = thickness;
    bar[across] += 2. * scale;
    bar[across + 2] = (bar[across + 2] - 4. * scale).max(0.);
    Some(Cue::Insert {
        before: next.map(|(id, _)| (*id).to_owned()),
        bar,
    })
}

pub(super) fn tiled_split(window: &session::Window) -> bool {
    !layout::any_zoomed(window)
        && rex_rune::layers::tiled(window).is_some_and(|l| {
            !rex_rune::layers::minimized(l)
                && l.layout
                    .as_ref()
                    .and_then(|r| r.split.as_ref())
                    .is_some_and(|split| split.before.is_some() && split.after.is_some())
        })
}

impl Workspace {
    fn drag_context(&self) -> (HostId, Option<String>, Option<String>) {
        (
            self.hosts[self.active].id,
            self.model().selected.clone(),
            self.model().window().and_then(|w| w.window_id.clone()),
        )
    }

    fn validate_drag(&self, g: &Gesture) -> Option<usize> {
        if self.panel.is_some() || self.in_copy_mode() || self.drag_context() != g.foreground {
            return None;
        }
        let host = self.host_index(g.item.host())?;
        let model = &self.hosts[host].model;
        if !model.connected || g.epoch != model.connection_epoch {
            return None;
        }
        let sidebar = self.sidebar.is_open() && !self.settings.hosts[host].collapsed;
        let session_exists = |id: &str| model.sessions.iter().any(|s| s.session_id == id);
        let valid =
            match &g.item {
                Item::Tab(rail, _, session, id) => {
                    let shown = match rail {
                        Rail::Toolbar => {
                            host == self.active
                                && !self.sidebar_tabs()
                                && model.selected.as_ref() == Some(session)
                        }
                        Rail::Sidebar => sidebar && self.sidebar_tabs(),
                    };
                    shown
                        && session_exists(session)
                        && model.views.get(session).is_some_and(|v| {
                            v.windows.iter().any(|w| w.window_id.as_ref() == Some(id))
                        })
                }
                Item::Session(_, id) => sidebar && session_exists(id),
                Item::Pane(_, window, id) => {
                    host == self.active
                        && model.window().is_some_and(|w| {
                            w.window_id.as_ref() == Some(window)
                                && tiled_split(w)
                                && layout::placements(w, None).0.iter().any(|p| {
                                    &p.block == id && w.layers[p.layer_index].kind == "tiled"
                                })
                        })
                }
            };
        valid.then_some(host)
    }

    fn destination(&self, g: &Gesture, host: usize, point: [f32; 2]) -> Option<Cue> {
        let state = &self.rearrange;
        let model = &self.hosts[host].model;
        let host_id = g.item.host();
        let scale = self.ui_scale();
        let sidebar = state.rect(Measure::Rail(Rail::Sidebar)).filter(|_| {
            self.sidebar.is_open()
                && !self.settings.hosts[host].collapsed
                && point[1] >= super::toolbar_reveal::shown_height(self) * scale
        });
        match &g.item {
            Item::Tab(rail, _, session, id) => {
                if sidebar.is_some_and(|r| contains(r, point))
                    && model
                        .protocol
                        .api_revision_at_least(TRANSFER_WINDOW_MIN_API_REVISION)
                    && let Some(to) = model.sessions.iter().find(|to| {
                        &to.session_id != session
                            && state
                                .item_rect(Item::Session(host_id, to.session_id.clone()))
                                .is_some_and(|r| contains(r, point))
                    })
                {
                    return Some(Cue::Transfer {
                        host: host_id,
                        session: to.session_id.clone(),
                    });
                }
                let clip = match rail {
                    Rail::Toolbar => state.rect(Measure::Rail(*rail))?,
                    Rail::Sidebar => sidebar?,
                };
                let rows = model
                    .views
                    .get(session)?
                    .windows
                    .iter()
                    .filter_map(|w| w.window_id.as_deref())
                    .map(|id| {
                        Some((
                            id,
                            state.item_rect(Item::Tab(
                                *rail,
                                host_id,
                                session.clone(),
                                id.to_owned(),
                            ))?,
                        ))
                    })
                    .collect::<Option<Vec<_>>>()?;
                insertion(&rows, id, *rail, clip, point, scale, None)
            }
            Item::Session(_, id) => {
                let rows = model
                    .sessions
                    .iter()
                    .map(|s| {
                        let id = s.session_id.as_str();
                        Some((id, state.item_rect(Item::Session(host_id, id.to_owned()))?))
                    })
                    .collect::<Option<Vec<_>>>()?;
                // In vertical-tabs mode, a session and its tabs move as a group.
                // The final slot is below the last tab, not between its session and tabs.
                let mut end = None;
                if self.sidebar_tabs()
                    && let Some((session, _)) = rows.last()
                    && let Some(tab) = model
                        .views
                        .get(*session)
                        .and_then(|v| v.windows.iter().rev().find_map(|w| w.window_id.as_ref()))
                    && let Some(tail) = state.item_rect(Item::Tab(
                        Rail::Sidebar,
                        host_id,
                        (*session).to_owned(),
                        tab.clone(),
                    ))
                {
                    end = Some(tail[1] + tail[3]);
                }
                insertion(&rows, id, Rail::Sidebar, sidebar?, point, scale, end)
            }
            Item::Pane(_, window, id) => {
                if point[1] < super::toolbar_reveal::shown_height(self) * scale {
                    return None;
                }
                let w = model.window()?;
                // Frontmost card wins, including a floating card that occludes a tiled one.
                let pane = layout::placements(w, None).0.into_iter().rev().find(|p| {
                    state
                        .item_rect(Item::Pane(host_id, window.clone(), p.block.clone()))
                        .is_some_and(|r| contains(r, point))
                })?;
                (w.layers[pane.layer_index].kind == "tiled" && &pane.block != id)
                    .then_some(Cue::Swap(pane.block))
            }
        }
    }

    pub(super) fn cancel_rearrange(&mut self) {
        self.rearrange.gesture = None;
        self.rearrange.cue = None;
    }

    /// Revalidate on host and view changes and follow scrolling under a stationary pointer.
    pub(super) fn sync_rearrange(&mut self) -> bool {
        let Some(g) = &self.rearrange.gesture else {
            return false;
        };
        let Some(host) = self.validate_drag(g) else {
            self.cancel_rearrange();
            return true;
        };
        let cue = g.point.and_then(|point| self.destination(g, host, point));
        if self.rearrange.cue == cue {
            return false;
        }
        self.rearrange.cue = cue;
        true
    }

    fn finish_rearrange(&mut self, point: [f32; 2]) {
        self.rearrange.cue = None;
        let Some(g) = self.rearrange.gesture.take().filter(|g| g.point.is_some()) else {
            return;
        };
        let Some(host) = self.validate_drag(&g) else {
            return;
        };
        let Some(target) = self.destination(&g, host, point) else {
            return;
        };
        let operation = match (g.item, target) {
            (Item::Tab(_, _, session, id), Cue::Insert { before, .. }) => {
                let mut p = session::MoveWindow::new(session, id);
                p.before_window_id = before;
                LayoutDrop::MoveWindow(p)
            }
            (Item::Tab(_, _, session, id), Cue::Transfer { session: to, .. }) => {
                LayoutDrop::TransferWindow(session::TransferWindow::new(session, id, to))
            }
            (Item::Session(_, id), Cue::Insert { before, .. }) => {
                let mut p = session::Move::new(id);
                p.before_session_id = before;
                LayoutDrop::MoveSession(p)
            }
            (Item::Pane(.., id), Cue::Swap(to)) => {
                let Some(session) = g.foreground.1 else {
                    return;
                };
                LayoutDrop::SwapBlocks(session::SwapBlocks::new(session, id, to))
            }
            _ => return,
        };
        self.send(
            host,
            HostCommand::LayoutDrop {
                epoch: g.epoch,
                operation,
            },
        );
    }
}

/// Rune suppresses primary press and double-press past the same slop. Leave those handlers
/// in place for short clicks and keyboard activation; middle and secondary stay independent.
pub(super) fn attach(e: El, item: Item) -> El {
    e.on_drag(move |s: &mut Workspace, d, tx| {
        if tx.button() != PointerButton::Primary {
            return;
        }
        match d.phase {
            Phase::Down => {
                s.cancel_rearrange();
                let Some(host) = s.host_index(item.host()) else {
                    return;
                };
                let gesture = Gesture {
                    item: item.clone(),
                    epoch: s.hosts[host].model.connection_epoch,
                    foreground: s.drag_context(),
                    point: None,
                };
                if s.validate_drag(&gesture).is_some() {
                    s.clear_focus_history_walk();
                    s.rearrange.gesture = Some(gesture);
                }
            }
            Phase::Move => {
                let Some(g) = &mut s.rearrange.gesture else {
                    return;
                };
                // Match Rune's per-axis click slop, so a release can't be both a click and a drop.
                if g.item != item || (g.point.is_none() && d.dx.abs().max(d.dy.abs()) <= SLOP) {
                    return;
                }
                g.point = Some(d.window_point());
                s.sync_rearrange();
                tx.instant();
            }
            Phase::Up => {
                let Some(g) = s.rearrange.gesture.as_ref().filter(|g| g.item == item) else {
                    return;
                };
                let was_armed = g.point.is_some();
                if d.cancelled() {
                    s.cancel_rearrange();
                } else {
                    s.finish_rearrange(d.window_point());
                }
                if was_armed {
                    tx.instant();
                }
            }
        }
    })
}
