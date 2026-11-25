//! Mosh-style speculative local echo engine.
//!
//! Printable ASCII and Backspace are replayed on a copy of the active row. Nothing is
//! visible until a newer server frame supplies evidence of unmasked echo in this epoch.
use super::Frame;
use crate::settings::terminal::LocalEcho;
use serde_json::Value;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// RTT at or above which `LocalEcho::Adaptive` engages prediction.
pub const ADAPTIVE_ON_MS: u32 = 30;
/// RTT below which `LocalEcho::Adaptive` disengages prediction (hysteresis).
pub const ADAPTIVE_OFF_MS: u32 = 20;
/// RTT at or above which `LocalEcho::Adaptive` underlines pending predictions immediately.
pub const UNDERLINE_RTT_MS: u32 = 80;
/// Wait time after which an unconfirmed prediction is underlined in `LocalEcho::Adaptive`.
pub const UNDERLINE_AGE: Duration = Duration::from_millis(100);
/// Minimum expiry window before an unanswered prediction is silently cleared.
const MIN_EXPIRE: Duration = Duration::from_millis(1000);
/// Maximum expiry window before an unanswered prediction is silently cleared.
const MAX_EXPIRE: Duration = Duration::from_millis(4000);
/// Hard cap on queued predictions so a held key cannot grow unboundedly.
const MAX_PENDING: usize = 128;

/// One cell of a local-echo overlay on the edited row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PredictedCell {
    /// `Some(ch)` draws `ch`; `None` blanks the underlying cell (predicted Backspace).
    pub ch: Option<char>,
    /// Whether this cell should carry an underline cue as an unconfirmed prediction.
    pub underline: bool,
}

/// Visual overlay computed from pending confirmed-epoch predictions against a server [`Frame`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Overlay {
    /// Row index in `Frame::cells`.
    pub row: u16,
    /// First column covered by `cells`.
    pub start_col: u16,
    /// Contiguous cells starting at `start_col`.
    pub cells: Vec<PredictedCell>,
    /// Predicted cursor cell `(col, row)`, replacing `Frame::cursor` visually.
    pub cursor: (u16, u16),
}

/// A local input event classified for the prediction engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputAction {
    /// Ignore completely (key release, pointer motion, wheel, local selection).
    Ignore,
    /// Printable single-width ASCII character press/repeat.
    Insert(char),
    /// Plain Backspace press/repeat.
    Backspace,
    /// Any other terminal input (Enter, Tab, Escape, arrows, Ctrl/Alt chords, paste, click):
    /// clear pending predictions and start a fresh unconfirmed epoch.
    Reset,
}

/// Classify a `client.key` JSON payload into a local-echo [`InputAction`].
pub fn classify_key(payload: &Value) -> InputAction {
    let action = payload["action"].as_str().unwrap_or("");
    if action == "release" {
        return InputAction::Ignore;
    }
    if payload["composing"].as_bool().unwrap_or(false) {
        return InputAction::Reset;
    }
    let mods = payload["mods"].as_str().unwrap_or("");
    let consumed = payload["consumed_mods"].as_str().unwrap_or("");
    let has = |list: &str, name: &str| list.split('+').any(|m| m == name);
    if has(mods, "ctrl") || has(mods, "super") {
        return InputAction::Reset;
    }
    if has(mods, "alt") && !has(consumed, "alt") {
        return InputAction::Reset;
    }
    if payload["key"].as_str() == Some("backspace") && !has(mods, "shift") && !has(mods, "alt") {
        return InputAction::Backspace;
    }
    let text = payload["text"].as_str().unwrap_or("");
    let mut chars = text.chars();
    if let Some(ch) = chars.next()
        && chars.next().is_none()
        && ch.is_ascii()
        && !ch.is_ascii_control()
    {
        return InputAction::Insert(ch);
    }
    InputAction::Reset
}

#[derive(Clone, Copy)]
struct Edit {
    action: InputAction,
    sent: Instant,
}

/// Replay a short run of edits on one plain-text row. A double blank separates the
/// editable tail from a right prompt. Protected cells are never copied or covered.
struct Row<'a> {
    frame: &'a Frame,
    y: u16,
    x: usize,
    cells: Vec<PredictedCell>,
    links: Vec<std::ops::Range<usize>>,
    touched: Option<std::ops::RangeInclusive<usize>>,
}

impl<'a> Row<'a> {
    fn new(frame: &'a Frame) -> Option<Self> {
        let (x, y) = frame.cursor?;
        if frame.alt_screen
            || !frame.viewport_active
            || frame.mouse_tracking
            || frame.copy.is_some()
            || frame.search.is_some()
            || !frame.images.is_empty()
            || frame.selection_text.is_some()
            || frame.wrapped.get(usize::from(y)) == Some(&true)
            || (y > 0 && frame.wrapped.get(usize::from(y - 1)) == Some(&true))
            || x >= frame.cols
            || y >= frame.rows
        {
            return None;
        }
        let row = frame.cells.get(usize::from(y))?;
        if row.len() != usize::from(frame.cols) {
            return None;
        }
        // Keep the usual append path cheap. If a URL is possible, find all protected
        // columns once, rather than re-scanning the logical line for every shifted cell.
        let links = if row.iter().any(|c| c.text.contains(':')) {
            let mut text = String::new();
            let mut starts = Vec::with_capacity(row.len());
            for c in row {
                starts.push(text.len());
                if !c.spacer_tail {
                    text.push_str(if c.text.is_empty() { " " } else { &c.text });
                }
            }
            crate::links::find_urls(&text)
                .into_iter()
                .map(|r| {
                    starts.partition_point(|&i| i < r.start)..starts.partition_point(|&i| i < r.end)
                })
                .collect()
        } else {
            Vec::new()
        };
        let cells = row
            .iter()
            .map(|cell| PredictedCell {
                ch: cell.text.chars().next().filter(|&c| c != ' '),
                underline: false,
            })
            .collect();
        Some(Self {
            frame,
            x: x.into(),
            y,
            cells,
            links,
            touched: None,
        })
    }
    fn plain(&self, x: usize) -> bool {
        let row = &self.frame.cells[usize::from(self.y)];
        let c = &row[x];
        !c.explicit_bg
            && !c.spacer_tail
            && !row.get(x + 1).is_some_and(|c| c.spacer_tail)
            && c.style.is_default()
            && c.bg == self.frame.bg
            && c.fg == self.frame.fg
            && c.link.is_none()
            && c.text.len() <= 1
            && c.text.is_ascii()
            && !self.links.iter().any(|r| r.contains(&x))
    }
    fn apply(&mut self, action: InputAction) -> Option<()> {
        let (at, insert) = match action {
            InputAction::Insert(c @ ' '..='~') if self.x + 1 < self.cells.len() => {
                (self.x, Some(c))
            }
            InputAction::Backspace => (self.x.checked_sub(1)?, None),
            _ => return None,
        };
        let blank = |i: usize| {
            self.cells
                .get(i)
                .is_none_or(|c| c.ch.is_none() && !c.underline)
        };
        let mut end = at + usize::from(insert.is_none());
        while end < self.cells.len() && !(blank(end) && blank(end + 1)) {
            end += 1;
        }
        if end >= self.cells.len() {
            return None;
        }
        let last = if insert.is_some() {
            end
        } else {
            (end - 1).max(at)
        };
        if !(at..=last).all(|i| self.plain(i)) {
            return None;
        }
        if let Some(c) = insert {
            // The speculative block cursor must not cover a protected neighbour either.
            if !self.plain(at + 1) {
                return None;
            }
            self.cells.copy_within(at..end, at + 1);
            self.cells[at] = PredictedCell {
                ch: Some(c),
                underline: true,
            };
            self.x += 1;
        } else {
            self.cells.copy_within(at + 1..end, at);
            self.cells[last] = PredictedCell {
                ch: None,
                underline: false,
            };
            self.x = at;
        }
        self.touched = Some(
            self.touched
                .take()
                .map_or(at..=last, |r| at.min(*r.start())..=last.max(*r.end())),
        );
        Some(())
    }
    fn matches(&self, next: &Frame) -> bool {
        let Some(range) = self.touched.clone() else {
            return false;
        };
        let y = usize::from(self.y);
        let (a, b) = (&self.frame.cells[y], &next.cells[y]);
        next.cursor == Some((self.x as u16, self.y))
            && a[..*range.start()] == b[..*range.start()]
            && a[*range.end() + 1..] == b[*range.end() + 1..]
            && range.into_iter().all(|x| {
                let c = &next.cells[usize::from(self.y)][x];
                let expected = self.cells[x].ch.unwrap_or(' ');
                !c.explicit_bg
                    && c.style.is_default()
                    && !c.spacer_tail
                    && c.link.is_none()
                    && c.fg == next.fg
                    && c.bg == next.bg
                    && (if c.text.is_empty() {
                        expected == ' '
                    } else {
                        c.text.len() == 1 && c.text.starts_with(expected)
                    })
            })
    }
}

#[derive(Default)]
pub struct Predictor {
    pending: VecDeque<Edit>,
    confirmed: bool,
    adaptive: bool,
    barrier: Option<u64>,
    /// Reset sequence and earliest safe time to use a post-ack frame.
    barrier_ack: Option<(u64, Instant)>,
    underline_notified: Option<Instant>,
}
impl Predictor {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    /// A local action can precede its remote effects by a round trip. A local
    /// redraw at the same stream position cannot establish a new input baseline.
    pub fn interrupt(&mut self, frame: Option<&Frame>) {
        self.pending.clear();
        self.confirmed = false;
        self.adaptive = false;
        self.underline_notified = None;
        self.barrier = self.barrier.max(frame.map(|f| f.output_position));
    }
    pub fn sent_reset(&mut self, seq: u64, frame: Option<&Frame>, now: Instant, rtt: Option<u32>) {
        self.interrupt(frame);
        self.barrier_ack = Some((
            seq,
            now + Duration::from_millis(u64::from(rtt.unwrap_or(0))),
        ));
    }
    pub fn potentially_visible(&self) -> bool {
        self.confirmed && self.has_pending()
    }
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
    fn enabled(&mut self, mode: LocalEcho, rtt: Option<u32>) -> bool {
        self.adaptive = mode == LocalEcho::Adaptive
            && rtt.is_some_and(|ms| {
                ms >= if self.adaptive {
                    ADAPTIVE_OFF_MS
                } else {
                    ADAPTIVE_ON_MS
                }
            });
        mode == LocalEcho::Always || self.adaptive
    }
    fn expire_after(rtt: Option<u32>) -> Duration {
        Duration::from_millis(u64::from(rtt.unwrap_or(0)) * 3).clamp(MIN_EXPIRE, MAX_EXPIRE)
    }
    fn expired(&self, rtt: Option<u32>, now: Instant) -> bool {
        self.pending
            .front()
            .is_some_and(|e| now.saturating_duration_since(e.sent) >= Self::expire_after(rtt))
    }
    fn row<'a>(&self, frame: &'a Frame) -> Option<Row<'a>> {
        let mut row = Row::new(frame)?;
        for edit in &self.pending {
            row.apply(edit.action)?;
        }
        Some(row)
    }
    pub fn on_input(
        &mut self,
        action: InputAction,
        frame: Option<&Frame>,
        mode: LocalEcho,
        rtt: Option<u32>,
        now: Instant,
    ) {
        if mode == LocalEcho::Off || action == InputAction::Ignore {
            return;
        }
        if action == InputAction::Reset
            || !self.enabled(mode, rtt)
            || self.expired(rtt, now)
            || self.pending.len() >= MAX_PENDING
        {
            self.interrupt(frame);
            return;
        }
        let Some(f) = frame else {
            self.interrupt(None);
            return;
        };
        if self
            .barrier_ack
            .is_some_and(|(seq, ready)| f.acked_input < seq || now < ready)
            || self.barrier.is_some_and(|p| f.output_position <= p)
        {
            return;
        }
        self.barrier_ack = None;
        self.barrier = None;
        // Even a later frame that echoes the insert alone is no proof once the user erased it.
        if (!self.confirmed && action == InputAction::Backspace)
            || self.row(f).and_then(|mut r| r.apply(action)).is_none()
        {
            self.interrupt(frame);
            return;
        }
        self.pending.push_back(Edit { action, sent: now });
    }
    /// Called exactly once per changed frame, with its authoritative predecessor.
    pub fn reconcile(
        &mut self,
        prev: &Frame,
        next: &Frame,
        mode: LocalEcho,
        rtt: Option<u32>,
        now: Instant,
    ) {
        if mode == LocalEcho::Off {
            return;
        }
        if self
            .barrier_ack
            .is_some_and(|(seq, _)| next.acked_input < seq)
        {
            self.barrier = self.barrier.max(Some(next.output_position));
        }
        if !self.enabled(mode, rtt)
            || self.expired(rtt, now)
            || self.pending.is_empty()
            || next.output_position <= prev.output_position
            || (prev.cols, prev.rows) != (next.cols, next.rows)
            || Row::new(next).is_none()
        {
            self.pending.clear();
            self.confirmed = false;
            self.underline_notified = None;
            return;
        }
        let Some(mut row) = Row::new(prev) else {
            self.interrupt(Some(next));
            return;
        };
        let mut matched = 0;
        let mut evidence = false;
        let mut confirmed = self.confirmed;
        for (i, e) in self.pending.iter().enumerate() {
            let x = row.x;
            if row.apply(e.action).is_none() {
                break;
            }
            // Unconfirmed runs contain only inserts, so this witness survives every prefix.
            if let InputAction::Insert(ch @ '!'..='~') = e.action
                && ch != '*'
                && !prev.cells[usize::from(row.y)][x].text.starts_with(ch)
            {
                evidence = true;
            }
            if row.matches(next) {
                matched = i + 1;
                confirmed |= evidence;
            }
        }
        if matched == 0 {
            self.interrupt(Some(next));
        } else {
            self.confirmed = confirmed;
            self.pending.drain(..matched);
            self.underline_notified = None;
            if self.has_pending() && self.row(next).is_none() {
                self.interrupt(Some(next));
            }
        }
    }
    pub fn poll(
        &mut self,
        mode: LocalEcho,
        rtt: Option<u32>,
        now: Instant,
    ) -> (bool, Option<Instant>) {
        if mode == LocalEcho::Off {
            return (false, None);
        }
        if !self.enabled(mode, rtt) || self.expired(rtt, now) {
            let changed = self.has_pending();
            self.confirmed = false;
            self.pending.clear();
            self.underline_notified = None;
            return (changed, None);
        }
        let Some(e) = self.pending.front() else {
            return (false, None);
        };
        let expiry = e.sent + Self::expire_after(rtt);
        if mode == LocalEcho::Adaptive
            && self.confirmed
            && rtt.is_some_and(|ms| ms < UNDERLINE_RTT_MS)
        {
            let underline = e.sent + UNDERLINE_AGE;
            if now < underline {
                return (false, Some(underline));
            }
            return (
                self.underline_notified.replace(e.sent) != Some(e.sent),
                Some(expiry),
            );
        }
        (false, Some(expiry))
    }
    pub fn overlay(
        &self,
        frame: &Frame,
        mode: LocalEcho,
        rtt: Option<u32>,
        now: Instant,
    ) -> Option<Overlay> {
        if mode == LocalEcho::Off {
            return None;
        }
        let first = self.pending.front()?;
        if !self.confirmed
            || self.expired(rtt, now)
            || (mode == LocalEcho::Adaptive
                && !(self.adaptive && rtt.is_some_and(|ms| ms >= ADAPTIVE_OFF_MS)))
        {
            return None;
        }
        let row = self.row(frame)?;
        let range = row.touched?;
        let start_col = *range.start() as u16;
        let mut cells = row.cells[range].to_vec();
        if mode != LocalEcho::Always
            && rtt.is_none_or(|ms| ms < UNDERLINE_RTT_MS)
            && now.saturating_duration_since(first.sent) < UNDERLINE_AGE
        {
            for c in &mut cells {
                c.underline = false;
            }
        }
        Some(Overlay {
            row: row.y,
            start_col,
            cells,
            cursor: (row.x as u16, row.y),
        })
    }
}

#[cfg(test)]
mod tests;
