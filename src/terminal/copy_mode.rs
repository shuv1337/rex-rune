//! Pure VT copy-mode engine on Ghostty's terminal state.
//!
//! Tracks the copy cursor and optional visual selection anchor with Ghostty's
//! [`TrackedGridRef`] so coordinates survive scrolling, incoming PTY output, and
//! resize/reflow. All three selection units (`Char`, `Line`, `Block`) drive
//! Ghostty's own [`Selection`] and formatting pipeline so rendered highlights
//! (`Frame::selection_rows`) and yanked text match pointer selection, including
//! soft-wrap unwrapping, hard-newline preservation, and wide CJK/emoji cells.
//!
//! Copy mode is allowed on both the primary screen (full scrollback) and the
//! alternate screen (viewport-only). Switching between screens, losing a tracked
//! reference to scrollback eviction or reset, or replacing the terminal with a
//! fresh snapshot automatically ends copy mode and advances `ended_epoch` so the
//! UI reconciles once.

use crate::terminal::{search, selection};
use anyhow::Result;
use ghostty_vt::{
    Terminal,
    error::Error,
    screen::{CellWide, Screen, TrackedGridRef},
    selection::{FormatOptions, Selection},
    terminal::{Point, PointCoordinate, PointSpace, ScrollViewport},
};

/// Upper bound on repeat counts for copy-mode motions and `YankLine`.
pub const MAX_COUNT: u32 = 9999;

/// Bounded row-scan budget per word/WORD motion so pathological empty scrollback
/// buffers cannot stall the pane worker.
const MAX_WORD_SCAN_ROWS: usize = 4096;

/// Visual selection unit in copy mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CopyUnit {
    /// Character-wise linear selection (`v`).
    Char,
    /// Full-row linear selection (`V`).
    Line,
    /// Rectangular/block selection (`Ctrl-V`, `Selection::rectangle = true`).
    Block,
}

/// Cursor motion within the terminal's screen and scrollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CopyMotion {
    Left,
    Down,
    Up,
    Right,
    WordForward,
    WordBackward,
    WordEnd,
    BigWordForward,
    BigWordBackward,
    BigWordEnd,
    LineStart,
    FirstNonBlank,
    LineEnd,
    HalfPageUp,
    HalfPageDown,
    PageUp,
    PageDown,
    Top,
    Bottom,
    /// 1-based line number from the top of the screen/scrollback (`Ngg`/`NG`).
    LineNumber(usize),
    ViewportTop,
    ViewportMiddle,
    ViewportBottom,
}

/// One copy-mode operation directed at a pane's terminal worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyModeOp {
    /// Enter copy mode at the program cursor inside the viewport (normalized to a
    /// wide-char head cell) or `(0, 0)` when the program cursor is off-screen.
    Begin,
    /// Exit copy mode and clear any copy-mode selection.
    End,
    /// Move the copy cursor, clamping `count` to `1..=MAX_COUNT`.
    Move { motion: CopyMotion, count: u32 },
    /// Start, clear (if same unit), or switch (preserving anchor) a visual selection.
    Select(CopyUnit),
    /// Clear an active visual selection while staying in copy mode.
    ClearSelection,
    /// Swap cursor and selection anchor (`o`); no-op when no selection is active.
    SwapEnds,
    /// Scroll the viewport by `delta` rows (negative = up, positive = down) and
    /// clamp the copy cursor so it stays visible.
    Scroll(i32),
    /// Move the copy cursor to the active search match's start cell, revealing it
    /// in the viewport and extending any active visual selection.
    ToMatch,
    /// Format the active visual selection via Ghostty's unwrap+trim formatter,
    /// emit a [`CopyYank`], and clear the visual selection. No-op without a selection.
    Yank,
    /// Format `count` lines starting at the copy cursor via Ghostty's unwrap+trim
    /// formatter and emit a [`CopyYank`] without leaving a selection behind.
    YankLine { count: u32 },
}

/// Stamped copy-mode input delivered to a pane's data worker.
///
/// `generation` guards against stale inputs sent before a snapshot restore;
/// `epoch` guards against stale inputs from an earlier copy-mode session
/// (callers should use monotonically increasing non-zero epochs starting at 1);
/// `revision` stamps `Frame::selection_revision` and [`CopyYank::revision`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyModeInput {
    pub generation: u64,
    pub epoch: u64,
    pub revision: u64,
    pub visible_rows: Option<u16>,
    pub op: CopyModeOp,
}

/// Rendered copy-mode overlay state published on [`crate::terminal::Frame::copy`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CopyView {
    pub epoch: u64,
    /// Viewport `(col, row)` of the copy cursor, always normalized to a wide head cell.
    pub cursor: (u16, u16),
    /// `true` iff the cell under `cursor` is a wide character (`CellWide::Wide`).
    pub wide: bool,
    pub unit: Option<CopyUnit>,
    /// First viewport row shown on a cropped pane (`0` on uncropped panes).
    /// Single source of truth consumed by [`crate::terminal::Frame::display_row_start`].
    pub display_start: u16,
}

/// Race-free yank payload emitted by [`CopyModeOp::Yank`] or [`CopyModeOp::YankLine`].
///
/// Stored on [`crate::connection::Model::copy_yank`] separately from
/// `Frame::selection_text` so a caller that sends `Yank` followed immediately by
/// `End` still receives the formatted clipboard text on reply without racing with
/// selection clearing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyYank {
    pub epoch: u64,
    pub revision: u64,
    pub text: String,
}

/// Result of applying a [`CopyModeInput`] to [`Controller`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyOutcome {
    /// Whether the input matched the active epoch (or started a new one via `Begin`)
    /// and was executed rather than ignored as stale.
    pub accepted: bool,
    /// Formatted yank payload when `Yank` or `YankLine` completed.
    pub yank: Option<CopyYank>,
}

struct Active {
    epoch: u64,
    screen: Screen,
    cursor: TrackedGridRef,
    desired_col: u16,
    anchor: Option<TrackedGridRef>,
    unit: Option<CopyUnit>,
    visible_rows: Option<u16>,
    crop_start: usize,
}

/// Per-pane copy-mode controller owned by the terminal data worker.
#[derive(Default)]
pub struct Controller {
    active: Option<Active>,
    ended_epoch: u64,
    copy_buffer: Vec<u8>,
}

impl Controller {
    /// Whether copy mode is currently active.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Epoch of the currently active copy-mode session, if any.
    #[must_use]
    pub fn active_epoch(&self) -> Option<u64> {
        self.active.as_ref().map(|a| a.epoch)
    }

    /// Highest copy-mode epoch ended automatically by the worker (screen switch,
    /// lost tracked reference, or terminal reset/snapshot replacement).
    #[must_use]
    pub fn ended_epoch(&self) -> u64 {
        self.ended_epoch
    }

    /// Drop copy-mode state when a fresh snapshot replaces the terminal.
    /// Returns the epoch that was active, if any, after advancing `ended_epoch`.
    pub fn reset(&mut self) -> Option<u64> {
        let active = self.active.take()?;
        self.ended_epoch = self.ended_epoch.max(active.epoch);
        Some(active.epoch)
    }

    /// Drop any copy-mode visual selection without exiting copy mode (for example
    /// when an explicit pointer selection takes over Ghostty's selection).
    pub fn clear_owned_selection(&mut self) {
        if let Some(active) = &mut self.active {
            active.anchor = None;
            active.unit = None;
        }
    }

    /// Apply one copy-mode operation against `terminal`.
    ///
    /// Ignores stale epochs (anything other than `Begin` when inactive or whose
    /// `epoch` differs from the active epoch) without mutating terminal state.
    pub fn apply(
        &mut self,
        terminal: &mut Terminal,
        search: Option<&search::Controller>,
        input: CopyModeInput,
    ) -> Result<ApplyOutcome> {
        if matches!(input.op, CopyModeOp::Begin) {
            if input.epoch == 0 || input.epoch <= self.ended_epoch {
                return Ok(ApplyOutcome {
                    accepted: false,
                    yank: None,
                });
            }
            self.begin(terminal, input.epoch, input.visible_rows)?;
            return Ok(ApplyOutcome {
                accepted: true,
                yank: None,
            });
        }

        // Reject stale epochs before touching terminal state.
        let Some(current_epoch) = self.active.as_ref().map(|a| a.epoch) else {
            return Ok(ApplyOutcome {
                accepted: false,
                yank: None,
            });
        };
        if current_epoch != input.epoch {
            return Ok(ApplyOutcome {
                accepted: false,
                yank: None,
            });
        }

        // Verify tracked refs and screen match before acting; auto-end if lost.
        if !self.validate_alive(terminal)? {
            return Ok(ApplyOutcome {
                accepted: true,
                yank: None,
            });
        }

        if let Some(active) = &mut self.active
            && input.visible_rows.is_some()
        {
            active.visible_rows = input.visible_rows;
        }

        match input.op {
            CopyModeOp::Begin => unreachable!(),
            CopyModeOp::End => {
                self.end_explicit(terminal)?;
                Ok(ApplyOutcome {
                    accepted: true,
                    yank: None,
                })
            }
            CopyModeOp::Move { motion, count } => {
                let count = count.clamp(1, MAX_COUNT);
                self.apply_move(terminal, motion, count)?;
                Ok(ApplyOutcome {
                    accepted: true,
                    yank: None,
                })
            }
            CopyModeOp::Select(unit) => {
                self.apply_select(terminal, unit)?;
                Ok(ApplyOutcome {
                    accepted: true,
                    yank: None,
                })
            }
            CopyModeOp::ClearSelection => {
                self.clear_selection(terminal)?;
                Ok(ApplyOutcome {
                    accepted: true,
                    yank: None,
                })
            }
            CopyModeOp::SwapEnds => {
                self.swap_ends(terminal)?;
                Ok(ApplyOutcome {
                    accepted: true,
                    yank: None,
                })
            }
            CopyModeOp::Scroll(delta) => {
                self.scroll_view(terminal, delta as isize)?;
                Ok(ApplyOutcome {
                    accepted: true,
                    yank: None,
                })
            }
            CopyModeOp::ToMatch => {
                if let Some(search) = search {
                    self.move_to_match(terminal, search)?;
                }
                Ok(ApplyOutcome {
                    accepted: true,
                    yank: None,
                })
            }
            CopyModeOp::Yank => {
                let yank = self.yank_selection(terminal, input.epoch, input.revision)?;
                Ok(ApplyOutcome {
                    accepted: true,
                    yank,
                })
            }
            CopyModeOp::YankLine { count } => {
                let count = count.clamp(1, MAX_COUNT);
                let yank = self.yank_lines(terminal, input.epoch, input.revision, count)?;
                Ok(ApplyOutcome {
                    accepted: true,
                    yank,
                })
            }
        }
    }

    /// Reconcile copy mode after incoming PTY output, resize/reflow, or screen switches.
    ///
    /// Keeps the tracked cursor and anchor pinned to their exact content, reveals the
    /// cursor if new output pushed it off the active viewport, re-normalizes both off
    /// `SpacerTail`/`SpacerHead`, re-syncs any active selection, and auto-ends cleanly
    /// on screen switch or lost tracked ref.
    pub fn sync(&mut self, terminal: &mut Terminal) -> Result<()> {
        if !self.validate_alive(terminal)? {
            return Ok(());
        }
        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };
        let Some(pt) = active.cursor.point(PointSpace::Screen)? else {
            self.auto_end(terminal, false)?;
            return Ok(());
        };
        let total_rows = terminal.total_rows()?;
        let cols = terminal.cols()?;
        if total_rows == 0 || cols == 0 {
            self.auto_end(terminal, false)?;
            return Ok(());
        }
        let y = (pt.y as usize).min(total_rows - 1);
        let norm_x = normalize_col(terminal, y, pt.x)?;
        if norm_x != pt.x || y as u32 != pt.y {
            active.cursor.set(
                terminal,
                Point::Screen(PointCoordinate {
                    x: norm_x,
                    y: y as u32,
                }),
            )?;
        }
        if let Some(anchor) = active.anchor.as_mut() {
            let Some(apt) = anchor.point(PointSpace::Screen)? else {
                self.auto_end(terminal, true)?;
                return Ok(());
            };
            let ay = (apt.y as usize).min(total_rows - 1);
            let ax = normalize_col(terminal, ay, apt.x)?;
            if ax != apt.x || ay as u32 != apt.y {
                anchor.set(
                    terminal,
                    Point::Screen(PointCoordinate {
                        x: ax,
                        y: ay as u32,
                    }),
                )?;
            }
        }
        ensure_visible(terminal, active, y)?;
        sync_selection(terminal, active)?;
        Ok(())
    }

    /// Clamp the copy cursor into the current viewport after an explicit scroll
    /// (`PromptStep`) while keeping copy mode active.
    pub fn after_viewport_scroll(&mut self, terminal: &mut Terminal) -> Result<()> {
        if !self.validate_alive(terminal)? {
            return Ok(());
        }
        self.clamp_cursor_to_viewport(terminal)
    }

    /// Reconcile copy mode after an explicit delta scroll (`DataCommand::Scroll`),
    /// shifting the cropped window when Ghostty's viewport saturates at top or bottom
    /// so every row remains reachable on cropped panes.
    pub fn after_viewport_scroll_delta(
        &mut self,
        terminal: &mut Terminal,
        before_offset: usize,
        delta: isize,
    ) -> Result<()> {
        if !self.validate_alive(terminal)? {
            return Ok(());
        }
        self.apply_crop_overflow(terminal, before_offset, delta)?;
        self.clamp_cursor_to_viewport(terminal)
    }

    /// Reconcile copy mode after scrolling to bottom (`DataCommand::Bottom`), advancing
    /// a cropped pane's `crop_start` to `rows - visible_rows` so the newest rows are
    /// shown and clamping the cursor into the visible slice.
    pub fn after_scroll_bottom(&mut self, terminal: &mut Terminal) -> Result<()> {
        if !self.validate_alive(terminal)? {
            return Ok(());
        }
        if let Some(active) = &mut self.active {
            let rows = terminal.rows()?;
            let vis = effective_visible(active.visible_rows, rows);
            active.crop_start = usize::from(rows).saturating_sub(vis);
        }
        self.clamp_cursor_to_viewport(terminal)
    }

    fn scroll_view(&mut self, terminal: &mut Terminal, delta: isize) -> Result<()> {
        let before = terminal.scrollbar()?.offset as usize;
        terminal.scroll_viewport(ScrollViewport::Delta(delta));
        self.apply_crop_overflow(terminal, before, delta)?;
        self.clamp_cursor_to_viewport(terminal)
    }

    fn apply_crop_overflow(
        &mut self,
        terminal: &Terminal,
        before_offset: usize,
        delta: isize,
    ) -> Result<()> {
        let Some(active) = &mut self.active else {
            return Ok(());
        };
        let rows = terminal.rows()?;
        if rows == 0 {
            return Ok(());
        }
        let after = terminal.scrollbar()?.offset as usize;
        let vis = effective_visible(active.visible_rows, rows);
        let max_crop = usize::from(rows).saturating_sub(vis);
        active.crop_start = active.crop_start.min(max_crop);
        if delta < 0 {
            let wanted = delta.unsigned_abs();
            let moved = before_offset.saturating_sub(after);
            let leftover = wanted.saturating_sub(moved);
            active.crop_start = active.crop_start.saturating_sub(leftover);
        } else if delta > 0 {
            let wanted = delta as usize;
            let moved = after.saturating_sub(before_offset);
            let leftover = wanted.saturating_sub(moved);
            active.crop_start = active.crop_start.saturating_add(leftover).min(max_crop);
        }
        Ok(())
    }

    /// Move the copy cursor to the active search match's start cell, reveal it, and
    /// extend any active visual selection. No-op when copy mode is inactive or no
    /// match is selected.
    pub fn move_to_match(
        &mut self,
        terminal: &mut Terminal,
        search: &search::Controller,
    ) -> Result<()> {
        if !self.validate_alive(terminal)? {
            return Ok(());
        }
        let Some(target) = search.selected_match_screen(terminal)? else {
            return Ok(());
        };
        let total_rows = terminal.total_rows()?;
        let cols = terminal.cols()?;
        if total_rows == 0 || cols == 0 {
            return Ok(());
        }
        let y = (target.y as usize).min(total_rows - 1);
        let x = normalize_col(terminal, y, target.x)?;
        let active = self.active.as_mut().expect("validated active");
        active
            .cursor
            .set(terminal, Point::Screen(PointCoordinate { x, y: y as u32 }))?;
        active.desired_col = x;
        ensure_visible(terminal, active, y)?;
        sync_selection(terminal, active)?;
        Ok(())
    }

    /// Build the [`CopyView`] for the current frame, or `None` when inactive.
    pub fn view(&self, terminal: &Terminal) -> Result<Option<CopyView>> {
        let Some(active) = &self.active else {
            return Ok(None);
        };
        if terminal.active_screen()? != active.screen || !active.cursor.has_value() {
            return Ok(None);
        }
        let Some(pt) = active.cursor.point(PointSpace::Screen)? else {
            return Ok(None);
        };
        let cols = terminal.cols()?;
        let rows = terminal.rows()?;
        if cols == 0 || rows == 0 {
            return Ok(None);
        }
        let sb = terminal.scrollbar()?;
        let vp_top = sb.offset as u32;
        if pt.y < vp_top || pt.y >= vp_top.saturating_add(u32::from(rows)) {
            return Ok(None);
        }
        let vp_y = (pt.y - vp_top) as u16;
        let vp_x = pt.x.min(cols.saturating_sub(1));
        let wide = match active.cursor.snapshot(terminal)? {
            Some(gref) => gref.cell()?.wide()? == CellWide::Wide,
            None => false,
        };
        let vis = effective_visible(active.visible_rows, rows);
        let max_crop = usize::from(rows).saturating_sub(vis);
        let display_start = active.crop_start.min(max_crop) as u16;
        Ok(Some(CopyView {
            epoch: active.epoch,
            cursor: (vp_x, vp_y),
            wide,
            unit: active.unit,
            display_start,
        }))
    }

    fn begin(
        &mut self,
        terminal: &mut Terminal,
        epoch: u64,
        visible_rows: Option<u16>,
    ) -> Result<()> {
        // Always clear any pre-existing selection so copy mode starts clean.
        terminal.set_selection(None)?;
        let cols = terminal.cols()?;
        let rows = terminal.rows()?;
        let total_rows = terminal.total_rows()?;
        if cols == 0 || rows == 0 || total_rows == 0 {
            self.active = None;
            self.ended_epoch = self.ended_epoch.max(epoch);
            return Ok(());
        }
        let screen = terminal.active_screen()?;
        let sb = terminal.scrollbar()?;
        let vp_top = sb.offset as usize;
        let vp_rows = rows as usize;
        let vis = effective_visible(visible_rows, rows);
        let max_crop = vp_rows.saturating_sub(vis);

        // Place cursor at the program cursor if it is currently visible inside the
        // viewport; otherwise start at viewport (0, 0).
        let prog_screen = program_cursor_screen(terminal)?;
        let (start_x, start_y, crop_start) = match prog_screen {
            Some(pt)
                if (pt.y as usize) >= vp_top
                    && (pt.y as usize) < vp_top.saturating_add(vp_rows) =>
            {
                let y = (pt.y as usize).min(total_rows - 1);
                let x = normalize_col(terminal, y, pt.x)?;
                let vp_y = y - vp_top;
                // Match Frame::display_row_start baseline so entering copy mode on a
                // cropped pane never jumps the visible slice.
                let crop = max_crop.min(vp_y);
                (x, y, crop)
            }
            _ => {
                let y = vp_top.min(total_rows - 1);
                let x = normalize_col(terminal, y, 0)?;
                (x, y, 0)
            }
        };

        let cursor = terminal.track_grid_ref(Point::Screen(PointCoordinate {
            x: start_x,
            y: start_y as u32,
        }))?;
        self.active = Some(Active {
            epoch,
            screen,
            cursor,
            desired_col: start_x,
            anchor: None,
            unit: None,
            visible_rows,
            crop_start,
        });
        Ok(())
    }

    fn end_explicit(&mut self, terminal: &Terminal) -> Result<()> {
        if let Some(active) = self.active.take()
            && active.unit.is_some()
            && terminal.active_screen()? == active.screen
        {
            terminal.set_selection(None)?;
        }
        Ok(())
    }

    fn auto_end(&mut self, terminal: &Terminal, clear_selection: bool) -> Result<()> {
        if let Some(active) = self.active.take() {
            self.ended_epoch = self.ended_epoch.max(active.epoch);
            if clear_selection && active.unit.is_some() {
                terminal.set_selection(None)?;
            }
        }
        Ok(())
    }

    /// Returns `true` if copy mode is active and all tracked references remain valid
    /// on the same screen. Auto-ends cleanly and returns `false` otherwise.
    fn validate_alive(&mut self, terminal: &Terminal) -> Result<bool> {
        let Some(active) = &self.active else {
            return Ok(false);
        };
        let current_screen = terminal.active_screen()?;
        if current_screen != active.screen {
            // Do not wipe a selection belonging to the newly active screen.
            self.auto_end(terminal, false)?;
            return Ok(false);
        }
        let cursor_ok =
            active.cursor.has_value() && active.cursor.point(PointSpace::Screen)?.is_some();
        let anchor_ok = match &active.anchor {
            Some(anchor) => anchor.has_value() && anchor.point(PointSpace::Screen)?.is_some(),
            None => true,
        };
        if !cursor_ok || !anchor_ok {
            self.auto_end(terminal, true)?;
            return Ok(false);
        }
        Ok(true)
    }

    fn apply_select(&mut self, terminal: &Terminal, unit: CopyUnit) -> Result<()> {
        let Some(active) = &mut self.active else {
            return Ok(());
        };
        match active.unit {
            Some(existing) if existing == unit => {
                active.unit = None;
                active.anchor = None;
                terminal.set_selection(None)?;
            }
            Some(_) => {
                // Switch unit while preserving the existing anchor.
                active.unit = Some(unit);
                sync_selection(terminal, active)?;
            }
            None => {
                let Some(pt) = active.cursor.point(PointSpace::Screen)? else {
                    self.auto_end(terminal, true)?;
                    return Ok(());
                };
                let anchor = terminal.track_grid_ref(Point::Screen(pt))?;
                active.anchor = Some(anchor);
                active.unit = Some(unit);
                sync_selection(terminal, active)?;
            }
        }
        Ok(())
    }

    fn clear_selection(&mut self, terminal: &Terminal) -> Result<()> {
        if let Some(active) = &mut self.active {
            active.anchor = None;
            active.unit = None;
            terminal.set_selection(None)?;
        }
        Ok(())
    }

    fn swap_ends(&mut self, terminal: &mut Terminal) -> Result<()> {
        let Some(active) = &mut self.active else {
            return Ok(());
        };
        let Some(anchor) = active.anchor.as_mut() else {
            return Ok(());
        };
        std::mem::swap(&mut active.cursor, anchor);
        let Some(pt) = active.cursor.point(PointSpace::Screen)? else {
            self.auto_end(terminal, true)?;
            return Ok(());
        };
        let total_rows = terminal.total_rows()?;
        let y = (pt.y as usize).min(total_rows.saturating_sub(1));
        let x = normalize_col(terminal, y, pt.x)?;
        if x != pt.x {
            active
                .cursor
                .set(terminal, Point::Screen(PointCoordinate { x, y: y as u32 }))?;
        }
        active.desired_col = x;
        ensure_visible(terminal, active, y)?;
        sync_selection(terminal, active)?;
        Ok(())
    }

    fn clamp_cursor_to_viewport(&mut self, terminal: &mut Terminal) -> Result<()> {
        let Some(active) = &mut self.active else {
            return Ok(());
        };
        let total_rows = terminal.total_rows()?;
        let rows = terminal.rows()?;
        let cols = terminal.cols()?;
        if total_rows == 0 || rows == 0 || cols == 0 {
            return Ok(());
        }
        let Some(pt) = active.cursor.point(PointSpace::Screen)? else {
            self.auto_end(terminal, true)?;
            return Ok(());
        };
        let sb = terminal.scrollbar()?;
        let vp_top = sb.offset as usize;
        let vis = effective_visible(active.visible_rows, rows);
        let max_crop = (rows as usize).saturating_sub(vis);
        active.crop_start = active.crop_start.min(max_crop);
        let max_row = total_rows.saturating_sub(1);
        let vis_top = vp_top.saturating_add(active.crop_start).min(max_row);
        let vis_bot = vis_top.saturating_add(vis.saturating_sub(1)).min(max_row);
        let cur_y = (pt.y as usize).min(max_row);
        let clamped_y = cur_y.clamp(vis_top, vis_bot);
        if clamped_y != cur_y {
            let x = normalize_col(terminal, clamped_y, active.desired_col)?;
            active.cursor.set(
                terminal,
                Point::Screen(PointCoordinate {
                    x,
                    y: clamped_y as u32,
                }),
            )?;
            sync_selection(terminal, active)?;
        }
        Ok(())
    }

    fn yank_selection(
        &mut self,
        terminal: &Terminal,
        epoch: u64,
        revision: u64,
    ) -> Result<Option<CopyYank>> {
        let Some(active) = &mut self.active else {
            return Ok(None);
        };
        if active.unit.is_none() || active.anchor.is_none() {
            return Ok(None);
        }
        sync_selection(terminal, active)?;
        let text = format_active_selection(terminal, &mut self.copy_buffer)?;
        active.anchor = None;
        active.unit = None;
        terminal.set_selection(None)?;
        Ok(Some(CopyYank {
            epoch,
            revision,
            text,
        }))
    }

    fn yank_lines(
        &mut self,
        terminal: &Terminal,
        epoch: u64,
        revision: u64,
        count: u32,
    ) -> Result<Option<CopyYank>> {
        let Some(active) = &mut self.active else {
            return Ok(None);
        };
        let total_rows = terminal.total_rows()?;
        let cols = terminal.cols()?;
        let Some(pt) = active.cursor.point(PointSpace::Screen)? else {
            self.auto_end(terminal, true)?;
            return Ok(None);
        };
        let text = if total_rows == 0 || cols == 0 {
            String::new()
        } else {
            let cur_y = (pt.y as usize).min(total_rows - 1);
            let (start_y, mut end_y) = logical_line_bounds(terminal, cur_y, total_rows)?;
            for _ in 1..count {
                if end_y + 1 >= total_rows {
                    break;
                }
                end_y = logical_line_bounds(terminal, end_y + 1, total_rows)?.1;
            }
            let start = terminal.grid_ref(Point::Screen(PointCoordinate {
                x: 0,
                y: start_y as u32,
            }))?;
            let end = terminal.grid_ref(Point::Screen(PointCoordinate {
                x: cols - 1,
                y: end_y as u32,
            }))?;
            let sel = Selection::new(start, end, false);
            format_selection(terminal, Some(&sel), &mut self.copy_buffer)?
        };
        // Clear any visual selection if one was active so yy leaves normal copy mode.
        if active.unit.is_some() {
            active.anchor = None;
            active.unit = None;
            terminal.set_selection(None)?;
        }
        Ok(Some(CopyYank {
            epoch,
            revision,
            text,
        }))
    }

    fn apply_move(
        &mut self,
        terminal: &mut Terminal,
        motion: CopyMotion,
        count: u32,
    ) -> Result<()> {
        let Some(active) = &mut self.active else {
            return Ok(());
        };
        let total_rows = terminal.total_rows()?;
        let cols = terminal.cols()?;
        let rows = terminal.rows()?;
        if total_rows == 0 || cols == 0 || rows == 0 {
            return Ok(());
        }
        let Some(pt) = active.cursor.point(PointSpace::Screen)? else {
            self.auto_end(terminal, true)?;
            return Ok(());
        };
        let cur_y = (pt.y as usize).min(total_rows - 1);
        let cur_x = normalize_col(terminal, cur_y, pt.x)?;
        let vis = effective_visible(active.visible_rows, rows);
        let count_usize = count as usize;

        let (new_x, new_y, update_desired) = match motion {
            CopyMotion::Left => {
                let mut x = cur_x;
                let row = read_row(terminal, cur_y, cols)?;
                for _ in 0..count_usize {
                    if x == 0 {
                        break;
                    }
                    x = row.normalize_col(x - 1);
                }
                (x, cur_y, true)
            }
            CopyMotion::Right => {
                let mut x = cur_x;
                let row = read_row(terminal, cur_y, cols)?;
                for _ in 0..count_usize {
                    let step = if row.is_wide(x) { 2 } else { 1 };
                    let next = x.saturating_add(step);
                    if next >= cols {
                        break;
                    }
                    let next_norm = row.normalize_col(next);
                    if next_norm <= x {
                        break;
                    }
                    x = next_norm;
                }
                (x, cur_y, true)
            }
            CopyMotion::Up => {
                let y = cur_y.saturating_sub(count_usize);
                let x = normalize_col(terminal, y, active.desired_col)?;
                (x, y, false)
            }
            CopyMotion::Down => {
                let y = cur_y.saturating_add(count_usize).min(total_rows - 1);
                let x = normalize_col(terminal, y, active.desired_col)?;
                (x, y, false)
            }
            CopyMotion::LineStart => (0, cur_y, true),
            CopyMotion::FirstNonBlank => {
                let row = read_row(terminal, cur_y, cols)?;
                (row.first_non_blank_col().unwrap_or(0), cur_y, true)
            }
            CopyMotion::LineEnd => {
                let row = read_row(terminal, cur_y, cols)?;
                (row.last_non_blank_col().unwrap_or(0), cur_y, true)
            }
            CopyMotion::Top => {
                let y = 0;
                let x = normalize_col(terminal, y, active.desired_col)?;
                (x, y, false)
            }
            CopyMotion::Bottom => {
                let y = total_rows - 1;
                let x = normalize_col(terminal, y, active.desired_col)?;
                (x, y, false)
            }
            CopyMotion::LineNumber(line) => {
                let y = line.saturating_sub(1).min(total_rows - 1);
                let x = normalize_col(terminal, y, active.desired_col)?;
                (x, y, false)
            }
            CopyMotion::ViewportTop => {
                let (vis_top, vis_bot) = visible_window(terminal, active, total_rows, rows)?;
                let y = vis_top
                    .saturating_add(count_usize.saturating_sub(1))
                    .min(vis_bot);
                let x = normalize_col(terminal, y, active.desired_col)?;
                (x, y, false)
            }
            CopyMotion::ViewportMiddle => {
                let (vis_top, vis_bot) = visible_window(terminal, active, total_rows, rows)?;
                let y = vis_top + (vis_bot - vis_top) / 2;
                let x = normalize_col(terminal, y, active.desired_col)?;
                (x, y, false)
            }
            CopyMotion::ViewportBottom => {
                let (vis_top, vis_bot) = visible_window(terminal, active, total_rows, rows)?;
                let y = vis_bot
                    .saturating_sub(count_usize.saturating_sub(1))
                    .max(vis_top);
                let x = normalize_col(terminal, y, active.desired_col)?;
                (x, y, false)
            }
            CopyMotion::HalfPageUp
            | CopyMotion::HalfPageDown
            | CopyMotion::PageUp
            | CopyMotion::PageDown => {
                let step = match motion {
                    CopyMotion::HalfPageUp | CopyMotion::HalfPageDown => (vis / 2).max(1),
                    CopyMotion::PageUp | CopyMotion::PageDown => vis.max(1),
                    _ => unreachable!(),
                };
                let delta = step.saturating_mul(count_usize);
                let up = matches!(motion, CopyMotion::HalfPageUp | CopyMotion::PageUp);
                let y = if up {
                    cur_y.saturating_sub(delta)
                } else {
                    cur_y.saturating_add(delta).min(total_rows - 1)
                };
                // On uncropped panes, scroll the viewport alongside the cursor so
                // page motions keep the cursor's relative screen position like Vim.
                if vis == rows as usize {
                    let scroll = if up {
                        -(delta.min(isize::MAX as usize) as isize)
                    } else {
                        delta.min(isize::MAX as usize) as isize
                    };
                    terminal.scroll_viewport(ScrollViewport::Delta(scroll));
                }
                let x = normalize_col(terminal, y, active.desired_col)?;
                (x, y, false)
            }
            CopyMotion::WordForward => {
                let (x, y) =
                    move_word_forward(terminal, cur_x, cur_y, cols, total_rows, count, false)?;
                (x, y, true)
            }
            CopyMotion::BigWordForward => {
                let (x, y) =
                    move_word_forward(terminal, cur_x, cur_y, cols, total_rows, count, true)?;
                (x, y, true)
            }
            CopyMotion::WordBackward => {
                let (x, y) =
                    move_word_backward(terminal, cur_x, cur_y, cols, total_rows, count, false)?;
                (x, y, true)
            }
            CopyMotion::BigWordBackward => {
                let (x, y) =
                    move_word_backward(terminal, cur_x, cur_y, cols, total_rows, count, true)?;
                (x, y, true)
            }
            CopyMotion::WordEnd => {
                let (x, y) = move_word_end(terminal, cur_x, cur_y, cols, total_rows, count, false)?;
                (x, y, true)
            }
            CopyMotion::BigWordEnd => {
                let (x, y) = move_word_end(terminal, cur_x, cur_y, cols, total_rows, count, true)?;
                (x, y, true)
            }
        };

        active.cursor.set(
            terminal,
            Point::Screen(PointCoordinate {
                x: new_x,
                y: new_y as u32,
            }),
        )?;
        if update_desired {
            active.desired_col = new_x;
        }
        ensure_visible(terminal, active, new_y)?;
        sync_selection(terminal, active)?;
        Ok(())
    }
}

fn program_cursor_screen(terminal: &Terminal) -> Result<Option<PointCoordinate>> {
    let cols = terminal.cols()?;
    let rows = terminal.rows()?;
    if cols == 0 || rows == 0 {
        return Ok(None);
    }
    let cx = terminal.cursor_x()?.min(cols.saturating_sub(1));
    let cy = terminal.cursor_y()?.min(rows.saturating_sub(1));
    let gref = terminal.grid_ref(Point::Active(PointCoordinate {
        x: cx,
        y: u32::from(cy),
    }))?;
    Ok(terminal.point_from_grid_ref(&gref, PointSpace::Screen)?)
}

fn effective_visible(visible_rows: Option<u16>, rows: u16) -> usize {
    let rows = usize::from(rows).max(1);
    visible_rows.map_or(rows, |v| usize::from(v).clamp(1, rows))
}

fn visible_window(
    terminal: &Terminal,
    active: &mut Active,
    total_rows: usize,
    rows: u16,
) -> Result<(usize, usize)> {
    let vp_top = terminal.scrollbar()?.offset as usize;
    let vis = effective_visible(active.visible_rows, rows);
    let max_crop = (rows as usize).saturating_sub(vis);
    active.crop_start = active.crop_start.min(max_crop);
    let max_row = total_rows.saturating_sub(1);
    let vis_top = vp_top.saturating_add(active.crop_start).min(max_row);
    let vis_bot = vis_top.saturating_add(vis.saturating_sub(1)).min(max_row);
    Ok((vis_top, vis_bot))
}

fn ensure_visible(terminal: &mut Terminal, active: &mut Active, y: usize) -> Result<()> {
    let rows = terminal.rows()?;
    let vp_rows = usize::from(rows).max(1);
    let vis = effective_visible(active.visible_rows, rows);
    let max_crop = vp_rows.saturating_sub(vis);
    let sb = terminal.scrollbar()?;
    let vp_top = sb.offset as usize;

    if y < vp_top {
        terminal.scroll_viewport(ScrollViewport::Row(y));
        let new_top = terminal.scrollbar()?.offset as usize;
        active.crop_start = y.saturating_sub(new_top).min(max_crop);
    } else if y >= vp_top.saturating_add(vp_rows) {
        let target_top = y.saturating_add(1).saturating_sub(vp_rows);
        terminal.scroll_viewport(ScrollViewport::Row(target_top));
        let new_top = terminal.scrollbar()?.offset as usize;
        let vp_y = y.saturating_sub(new_top);
        active.crop_start = vp_y.saturating_add(1).saturating_sub(vis).min(max_crop);
    } else {
        let vp_y = y - vp_top;
        active.crop_start = active.crop_start.min(max_crop);
        if vp_y < active.crop_start {
            active.crop_start = vp_y;
        } else if vp_y >= active.crop_start.saturating_add(vis) {
            active.crop_start = vp_y.saturating_add(1).saturating_sub(vis).min(max_crop);
        }
    }
    Ok(())
}

fn normalize_col(terminal: &Terminal, y: usize, col: u16) -> Result<u16> {
    let cols = terminal.cols()?;
    if cols == 0 {
        return Ok(0);
    }
    let mut x = col.min(cols - 1);
    while x > 0 {
        let gref = terminal.grid_ref(Point::Screen(PointCoordinate { x, y: y as u32 }))?;
        if matches!(
            gref.cell()?.wide()?,
            CellWide::SpacerTail | CellWide::SpacerHead
        ) {
            x -= 1;
        } else {
            break;
        }
    }
    Ok(x)
}

/// Inclusive screen row bounds `(start_y, end_y)` of the soft-wrapped logical line
/// containing row `y`, bounded by `MAX_WORD_SCAN_ROWS` in each direction.
fn logical_line_bounds(terminal: &Terminal, y: usize, total_rows: usize) -> Result<(usize, usize)> {
    let max_row = total_rows.saturating_sub(1);
    let mut start_y = y.min(max_row);
    let mut steps = 0;
    while start_y > 0 && steps < MAX_WORD_SCAN_ROWS {
        let prev = terminal.grid_ref(Point::Screen(PointCoordinate {
            x: 0,
            y: (start_y - 1) as u32,
        }))?;
        if !prev.row()?.is_wrapped()? {
            break;
        }
        start_y -= 1;
        steps += 1;
    }
    let mut end_y = y.min(max_row);
    let mut steps = 0;
    while end_y < max_row && steps < MAX_WORD_SCAN_ROWS {
        let cur = terminal.grid_ref(Point::Screen(PointCoordinate {
            x: 0,
            y: end_y as u32,
        }))?;
        if !cur.row()?.is_wrapped()? {
            break;
        }
        end_y += 1;
        steps += 1;
    }
    Ok((start_y, end_y))
}

fn wide_end_col(terminal: &Terminal, y: u32, col: u16, cols: u16) -> Result<u16> {
    let x = col.min(cols.saturating_sub(1));
    let gref = terminal.grid_ref(Point::Screen(PointCoordinate { x, y }))?;
    if gref.cell()?.wide()? == CellWide::Wide && x + 1 < cols {
        Ok(x + 1)
    } else {
        Ok(x)
    }
}

fn sync_selection(terminal: &Terminal, active: &Active) -> Result<()> {
    let (Some(unit), Some(anchor)) = (active.unit, &active.anchor) else {
        return Ok(());
    };
    let (Some(a_raw), Some(c_raw)) = (
        anchor.point(PointSpace::Screen)?,
        active.cursor.point(PointSpace::Screen)?,
    ) else {
        return Ok(());
    };
    let cols = terminal.cols()?;
    let total_rows = terminal.total_rows()?;
    if cols == 0 || total_rows == 0 {
        return Ok(());
    }
    let max_row = total_rows - 1;
    let a = PointCoordinate {
        x: normalize_col(terminal, (a_raw.y as usize).min(max_row), a_raw.x)?,
        y: (a_raw.y as usize).min(max_row) as u32,
    };
    let c = PointCoordinate {
        x: normalize_col(terminal, (c_raw.y as usize).min(max_row), c_raw.x)?,
        y: (c_raw.y as usize).min(max_row) as u32,
    };
    let sel = match unit {
        CopyUnit::Char => {
            let (first, last) = if (a.y, a.x) <= (c.y, c.x) {
                (a, c)
            } else {
                (c, a)
            };
            let end_x = wide_end_col(terminal, last.y, last.x, cols)?;
            let start_ref = terminal.grid_ref(Point::Screen(first))?;
            let end_ref = terminal.grid_ref(Point::Screen(PointCoordinate {
                x: end_x,
                y: last.y,
            }))?;
            Selection::new(start_ref, end_ref, false)
        }
        CopyUnit::Line => {
            let top_y = logical_line_bounds(terminal, a.y.min(c.y) as usize, total_rows)?.0;
            let bot_y = logical_line_bounds(terminal, a.y.max(c.y) as usize, total_rows)?.1;
            let start_ref = terminal.grid_ref(Point::Screen(PointCoordinate {
                x: 0,
                y: top_y as u32,
            }))?;
            let end_ref = terminal.grid_ref(Point::Screen(PointCoordinate {
                x: cols - 1,
                y: bot_y as u32,
            }))?;
            Selection::new(start_ref, end_ref, false)
        }
        CopyUnit::Block => {
            let top_y = a.y.min(c.y);
            let bot_y = a.y.max(c.y);
            let left_x = a.x.min(c.x);
            let right_x = a.x.max(c.x);
            // If the rightmost corner sits on a wide glyph's head cell, include its
            // tail cell so the block encloses the full character under that corner.
            let right_end = {
                let ax_end = if a.x == right_x {
                    wide_end_col(terminal, a.y, a.x, cols)?
                } else {
                    right_x
                };
                let cx_end = if c.x == right_x {
                    wide_end_col(terminal, c.y, c.x, cols)?
                } else {
                    right_x
                };
                ax_end.max(cx_end)
            };
            let start_ref = terminal.grid_ref(Point::Screen(PointCoordinate {
                x: left_x,
                y: top_y,
            }))?;
            let end_ref = terminal.grid_ref(Point::Screen(PointCoordinate {
                x: right_end,
                y: bot_y,
            }))?;
            Selection::new(start_ref, end_ref, true)
        }
    };
    terminal.set_selection(Some(&sel))?;
    Ok(())
}

fn format_active_selection(terminal: &Terminal, buffer: &mut Vec<u8>) -> Result<String> {
    format_selection(terminal, None, buffer)
}

fn format_selection(
    terminal: &Terminal,
    selection: Option<&Selection<'_>>,
    buffer: &mut Vec<u8>,
) -> Result<String> {
    let make_options = || {
        let opts = FormatOptions::new().with_unwrap(true).with_trim(true);
        match selection {
            Some(sel) => opts.with_selection(sel),
            None => opts,
        }
    };
    if buffer.is_empty() {
        buffer.resize(4096, 0);
    }
    let length = match terminal.format_selection_buf(make_options(), buffer) {
        Err(Error::OutOfSpace { required }) if required <= selection::COPY_LIMIT => {
            buffer.resize(required, 0);
            terminal.format_selection_buf(make_options(), buffer)?
        }
        Err(Error::OutOfSpace { .. }) => {
            anyhow::bail!("Selection exceeds the 8 MiB copy limit")
        }
        result => result?,
    };
    match length {
        Some(n) => Ok(std::str::from_utf8(&buffer[..n])?.to_owned()),
        None => Ok(String::new()),
    }
}

// ---------------------------------------------------------------------------
// Row inspection & Vim word / WORD motion engine
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CharClass {
    Space,
    Word,
    Punct,
}

#[derive(Clone, Copy, Debug)]
struct CellInfo {
    wide: CellWide,
    ch: Option<char>,
}

impl CellInfo {
    fn is_spacer(self) -> bool {
        matches!(self.wide, CellWide::SpacerTail | CellWide::SpacerHead)
    }

    fn is_blank(self) -> bool {
        self.ch.is_none_or(char::is_whitespace)
    }

    fn class(self, big: bool) -> CharClass {
        match self.ch {
            None => CharClass::Space,
            Some(c) if c.is_whitespace() => CharClass::Space,
            Some(_) if big => CharClass::Word,
            Some(c) if c.is_alphanumeric() || c == '_' => CharClass::Word,
            Some(_) => CharClass::Punct,
        }
    }
}

struct RowData {
    is_wrapped: bool,
    cells: Vec<CellInfo>,
    /// Cached logical (non-spacer, trailing-padding-trimmed on non-wrapped rows) cells
    /// so repeated word/WORD steps on long wrapped lines stay O(N).
    logical: Vec<(u16, CellInfo)>,
}

impl RowData {
    fn normalize_col(&self, col: u16) -> u16 {
        if self.cells.is_empty() {
            return 0;
        }
        let mut x = (col as usize).min(self.cells.len() - 1);
        while x > 0
            && matches!(
                self.cells[x].wide,
                CellWide::SpacerTail | CellWide::SpacerHead
            )
        {
            x -= 1;
        }
        x as u16
    }

    fn is_wide(&self, col: u16) -> bool {
        self.cells
            .get(col as usize)
            .is_some_and(|c| c.wide == CellWide::Wide)
    }

    fn first_non_blank_col(&self) -> Option<u16> {
        self.cells
            .iter()
            .enumerate()
            .find(|(_, c)| !c.is_spacer() && !c.is_blank())
            .map(|(i, _)| i as u16)
    }

    fn last_non_blank_col(&self) -> Option<u16> {
        self.cells
            .iter()
            .enumerate()
            .rev()
            .find(|(_, c)| !c.is_spacer() && !c.is_blank())
            .map(|(i, _)| i as u16)
    }

    /// Cached columns holding logical characters on this row (skipping `SpacerTail` and
    /// `SpacerHead`). On a non-wrapped row, trailing blank padding after the last
    /// non-blank character is excluded so hard newlines break cleanly.
    fn logical_cols(&self) -> &[(u16, CellInfo)] {
        &self.logical
    }
}

fn read_row(terminal: &Terminal, y: usize, cols: u16) -> Result<RowData> {
    let mut cells = Vec::with_capacity(usize::from(cols));
    let mut is_wrapped = false;
    for x in 0..cols {
        let gref = terminal.grid_ref(Point::Screen(PointCoordinate { x, y: y as u32 }))?;
        if x == 0 {
            is_wrapped = gref.row()?.is_wrapped()?;
        }
        let raw = gref.cell()?;
        let wide = raw.wide()?;
        let ch =
            if matches!(wide, CellWide::SpacerTail | CellWide::SpacerHead) || !raw.has_text()? {
                None
            } else {
                let cp = raw.codepoint()?;
                if cp == 0 { None } else { char::from_u32(cp) }
            };
        cells.push(CellInfo { wide, ch });
    }
    let limit = if is_wrapped {
        cells.len()
    } else {
        cells
            .iter()
            .enumerate()
            .rev()
            .find(|(_, c)| !c.is_spacer() && !c.is_blank())
            .map_or(0, |(i, _)| i + 1)
    };
    let mut logical = Vec::with_capacity(limit);
    for (i, &cell) in cells.iter().take(limit).enumerate() {
        if !cell.is_spacer() {
            logical.push((i as u16, cell));
        }
    }
    Ok(RowData {
        is_wrapped,
        cells,
        logical,
    })
}

struct RowCache<'a> {
    terminal: &'a Terminal,
    cols: u16,
    total_rows: usize,
    cached_y: Option<usize>,
    cached: Option<RowData>,
    scanned: usize,
}

impl<'a> RowCache<'a> {
    fn new(terminal: &'a Terminal, cols: u16, total_rows: usize) -> Self {
        Self {
            terminal,
            cols,
            total_rows,
            cached_y: None,
            cached: None,
            scanned: 0,
        }
    }

    fn row(&mut self, y: usize) -> Result<Option<&RowData>> {
        if y >= self.total_rows {
            return Ok(None);
        }
        if self.cached_y != Some(y) {
            if self.scanned >= MAX_WORD_SCAN_ROWS {
                return Ok(None);
            }
            self.scanned += 1;
            self.cached = Some(read_row(self.terminal, y, self.cols)?);
            self.cached_y = Some(y);
        }
        Ok(self.cached.as_ref())
    }
}

/// Next logical cell on the same logical line (crossing soft-wraps, stopping at hard newlines).
fn next_in_logical_line(
    cache: &mut RowCache<'_>,
    x: u16,
    y: usize,
) -> Result<Option<(u16, usize, CellInfo)>> {
    let Some(row) = cache.row(y)? else {
        return Ok(None);
    };
    let wrapped = row.is_wrapped;
    let logical = row.logical_cols();
    if let Some(&(nx, cell)) = logical.iter().find(|&&(col, _)| col > x) {
        return Ok(Some((nx, y, cell)));
    }
    if !wrapped {
        return Ok(None);
    }
    // Soft-wrapped into subsequent rows; skip any edge case row that has only spacers.
    let mut ny = y + 1;
    while ny < cache.total_rows {
        let Some(next_row) = cache.row(ny)? else {
            return Ok(None);
        };
        let next_wrapped = next_row.is_wrapped;
        let next_logical = next_row.logical_cols();
        if let Some(&(nx, cell)) = next_logical.first() {
            return Ok(Some((nx, ny, cell)));
        }
        if !next_wrapped {
            return Ok(None);
        }
        ny += 1;
    }
    Ok(None)
}

/// Previous logical cell on the same logical line (crossing soft-wraps backward,
/// stopping at hard newlines).
fn prev_in_logical_line(
    cache: &mut RowCache<'_>,
    x: u16,
    y: usize,
) -> Result<Option<(u16, usize, CellInfo)>> {
    let Some(row) = cache.row(y)? else {
        return Ok(None);
    };
    let logical = row.logical_cols();
    if let Some(&(px, cell)) = logical.iter().rev().find(|&&(col, _)| col < x) {
        return Ok(Some((px, y, cell)));
    }
    let mut py = y;
    while py > 0 {
        py -= 1;
        let Some(prev_row) = cache.row(py)? else {
            return Ok(None);
        };
        if !prev_row.is_wrapped {
            return Ok(None);
        }
        let prev_logical = prev_row.logical_cols();
        if let Some(&(px, cell)) = prev_logical.last() {
            return Ok(Some((px, py, cell)));
        }
    }
    Ok(None)
}

/// First non-space cell strictly after `(x, y)` (crossing soft-wraps and hard newlines).
fn find_next_non_space(
    cache: &mut RowCache<'_>,
    x: u16,
    y: usize,
    big: bool,
) -> Result<Option<(u16, usize, CellInfo)>> {
    if let Some(row) = cache.row(y)? {
        for &(col, cell) in row.logical_cols() {
            if col > x && cell.class(big) != CharClass::Space {
                return Ok(Some((col, y, cell)));
            }
        }
    }
    let mut ny = y + 1;
    while ny < cache.total_rows {
        let Some(row) = cache.row(ny)? else {
            return Ok(None);
        };
        for &(col, cell) in row.logical_cols() {
            if cell.class(big) != CharClass::Space {
                return Ok(Some((col, ny, cell)));
            }
        }
        ny += 1;
    }
    Ok(None)
}

/// Last non-space cell strictly before `(x, y)` (crossing soft-wraps and hard newlines).
fn find_prev_non_space(
    cache: &mut RowCache<'_>,
    x: u16,
    y: usize,
    big: bool,
) -> Result<Option<(u16, usize, CellInfo)>> {
    if let Some(row) = cache.row(y)? {
        for &(col, cell) in row.logical_cols().iter().rev() {
            if col < x && cell.class(big) != CharClass::Space {
                return Ok(Some((col, y, cell)));
            }
        }
    }
    let mut py = y;
    while py > 0 {
        py -= 1;
        let Some(row) = cache.row(py)? else {
            return Ok(None);
        };
        for &(col, cell) in row.logical_cols().iter().rev() {
            if cell.class(big) != CharClass::Space {
                return Ok(Some((col, py, cell)));
            }
        }
    }
    Ok(None)
}

fn cell_at(cache: &mut RowCache<'_>, x: u16, y: usize) -> Result<Option<CellInfo>> {
    Ok(cache.row(y)?.and_then(|r| r.cells.get(x as usize).copied()))
}

fn move_word_forward(
    terminal: &Terminal,
    mut x: u16,
    mut y: usize,
    cols: u16,
    total_rows: usize,
    count: u32,
    big: bool,
) -> Result<(u16, usize)> {
    let mut cache = RowCache::new(terminal, cols, total_rows);
    for _ in 0..count {
        let Some(cur) = cell_at(&mut cache, x, y)? else {
            break;
        };
        let start_class = cur.class(big);
        if start_class == CharClass::Space {
            if let Some((nx, ny, _)) = find_next_non_space(&mut cache, x, y, big)? {
                x = nx;
                y = ny;
            } else {
                break;
            }
        } else {
            let mut last = (x, y);
            let mut switched: Option<(u16, usize)> = None;
            while let Some((nx, ny, cell)) = next_in_logical_line(&mut cache, last.0, last.1)? {
                let cls = cell.class(big);
                if cls == start_class {
                    last = (nx, ny);
                } else if cls != CharClass::Space {
                    switched = Some((nx, ny));
                    break;
                } else {
                    break;
                }
            }
            if let Some((nx, ny)) = switched {
                x = nx;
                y = ny;
            } else if let Some((nx, ny, _)) = find_next_non_space(&mut cache, last.0, last.1, big)?
            {
                x = nx;
                y = ny;
            } else {
                // At EOF with no following word: land on the last char of the final word.
                if last == (x, y) {
                    break;
                }
                x = last.0;
                y = last.1;
            }
        }
    }
    Ok((x, y))
}

fn move_word_end(
    terminal: &Terminal,
    mut x: u16,
    mut y: usize,
    cols: u16,
    total_rows: usize,
    count: u32,
    big: bool,
) -> Result<(u16, usize)> {
    let mut cache = RowCache::new(terminal, cols, total_rows);
    for _ in 0..count {
        // Step at least one logical position forward to find the next non-space cell.
        let Some((mut cx, mut cy, cell)) = find_next_non_space(&mut cache, x, y, big)? else {
            break;
        };
        let target_class = cell.class(big);
        while let Some((nx, ny, next_cell)) = next_in_logical_line(&mut cache, cx, cy)? {
            if next_cell.class(big) == target_class {
                cx = nx;
                cy = ny;
            } else {
                break;
            }
        }
        x = cx;
        y = cy;
    }
    Ok((x, y))
}

fn move_word_backward(
    terminal: &Terminal,
    mut x: u16,
    mut y: usize,
    cols: u16,
    total_rows: usize,
    count: u32,
    big: bool,
) -> Result<(u16, usize)> {
    let mut cache = RowCache::new(terminal, cols, total_rows);
    for _ in 0..count {
        let Some((mut cx, mut cy, cell)) = find_prev_non_space(&mut cache, x, y, big)? else {
            break;
        };
        let target_class = cell.class(big);
        while let Some((px, py, prev_cell)) = prev_in_logical_line(&mut cache, cx, cy)? {
            if prev_cell.class(big) == target_class {
                cx = px;
                cy = py;
            } else {
                break;
            }
        }
        x = cx;
        y = cy;
    }
    Ok((x, y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Renderer;

    fn make_terminal(cols: u16, rows: u16, max_scrollback: usize) -> Terminal {
        Terminal::new(ghostty_vt::TerminalOptions {
            cols,
            rows,
            max_scrollback,
        })
        .unwrap()
    }

    fn cmd(epoch: u64, revision: u64, visible_rows: Option<u16>, op: CopyModeOp) -> CopyModeInput {
        CopyModeInput {
            generation: 0,
            epoch,
            revision,
            visible_rows,
            op,
        }
    }

    fn step(
        ctrl: &mut Controller,
        term: &mut Terminal,
        epoch: u64,
        op: CopyModeOp,
    ) -> ApplyOutcome {
        ctrl.apply(term, None, cmd(epoch, 1, None, op)).unwrap()
    }

    fn cursor_screen(ctrl: &Controller) -> (u16, u32) {
        let pt = ctrl
            .active
            .as_ref()
            .unwrap()
            .cursor
            .point(PointSpace::Screen)
            .unwrap()
            .unwrap();
        (pt.x, pt.y)
    }

    fn finish_search(search: &mut search::Controller, term: &Terminal) {
        for _ in 0..10_000 {
            if !search.update(term).unwrap() {
                return;
            }
        }
        panic!("search did not finish");
    }

    #[test]
    fn full_scrollback_navigation_and_cropped_panes() {
        let mut term = make_terminal(20, 6, 1 << 16);
        for i in 1..=25 {
            let nl = if i < 25 { "\r\n" } else { "" };
            term.vt_write(format!("line-{i:02}{nl}").as_bytes());
        }
        let total = term.total_rows().unwrap();
        assert_eq!(total, 25);

        let mut ctrl = Controller::default();
        step(&mut ctrl, &mut term, 1, CopyModeOp::Begin);
        // Program cursor is at end of "line-25" (col 7, screen row 24, viewport row 5).
        assert_eq!(cursor_screen(&ctrl), (7, 24));
        assert_eq!(
            ctrl.view(&term).unwrap(),
            Some(CopyView {
                epoch: 1,
                cursor: (7, 5),
                wide: false,
                unit: None,
                display_start: 0,
            })
        );

        // Move up past the viewport top: Ghostty scrolls so cursor stays on screen.
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Up,
                count: 8,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (7, 16));
        assert_eq!(ctrl.view(&term).unwrap().unwrap().cursor, (7, 0));

        // Top (`gg`) reaches oldest scrollback row 0.
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Top,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (7, 0));
        assert_eq!(ctrl.view(&term).unwrap().unwrap().cursor, (7, 0));
        assert_eq!(term.scrollbar().unwrap().offset, 0);

        // Bottom (`G`) reaches the newest row.
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Bottom,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (7, 24));
        assert_eq!(ctrl.view(&term).unwrap().unwrap().cursor, (7, 5));

        // LineNumber (1-based) moves to exact row and clamps out-of-range values.
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineNumber(10),
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (7, 9));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineNumber(9999),
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (7, 24));

        // HalfPageUp / HalfPageDown / PageUp / PageDown.
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::HalfPageUp,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (7, 21));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::PageUp,
                count: 2,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (7, 9));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::PageDown,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (7, 15));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::HalfPageDown,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (7, 18));

        // H / M / L on uncropped 6-row viewport.
        let vp_top = term.scrollbar().unwrap().offset as u32;
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::ViewportTop,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl).1, vp_top);
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::ViewportMiddle,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl).1, vp_top + 2);
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::ViewportBottom,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl).1, vp_top + 5);

        // Cropped non-owner pane (`visible_rows = 3 < rows = 6`):
        // ViewportTop/Middle/Bottom and HalfPage/Page respect the 3-row slice,
        // and Frame::display_row_start_at keeps cursor visible without double-scrolling.
        term.scroll_viewport(ScrollViewport::Bottom);
        ctrl.apply(&mut term, None, cmd(2, 1, Some(3), CopyModeOp::Begin))
            .unwrap();
        // Bottom-cropped: viewport shows screen rows 19..25, crop shows 22..25 (vp rows 3..6).
        assert_eq!(cursor_screen(&ctrl), (7, 24));
        ctrl.apply(
            &mut term,
            None,
            cmd(
                2,
                2,
                Some(3),
                CopyModeOp::Move {
                    motion: CopyMotion::ViewportTop,
                    count: 1,
                },
            ),
        )
        .unwrap();
        assert_eq!(cursor_screen(&ctrl).1, 22, "H lands on cropped top");
        let offset_before = term.scrollbar().unwrap().offset;
        // Moving up 1 row shifts crop window without scrolling Ghostty yet.
        ctrl.apply(
            &mut term,
            None,
            cmd(
                2,
                3,
                Some(3),
                CopyModeOp::Move {
                    motion: CopyMotion::Up,
                    count: 1,
                },
            ),
        )
        .unwrap();
        assert_eq!(cursor_screen(&ctrl).1, 21);
        assert_eq!(
            term.scrollbar().unwrap().offset,
            offset_before,
            "no double-scroll while inside Ghostty viewport"
        );
        let mut renderer = Renderer::new().unwrap();
        let mut frame = renderer.frame(&term).unwrap();
        frame.copy = ctrl.view(&term).unwrap();
        let view = frame.copy.unwrap();
        let crop = frame.display_row_start(3);
        assert_eq!(
            crop,
            usize::from(view.display_start),
            "Frame::display_row_start uses CopyView::display_start as single source of truth"
        );
        assert_eq!(crop, 2, "cropped view shifts by 1 to show vp row 2");
        assert!(view.cursor.1 as usize >= crop);
        assert!((view.cursor.1 as usize) < crop + 3);
    }

    #[test]
    fn cropped_pane_scroll_and_bottom_reach_every_row_with_and_without_scrollback() {
        let mut renderer = Renderer::new().unwrap();

        // Case A: With scrollback (rows = 6, vis = 3, total = 20 -> max_vp_top = 14, max_crop = 3).
        let mut term = make_terminal(16, 6, 1 << 16);
        for i in 0..20 {
            let nl = if i < 19 { "\r\n" } else { "" };
            term.vt_write(format!("row-{i:02}{nl}").as_bytes());
        }
        assert_eq!(term.total_rows().unwrap(), 20);
        let mut ctrl = Controller::default();
        ctrl.apply(&mut term, None, cmd(1, 1, Some(3), CopyModeOp::Begin))
            .unwrap();
        assert_eq!(cursor_screen(&ctrl).1, 19);

        // Scroll up past scrollback top: vp_top saturates at 0 and crop_start shifts to 0.
        ctrl.apply(&mut term, None, cmd(1, 2, Some(3), CopyModeOp::Scroll(-50)))
            .unwrap();
        assert_eq!(term.scrollbar().unwrap().offset, 0);
        let view = ctrl.view(&term).unwrap().unwrap();
        assert_eq!(view.display_start, 0, "top rows 0..3 are reachable");
        assert!(cursor_screen(&ctrl).1 <= 2);
        let mut frame = renderer.frame(&term).unwrap();
        frame.copy = Some(view);
        assert_eq!(frame.display_row_start(3), usize::from(view.display_start));

        // Scroll down past bottom: vp_top saturates at 14 and crop_start shifts to 3.
        ctrl.apply(&mut term, None, cmd(1, 3, Some(3), CopyModeOp::Scroll(50)))
            .unwrap();
        assert_eq!(term.scrollbar().unwrap().offset, 14);
        let view = ctrl.view(&term).unwrap().unwrap();
        assert_eq!(view.display_start, 3, "newest rows 17..20 are reachable");
        assert!(cursor_screen(&ctrl).1 >= 17);
        let mut frame = renderer.frame(&term).unwrap();
        frame.copy = Some(view);
        assert_eq!(frame.display_row_start(3), usize::from(view.display_start));

        // Scroll back to top, then Bottom (`after_scroll_bottom`) must restore crop_start = 3.
        ctrl.apply(&mut term, None, cmd(1, 4, Some(3), CopyModeOp::Scroll(-50)))
            .unwrap();
        assert_eq!(ctrl.view(&term).unwrap().unwrap().display_start, 0);
        term.scroll_viewport(ScrollViewport::Bottom);
        ctrl.after_scroll_bottom(&mut term).unwrap();
        let view = ctrl.view(&term).unwrap().unwrap();
        assert_eq!(view.display_start, 3);
        assert!(cursor_screen(&ctrl).1 >= 17);

        // Case B: Without scrollback / alternate screen (rows = 6, vis = 3, total = 6, max_vp_top = 0).
        let mut alt = make_terminal(16, 6, 1 << 16);
        alt.vt_write(b"\x1b[?1049h\x1b[H0\r\n1\r\n2\r\n3\r\n4\r\n5");
        let mut alt_ctrl = Controller::default();
        alt_ctrl
            .apply(&mut alt, None, cmd(1, 1, Some(3), CopyModeOp::Begin))
            .unwrap();
        assert_eq!(alt_ctrl.view(&alt).unwrap().unwrap().display_start, 3);
        // Scroll up reaches row 0 even when Ghostty has zero scrollback.
        alt_ctrl
            .apply(&mut alt, None, cmd(1, 2, Some(3), CopyModeOp::Scroll(-10)))
            .unwrap();
        let view = alt_ctrl.view(&alt).unwrap().unwrap();
        assert_eq!(view.display_start, 0);
        assert!(cursor_screen(&alt_ctrl).1 <= 2);
        // Scroll down reaches row 5 again.
        alt_ctrl
            .apply(&mut alt, None, cmd(1, 3, Some(3), CopyModeOp::Scroll(10)))
            .unwrap();
        let view = alt_ctrl.view(&alt).unwrap().unwrap();
        assert_eq!(view.display_start, 3);
        assert!(cursor_screen(&alt_ctrl).1 >= 3);
    }

    #[test]
    fn line_selection_and_yank_line_expand_across_soft_wrapped_logical_lines() {
        let mut term = make_terminal(5, 8, 1 << 16);
        // Line 1 (3 soft-wrapped rows 0..=2): "aaaaabbbbbccccc"
        // Line 2 (3 soft-wrapped rows 3..=5): "ddddddeeeeeefff"
        // Line 3 (1 row 6): "tail"
        term.vt_write(b"aaaaabbbbbccccc\r\nddddddeeeeeefff\r\ntail");
        let mut ctrl = Controller::default();
        step(&mut ctrl, &mut term, 1, CopyModeOp::Begin);

        // Move to middle row (row 1) of the first 3-row wrapped line.
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineNumber(2),
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl).1, 1);

        // Visual Line (`V`) on row 1 must expand to cover rows 0..=2 and yank whole line 1.
        step(&mut ctrl, &mut term, 1, CopyModeOp::Select(CopyUnit::Line));
        let mut renderer = Renderer::new().unwrap();
        let frame = renderer.frame(&term).unwrap();
        assert_eq!(frame.selection_rows[0], Some((0, 4)));
        assert_eq!(frame.selection_rows[1], Some((0, 4)));
        assert_eq!(frame.selection_rows[2], Some((0, 4)));
        assert_eq!(frame.selection_rows[3], None);
        let v_yank = ctrl
            .apply(&mut term, None, cmd(1, 2, None, CopyModeOp::Yank))
            .unwrap()
            .yank
            .unwrap();
        assert_eq!(v_yank.text, "aaaaabbbbbccccc");

        // `yy` (`YankLine { count: 1 }`) from row 1 yanks the entire 3-row wrapped logical line.
        let yy = ctrl
            .apply(
                &mut term,
                None,
                cmd(1, 3, None, CopyModeOp::YankLine { count: 1 }),
            )
            .unwrap()
            .yank
            .unwrap();
        assert_eq!(yy.text, "aaaaabbbbbccccc");

        // `2yy` (`YankLine { count: 2 }`) counts whole logical lines, yanking both 3-row lines.
        let two_yy = ctrl
            .apply(
                &mut term,
                None,
                cmd(1, 4, None, CopyModeOp::YankLine { count: 2 }),
            )
            .unwrap()
            .yank
            .unwrap();
        assert_eq!(two_yy.text, "aaaaabbbbbccccc\nddddddeeeeeefff");
    }

    #[test]
    fn spacer_head_normalization_and_right_guard_on_narrow_wide_wrap() {
        let mut term = make_terminal(4, 4, 1 << 16);
        // cols = 4:
        // Row 0: 'a'(0), '世'(1), SpacerTail(2), SpacerHead(3) [wrapped]
        // Row 1: '界'(0), SpacerTail(1)
        // Row 2: "0123"
        term.vt_write("a世界\r\n0123".as_bytes());
        let mut ctrl = Controller::default();
        step(&mut ctrl, &mut term, 1, CopyModeOp::Begin);

        // Move to row 0, col 0 ('a'), then Right -> '世' (1), then Right again must NOT land on SpacerHead (3).
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Top,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineStart,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Right,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (1, 0));
        assert!(ctrl.view(&term).unwrap().unwrap().wide);
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Right,
                count: 5,
            },
        );
        assert_eq!(
            cursor_screen(&ctrl),
            (1, 0),
            "Right from '世' must not park on SpacerHead col 3"
        );

        // Vertical Up from row 2 col 3 (desired_col = 3) onto row 0 must normalize off SpacerHead to '世' (1).
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineNumber(3),
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineEnd,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (3, 2));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Up,
                count: 2,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (1, 0));
        assert!(ctrl.view(&term).unwrap().unwrap().wide);

        // WordEnd from 'a' (0, 0) crosses SpacerHead without splitting "a世界", landing on '界' (0, 1).
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineStart,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::WordEnd,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (0, 1));
        assert!(ctrl.view(&term).unwrap().unwrap().wide);
    }

    #[test]
    fn sync_normalizes_anchor_off_wide_spacer_tail() {
        let mut term = make_terminal(10, 3, 1 << 16);
        term.vt_write(b"abcd\r\n");
        let mut ctrl = Controller::default();
        step(&mut ctrl, &mut term, 1, CopyModeOp::Begin);
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Top,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineStart,
                count: 1,
            },
        );
        // Anchor at col 2 ('c'), extend cursor to col 3 ('d').
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Right,
                count: 2,
            },
        );
        step(&mut ctrl, &mut term, 1, CopyModeOp::Select(CopyUnit::Char));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Right,
                count: 1,
            },
        );
        // Overwrite cols 1..=2 with wide '世' so the anchor at col 2 becomes SpacerTail.
        term.vt_write("\x1b[1;2H世".as_bytes());
        ctrl.sync(&mut term).unwrap();
        let yank = ctrl
            .apply(&mut term, None, cmd(1, 2, None, CopyModeOp::Yank))
            .unwrap()
            .yank
            .unwrap();
        assert_eq!(
            yank.text, "世d",
            "anchor normalized off SpacerTail keeps '世' intact"
        );
    }

    #[test]
    fn wide_cjk_emoji_and_combining_marks_never_land_on_spacer_tail() {
        let mut term = make_terminal(20, 4, 1 << 16);
        // Row 0: "a世界 🦀 e\u{301} z"
        // cols: 0='a', 1..2='世', 3..4='界', 5=' ', 6..7='🦀', 8=' ', 9='é', 10=' ', 11='z'
        term.vt_write("a世界 🦀 e\u{301} z\r\n0123456789ab".as_bytes());
        let mut ctrl = Controller::default();
        step(&mut ctrl, &mut term, 1, CopyModeOp::Begin);
        // Move to row 0, col 0.
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Top,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineStart,
                count: 1,
            },
        );
        assert_eq!(ctrl.view(&term).unwrap().unwrap().cursor, (0, 0));
        assert!(!ctrl.view(&term).unwrap().unwrap().wide);

        // l -> '世' at col 1 (wide == true)
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Right,
                count: 1,
            },
        );
        let v = ctrl.view(&term).unwrap().unwrap();
        assert_eq!(v.cursor, (1, 0));
        assert!(v.wide);

        // l steps over wide cell by 2 columns -> '界' at col 3 (wide == true)
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Right,
                count: 1,
            },
        );
        let v = ctrl.view(&term).unwrap().unwrap();
        assert_eq!(v.cursor, (3, 0));
        assert!(v.wide);

        // l -> ' ' at col 5 -> l -> '🦀' at col 6 (wide == true)
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Right,
                count: 2,
            },
        );
        let v = ctrl.view(&term).unwrap().unwrap();
        assert_eq!(v.cursor, (6, 0));
        assert!(v.wide);

        // l -> ' ' at col 8 -> l -> 'é' at col 9 (narrow combining cluster)
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Right,
                count: 2,
            },
        );
        let v = ctrl.view(&term).unwrap().unwrap();
        assert_eq!(v.cursor, (9, 0));
        assert!(!v.wide);

        // h back to ' ' (8), then h snaps to '🦀' head cell (6), never SpacerTail (7)!
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Left,
                count: 2,
            },
        );
        let v = ctrl.view(&term).unwrap().unwrap();
        assert_eq!(v.cursor, (6, 0));
        assert!(v.wide);

        // Vertical move from col 2 on row 1 up to row 0 lands on '世' head col 1 (not SpacerTail col 2).
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Down,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineStart,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Right,
                count: 2,
            },
        );
        assert_eq!(ctrl.view(&term).unwrap().unwrap().cursor, (2, 1));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Up,
                count: 1,
            },
        );
        let v = ctrl.view(&term).unwrap().unwrap();
        assert_eq!(v.cursor, (1, 0), "snapped off SpacerTail to head cell");
        assert!(v.wide);

        // Word motions treat "a世界" as one word (never splitting on SpacerTail!),
        // then '🦀' (punct), then "é", then "z".
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineStart,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::WordForward,
                count: 1,
            },
        );
        assert_eq!(ctrl.view(&term).unwrap().unwrap().cursor, (6, 0));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::WordForward,
                count: 1,
            },
        );
        assert_eq!(ctrl.view(&term).unwrap().unwrap().cursor, (9, 0));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::WordBackward,
                count: 1,
            },
        );
        assert_eq!(ctrl.view(&term).unwrap().unwrap().cursor, (6, 0));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::WordBackward,
                count: 1,
            },
        );
        assert_eq!(ctrl.view(&term).unwrap().unwrap().cursor, (0, 0));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::WordEnd,
                count: 1,
            },
        );
        let v = ctrl.view(&term).unwrap().unwrap();
        assert_eq!(v.cursor, (3, 0), "end of 'a世界' is '界' head cell");
        assert!(v.wide);
    }

    #[test]
    fn word_vs_big_word_boundaries_line_edges_and_soft_wraps() {
        let mut term = make_terminal(12, 8, 1 << 16);
        // Row 0: "foo.bar baz " (trailing space padding)
        // Row 1: "  /tmp/a-b  "
        // Row 2: "" (empty row)
        // Row 3 (soft-wraps into Row 4): "wrapword_123" + "456 next"
        // Row 5 (hard newline from Row 4): "hard\r\n"
        // Row 6: "end 界"
        term.vt_write(
            "foo.bar baz \r\n  /tmp/a-b  \r\n\r\nwrapword_123456 next\r\nhard\r\nend 界".as_bytes(),
        );
        let mut ctrl = Controller::default();
        step(&mut ctrl, &mut term, 1, CopyModeOp::Begin);
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Top,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineStart,
                count: 1,
            },
        );

        // Row 0: "foo.bar baz " -> w stops at '.', 'b' (bar), 'b' (baz)
        let mut stops = vec![cursor_screen(&ctrl)];
        for _ in 0..3 {
            step(
                &mut ctrl,
                &mut term,
                1,
                CopyModeOp::Move {
                    motion: CopyMotion::WordForward,
                    count: 1,
                },
            );
            stops.push(cursor_screen(&ctrl));
        }
        assert_eq!(stops, [(0, 0), (3, 0), (4, 0), (8, 0)]);

        // W jumps whole WORD "foo.bar" straight to "baz".
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineStart,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::BigWordForward,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (8, 0));

        // E from col 0 lands on 'r' (6), then 'z' (10).
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineStart,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::BigWordEnd,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (6, 0));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::BigWordEnd,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (10, 0));

        // B from 'z' (10) lands on 'b' (8), then 'f' (0).
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::BigWordBackward,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (8, 0));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::BigWordBackward,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (0, 0));

        // Row 1: "  /tmp/a-b  " -> ^ lands on '/' (2), $ lands on 'b' (9).
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Down,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::FirstNonBlank,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (2, 1));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineEnd,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (9, 1));

        // w walks '/', 'tmp', '/', 'a', '-', 'b' on row 1.
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::FirstNonBlank,
                count: 1,
            },
        );
        let mut path_stops = vec![cursor_screen(&ctrl).0];
        for _ in 0..5 {
            step(
                &mut ctrl,
                &mut term,
                1,
                CopyModeOp::Move {
                    motion: CopyMotion::WordForward,
                    count: 1,
                },
            );
            path_stops.push(cursor_screen(&ctrl).0);
        }
        assert_eq!(path_stops, [2, 3, 6, 7, 8, 9]);

        // Empty row (row 2): ^ and $ both land on col 0.
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Down,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::FirstNonBlank,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (0, 2));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineEnd,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (0, 2));

        // Soft-wrap continuation (row 3 -> row 4: "wrapword_123" + "456 next"):
        // e from (0, 3) flows across the soft wrap to '6' at (2, 4).
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::WordForward,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (0, 3));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::WordEnd,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (2, 4), "flows across soft-wrap");
        // b from '6' at (2, 4) flows back across the soft wrap to 'w' at (0, 3).
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::WordBackward,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (0, 3));
        // w from 'w' at (0, 3) skips the whole wrapped word and lands on 'n' of "next" at (4, 4).
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::WordForward,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (4, 4));

        // Hard newline between row 4 ("next") and row 5 ("hard") breaks the word!
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::WordForward,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (0, 5));

        // Row 6 ("end 界"): $ lands on wide head cell col 4 (not SpacerTail col 5).
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Down,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineEnd,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&ctrl), (4, 6));
        assert!(ctrl.view(&term).unwrap().unwrap().wide);
    }

    #[test]
    fn char_line_block_selections_swap_toggle_switch_and_yank() {
        let mut term = make_terminal(12, 5, 1 << 16);
        // Create scrollback + soft-wrap + hard-newlines + wide chars.
        // Rows 0..10: "r00 abcdef" .. "r09 abcdef"
        // Row 10+11 (soft-wrapped 18 chars): "0123456789ABCDEFGH"
        // Row 12: "xy世界pq"
        // Row 13: "mn世界rs"
        for i in 0..10 {
            term.vt_write(format!("r{i:02} abcdef\r\n").as_bytes());
        }
        term.vt_write("0123456789ABCDEFGH\r\nxy世界pq\r\nmn世界rs".as_bytes());

        let mut ctrl = Controller::default();
        let mut renderer = Renderer::new().unwrap();
        step(&mut ctrl, &mut term, 1, CopyModeOp::Begin);

        // 1. Char selection across multiple screens of scrollback (row 1 -> row 11 soft-wrapped).
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineNumber(2), // row 1 ("r01 abcdef")
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineStart,
                count: 1,
            },
        );
        // SwapEnds without selection is a no-op.
        step(&mut ctrl, &mut term, 1, CopyModeOp::SwapEnds);
        assert_eq!(cursor_screen(&ctrl), (0, 1));

        step(&mut ctrl, &mut term, 1, CopyModeOp::Select(CopyUnit::Char));
        assert_eq!(
            ctrl.view(&term).unwrap().unwrap().unit,
            Some(CopyUnit::Char)
        );

        // Switch unit Char -> Line preserves anchor at (0, 1).
        step(&mut ctrl, &mut term, 1, CopyModeOp::Select(CopyUnit::Line));
        assert_eq!(
            ctrl.view(&term).unwrap().unwrap().unit,
            Some(CopyUnit::Line)
        );

        // Toggle same unit Line -> clears selection.
        step(&mut ctrl, &mut term, 1, CopyModeOp::Select(CopyUnit::Line));
        assert_eq!(ctrl.view(&term).unwrap().unwrap().unit, None);
        assert!(renderer.frame(&term).unwrap().selection_text.is_none());

        // Select Char across soft-wrapped rows 10..11 and yank: must unwrap soft wrap!
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineNumber(11), // row 10 ("0123456789AB")
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineStart,
                count: 1,
            },
        );
        step(&mut ctrl, &mut term, 1, CopyModeOp::Select(CopyUnit::Char));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Down,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineEnd,
                count: 1,
            },
        );
        // SwapEnds (`o`) jumps cursor back to anchor (0, 10) and keeps selection intact.
        step(&mut ctrl, &mut term, 1, CopyModeOp::SwapEnds);
        assert_eq!(cursor_screen(&ctrl), (0, 10));
        let out = ctrl
            .apply(&mut term, None, cmd(1, 7, None, CopyModeOp::Yank))
            .unwrap();
        let yank = out.yank.unwrap();
        assert_eq!(yank.epoch, 1);
        assert_eq!(yank.revision, 7);
        assert_eq!(yank.text, "0123456789ABCDEFGH");
        assert_eq!(ctrl.view(&term).unwrap().unwrap().unit, None);

        // 2. Line selection across hard-newlined scrollback rows 0..=2.
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Top,
                count: 1,
            },
        );
        step(&mut ctrl, &mut term, 1, CopyModeOp::Select(CopyUnit::Line));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Down,
                count: 2,
            },
        );
        let out = ctrl
            .apply(&mut term, None, cmd(1, 8, None, CopyModeOp::Yank))
            .unwrap();
        assert_eq!(out.yank.unwrap().text, "r00 abcdef\nr01 abcdef\nr02 abcdef");

        // 3. Block (rectangle) selection with wide chars at block edges!
        // Row 12: "xy世界pq" (cols: 0='x', 1='y', 2..3='世', 4..5='界', 6='p', 7='q')
        // Row 13: "mn世界rs"
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineNumber(13), // row 12
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineStart,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Right,
                count: 2, // col 2 = '世'
            },
        );
        step(&mut ctrl, &mut term, 1, CopyModeOp::Select(CopyUnit::Block));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Down,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Right,
                count: 1, // col 4 = '界' (wide head; tail is col 5)
            },
        );
        let frame = renderer.frame(&term).unwrap();
        let selected_rows: Vec<_> = frame.selection_rows.iter().filter_map(|r| *r).collect();
        assert_eq!(
            selected_rows,
            vec![(2, 5), (2, 5)],
            "rectangle selection highlights exact per-row column band including wide tail"
        );
        let out = ctrl
            .apply(&mut term, None, cmd(1, 9, None, CopyModeOp::Yank))
            .unwrap();
        assert_eq!(out.yank.unwrap().text, "世界\n世界");

        // 4. YankLine (`yy` and `Nyy`).
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineNumber(1), // row 0
                count: 1,
            },
        );
        let yy = ctrl
            .apply(
                &mut term,
                None,
                cmd(1, 10, None, CopyModeOp::YankLine { count: 1 }),
            )
            .unwrap()
            .yank
            .unwrap();
        assert_eq!(yy.text, "r00 abcdef");
        let two_yy = ctrl
            .apply(
                &mut term,
                None,
                cmd(1, 11, None, CopyModeOp::YankLine { count: 2 }),
            )
            .unwrap()
            .yank
            .unwrap();
        assert_eq!(two_yy.text, "r00 abcdef\nr01 abcdef");
    }

    #[test]
    fn incoming_pty_output_and_resize_reflow_keep_cursor_and_selection_pinned() {
        let mut term = make_terminal(20, 6, 1 << 16);
        for i in 0..12 {
            term.vt_write(format!("item-{i:02} alpha beta\r\n").as_bytes());
        }
        let mut ctrl = Controller::default();
        step(&mut ctrl, &mut term, 1, CopyModeOp::Begin);
        // Navigate to "item-03 alpha beta" (row 3), select "alpha".
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineNumber(4),
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineStart,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::BigWordForward,
                count: 1, // 'a' of "alpha"
            },
        );
        step(&mut ctrl, &mut term, 1, CopyModeOp::Select(CopyUnit::Char));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::WordEnd,
                count: 1, // 'a' at end of "alpha"
            },
        );
        let mut renderer = Renderer::new().unwrap();
        assert_eq!(
            renderer.frame(&term).unwrap().selection_text.as_deref(),
            Some("alpha")
        );

        // Stream 20 new lines of PTY output while copy mode is active.
        for i in 12..32 {
            term.vt_write(format!("item-{i:02} gamma delta\r\n").as_bytes());
        }
        ctrl.sync(&mut term).unwrap();
        assert!(ctrl.is_active());
        assert!(
            ctrl.view(&term).unwrap().is_some(),
            "cursor remains visible"
        );
        assert_eq!(
            renderer.frame(&term).unwrap().selection_text.as_deref(),
            Some("alpha"),
            "selection stays pinned to exact text across PTY output"
        );

        // Resize narrower to trigger reflow, then wider again.
        term.resize(12, 6, 0, 0).unwrap();
        ctrl.sync(&mut term).unwrap();
        assert!(ctrl.is_active());
        assert!(ctrl.view(&term).unwrap().is_some());
        assert_eq!(
            renderer.frame(&term).unwrap().selection_text.as_deref(),
            Some("alpha"),
            "selection stays pinned across resize/reflow"
        );
        term.resize(24, 6, 0, 0).unwrap();
        ctrl.sync(&mut term).unwrap();
        assert_eq!(
            renderer.frame(&term).unwrap().selection_text.as_deref(),
            Some("alpha")
        );
    }

    #[test]
    fn search_to_match_jumps_to_scrollback_and_extends_selection() {
        let mut term = make_terminal(20, 4, 1 << 16);
        term.vt_write(b"first target here\r\n");
        for i in 0..10 {
            term.vt_write(format!("filler {i}\r\n").as_bytes());
        }
        term.vt_write(b"last target here");

        let mut search = search::Controller::default();
        search.set_needle(Some(&term), "target".into()).unwrap();
        finish_search(&mut search, &term);

        let mut ctrl = Controller::default();
        ctrl.apply(&mut term, Some(&search), cmd(1, 1, None, CopyModeOp::Begin))
            .unwrap();
        // Jump to newest match ("last target here", col 5).
        ctrl.apply(
            &mut term,
            Some(&search),
            cmd(1, 2, None, CopyModeOp::ToMatch),
        )
        .unwrap();
        let newest_y = cursor_screen(&ctrl).1;
        assert_eq!(cursor_screen(&ctrl).0, 5);

        // Start Char selection, step search to older match in scrollback, and follow.
        ctrl.apply(
            &mut term,
            Some(&search),
            cmd(1, 3, None, CopyModeOp::Select(CopyUnit::Char)),
        )
        .unwrap();
        search.step(&term, true).unwrap();
        ctrl.move_to_match(&mut term, &search).unwrap();
        // "first target here" -> 't' is at col 6 on row 0.
        assert_eq!(cursor_screen(&ctrl), (6, 0));
        assert!(newest_y > 5);
        assert!(ctrl.view(&term).unwrap().is_some());

        let yank = ctrl
            .apply(&mut term, Some(&search), cmd(1, 4, None, CopyModeOp::Yank))
            .unwrap()
            .yank
            .unwrap();
        assert!(yank.text.starts_with("target here\nfiller 0"));
        assert!(yank.text.ends_with("last t"));
    }

    #[test]
    fn block_orientations_scroll_op_clear_spacer_head_and_ris_reset() {
        // 1. All 4 diagonal Block orientations + wide char on left edge + ClearSelection + Yank without selection.
        let mut term = make_terminal(12, 5, 1 << 16);
        // Row 0: "ab世界ef"  (0='a',1='b',2..3='世',4..5='界',6='e',7='f')
        // Row 1: "gh世界kl"
        term.vt_write("ab世界ef\r\ngh世界kl".as_bytes());
        let mut ctrl = Controller::default();
        step(&mut ctrl, &mut term, 1, CopyModeOp::Begin);

        // Yank without an active selection returns accepted=true, yank=None.
        let empty_yank = ctrl
            .apply(&mut term, None, cmd(1, 1, None, CopyModeOp::Yank))
            .unwrap();
        assert!(empty_yank.accepted);
        assert!(empty_yank.yank.is_none());

        // MirroredForward: anchor top-right (6, 0) -> cursor bottom-left (2, 1) ('世').
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Top,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineStart,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Right,
                count: 4, // 'a'(0)->'b'(1)->'世'(2)->'界'(4)->'e'(6)
            },
        );
        assert_eq!(cursor_screen(&ctrl), (6, 0));
        step(&mut ctrl, &mut term, 1, CopyModeOp::Select(CopyUnit::Block));
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Down,
                count: 1,
            },
        );
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Left,
                count: 2, // col 6 -> col 4 ('界') -> col 2 ('世')
            },
        );
        assert_eq!(cursor_screen(&ctrl), (2, 1));
        let mirrored = ctrl
            .apply(&mut term, None, cmd(1, 2, None, CopyModeOp::Yank))
            .unwrap()
            .yank
            .unwrap();
        assert_eq!(mirrored.text, "世界e\n世界k");

        // ClearSelection clears active selection while staying in copy mode.
        step(&mut ctrl, &mut term, 1, CopyModeOp::Select(CopyUnit::Char));
        assert_eq!(
            ctrl.view(&term).unwrap().unwrap().unit,
            Some(CopyUnit::Char)
        );
        step(&mut ctrl, &mut term, 1, CopyModeOp::ClearSelection);
        assert!(ctrl.is_active());
        assert_eq!(ctrl.view(&term).unwrap().unwrap().unit, None);

        // 2. Wide char soft-wrap with SpacerHead at end of row 0:
        //    cols=5, "abcd世界" -> row 0 has "abcd" + SpacerHead(4), row 1 has "世界".
        let mut wrap_term = make_terminal(5, 3, 1 << 16);
        wrap_term.vt_write("abcd世界 xy".as_bytes());
        let mut wrap_ctrl = Controller::default();
        step(&mut wrap_ctrl, &mut wrap_term, 1, CopyModeOp::Begin);
        step(
            &mut wrap_ctrl,
            &mut wrap_term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Top,
                count: 1,
            },
        );
        step(
            &mut wrap_ctrl,
            &mut wrap_term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::LineStart,
                count: 1,
            },
        );
        // WordEnd flows seamlessly across SpacerHead from 'a' (0,0) to '界' (2,1).
        step(
            &mut wrap_ctrl,
            &mut wrap_term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::WordEnd,
                count: 1,
            },
        );
        assert_eq!(cursor_screen(&wrap_ctrl), (2, 1));
        assert!(wrap_ctrl.view(&wrap_term).unwrap().unwrap().wide);
        // YankLine { count: 1 } (`yy`) expands across all 3 wrapped rows of the logical line.
        step(
            &mut wrap_ctrl,
            &mut wrap_term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Top,
                count: 1,
            },
        );
        let unwrapped = wrap_ctrl
            .apply(
                &mut wrap_term,
                None,
                cmd(1, 3, None, CopyModeOp::YankLine { count: 1 }),
            )
            .unwrap()
            .yank
            .unwrap();
        assert_eq!(unwrapped.text, "abcd世界 xy");

        // 3. Scroll(delta) clamps cursor into viewport, and Begin while scrolled up starts at (0, 0).
        let mut scroll_term = make_terminal(16, 4, 1 << 16);
        for i in 0..20 {
            scroll_term.vt_write(format!("s-{i:02}\r\n").as_bytes());
        }
        scroll_term.scroll_viewport(ScrollViewport::Top);
        let mut scroll_ctrl = Controller::default();
        step(&mut scroll_ctrl, &mut scroll_term, 1, CopyModeOp::Begin);
        // Program cursor is at bottom (off-screen), so Begin starts at viewport (0, 0) = screen (0, 0).
        assert_eq!(cursor_screen(&scroll_ctrl), (0, 0));
        // Scroll down by 10 rows -> cursor clamps into new visible viewport.
        step(
            &mut scroll_ctrl,
            &mut scroll_term,
            1,
            CopyModeOp::Scroll(10),
        );
        let sb_top = scroll_term.scrollbar().unwrap().offset as u32;
        assert_eq!(cursor_screen(&scroll_ctrl).1, sb_top);
        assert!(scroll_ctrl.view(&scroll_term).unwrap().is_some());

        // 4. count=0 and visible_rows=Some(0) clamp safely without panic or overflow.
        scroll_ctrl
            .apply(
                &mut scroll_term,
                None,
                cmd(
                    1,
                    4,
                    Some(0),
                    CopyModeOp::Move {
                        motion: CopyMotion::Down,
                        count: 0,
                    },
                ),
            )
            .unwrap();
        let zero_yy = scroll_ctrl
            .apply(
                &mut scroll_term,
                None,
                cmd(1, 5, Some(0), CopyModeOp::YankLine { count: 0 }),
            )
            .unwrap()
            .yank
            .unwrap();
        assert!(!zero_yy.text.is_empty());

        // 5. Terminal RIS (`\x1bc`) / `terminal.reset()` ends copy mode cleanly or stays valid without panic.
        let mut ris_term = make_terminal(16, 4, 1 << 16);
        for i in 0..12 {
            ris_term.vt_write(format!("pre-{i:02}\r\n").as_bytes());
        }
        let mut ris_ctrl = Controller::default();
        step(&mut ris_ctrl, &mut ris_term, 7, CopyModeOp::Begin);
        step(
            &mut ris_ctrl,
            &mut ris_term,
            7,
            CopyModeOp::Move {
                motion: CopyMotion::Top,
                count: 1,
            },
        );
        // Cursor is pinned in scrollback (row 0); RIS clears scrollback and invalidates the ref.
        ris_term.vt_write(b"\x1bc");
        ris_ctrl.sync(&mut ris_term).unwrap();
        assert!(
            !ris_ctrl.is_active(),
            "RIS clearing scrollback exits copy mode"
        );
        assert_eq!(ris_ctrl.ended_epoch(), 7);
    }

    #[test]
    fn count_clamping_stale_epochs_screen_switch_eviction_and_reset_are_safe() {
        let mut term = make_terminal(20, 4, 1 << 16);
        for i in 0..8 {
            term.vt_write(format!("row {i}\r\n").as_bytes());
        }
        let mut ctrl = Controller::default();

        // Non-Begin while inactive is ignored.
        let ignored = step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Up,
                count: 1,
            },
        );
        assert!(!ignored.accepted);
        assert!(!ctrl.is_active());

        // Huge count clamps safely without overflow.
        step(&mut ctrl, &mut term, 1, CopyModeOp::Begin);
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Up,
                count: 99_999_999,
            },
        );
        assert_eq!(cursor_screen(&ctrl).1, 0);
        step(
            &mut ctrl,
            &mut term,
            1,
            CopyModeOp::Move {
                motion: CopyMotion::Down,
                count: 99_999_999,
            },
        );
        assert_eq!(
            cursor_screen(&ctrl).1,
            (term.total_rows().unwrap() - 1) as u32
        );

        // Stale epoch is ignored without moving cursor.
        let before = cursor_screen(&ctrl);
        let stale = step(
            &mut ctrl,
            &mut term,
            99,
            CopyModeOp::Move {
                motion: CopyMotion::Top,
                count: 1,
            },
        );
        assert!(!stale.accepted);
        assert_eq!(cursor_screen(&ctrl), before);

        // Entering copy mode on alternate screen works (viewport-only), and switching
        // back to primary auto-ends with ended_epoch advanced.
        step(&mut ctrl, &mut term, 1, CopyModeOp::End);
        assert_eq!(ctrl.ended_epoch(), 0, "explicit End does not set auto-end");
        term.vt_write(b"\x1b[?1049h\x1b[Halt-row-0\r\nalt-row-1");
        step(&mut ctrl, &mut term, 2, CopyModeOp::Begin);
        assert!(ctrl.is_active());
        step(
            &mut ctrl,
            &mut term,
            2,
            CopyModeOp::Move {
                motion: CopyMotion::Top,
                count: 1,
            },
        );
        let alt_yank = ctrl
            .apply(
                &mut term,
                None,
                cmd(2, 1, None, CopyModeOp::YankLine { count: 1 }),
            )
            .unwrap()
            .yank
            .unwrap();
        assert_eq!(alt_yank.text, "alt-row-0");

        // Switch alternate -> primary triggers clean auto-exit.
        term.vt_write(b"\x1b[?1049l");
        ctrl.sync(&mut term).unwrap();
        assert!(!ctrl.is_active());
        assert_eq!(ctrl.ended_epoch(), 2);

        // Switch primary -> alternate triggers clean auto-exit too.
        step(&mut ctrl, &mut term, 3, CopyModeOp::Begin);
        assert!(ctrl.is_active());
        term.vt_write(b"\x1b[?1049h");
        ctrl.sync(&mut term).unwrap();
        assert!(!ctrl.is_active());
        assert_eq!(ctrl.ended_epoch(), 3);
        term.vt_write(b"\x1b[?1049l");

        // Scrollback eviction/pruning invalidates tracked ref and exits cleanly without panic.
        let mut tiny = make_terminal(20, 4, 1024);
        tiny.set_scrollback_max_lines(Some(10)).unwrap();
        for i in 0..30 {
            tiny.vt_write(format!("evict-{i:04}\r\n").as_bytes());
        }
        let mut evict_ctrl = Controller::default();
        step(&mut evict_ctrl, &mut tiny, 4, CopyModeOp::Begin);
        step(
            &mut evict_ctrl,
            &mut tiny,
            4,
            CopyModeOp::Move {
                motion: CopyMotion::Top,
                count: 1,
            },
        );
        // Disable scrollback (max = 0) to prune history pages immediately.
        tiny.set_scrollback_max_bytes(Some(0)).unwrap();
        evict_ctrl.sync(&mut tiny).unwrap();
        assert!(!evict_ctrl.is_active());
        assert_eq!(evict_ctrl.ended_epoch(), 4);

        // Fresh snapshot / reconnect reset exits cleanly and advances ended_epoch.
        step(&mut ctrl, &mut term, 5, CopyModeOp::Begin);
        assert_eq!(ctrl.reset(), Some(5));
        assert!(!ctrl.is_active());
        assert_eq!(ctrl.ended_epoch(), 5);

        // Stale Begin with epoch == 0 or epoch <= ended_epoch is ignored.
        assert!(!step(&mut ctrl, &mut term, 0, CopyModeOp::Begin).accepted);
        assert!(!step(&mut ctrl, &mut term, 5, CopyModeOp::Begin).accepted);
        assert!(!ctrl.is_active());
    }
}
