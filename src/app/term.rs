//! One terminal pane: its stream, local view state (selection, preedit, retained-frame offset)
//! and the cell grid, drawn on a fixed column grid.
use super::{
    keys,
    sprites::Sprite,
    theme::{
        MIDNIGHT, Palette, SEARCH_CURRENT, SEARCH_MATCH, SEARCH_OUTLINE, SEARCH_RADIUS, mix, rgb,
    },
};
use ghostty_vt::style::Underline;
use rex_rune::{
    connection::{Command, Connection},
    links::Link,
    settings::{
        terminal::{SelectionColor, TerminalConfig},
        typography::Typography,
    },
    terminal::{
        Cell, CopyModeOp, CopyYank, CursorShape, Frame, SelectionUnit,
        graphics::{Image as TermImage, ImagePixels},
    },
    terminal_theme::{TerminalTheme, is_light},
};
use rune::{
    El, LineCap, Mods, Path, Shape, Span, TextEdit, TextStyle, canvas, el, memo, rich, text,
};
use serde_json::json;
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
    time::{Duration, Instant},
};

pub mod preview;
#[cfg(test)]
mod selection_tests;
#[cfg(test)]
mod typography_tests;

/// The radius of the selection's outer corners, when they are rounded.
const SELECTION_RADIUS: f32 = 3.;

pub struct Term {
    pub connection: Connection,
    pub frame: Option<Arc<Frame>>,
    pub last_size: Option<(u16, u16)>,
    /// The grid's rect in window points (inside the padding).
    pub rect: [f32; 4],
    /// While this client owns a live pane, its frames stay hidden until one arrives at this
    /// grid's size.
    initial_size: InitialSize,
    pub selection: Option<(usize, usize)>,
    selection_anchor: Option<(usize, usize)>,
    selection_unit: SelectionUnit,
    pub selecting: bool,
    pub(super) selection_revision: u64,
    last_selection_input: Option<rex_rune::terminal::selection::Input>,
    copy_wanted: Option<u64>,
    cursor_style: CursorShape,
    /// Local middle click origin; movement cancels primary paste.
    pub primary_press: Option<(f32, f32)>,
    restore_generation: u64,
    pub preedit: String,
    pub notice: Option<String>,
    pub scroll: f32,
    /// A button press forwarded to the program, until its release: the gesture keeps going to
    /// the program even if the program stops tracking or Shift goes down mid-drag.
    pub mouse: Option<MouseGesture>,
    /// The last cell a motion report named, so moves within a cell send nothing.
    pub mouse_cell: Option<(u16, u16)>,
    images: HashMap<usize, (Arc<ImagePixels>, rune::Image)>,
    /// The link under the pointer while Ctrl is held, with the cell the pointer is on.
    pub link: Option<(usize, Link)>,
    /// The link a Ctrl+press went down on; releasing over the same link opens it.
    pub pressed_link: Option<String>,
    /// The search field over the grid, while this terminal is being searched.
    pub search: Option<TextEdit>,
    /// The last text searched here and the corner the find bar is in.
    pub find: super::find::Memory,
    /// Monotonically increasing copy-mode epoch allocated on this pane.
    copy_epoch: u64,
    /// Active copy-mode epoch on this pane, if any (for defense-in-depth guards).
    copy_mode_active: Option<u64>,
    /// Highest copy-mode epoch ended automatically by the pane worker.
    copy_ended_epoch: u64,
    /// Pending yank taken from `Model::copy_yank`, ready for one-shot clipboard delivery.
    pending_copy_yank: Option<CopyYank>,
    /// Last delivered `(epoch, revision)` so each yank is copied at most once.
    last_copy_yank: Option<(u64, u64)>,
    /// Defense-in-depth: while a `rex.ui` prompt modal is open, block all PTY input commands.
    pub prompt_open: bool,
}

#[derive(Clone, Copy)]
pub struct MouseGesture {
    pub button: &'static str,
}

/// The first frame of a pane is at whatever size the server last had for it; the resize this
/// client sends only lands a moment later. During each uninterrupted live ownership epoch, an
/// owner hides frames until one matches its first grid size. A non-owner never resizes, so its
/// frames may never match and are drawn as they come.
/// A frame at the asked size may never come (the server clamps or drops a resize), so the
/// wait is bounded: after this long the frame is drawn as it is, which is at worst the brief
/// flash of a stale size the gate exists to avoid.
const INITIAL_SIZE_PATIENCE: Duration = Duration::from_millis(500);

#[derive(Default)]
struct InitialSize {
    expected: Option<(u16, u16)>,
    ready: bool,
    live_owner: bool,
    /// When the wait began, for the bound above.
    since: Option<Instant>,
}

impl InitialSize {
    fn transition(&mut self, status: &Status) {
        let live_owner = status.live && status.owner;
        if live_owner != self.live_owner {
            self.expected = None;
            self.ready = false;
            self.since = None;
            self.live_owner = live_owner;
        }
    }

    fn layout(&mut self, size: (u16, u16), frame: Option<(u16, u16)>) {
        if !self.live_owner || self.ready {
            return;
        }
        self.expected = Some(size);
        self.since.get_or_insert_with(Instant::now);
        self.observe(frame);
    }

    fn observe(&mut self, frame: Option<(u16, u16)>) {
        if self.expected.is_some() && self.expected == frame {
            self.expected = None;
            self.since = None;
            self.ready = true;
        }
    }

    fn renders(&self, status: &Status) -> bool {
        self.renders_at(status, Instant::now())
    }

    fn renders_at(&self, status: &Status, now: Instant) -> bool {
        !status.live
            || !status.owner
            || (self.live_owner && self.ready)
            || self.deadline().is_some_and(|deadline| now >= deadline)
    }

    /// When the wait gives up, while one is on.
    fn deadline(&self) -> Option<Instant> {
        self.since
            .filter(|_| self.live_owner && !self.ready)
            .map(|since| since + INITIAL_SIZE_PATIENCE)
    }

    /// Ends a wait past its bound for good; true when that just happened.
    fn give_up(&mut self, now: Instant) -> bool {
        let due = self.deadline().is_some_and(|deadline| now >= deadline);
        if due {
            self.expected = None;
            self.since = None;
            self.ready = true;
        }
        due
    }
}

#[derive(Clone, Copy, PartialEq)]
pub struct Metrics {
    pub cell: (f32, f32),
    pub style: TextStyle,
    /// Device pixels per point the grid is drawn at, so box drawing lands on whole pixels:
    /// 1 from `metrics`, the window's own in the pane view.
    pub px: f32,
    /// Whether the bold and the italic attributes programs set are drawn (Settings →
    /// Terminal → Advanced font settings → Render bold and italic).
    pub bold: bool,
    pub italic: bool,
    /// How far right of its cell's edge a glyph starts: half the character spacing, so the
    /// space added to a cell is shared between its two sides.
    pub inset: f32,
    /// How far the text's own line box is in from the top and the bottom of its row: half
    /// of what the line height setting adds, in whole points. Underlines and overlines stay
    /// with the text by it.
    pub leading: f32,
}

const FEATURES: &str = "'liga' 0, 'calt' 0, 'clig' 0, 'kern' 0";

/// The cell and text style at `size` with the typography settings at their defaults.
#[cfg(test)]
pub fn metrics(size: f32) -> Metrics {
    metrics_for(size, &Typography::default())
}

/// The cell and text style at `size` under `font`. The cell is the primary face's advance
/// plus the character spacing, by the row height the size gives scaled by the line height
/// setting; sprites and decorations are drawn from the cell, so they follow both.
pub fn metrics_for(size: f32, font: &Typography) -> Metrics {
    let natural = (size * 1.4).ceil();
    let height = (natural * font.line_scale()).ceil().max(1.);
    let mut base = TextStyle::new(size, if font.style.bold() { 700. } else { 400. })
        .lh(height)
        .features(FEATURES);
    if font.style.italic() {
        base = base.italic();
    }
    // The mono slot keeps the primary face first and supplies missing glyphs from fallbacks.
    let style = base.mono();
    let advance = rune::text::advance("M", style).max(1.);
    // Never so tight that a cell vanishes.
    let spacing = font.spacing_points(advance).max(1. - advance);
    Metrics {
        cell: (advance + spacing, height),
        style: if spacing == 0. {
            style
        } else {
            style.tracking(spacing)
        },
        px: 1.,
        bold: font.bold_italic.bold(),
        italic: font.bold_italic.italic(),
        inset: spacing / 2.,
        leading: ((height - natural) / 2.).floor(),
    }
}

pub struct Status {
    pub live: bool,
    pub owner: bool,
    pub status: String,
}

impl Term {
    pub fn new(connection: Connection) -> Self {
        let (frame, restore_generation, copy_ended_epoch) = {
            let model = connection.model.lock().expect("terminal model");
            (
                model.frame.clone(),
                model.restore_generation,
                model.copy_ended_epoch,
            )
        };
        Self {
            connection,
            frame,
            last_size: None,
            rect: [0.; 4],
            initial_size: InitialSize::default(),
            selection: None,
            selection_anchor: None,
            selection_unit: SelectionUnit::Cell,
            selecting: false,
            selection_revision: 0,
            last_selection_input: None,
            copy_wanted: None,
            cursor_style: CursorShape::Block,
            primary_press: None,
            restore_generation,
            preedit: String::new(),
            notice: None,
            scroll: 0.,
            mouse: None,
            mouse_cell: None,
            images: HashMap::new(),
            link: None,
            pressed_link: None,
            search: None,
            find: Default::default(),
            copy_epoch: 0,
            copy_mode_active: None,
            copy_ended_epoch,
            pending_copy_yank: None,
            last_copy_yank: None,
            prompt_open: false,
        }
    }

    pub fn refresh(&mut self) {
        let (frame, generation, notice, copy_yank, copy_ended_epoch) = {
            let mut model = self.connection.model.lock().expect("terminal model");
            (
                model.frame.clone(),
                model.restore_generation,
                model.notice.take(),
                model.copy_yank.take(),
                model.copy_ended_epoch,
            )
        };
        if notice.is_some() {
            self.notice = notice;
        }
        self.copy_ended_epoch = self.copy_ended_epoch.max(copy_ended_epoch);
        if generation != self.restore_generation {
            self.selection = None;
            self.selection_anchor = None;
            self.selecting = false;
            self.last_selection_input = None;
            self.copy_wanted = None;
            self.copy_mode_active = None;
            self.pending_copy_yank = None;
            self.restore_generation = generation;
        }
        if self
            .copy_mode_active
            .is_some_and(|epoch| self.copy_ended_epoch >= epoch)
        {
            self.copy_mode_active = None;
        }
        if let Some(yank) = copy_yank
            && Some((yank.epoch, yank.revision)) != self.last_copy_yank
        {
            self.pending_copy_yank = Some(yank);
        }
        if let Some(frame) = &frame {
            if frame.selection_error != self.frame.as_ref().and_then(|f| f.selection_error.clone())
            {
                self.notice = frame.selection_error.clone();
            }
            if frame.copy.is_some() {
                // Copy mode owns Ghostty's selection; never fall back to pointer (0, 0)
                // selection or let visual cursor motions trigger copy_on_selection.
                self.selection = None;
                self.selection_anchor = None;
                self.selecting = false;
                self.copy_wanted = None;
            } else if frame.selection_revision == self.selection_revision {
                if frame.selection_text.is_some() && self.selection.is_none() {
                    self.selection = Some((0, 0));
                } else if !self.selecting && frame.selection_text.is_none() {
                    self.selection = None;
                }
            }
        }
        self.initial_size
            .observe(frame.as_ref().map(|frame| (frame.cols, frame.rows)));
        self.frame = frame;
        // The same cell may hold a different link, or none, after output.
        if let Some((index, _)) = self.link.take() {
            self.link = self
                .frame
                .as_ref()
                .and_then(|f| f.link_at(index))
                .map(|l| (index, l));
        }
    }

    pub fn cache_images(&mut self, cache: Option<&rune::ImageCache>) {
        let (Some(cache), Some(frame)) = (cache, &self.frame) else {
            self.images.clear();
            return;
        };
        // Keeping the pixel Arc beside the texture prevents pointer reuse from making
        // a fresh snapshot look like the previous image with the same protocol id.
        for image in &frame.images {
            let pixels = &image.pixels;
            self.images
                .entry(Arc::as_ptr(pixels) as usize)
                .or_insert_with(|| {
                    (
                        Arc::clone(pixels),
                        cache.rgba(pixels.width, pixels.height, Arc::clone(&pixels.rgba)),
                    )
                });
        }
        self.images.retain(|key, _| {
            frame
                .images
                .iter()
                .any(|image| Arc::as_ptr(&image.pixels) as usize == *key)
        });
    }

    fn display_frame(&self) -> Option<&Arc<Frame>> {
        if self.initial_size.renders(&self.status()) {
            self.frame.as_ref()
        } else {
            None
        }
    }

    /// When this pane stops waiting for a frame at its size and draws anyway, while it waits.
    pub fn draws_anyway_at(&self) -> Option<Instant> {
        self.initial_size.deadline()
    }

    /// Stops waiting for a frame at this pane's size once the bound has passed; true the one
    /// time that changes what it draws.
    pub fn draw_anyway(&mut self, now: Instant) -> bool {
        self.initial_size.give_up(now)
    }

    pub fn status(&self) -> Status {
        let m = self.connection.model.lock().expect("terminal model");
        Status {
            live: m.connected && m.control_connected,
            owner: m.owner,
            status: m.status.clone(),
        }
    }

    pub fn send(&mut self, command: Command) {
        self.try_send(command);
    }

    /// The same send and notice, with acceptance feedback for local path history.
    /// Defense-in-depth: blocks PTY input while a `rex.ui` prompt modal is open, and exits
    /// copy mode before any PTY input command reaches the worker.
    pub fn try_send(&mut self, command: Command) -> bool {
        let is_any_pty_input = matches!(
            &command,
            Command::Key(_)
                | Command::KeyWithPrefix { .. }
                | Command::Paste(_)
                | Command::SendKey(_)
                | Command::Mouse(_)
                | Command::Wheel(_)
                | Command::Reset
                | Command::Clear
        );
        if self.prompt_open && is_any_pty_input {
            return false;
        }
        let is_pty_input = match &command {
            Command::Key(payload) => {
                payload.get("action").and_then(|v| v.as_str()) != Some("release")
            }
            Command::KeyWithPrefix { .. }
            | Command::Paste(_)
            | Command::SendKey(_)
            | Command::Mouse(_)
            | Command::Wheel(_)
            | Command::Reset
            | Command::Clear => true,
            _ => false,
        };
        if is_pty_input && let Some(epoch) = self.copy_mode_active.take() {
            self.selection_revision = self.selection_revision.wrapping_add(1);
            let _ = self
                .connection
                .commands
                .try_send(Command::CopyMode(super::copy_mode::input(
                    self.restore_generation,
                    epoch,
                    self.selection_revision,
                    None,
                    CopyModeOp::End,
                )));
        }
        if let Err(error) = self.connection.commands.try_send(command) {
            self.notice = Some(format!("Input not sent: {error}"));
            return false;
        }
        true
    }

    pub fn restore_generation(&self) -> u64 {
        self.restore_generation
    }

    pub fn copy_ended_epoch(&self) -> u64 {
        self.copy_ended_epoch
    }

    pub fn copy_mode_is_active(&self, epoch: u64) -> bool {
        self.copy_mode_active == Some(epoch)
    }

    pub fn clear_copy_mode_active(&mut self) {
        self.copy_mode_active = None;
    }

    pub fn copy_visible_rows(&self, cell: (f32, f32)) -> Option<u16> {
        if self.rect[3] <= 0. {
            return None;
        }
        Some(self.visible_rows(cell).min(usize::from(u16::MAX)) as u16)
    }

    /// Allocate a monotonically increasing non-zero copy-mode epoch strictly greater than
    /// `model.copy_ended_epoch`, clear local pointer selection, and send `CopyModeOp::Begin`.
    pub fn begin_copy_mode(&mut self, cell: (f32, f32)) -> (u64, u64) {
        {
            let model = self.connection.model.lock().expect("terminal model");
            self.restore_generation = model.restore_generation;
            self.copy_ended_epoch = self.copy_ended_epoch.max(model.copy_ended_epoch);
        }
        let epoch = self
            .copy_epoch
            .max(self.copy_ended_epoch)
            .saturating_add(1)
            .max(1);
        self.copy_epoch = epoch;
        self.copy_mode_active = Some(epoch);
        self.selection = None;
        self.selection_anchor = None;
        self.selecting = false;
        self.copy_wanted = None;
        self.last_selection_input = None;
        self.primary_press = None;
        self.mouse = None;
        self.send_copy_mode(self.restore_generation, epoch, cell, CopyModeOp::Begin);
        (self.restore_generation, epoch)
    }

    /// Send one stamped copy-mode operation to the pane worker.
    pub fn send_copy_mode(
        &mut self,
        generation: u64,
        epoch: u64,
        cell: (f32, f32),
        op: CopyModeOp,
    ) {
        self.selection_revision = self.selection_revision.wrapping_add(1);
        self.copy_wanted = None;
        let visible_rows = self.copy_visible_rows(cell);
        self.send(Command::CopyMode(super::copy_mode::input(
            generation,
            epoch,
            self.selection_revision,
            visible_rows,
            op,
        )));
    }

    /// Take the latest pending copy-mode yank once, deduplicated by `(epoch, revision)`.
    pub fn take_copy_yank(&mut self) -> Option<CopyYank> {
        let yank = self.pending_copy_yank.take()?;
        if yank.epoch != self.copy_epoch || Some((yank.epoch, yank.revision)) == self.last_copy_yank
        {
            return None;
        }
        self.last_copy_yank = Some((yank.epoch, yank.revision));
        Some(yank)
    }

    pub fn visible_rows(&self, cell: (f32, f32)) -> usize {
        (self.rect[3] / cell.1).floor().max(1.) as usize
    }

    pub fn visible_cols(&self, cell: (f32, f32)) -> usize {
        if self.rect[2] <= 0. {
            return self
                .frame
                .as_ref()
                .map_or(1, |f| usize::from(f.cols).max(1));
        }
        (self.rect[2] / cell.0).floor().max(1.) as usize
    }

    pub fn display_row(&self, cell: (f32, f32)) -> usize {
        self.frame
            .as_ref()
            .map_or(0, |f| f.display_row_start(self.visible_rows(cell)))
    }

    pub fn hidden_rows(&self, cell: (f32, f32)) -> usize {
        if self.rect[3] <= 0. {
            return 0;
        }
        self.frame.as_ref().map_or(0, |f| {
            (f.rows as usize).saturating_sub(self.visible_rows(cell))
        })
    }

    /// A non-owner never resizes; an owner sends its grid size whenever it changes.
    pub fn resize_if_owner(&mut self, cell: (f32, f32)) {
        let status = self.status();
        self.initial_size.transition(&status);
        if self.rect[2] <= 0. || self.rect[3] <= 0. {
            return;
        }
        let cols = (self.rect[2] / cell.0).floor().clamp(1., 65535.) as u16;
        let rows = (self.rect[3] / cell.1).floor().clamp(1., 65535.) as u16;
        self.initial_size.layout(
            (cols, rows),
            self.frame.as_ref().map(|frame| (frame.cols, frame.rows)),
        );
        if !status.owner || !status.live {
            self.last_size = None;
        } else if self.last_size != Some((cols, rows)) {
            self.last_size = Some((cols, rows));
            self.send(Command::Resize(
                cols,
                rows,
                cell.0.ceil() as u16,
                cell.1.ceil() as u16,
            ));
        }
    }

    /// The cell index under a point in the grid's own coordinates.
    pub fn index(&self, x: f32, y: f32, cell: (f32, f32)) -> usize {
        let cols = self.frame.as_ref().map_or(1, |f| f.cols) as usize;
        let rows = self.frame.as_ref().map_or(1, |f| f.rows) as usize;
        let col = (x / cell.0).max(0.) as usize;
        let row = ((y / cell.1).max(0.) as usize).min(self.visible_rows(cell).saturating_sub(1));
        (row + self.display_row(cell)).min(rows - 1) * cols + col.min(cols - 1)
    }

    /// The program has asked for mouse reports.
    pub fn tracking(&self) -> bool {
        self.frame.as_ref().is_some_and(|f| f.mouse_tracking)
    }

    /// The openable link under a point in the grid's own coordinates, with its cell index.
    pub fn link_at(&self, x: f32, y: f32, cell: (f32, f32)) -> Option<(usize, Link)> {
        let index = self.index(x, y, cell);
        self.frame.as_ref()?.link_at(index).map(|l| (index, l))
    }

    /// The server's cell (column, row) under a point in the grid's own coordinates, clamped
    /// to the grid, so a drag past the edge reports the edge cell.
    pub fn cell_at(&self, x: f32, y: f32, cell: (f32, f32)) -> (u16, u16) {
        let cols = self.frame.as_ref().map_or(1, |f| f.cols).max(1);
        let rows = self.frame.as_ref().map_or(1, |f| f.rows).max(1);
        let col = (x / cell.0).floor().max(0.) as usize;
        let row = (y / cell.1).floor().max(0.) as usize + self.display_row(cell);
        (
            col.min(cols as usize - 1) as u16,
            row.min(rows as usize - 1) as u16,
        )
    }

    /// One mouse report. Shift never goes along: it is the local-selection override, and a
    /// Shift press leaves the server's button reporting stuck for the block.
    pub fn send_mouse(&mut self, action: &str, button: Option<&str>, at: (u16, u16), mods: Mods) {
        let mut payload = json!({
            "action": action,
            "x": at.0,
            "y": at.1,
            "mods": keys::mods(Mods { shift: false, ..mods }),
        });
        if let Some(button) = button {
            payload["button"] = json!(button);
        }
        self.mouse_cell = Some(at);
        self.send(Command::Mouse(payload));
    }

    /// A wheel turn of whole lines at a cell: negative scrolls up, positive down.
    pub fn send_wheel(&mut self, at: (u16, u16), lines: i32) {
        let step = lines.signum();
        for _ in 0..lines.unsigned_abs().min(8) {
            self.send(Command::Wheel(
                json!({"x": at.0, "y": at.1, "delta_x": 0, "delta_y": step}),
            ));
        }
    }

    pub fn begin_selection(&mut self, index: usize, clicks: u32) {
        self.selection_unit = match clicks {
            0 | 1 => SelectionUnit::Cell,
            2 => SelectionUnit::Word,
            _ => SelectionUnit::Line,
        };
        self.selection_anchor = self
            .frame
            .as_ref()
            .and_then(|f| f.selection_range(index, self.selection_unit));
        // A focus click alone must not copy the character under the pointer.
        self.selection = (self.selection_unit != SelectionUnit::Cell)
            .then_some(self.selection_anchor)
            .flatten();
    }

    pub fn extend_selection(&mut self, index: usize) {
        let Some(anchor) = self.selection_anchor else {
            return;
        };
        let Some(target) = self
            .frame
            .as_ref()
            .and_then(|f| f.selection_range(index, self.selection_unit))
        else {
            return;
        };
        self.selection = if self.selection_unit == SelectionUnit::Cell && anchor == target {
            None
        } else {
            Some((anchor.0.min(target.0), anchor.1.max(target.1)))
        };
    }

    pub fn selection_event(
        &mut self,
        phase: rex_rune::terminal::selection::Phase,
        x: f32,
        y: f32,
        cell: (f32, f32),
    ) {
        use rex_rune::terminal::selection::{Geometry, Input, Phase};
        let index = self.index(x, y, cell);
        let cols = usize::from(self.frame.as_ref().map_or(1, |f| f.cols));
        self.selection_revision = self.selection_revision.wrapping_add(1);
        self.copy_wanted = None;
        let input = Input {
            generation: self.restore_generation,
            revision: self.selection_revision,
            phase,
            at: ((index % cols) as u16, (index / cols) as u16),
            position: (f64::from(x), f64::from(y)),
            geometry: Geometry {
                columns: cols as u32,
                cell_width: cell.0.ceil().max(1.) as u32,
                padding_left: 0,
                screen_height: self.rect[3].ceil().max(1.) as u32,
            },
        };
        self.selecting = matches!(phase, Phase::Down(_) | Phase::Move);
        self.last_selection_input = Some(input);
        self.send(Command::Selection(input));
    }

    pub fn end_selection(&mut self) {
        self.copy_wanted = None;
        self.primary_press = None;
        if self.selecting
            && let Some(mut input) = self.last_selection_input
        {
            self.selection_revision = self.selection_revision.wrapping_add(1);
            input.revision = self.selection_revision;
            input.phase = rex_rune::terminal::selection::Phase::Up;
            self.send(Command::Selection(input));
        }
        self.selecting = false;
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.selection_anchor = None;
        self.selecting = false;
        self.copy_wanted = None;
        if let Some(mut input) = self.last_selection_input.take() {
            self.selection_revision = self.selection_revision.wrapping_add(1);
            input.revision = self.selection_revision;
            input.phase = rex_rune::terminal::selection::Phase::Clear;
            self.send(Command::Selection(input));
        }
    }

    /// Select everything the terminal holds, scrollback included. The pane's worker makes
    /// the selection on its terminal, so the marked rows and the text to copy arrive with
    /// its next frame; until then nothing is selected.
    pub fn select_all(&mut self) {
        use rex_rune::terminal::selection::{Geometry, Input, Phase};
        self.selection = None;
        self.selection_anchor = None;
        self.selecting = false;
        self.copy_wanted = None;
        self.selection_revision = self.selection_revision.wrapping_add(1);
        let input = Input {
            generation: self.restore_generation,
            revision: self.selection_revision,
            phase: Phase::All,
            at: (0, 0),
            position: (0., 0.),
            geometry: Geometry {
                columns: u32::from(self.frame.as_ref().map_or(1, |f| f.cols)),
                cell_width: 1,
                padding_left: 0,
                screen_height: self.rect[3].ceil().max(1.) as u32,
            },
        };
        // Kept so that the next click, or anything else that clears, reaches the worker.
        self.last_selection_input = Some(input);
        self.send(Command::Selection(input));
    }

    /// Whether `copy_text` has text to give, without building it.
    pub fn has_selection(&self) -> bool {
        match (self.selection, &self.frame) {
            (Some(_), Some(frame)) => {
                frame.selection_revision != self.selection_revision
                    || frame.selection_text.as_ref().is_some_and(|t| !t.is_empty())
            }
            _ => false,
        }
    }

    pub fn copy_text(&self) -> Option<String> {
        let (a, b) = self.selection?;
        let frame = self.frame.as_ref()?;
        if frame.selection_revision == self.selection_revision {
            return frame.selection_text.as_ref().map(ToString::to_string);
        }
        Some(frame.selected_text(a, b))
    }

    /// The pointer has just finished a selection (its release was the last selection event):
    /// ask for its text once, when the frame that carries it arrives. Anything that changes
    /// or clears the selection before then withdraws the request.
    pub fn want_selected_text(&mut self) {
        self.copy_wanted = Some(self.selection_revision);
    }

    /// The text asked for with [`Term::want_selected_text`], the one time its frame is here.
    /// A click that selected nothing gives none.
    pub fn take_selected_text(&mut self) -> Option<String> {
        let frame = self.frame.as_ref()?;
        if self.copy_wanted != Some(frame.selection_revision) {
            return None;
        }
        self.copy_wanted = None;
        frame
            .selection_text
            .as_ref()
            .filter(|text| !text.is_empty())
            .map(ToString::to_string)
    }

    /// Have this pane draw `shape` until its program asks for a cursor of its own. The
    /// pane's worker is told only when the shape changes, so a pane left at the block it
    /// starts with is sent nothing.
    pub fn set_cursor_style(&mut self, shape: CursorShape) {
        if self.cursor_style != shape {
            self.cursor_style = shape;
            self.send(Command::CursorStyle(shape));
        }
    }
}

/// How the cursor draws: owning the keyboard (the shape the program asked for, a filled block
/// by default), visible but not (hollow), or not at all.
#[derive(Clone, Copy, Hash, PartialEq)]
pub enum CursorLook {
    Block,
    Hollow,
}

fn hash_cell(cell: &Cell, h: &mut DefaultHasher) {
    cell.text.hash(h);
    (cell.fg, cell.bg, cell.underline_color, cell.spacer_tail).hash(h);
    let s = &cell.style;
    (
        s.bold,
        s.italic,
        s.faint,
        s.strikethrough,
        s.underline,
        s.overline,
    )
        .hash(h);
}

#[derive(Clone, Copy, PartialEq)]
struct RunStyle {
    fg: u32,
    bold: bool,
    italic: bool,
    faint: bool,
    underline: Underline,
    /// None draws the underline in `fg`.
    underline_color: Option<u32>,
    overline: bool,
    strike: bool,
}
impl RunStyle {
    /// `default_bg` is the server's default background, which draws as `body` wherever it
    /// shows (inverse video's foreground included).
    fn of(cell: &Cell, default_bg: u32, body: u32) -> Self {
        let s = &cell.style;
        let tint = |c: u32| if c == default_bg { body } else { c };
        Self {
            // Faint text is pre-mixed toward its cell in sRGB.
            fg: if s.faint {
                mix(tint(cell.fg), tint(cell.bg), 0.65)
            } else {
                tint(cell.fg)
            },
            bold: s.bold,
            italic: s.italic,
            faint: s.faint,
            underline: s.underline,
            underline_color: cell.underline_color.map(tint),
            overline: s.overline,
            strike: s.strikethrough,
        }
    }
}

fn glyph_run(content: String, style: RunStyle, x: f32, y: f32, m: &Metrics) -> El {
    let font = face(m, style.bold, style.italic);
    let color = rgb(style.fg);
    let len = content.len();
    let mut e = text(content);
    if style.strike {
        e = e.strike(0, len, Some(color));
    }
    e.no_wrap().font(font).color(color).absolute(x + m.inset, y)
}

/// (character, bold, italic, the grid's text style) → advances exactly one cell.
type FitCache = std::collections::HashMap<(char, bool, bool, u64, u32, u64), bool>;

thread_local! {
    /// Whether a character, in a given face, advances exactly one cell: only those join
    /// multi-character segments, so nothing after a fallback or wider glyph drifts.
    static FITS: std::cell::RefCell<FitCache> = Default::default();
}

/// The grid's style for text a program marked bold or italic, where those are drawn.
fn face(m: &Metrics, bold: bool, italic: bool) -> TextStyle {
    let mut font = m.style;
    if bold && m.bold {
        font.weight = 700.;
    }
    if italic && m.italic {
        font = font.italic();
    }
    font
}

fn fits_cell(ch: char, bold: bool, italic: bool, m: &Metrics) -> bool {
    // All fonts permits proportional families too: even ASCII must advance one cell
    // before it may join a segment. A non-fitting glyph is placed in its own cell.
    // The style carries the size, the family, the face ordinary text is drawn in and the
    // character spacing: every one of them changes what fits.
    let (bold, italic) = (bold && m.bold, italic && m.italic);
    let mut style = DefaultHasher::new();
    m.style.hash(&mut style);
    let key = (
        ch,
        bold,
        italic,
        style.finish(),
        m.cell.0.to_bits(),
        rune::text::font_epoch(),
    );
    FITS.with_borrow_mut(|cache| {
        *cache.entry(key).or_insert_with(|| {
            let advance = rune::text::advance(&ch.to_string(), face(m, bold, italic));
            (advance - m.cell.0).abs() < 0.01
        })
    })
}

/// One text element for a run of single-cell characters starting at column `col`, with a span
/// per style change, so colours and weights never add elements.
struct Segment {
    col: usize,
    text: String,
    spans: Vec<Span>,
    strikes: Vec<(usize, usize, u32)>,
    style: Option<RunStyle>,
    start: usize,
}

impl Segment {
    fn new(col: usize) -> Self {
        Self {
            col,
            text: String::new(),
            spans: Vec::new(),
            strikes: Vec::new(),
            style: None,
            start: 0,
        }
    }
    fn close_span(&mut self) {
        if let Some(style) = self.style.take() {
            let end = self.text.len();
            if end > self.start {
                let color = rgb(style.fg);
                let mut span = Span::new(self.start, end);
                span.color = Some(color);
                if style.bold {
                    span.weight = Some(700.);
                }
                span.italic = style.italic;
                self.spans.push(span);
                if style.strike {
                    self.strikes.push((self.start, end, style.fg));
                }
            }
        }
    }
    fn push(&mut self, text: &str, style: RunStyle) {
        if self.style != Some(style) {
            self.close_span();
            self.style = Some(style);
            self.start = self.text.len();
        }
        self.text.push_str(text);
    }
    fn finish(mut self, m: &Metrics) -> Option<El> {
        self.close_span();
        // Trailing blanks need no glyph: underlines and overlines are drawn on their own.
        while self.text.ends_with(' ')
            && !self.strikes.iter().any(|&(_, e, _)| e == self.text.len())
        {
            self.text.pop();
            if let Some(last) = self.spans.last_mut() {
                last.end = last.end.min(self.text.len() as u32);
                if last.end <= last.start {
                    self.spans.pop();
                }
            }
        }
        if self.text.trim().is_empty() && self.strikes.is_empty() {
            return None;
        }
        let mut e = rich(self.text, self.spans)
            // The server owns line breaks. Fractional-DPI rounding must not wrap a run again.
            .no_wrap()
            .font(m.style)
            .absolute(self.col as f32 * m.cell.0 + m.inset, 0.);
        for (a, b, fg) in self.strikes {
            e = e.strike(a, b, Some(rgb(fg)));
        }
        Some(e)
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq)]
enum RowPass {
    All,
    Background,
    Text,
}

/// What the client marks on a row itself, as inclusive columns: the selection, and search
/// matches with whether each is the selected match.
#[derive(Default, Hash)]
struct Marks<'a> {
    selection: Option<(usize, usize)>,
    /// Which corners of the row's selection are outer corners of the whole selection, so
    /// they can be rounded: top-left, top-right, bottom-right, bottom-left.
    corners: [bool; 4],
    search: &'a [(usize, usize, bool)],
}

/// The outer corners of one row's selection `on`, given the rows above and below it: a
/// corner the neighbouring row's selection runs past stays square, so a selection of
/// several rows reads as one shape.
fn selection_corners(
    on: (usize, usize),
    above: Option<(usize, usize)>,
    below: Option<(usize, usize)>,
) -> [bool; 4] {
    let open =
        |row: Option<(usize, usize)>, col: usize| !row.is_some_and(|(a, b)| (a..=b).contains(&col));
    [
        open(above, on.0),
        open(above, on.1),
        open(below, on.1),
        open(below, on.0),
    ]
}

/// How the selection is drawn, from Settings → Terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SelectionLook {
    pub foreground: SelectionColor,
    pub background: SelectionColor,
    pub rounded: bool,
}
impl Default for SelectionLook {
    fn default() -> Self {
        Self::of(&Default::default())
    }
}
impl SelectionLook {
    pub fn of(config: &TerminalConfig) -> Self {
        Self {
            foreground: config.selection_foreground,
            background: config.selection_background,
            rounded: config.selection_rounded,
        }
    }
}

/// A selected cell's fill and text colour. `text` is the cell's text as it draws unselected
/// and `ground` its background (the default background already as the pane's own).
fn selected_colors(colors: &TermColors, text: u32, ground: u32) -> (u32, u32) {
    let look = colors.selection;
    let fill = match look.background {
        SelectionColor::Theme => colors.selection_bg,
        SelectionColor::CellForeground => text,
        SelectionColor::CellBackground => ground,
    };
    let ink = match look.foreground {
        SelectionColor::Theme => colors.selection_fg.unwrap_or(text),
        SelectionColor::CellForeground => text,
        SelectionColor::CellBackground => ground,
    };
    // A cell colour on either side can leave text in the colour of its fill (Cell
    // foreground as the background, under text that keeps its own): the text takes the
    // cell's other colour, which is what makes that choice the classic inverse selection.
    let cells =
        look.background != SelectionColor::Theme || look.foreground != SelectionColor::Theme;
    if cells && ink == fill && text != ground {
        (fill, if fill == ground { text } else { ground })
    } else {
        (fill, ink)
    }
}

/// The server's default background for a row and the colour this client draws it as.
#[derive(Clone, Copy)]
struct RowBg {
    frame: u32,
    body: u32,
}

#[cfg(test)]
fn row_el(cells: &[Cell], y: f32, frame_bg: u32, sel: Option<(usize, usize)>, m: &Metrics) -> El {
    let marks = Marks {
        selection: sel,
        corners: [true; 4],
        ..Marks::default()
    };
    let colors = TermColors::default();
    let bg = RowBg {
        frame: frame_bg,
        body: colors.body(frame_bg, false),
    };
    row_pass(cells, y, bg, &marks, m, RowPass::All, &colors)
}

fn row_pass(
    cells: &[Cell],
    y: f32,
    bg: RowBg,
    marks: &Marks<'_>,
    m: &Metrics,
    pass: RowPass,
    colors: &TermColors,
) -> El {
    let RowBg {
        frame: frame_bg,
        body,
    } = bg;
    let (cw, ch) = m.cell;
    let sel = marks.selection;
    // Memoized rows are flattened on their own, so each says again that it never animates:
    // otherwise a glyph run that leaves a changed row fades out over its replacement.
    let mut row = el().w_full().h(ch).absolute(0., y).instant_changes();
    // Images below text belong between the background and glyph passes.
    if pass != RowPass::Text {
        let mut start = 0;
        while start < cells.len() {
            let bg = cells[start].bg;
            let mut end = start + 1;
            while end < cells.len() && cells[end].bg == bg {
                end += 1;
            }
            if bg != frame_bg {
                row = row.child(
                    el().absolute(start as f32 * cw, 0.)
                        .size((end - start) as f32 * cw, ch)
                        .fill(rgb(bg)),
                );
            }
            start = end;
        }
        for &(a, b, selected) in marks.search {
            let mark = el()
                .absolute(a as f32 * cw, 0.)
                .size((b + 1 - a) as f32 * cw, ch)
                .radius(SEARCH_RADIUS);
            let tone = rgb(colors.search_match);
            // An unselected match keeps the terminal's own text, so its tint stays light
            // enough for dim text and an outline carries the mark.
            row = row.child(if selected {
                mark.fill(tone.alpha(SEARCH_CURRENT))
            } else {
                mark.fill(tone.alpha(SEARCH_MATCH))
                    .border(1., tone.alpha(SEARCH_OUTLINE))
            });
        }
        if let Some((a, b)) = sel {
            // One shape per run of cells that fill alike: the whole span with the theme's
            // colour, shorter runs when the fill is each cell's own.
            let b = b.min(cells.len().saturating_sub(1));
            let fill_at = |col: usize| {
                let style = RunStyle::of(&cells[col], frame_bg, body);
                selected_colors(colors, style.fg, ground_of(&cells[col], frame_bg, body)).0
            };
            let radius = if colors.selection.rounded {
                SELECTION_RADIUS.min(ch / 2.)
            } else {
                0.
            };
            let round = |on: bool| if on { radius } else { 0. };
            let mut start = a;
            while start <= b && start < cells.len() {
                let fill = fill_at(start);
                let mut end = start + 1;
                while end <= b && fill_at(end) == fill {
                    end += 1;
                }
                let (first, last) = (start == a, end > b);
                let [tl, tr, br, bl] = marks.corners;
                row = row.child(
                    el().absolute(start as f32 * cw, 0.)
                        .size((end - start) as f32 * cw, ch)
                        .corners([
                            round(first && tl),
                            round(last && tr),
                            round(last && br),
                            round(first && bl),
                        ])
                        .fill(rgb(fill)),
                );
                start = end;
            }
        }
    }
    if pass == RowPass::Background {
        return row;
    }
    // Selected text takes the theme's selection foreground, decorations included. The selected
    // search match, where the selection is not over it, takes the text colour of its mark: the
    // mark is too light for the terminal's own.
    let style_at = |col: usize| {
        let mut style = RunStyle::of(&cells[col], frame_bg, body);
        style.bold &= m.bold;
        style.italic &= m.italic;
        if sel.is_some_and(|(a, b)| (a..=b).contains(&col)) {
            let ground = ground_of(&cells[col], frame_bg, body);
            style.fg = selected_colors(colors, style.fg, ground).1;
        } else if marks
            .search
            .iter()
            .any(|&(a, b, selected)| selected && (a..=b).contains(&col))
        {
            style.fg = colors.search_text;
        }
        style
    };
    let mut segment: Option<Segment> = None;
    for (col, cell) in cells.iter().enumerate() {
        if cell.spacer_tail || cell.text.starts_with('\u{10eeee}') {
            continue;
        }
        let style = style_at(col);
        let text = if cell.text.is_empty() {
            " "
        } else {
            cell.text.as_str()
        };
        let mut chars = text.chars();
        let single = chars.next().filter(|_| chars.next().is_none());
        let wide = cells.get(col + 1).is_some_and(|c| c.spacer_tail);
        if let Some(sprite) = single.and_then(Sprite::of) {
            if let Some(done) = segment.take().and_then(|s| s.finish(m)) {
                row = row.child(done);
            }
            let width = if wide { 2. * cw } else { cw };
            row = sprite.draw(row, col as f32 * cw, (width, ch), m.px, rgb(style.fg));
            continue;
        }
        let fits = !wide && single.is_some_and(|c| fits_cell(c, style.bold, style.italic, m));
        if fits {
            if text == " " && segment.is_none() && !style.strike {
                continue;
            }
            segment
                .get_or_insert_with(|| Segment::new(col))
                .push(text, style);
        } else {
            if let Some(done) = segment.take().and_then(|s| s.finish(m)) {
                row = row.child(done);
            }
            row = row.child(glyph_run(text.to_owned(), style, col as f32 * cw, 0., m));
        }
    }
    if let Some(done) = segment.and_then(|s| s.finish(m)) {
        row = row.child(done);
    }
    // Underlines and overlines in the style and colour the program asked for, rather than
    // the font's single underline, one shape per run of cells that draw them alike.
    let mut start = 0;
    while start < cells.len() {
        let style = style_at(start);
        let mut end = start + 1;
        while end < cells.len() && same_decoration(&style_at(end), &style) {
            end += 1;
        }
        if !cells[start].text.starts_with('\u{10eeee}') {
            let width = (end - start) as f32 * cw;
            // In the text's own line box, so a taller or shorter row keeps them at the text.
            let line = ch - 2. * m.leading;
            let shape = decoration_shape(&style, width, line, m.px);
            if !shape.is_empty() {
                row = row.child(
                    canvas(shape)
                        .absolute(start as f32 * cw, m.leading)
                        .size(width, line),
                );
            }
        }
        start = end;
    }
    row
}

/// A cell's background as it draws: the server's default background as the pane's own.
fn ground_of(cell: &Cell, default_bg: u32, body: u32) -> u32 {
    if cell.bg == default_bg { body } else { cell.bg }
}

fn same_decoration(a: &RunStyle, b: &RunStyle) -> bool {
    (a.underline, a.underline_color, a.overline, a.fg)
        == (b.underline, b.underline_color, b.overline, b.fg)
}

/// A straight line's thickness in points: one point in whole device pixels, as sprites' are.
fn line_thickness(px: f32) -> f32 {
    px.round().max(1.) / px
}

/// The centre of a line that starts on a whole device pixel `top` points down the cell.
fn line_centre(top: f32, px: f32) -> f32 {
    (top * px).round() / px + line_thickness(px) / 2.
}

/// A run's underline (single, double, curly, dotted or dashed) and overline, `width` wide in a
/// cell `height` tall, drawn at `px` device pixels per point.
fn decoration_shape(style: &RunStyle, width: f32, height: f32, px: f32) -> Shape {
    let mut shape = Shape::new();
    let color = rgb(style.underline_color.unwrap_or(style.fg));
    let thick = line_thickness(px);
    let centre = |top: f32| line_centre(top, px);
    let y = centre(height - 2.);
    match style.underline {
        Underline::Single => {
            shape = shape.line([0., y], [width, y], thick, LineCap::Butt, color);
        }
        Underline::Double => {
            let upper = y - (2. * px).round() / px;
            shape = shape
                .line([0., upper], [width, upper], thick, LineCap::Butt, color)
                .line([0., y], [width, y], thick, LineCap::Butt, color);
        }
        Underline::Curly => {
            // Half waves 4 points long, alternately below and above the line.
            let mut path = Path::new().move_to([0., y]);
            let (mut x, mut below) = (0., true);
            while x < width {
                let end = (x + 4.).min(width);
                let bend = if below { 1.5 } else { -1.5 };
                path = path.quad_to([(x + end) / 2., y + bend], [end, y]);
                (x, below) = (end, !below);
            }
            shape = shape.stroke_path(&path, thick, LineCap::Round, color);
        }
        Underline::Dotted => {
            let mut x = 0.;
            while x < width {
                shape = shape.line([x, y], [x + 0.1, y], 1.5, LineCap::Round, color);
                x += 3.;
            }
        }
        Underline::Dashed => {
            let mut x = 0.;
            while x < width {
                shape = shape.line(
                    [x, y],
                    [(x + 4.).min(width), y],
                    thick,
                    LineCap::Butt,
                    color,
                );
                x += 7.;
            }
        }
        _ => {}
    }
    if style.overline {
        let y = centre(0.5);
        shape = shape.line([0., y], [width, y], thick, LineCap::Butt, rgb(style.fg));
    }
    shape
}

fn image_el(
    image: &TermImage,
    texture: &rune::Image,
    frame: &Frame,
    m: &Metrics,
    start: usize,
) -> El {
    let p = &image.placement;
    let scale = [
        m.cell.0 / frame.cell_pixels[0].max(1) as f32,
        m.cell.1 / frame.cell_pixels[1].max(1) as f32,
    ];
    let width = p.size[0] as f32 * scale[0];
    let height = p.size[1] as f32 * scale[1];
    let sx = width / p.source[2] as f32;
    let sy = height / p.source[3] as f32;
    el().absolute(
        p.cell[0] as f32 * m.cell.0 + p.offset[0] as f32 * scale[0],
        (p.cell[1] as f32 - start as f32) * m.cell.1 + p.offset[1] as f32 * scale[1],
    )
    .size(width, height)
    .clip_rounded(0.)
    .instant_changes()
    .child(
        el().absolute(-(p.source[0] as f32) * sx, -(p.source[1] as f32) * sy)
            .size(
                image.pixels.width as f32 * sx,
                image.pixels.height as f32 * sy,
            )
            .pixels(texture.pixels()),
    )
}

pub struct GridLook {
    pub cursor: Option<CursorLook>,
    /// The shape the pane with the keyboard draws whatever its program asked for (Force
    /// style); none draws the frame's own.
    pub cursor_force: Option<CursorShape>,
    pub ime: bool,
    pub colors: TermColors,
}

/// The colours the client draws in the terminal area itself. The terminal's own colours reach
/// the frame through the server; these are the default background's stand-in and the
/// selection, cursor-text and search colours, which the protocol has no field for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TermColors {
    /// The declared theme's background, else the card.
    pub background: u32,
    pub selection_bg: u32,
    pub selection_fg: Option<u32>,
    /// Where the selection's two colours come from, and its corners.
    pub selection: SelectionLook,
    pub cursor_text: Option<u32>,
    /// Search matches' highlight: the chrome's accent.
    pub search_match: u32,
    /// The selected match's text, over its highlight.
    pub search_text: u32,
    /// Keep a background colour a program set for its pane (OSC 11) instead of substituting
    /// `background`.
    pub keep_program_background: bool,
}
impl Default for TermColors {
    fn default() -> Self {
        Self::of(None, &MIDNIGHT)
    }
}
impl TermColors {
    /// For the theme this client declares, if any, in chrome `palette`.
    pub fn of(theme: Option<&TerminalTheme>, palette: &Palette) -> Self {
        let theme = theme.cloned().unwrap_or_default();
        Self {
            background: theme.background.unwrap_or(palette.card),
            selection_bg: theme
                .selection_background
                .unwrap_or(palette.terminal_selection),
            selection_fg: theme.selection_foreground,
            selection: SelectionLook::default(),
            cursor_text: theme.cursor_text,
            search_match: palette.accent,
            search_text: palette.on_accent,
            keep_program_background: false,
        }
    }

    /// With the selection drawn as `look` says.
    pub fn with_selection(mut self, look: SelectionLook) -> Self {
        self.selection = look;
        self
    }

    /// Whether a program-set default background (OSC 11) keeps its own colour.
    pub fn with_program_background(mut self, keep: bool) -> Self {
        self.keep_program_background = keep;
        self
    }

    /// What the server's default background `frame_bg` draws as: `frame_bg` when a program
    /// chose it (`from_program`) and `keep_program_background` is on; otherwise `background`
    /// when both are equally dark or light (Rex's built-in black, or another owner's theme),
    /// so a single pane reads as one surface with the chrome; otherwise the server's own, so
    /// the default text drawn for it stays readable. Only that exact colour is replaced, so
    /// explicit backgrounds and inverse video's block keep theirs.
    pub fn body(&self, frame_bg: u32, from_program: bool) -> u32 {
        if self.keep_program_background && from_program {
            frame_bg
        } else if is_light(frame_bg) == is_light(self.background) {
            self.background
        } else {
            frame_bg
        }
    }
}

impl Term {
    /// The fill behind this pane's grid.
    pub fn body(&self, colors: &TermColors) -> u32 {
        self.frame
            .as_ref()
            .map_or(colors.background, |f| colors.body(f.bg, f.bg_from_program))
    }
}

/// The visible rows of `term`'s frame, each memoized by its content.
pub fn grid(key: &str, term: &Term, m: &Metrics, look: GridLook) -> El {
    let Some(frame) = term.display_frame() else {
        return el().w_full().h_full();
    };
    let (cw, ch) = m.cell;
    let visible = term.visible_rows(m.cell);
    let start = frame.display_row_start(visible);
    let cols = frame.cols as usize;
    let colors = &look.colors;
    let body = colors.body(frame.bg, frame.bg_from_program);
    let selection = term.selection.map(|(a, b)| (a.min(b), a.max(b)));
    let in_copy = frame.copy.is_some();
    let selected_on = |y: usize| {
        if y >= frame.cells.len() {
            None
        } else if in_copy
            || (term.selection.is_some() && frame.selection_revision == term.selection_revision)
        {
            frame.selection_rows.get(y).copied().flatten()
        } else {
            selection.and_then(|(a, b)| {
                let (row0, row1) = (y * cols, y * cols + cols - 1);
                (a <= row1 && b >= row0).then(|| (a.max(row0) - row0, b.min(row1) - row0))
            })
        }
    };
    let rows = |mut grid: El, pass: RowPass| {
        for (y, cells) in frame.cells.iter().enumerate().skip(start).take(visible) {
            let selected = selected_on(y);
            let marks = Marks {
                selection: selected,
                corners: selected.map_or([false; 4], |on| {
                    selection_corners(
                        on,
                        y.checked_sub(1).and_then(selected_on),
                        selected_on(y + 1),
                    )
                }),
                search: frame
                    .search
                    .as_ref()
                    .map(|search| search.on_row(y))
                    .unwrap_or_default(),
            };
            let mut h = DefaultHasher::new();
            for cell in cells {
                hash_cell(cell, &mut h);
            }
            (
                &marks,
                frame.bg,
                body,
                cw.to_bits(),
                ch.to_bits(),
                m.px.to_bits(),
                m.style,
                (m.bold, m.italic, m.inset.to_bits(), m.leading.to_bits()),
                pass,
                colors,
            )
                .hash(&mut h);
            let top = (y - start) as f32 * ch;
            let id = format!("{key}/r{}:{pass:?}", y - start);
            let bg = RowBg {
                frame: frame.bg,
                body,
            };
            grid = grid.child(memo(id, h.finish(), || {
                row_pass(cells, top, bg, &marks, m, pass, colors)
            }));
        }
        grid
    };
    let images = |mut grid: El, layer: i32| {
        for (index, image) in frame
            .images
            .iter()
            .enumerate()
            .filter(|(_, image)| image.placement.layer == layer)
        {
            if let Some((_, texture)) = term.images.get(&(Arc::as_ptr(&image.pixels) as usize)) {
                grid = grid.child(
                    image_el(image, texture, frame, m, start).key(format!("{key}/image:{index}")),
                );
            }
        }
        grid
    };
    let mut grid = el().w_full().h_full().clip_rounded(0.).instant_changes();
    if frame.images.is_empty() {
        grid = rows(grid, RowPass::All);
    } else {
        grid = images(grid, 0);
        grid = rows(grid, RowPass::Background);
        grid = images(grid, 1);
        grid = rows(grid, RowPass::Text);
        grid = images(grid, 2);
    }
    if let Some((_, link)) = &term.link {
        for mark in link_marks(key, frame, link, start..start + visible, m, body) {
            grid = grid.child(mark);
        }
    }
    if let Some(view) = frame.copy {
        let (x, y) = view.cursor;
        if let Some(cursor_look) = look.cursor
            && (y as usize) >= start
            && (y as usize) < start + visible
        {
            let (px, py) = (x as f32 * cw, (y as usize - start) as f32 * ch);
            let width = if view.wide { 2. * cw } else { cw };
            let color = rgb(colors.search_match);
            let shape = match cursor_look {
                CursorLook::Block => CursorShape::Block,
                CursorLook::Hollow => CursorShape::Hollow,
            };
            let cursor =
                cursor_el(shape, (px, py), (width, ch), color).key(format!("{key}/copy-cursor"));
            grid = grid.child(cursor);
            if shape == CursorShape::Block
                && let Some(cell) = frame
                    .cells
                    .get(y as usize)
                    .and_then(|r| r.get(x as usize))
                    .filter(|c| !c.text.is_empty() && !c.text.starts_with('\u{10eeee}'))
            {
                let mut chars = cell.text.chars();
                let single = chars.next().filter(|_| chars.next().is_none());
                if let Some(sprite) = single.and_then(Sprite::of) {
                    let layer = el()
                        .key(format!("{key}/copy-cursor-sprite"))
                        .absolute(px, py)
                        .size(width, ch);
                    grid = grid.child(sprite.draw(
                        layer,
                        0.,
                        (width, ch),
                        m.px,
                        rgb(colors.search_text),
                    ));
                } else {
                    let mut style = RunStyle {
                        fg: colors.search_text,
                        ..RunStyle::of(cell, frame.bg, body)
                    };
                    style.bold &= m.bold;
                    style.italic &= m.italic;
                    grid = grid.child(glyph_run(cell.text.clone(), style, px, py, m));
                }
            }
        }
    } else if let Some((x, y)) = frame.cursor
        && let Some(cursor_look) = look.cursor
        && (y as usize) >= start
        && (y as usize) < start + visible
    {
        let (px, py) = (x as f32 * cw, (y as usize - start) as f32 * ch);
        let cell = frame.cells.get(y as usize).and_then(|r| r.get(x as usize));
        let color = rgb(frame.cursor_color);
        // A pane without the keyboard always shows a hollow block, whatever the program asked.
        let shape = match cursor_look {
            CursorLook::Block => look.cursor_force.unwrap_or(frame.cursor_shape),
            CursorLook::Hollow => CursorShape::Hollow,
        };
        let cursor = cursor_el(shape, (px, py), m.cell, color).key(format!("{key}/cursor"));
        grid = grid.child(cursor);
        if shape == CursorShape::Block
            && let Some(cell) =
                cell.filter(|c| !c.text.is_empty() && !c.text.starts_with('\u{10eeee}'))
        {
            let style = RunStyle {
                fg: colors
                    .cursor_text
                    .filter(|_| body == colors.background)
                    .unwrap_or(body),
                ..RunStyle::of(cell, frame.bg, body)
            };
            grid = grid.child(glyph_run(cell.text.clone(), style, px, py, m));
        }
    }
    if look.ime {
        let (col, pane_row) = ime_cell(frame, start, visible, term.visible_cols(m.cell));
        let (px, py) = (col as f32 * cw, pane_row as f32 * ch);
        grid = grid.child(
            el().key(format!("{key}/ime"))
                .absolute(px, py)
                .size(cw, ch)
                .ime_cursor(),
        );
        if !term.preedit.is_empty() {
            let len = term.preedit.len();
            grid = grid.child(
                rich(
                    term.preedit.clone(),
                    vec![Span {
                        underline: Some(rgb(frame.fg)),
                        ..Span::new(0, len)
                    }],
                )
                .key(format!("{key}/preedit"))
                .no_wrap()
                .font(m.style)
                .color(rgb(frame.fg))
                .fill(rgb(body))
                .absolute(px, py),
            );
        }
    }
    grid
}

/// The cell `(col, pane_row)` inside the visible grid where the IME anchor and inline preedit
/// sit: the terminal's cursor cell clamped to the visible viewport, or the bottom-left visible
/// cell when scrolled away from the cursor.
fn ime_cell(
    frame: &Frame,
    start: usize,
    visible_rows: usize,
    visible_cols: usize,
) -> (usize, usize) {
    let shown_rows = (frame.rows as usize)
        .saturating_sub(start)
        .min(visible_rows.max(1))
        .max(1);
    let max_row = start + shown_rows - 1;
    let max_col = visible_cols.max(1).min((frame.cols as usize).max(1)) - 1;
    match frame.cursor_at {
        Some((x, y)) => (
            (x as usize).min(max_col),
            (y as usize).clamp(start, max_row) - start,
        ),
        None => (0, shown_rows - 1),
    }
}

/// The cursor, `shape` on the cell at `at`.
fn cursor_el(shape: CursorShape, at: (f32, f32), cell: (f32, f32), color: rune::Color) -> El {
    let ((x, y), (cw, ch)) = (at, cell);
    match shape {
        CursorShape::Block => el().absolute(x, y).size(cw, ch).fill(color),
        CursorShape::Bar => el().absolute(x, y).size(2., ch).fill(color),
        CursorShape::Underline => el().absolute(x, y + ch - 2.).size(cw, 2.).fill(color),
        CursorShape::Hollow => el().absolute(x, y).size(cw, ch).border(1., color),
    }
}

/// The link Ctrl+click would open, underlined in its own colour, row by row, over the
/// `rows` on screen. Its tooltip names the target itself, since OSC 8 text can show a
/// different address.
fn link_marks(
    key: &str,
    frame: &Frame,
    link: &Link,
    rows: std::ops::Range<usize>,
    m: &Metrics,
    body: u32,
) -> Vec<El> {
    let (cw, ch) = m.cell;
    let cols = frame.cols as usize;
    let help = format!(
        "{}+click to open {}",
        super::platform::keys::primary_name(),
        link.uri
    );
    let color = frame
        .cells
        .get(link.start / cols)
        .and_then(|row| row.get(link.start % cols))
        .map_or(frame.fg, |c| RunStyle::of(c, frame.bg, body).fg);
    (link.start / cols..=link.end / cols)
        .filter(|y| rows.contains(y))
        .map(|y| {
            let c0 = if y == link.start / cols {
                link.start % cols
            } else {
                0
            };
            let c1 = if y == link.end / cols {
                link.end % cols
            } else {
                cols - 1
            };
            let width = (c1 + 1 - c0) as f32 * cw;
            el().key(format!("{key}/link{y}"))
                .absolute(c0 as f32 * cw, (y - rows.start) as f32 * ch)
                .size(width, ch)
                .help(help.clone())
                .child(
                    el().absolute(0., ch - 2. - m.leading)
                        .size(width, 1.)
                        .fill(rgb(color)),
                )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, Env, Runtime};

    struct RowState {
        cells: Vec<Cell>,
    }
    fn view(s: &RowState, _: &mut Cx<RowState>) -> El {
        let m = metrics(15.);
        el().w_full()
            .h_full()
            .child(row_el(&s.cells, 0., 0, None, &m))
    }

    #[test]
    fn an_owner_waits_once_per_live_ownership_epoch_and_a_non_owner_never_waits() {
        let status = |live, owner| Status {
            live,
            owner,
            status: String::new(),
        };
        let mut size = InitialSize::default();
        size.transition(&status(true, true));
        assert!(!size.renders(&status(true, true)), "hidden before layout");
        size.layout((80, 24), Some((160, 50)));
        assert!(
            !size.renders(&status(true, true)),
            "stale size stays hidden"
        );
        size.observe(Some((80, 24)));
        assert!(size.renders(&status(true, true)), "this size shows");
        size.layout((100, 30), Some((80, 24)));
        assert!(
            size.renders(&status(true, true)),
            "a later resize keeps drawing the previous frame"
        );

        size.transition(&status(true, false));
        assert!(
            size.renders(&status(true, false)),
            "a non-owner draws the size it gets"
        );
        size.transition(&status(true, true));
        size.layout((100, 30), Some((80, 24)));
        assert!(
            !size.renders(&status(true, true)),
            "taking ownership starts a new wait"
        );
        size.transition(&status(false, true));
        assert!(
            size.renders(&status(false, true)),
            "retained output stays visible offline"
        );
        size.transition(&status(true, true));
        size.layout((100, 30), Some((80, 24)));
        assert!(
            !size.renders(&status(true, true)),
            "reconnecting starts a new wait"
        );

        // A frame at the asked size may never come; the wait is bounded.
        let deadline = size.deadline().expect("a wait is on");
        assert!(
            !size.renders_at(&status(true, true), deadline - Duration::from_millis(1)),
            "hidden until the bound"
        );
        assert!(
            size.renders_at(&status(true, true), deadline),
            "drawn as it is once the bound passes"
        );
        size.layout((100, 30), Some((80, 24)));
        assert_eq!(
            size.deadline(),
            Some(deadline),
            "a relayout keeps the first wait's bound"
        );
        size.observe(Some((100, 30)));
        assert_eq!(size.deadline(), None, "no bound once the size arrived");
    }

    #[test]
    fn a_wait_that_runs_out_ends_once_even_if_the_size_never_comes() {
        let status = Status {
            live: true,
            owner: true,
            status: String::new(),
        };
        let mut size = InitialSize::default();
        size.transition(&status);
        size.layout((100, 30), Some((80, 24)));
        let deadline = size.deadline().expect("a wait is on");
        assert!(!size.give_up(deadline - Duration::from_millis(1)));
        assert!(size.give_up(deadline), "the bound passed");
        assert!(!size.give_up(deadline), "only once");
        assert_eq!(size.deadline(), None, "nothing left to wake for");
        assert!(size.renders(&status));
        size.layout((100, 30), Some((80, 24)));
        assert!(
            size.renders(&status),
            "the same ownership epoch doesn't wait again"
        );
    }

    #[test]
    fn typing_styled_terminal_runs_never_creates_a_second_ui_line() {
        let _families = rune::text::with_families("Inter", "IBM Plex Mono");
        let mut terminal = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
            cols: 253,
            rows: 4,
            ..Default::default()
        })
        .unwrap();
        let mut renderer = rex_rune::terminal::Renderer::new().unwrap();
        let mut rt = Runtime::with_text(
            RowState { cells: vec![] },
            view,
            (2300., 100.),
            Env::new(true, 1.5),
            bundled_row_fonts(),
        );
        // Shell prompts mix fallback glyphs, styles and ASCII. Rebuild on every keystroke,
        // including runs preceded by blanks, at the user's fractional display scale.
        for n in 1..80 {
            terminal.vt_write(
                format!(
                    "\x1b[2J\x1b[H\u{f179} \u{e0b1} \x1b[36m\u{f015}~\x1b[0m   {}",
                    "a".repeat(n)
                )
                .as_bytes(),
            );
            let frame = renderer.frame(&terminal).unwrap();
            rt.update(|s, _| s.cells = frame.cells[0].clone());
            rt.frame(1. / 60.);
            for e in rt.elements().into_iter().filter(|e| e.text.is_some()) {
                assert!(
                    e.rect[3] <= 22.,
                    "keystroke {n}: {:?} wraps to {:?}",
                    e.text,
                    e.rect
                );
            }
            for q in rt.instances.iter().filter(|q| rune::compose::is_glyph(q)) {
                assert!(
                    q.rect[1] + q.rect[3] <= 33.,
                    "keystroke {n}: glyph drawn on the next line: {:?}",
                    q.rect
                );
            }
        }
    }

    #[test]
    fn scaled_rows_do_not_replace_the_callers_font_metrics() {
        let _families = rune::text::with_families("Inter", "Inter");
        let _text = rune::text::TextSystem::default();
        let before = metrics(15.).cell;
        let epoch = rune::text::font_epoch();
        assert!(!scaled_rects("─█").is_empty());
        assert_eq!(metrics(15.).cell, before);
        assert_eq!(rune::text::font_epoch(), epoch);
    }

    #[test]
    fn typing_rows_do_not_replace_the_callers_font_metrics() {
        let _families = rune::text::with_families("Inter", "Inter");
        let _text = rune::text::TextSystem::default();
        let before = metrics(15.).cell;
        let epoch = rune::text::font_epoch();
        typing_styled_terminal_runs_never_creates_a_second_ui_line();
        assert_eq!(metrics(15.).cell, before);
        assert_eq!(rune::text::font_epoch(), epoch);
    }

    /// A fixed cell, since the measured one follows whichever fonts another test has loaded.
    fn clipped_metrics() -> Metrics {
        Metrics {
            cell: (10., 20.),
            ..metrics(15.)
        }
    }

    struct ClippedState {
        term: Term,
    }
    fn clipped_view(s: &ClippedState, _: &mut Cx<ClippedState>) -> El {
        let look = GridLook {
            cursor: None,
            cursor_force: None,
            ime: false,
            colors: TermColors::default(),
        };
        el().w_full()
            .h_full()
            .child(grid("g", &s.term, &clipped_metrics(), look))
    }

    /// The search highlights `term` draws: (first column, row in the pane, selected).
    fn drawn_matches(term: Term) -> Vec<(u32, u32, bool)> {
        let (cw, ch) = clipped_metrics().cell;
        let mut rt = Runtime::new(
            ClippedState { term },
            clipped_view,
            (600., 200.),
            Env::default(),
        );
        rt.frame(1. / 60.);
        let mut drawn: Vec<_> = rt
            .instances
            .iter()
            .filter(|q| q.params[1] == rune::render::KIND_RECT && q.fill[3] > 0.1)
            .map(|q| {
                (
                    (q.rect[0] / cw).round() as u32,
                    (q.rect[1] / ch).round() as u32,
                    q.fill[3] > 0.6,
                )
            })
            .collect();
        drawn.sort_unstable();
        drawn
    }

    #[test]
    fn search_matches_stay_on_their_rows_when_the_pane_clips_a_taller_terminal() {
        use rex_rune::{
            connection::{Connection, TerminalSender},
            terminal::{Renderer, search::Controller},
        };
        // Another client owns five rows; this pane shows two, the newest, so a screen row and
        // a row in the pane differ by three.
        let mut terminal = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
            cols: 20,
            rows: 5,
            ..Default::default()
        })
        .unwrap();
        terminal.vt_write(b"    needle\r\n\r\n\r\nneedle\r\n");
        let mut search = Controller::default();
        search.set_needle(Some(&terminal), "needle".into()).unwrap();
        while search.update(&terminal).unwrap() {}
        let term_at = |search: &Controller| {
            let mut frame = Renderer::new().unwrap().frame(&terminal).unwrap();
            frame.search = search.results(&terminal).unwrap();
            let (sender, _) = flume::unbounded();
            let mut term = Term::new(Connection {
                model: Default::default(),
                commands: TerminalSender::new(sender, "s".into(), "b".into()),
                wake: flume::unbounded().1,
            });
            term.frame = Some(Arc::new(frame));
            term.rect = [0., 0., 600., 2. * clipped_metrics().cell.1];
            term
        };

        let term = term_at(&search);
        assert_eq!(term.display_row(clipped_metrics().cell), 3);
        assert_eq!(
            drawn_matches(term),
            [(0, 0, true)],
            "the newest match, screen row 3, is the pane's first; the clipped one draws nothing"
        );

        // The older match is above what the pane shows, so the pane follows it there.
        search.step(&terminal, true).unwrap();
        let term = term_at(&search);
        assert_eq!(term.display_row(clipped_metrics().cell), 0);
        assert_eq!(drawn_matches(term), [(4, 0, true)]);
    }

    /// Real, supplied mono and symbol faces, not the installed font a test machine resolves.
    /// Construct only under a family guard: TextSystem also registers measurement fonts.
    fn bundled_row_fonts() -> rune::text::TextSystem {
        let fonts = crate::app::fonts::load("IBM Plex Mono", &[]);
        rune::text::TextSystem::new_with_paths(fonts.files, &[], fonts.families)
    }

    /// A row at 1.5 device pixels per point, placed off the pixel grid.
    fn scaled_view(s: &RowState, _: &mut Cx<RowState>) -> El {
        let m = Metrics {
            px: 1.5,
            ..metrics(15.)
        };
        el().absolute(0.37, 0.71)
            .w_full()
            .h_full()
            .child(row_el(&s.cells, 0., 0, None, &m))
    }

    /// The rectangles drawn for `text` in one row at 1.5 device pixels per point, in device
    /// pixels, sorted from the left. Every one has whole-pixel edges.
    fn scaled_rects(text: &str) -> Vec<[f32; 4]> {
        let _families = rune::text::with_families("Inter", "IBM Plex Mono");
        let mut terminal = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
            cols: 40,
            rows: 2,
            ..Default::default()
        })
        .unwrap();
        terminal.vt_write(text.as_bytes());
        let frame = rex_rune::terminal::Renderer::new()
            .unwrap()
            .frame(&terminal)
            .unwrap();
        let mut rt = Runtime::with_text(
            RowState {
                cells: frame.cells[0].clone(),
            },
            scaled_view,
            (600., 100.),
            Env::new(true, 1.5),
            bundled_row_fonts(),
        );
        rt.frame(1. / 60.);
        let mut rects: Vec<[f32; 4]> = rt
            .instances
            .iter()
            .filter(|q| q.params[1] == rune::render::KIND_RECT)
            .map(|q| q.rect)
            .collect();
        rects.sort_by(|a, b| a[0].total_cmp(&b[0]));
        for r in &rects {
            assert!(
                r.iter().all(|v| v.fract() == 0.),
                "{r:?} is off the pixel grid"
            );
        }
        rects
    }

    /// Each rectangle starts where an earlier one ends, or before.
    fn assert_unbroken(rects: &[[f32; 4]]) {
        let mut end = rects[0][0];
        for r in rects {
            assert!(r[0] <= end, "a gap before {r:?} in {rects:?}");
            end = end.max(r[0] + r[2]);
        }
    }

    #[test]
    fn box_lines_land_on_whole_pixels_and_meet_their_neighbours() {
        // Horizontal strokes: everything no taller than a heavy line.
        let lines: Vec<_> = scaled_rects("╭─┬──━━")
            .into_iter()
            .filter(|r| r[3] <= 4.)
            .collect();
        let (light, heavy): (Vec<&[f32; 4]>, Vec<_>) = lines.iter().partition(|r| r[3] == 2.);
        assert!(
            light.len() >= 6 && light.iter().all(|r| r[1] == light[0][1]),
            "light lines are one point, in whole pixels, on one row: {lines:?}"
        );
        assert_eq!(heavy.len(), 4, "heavy lines are twice as thick: {lines:?}");
        assert_unbroken(&lines);
    }

    #[test]
    fn decoration_lines_land_on_whole_device_pixels() {
        for px in [1., 1.25, 1.5, 1.75, 2., 2.5, 3.] {
            let thick = line_thickness(px) * px;
            assert_eq!(thick, thick.round(), "whole pixels thick at {px}");
            assert_eq!(thick, f32::max(px, 1.).round(), "one point at {px}");
            for top in [0.5, 17., 19., 20.5] {
                let edge = (line_centre(top, px) - line_thickness(px) / 2.) * px;
                assert!(
                    (edge - edge.round()).abs() < 1e-3,
                    "{top} at {px} starts on a pixel: {edge}"
                );
            }
        }
    }

    #[test]
    fn block_elements_fill_whole_pixels_without_seams() {
        let blocks = scaled_rects("████▌▐▀▄");
        assert_eq!(blocks.len(), 8, "one rectangle per block: {blocks:?}");
        let full = &blocks[..4];
        assert!(
            full.iter()
                .all(|r| r[1] == full[0][1] && r[3] == full[0][3]),
            "full blocks share their top and height: {full:?}"
        );
        assert_unbroken(full);
    }

    #[test]
    fn a_link_tooltip_names_the_hyperlink_target_not_its_text() {
        let mut terminal = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
            cols: 40,
            rows: 2,
            ..Default::default()
        })
        .unwrap();
        terminal
            .vt_write(b"\x1b]8;;https://evil.example/\x1b\\https://good.example/\x1b]8;;\x1b\\");
        let frame = rex_rune::terminal::Renderer::new()
            .unwrap()
            .frame(&terminal)
            .unwrap();
        let link = frame.link_at(3).expect("hyperlink");
        assert_eq!(link.uri, "https://evil.example/");
        struct LinkState {
            frame: Frame,
            link: Link,
        }
        let view = |s: &LinkState, _: &mut Cx<LinkState>| {
            let m = metrics(15.);
            let marks = link_marks("g", &s.frame, &s.link, 0..2, &m, MIDNIGHT.card);
            el().w_full().h_full().children(marks)
        };
        let mut rt = Runtime::new(
            LinkState { frame, link },
            view,
            (600., 100.),
            Env::default(),
        );
        rt.frame(0.);
        let help: Vec<String> = rt.elements().into_iter().filter_map(|e| e.help).collect();
        assert_eq!(help, ["Ctrl+click to open https://evil.example/"]);
    }

    #[test]
    fn search_highlights_follow_their_rows_when_the_pane_clips_a_taller_terminal() {
        use rex_rune::connection::{Connection, TerminalSender};
        let mut terminal = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
            cols: 12,
            rows: 5,
            ..Default::default()
        })
        .unwrap();
        terminal.vt_write(b"needle\r\na\r\nb\r\nc\r\nxx needle");
        let mut search = rex_rune::terminal::search::Controller::default();
        search.set_needle(Some(&terminal), "needle".into()).unwrap();
        while search.update(&terminal).unwrap() {}
        let mut frame = rex_rune::terminal::Renderer::new()
            .unwrap()
            .frame(&terminal)
            .unwrap();
        frame.search = search.results(&terminal).unwrap();
        assert_eq!(frame.search.as_ref().unwrap().matches.len(), 2);

        let m = clipped_metrics();
        let mut term = Term::new(Connection {
            model: Default::default(),
            commands: TerminalSender::new(flume::unbounded().0, "s".into(), "b".into()),
            wake: flume::unbounded().1,
        });
        term.frame = Some(Arc::new(frame));
        term.rect = [0., 0., 200., 2. * m.cell.1];
        assert_eq!(term.display_row(m.cell), 3, "rows 3 and 4 are on screen");
        let colors = TermColors::default();
        let view = |term: &Term, _: &mut Cx<Term>| {
            let look = GridLook {
                cursor: None,
                cursor_force: None,
                ime: false,
                colors: TermColors::default(),
            };
            grid("g", term, &clipped_metrics(), look)
        };
        let mut rt = Runtime::new(term, view, (200., 100.), Env::default());
        rt.frame(0.);
        let highlight = |alpha| rgb(colors.search_match).alpha(alpha);
        let marks: Vec<_> = rt
            .elements()
            .into_iter()
            .filter(|e| e.fill == highlight(SEARCH_CURRENT) || e.fill == highlight(SEARCH_MATCH))
            .map(|e| {
                let [x, y, w, _] = e.rect;
                let cells = [x / m.cell.0, y / m.cell.1, w / m.cell.0].map(f32::round);
                (cells, e.fill == highlight(SEARCH_CURRENT))
            })
            .collect();
        assert_eq!(
            marks,
            [([3., 1., 6.], true)],
            "only the selected match on row 4, the second row on screen; the one on row 0 is clipped"
        );
        // The selected match's six letters are drawn in its mark's text colour; the two
        // before it keep the terminal's.
        let glyphs = |colour: rune::Color| {
            let fill = [colour.r, colour.g, colour.b, colour.a];
            rt.instances
                .iter()
                .filter(|q| q.params[1] == rune::render::KIND_GLYPH && q.fill == fill)
                .count()
        };
        assert_eq!(glyphs(rgb(colors.search_text)), 6);
        assert_eq!(glyphs(rgb(0xffffff)), 3, "`c` on the row above, and `xx`");
    }

    /// A frame of default text, an explicit red background and inverse video, on a server
    /// whose default colours are `bg` and `fg`.
    fn themed_frame(bg: u32, fg: u32) -> Frame {
        let color = |c: u32| {
            Some(ghostty_vt::style::RgbColor {
                r: (c >> 16) as u8,
                g: (c >> 8) as u8,
                b: c as u8,
            })
        };
        let mut terminal = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
            cols: 4,
            rows: 1,
            ..Default::default()
        })
        .unwrap();
        terminal.set_default_bg_color(color(bg)).unwrap();
        terminal.set_default_fg_color(color(fg)).unwrap();
        terminal.vt_write(b"x\x1b[48;2;200;0;0mR\x1b[0m\x1b[7mI\x1b[0m");
        rex_rune::terminal::Renderer::new()
            .unwrap()
            .frame(&terminal)
            .unwrap()
    }

    #[test]
    fn the_default_background_takes_the_chromes_only_on_its_own_side() {
        let follow = TermColors::default();
        let card = MIDNIGHT.card;
        assert_eq!(follow.body(0x000000, false), card, "Rex's built-in black");
        assert_eq!(
            follow.body(0x1e1e2e, false),
            card,
            "another owner's dark theme"
        );
        assert_eq!(
            follow.body(0xfdf6e3, false),
            0xfdf6e3,
            "another owner's light theme"
        );
        let light = TermColors::of(
            Some(&TerminalTheme {
                background: Some(0xfdf6e3),
                ..TerminalTheme::default()
            }),
            &MIDNIGHT,
        );
        assert_eq!(light.body(0xfdf6e3, false), 0xfdf6e3);
        assert_eq!(light.body(0xeeeeee, false), 0xfdf6e3);
        assert_eq!(
            light.body(0x000000, false),
            0x000000,
            "a server without set_theme keeps its black under its light default text"
        );
        let midnight = TermColors::of(Some(&TerminalTheme::midnight()), &MIDNIGHT);
        assert_eq!(midnight, follow, "Midnight's background is the card's");
        assert_eq!(
            TermColors::of(Some(&TerminalTheme::default()), &MIDNIGHT),
            follow,
            "Rex's own colours draw like Follow server"
        );
        let rose = TermColors::of(None, &crate::app::theme::ROSE_PINE);
        assert_eq!(
            rose.body(0x000000, false),
            0x191724,
            "another chrome's card"
        );
        assert_eq!(rose.selection_bg, 0x403d52, "and its selection");
        assert_ne!(rose, follow, "so rows memoized on the colours draw again");

        for (bg, fg, body) in [(0x1e1e2e, 0xcdd6f4, card), (0xfdf6e3, 0x586e75, 0xfdf6e3)] {
            let frame = themed_frame(bg, fg);
            assert_eq!(frame.bg, bg);
            assert!(!frame.bg_from_program);
            let body_drawn = follow.body(frame.bg, frame.bg_from_program);
            assert_eq!(body_drawn, body);
            let [plain, red, inverse] = [0, 1, 2].map(|i| &frame.cells[0][i]);
            assert_eq!(plain.bg, frame.bg, "drawn as the pane's fill, {body:06x}");
            assert_eq!(red.bg, 0xc80000, "an explicit background is drawn as it is");
            assert_eq!(inverse.bg, fg, "inverse video's block keeps its colour");
            assert_eq!(RunStyle::of(plain, frame.bg, body_drawn).fg, fg);
            assert_eq!(RunStyle::of(red, frame.bg, body_drawn).fg, fg);
            assert_eq!(
                RunStyle::of(inverse, frame.bg, body_drawn).fg,
                body,
                "inverse text is cut out of the block in the pane's fill"
            );
        }
    }

    #[test]
    fn keep_program_background_preserves_osc11_and_invalidates_memo_and_cursor_text() {
        use rex_rune::connection::{Connection, TerminalSender};
        let hash_of = |colors: TermColors| {
            let mut h = DefaultHasher::new();
            colors.hash(&mut h);
            h.finish()
        };

        let follow = TermColors::default();
        let follow_keep = follow.with_program_background(true);
        assert_ne!(follow, follow_keep);
        assert_ne!(hash_of(follow), hash_of(follow_keep));

        // Issue #143: OSC 11 #1d2a2b over dark Midnight card (#001528) and dark declared theme
        // (#1a2116), plus non-program server backgrounds and light/dark combinations.
        let dark_declared = TermColors::of(
            Some(&TerminalTheme {
                background: Some(0x1a2116),
                cursor_text: Some(0xff00ff),
                ..TerminalTheme::default()
            }),
            &MIDNIGHT,
        );
        let dark_declared_keep = dark_declared.with_program_background(true);
        let light_declared = TermColors::of(
            Some(&TerminalTheme {
                background: Some(0xfdf6e3),
                ..TerminalTheme::default()
            }),
            &MIDNIGHT,
        );
        let light_declared_keep = light_declared.with_program_background(true);

        // Off: same-side is substituted even when from_program is true; opposite-side is kept.
        assert_eq!(follow.body(0x1d2a2b, false), MIDNIGHT.card);
        assert_eq!(follow.body(0x1d2a2b, true), MIDNIGHT.card);
        assert_eq!(follow.body(0xfdf6e3, false), 0xfdf6e3);
        assert_eq!(follow.body(0xfdf6e3, true), 0xfdf6e3);
        assert_eq!(dark_declared.body(0x1d2a2b, false), 0x1a2116);
        assert_eq!(dark_declared.body(0x1d2a2b, true), 0x1a2116);
        assert_eq!(light_declared.body(0xe8f0e8, false), 0xfdf6e3);
        assert_eq!(light_declared.body(0xe8f0e8, true), 0xfdf6e3);
        assert_eq!(light_declared.body(0x1d2a2b, false), 0x1d2a2b);
        assert_eq!(light_declared.body(0x1d2a2b, true), 0x1d2a2b);

        // On: only from_program overrides same-side substitution; non-program stays substituted.
        assert_eq!(follow_keep.body(0x1d2a2b, false), MIDNIGHT.card);
        assert_eq!(follow_keep.body(0x1d2a2b, true), 0x1d2a2b);
        assert_eq!(follow_keep.body(0xfdf6e3, false), 0xfdf6e3);
        assert_eq!(follow_keep.body(0xfdf6e3, true), 0xfdf6e3);
        assert_eq!(dark_declared_keep.body(0x1d2a2b, false), 0x1a2116);
        assert_eq!(dark_declared_keep.body(0x1d2a2b, true), 0x1d2a2b);
        assert_eq!(light_declared_keep.body(0xe8f0e8, false), 0xfdf6e3);
        assert_eq!(light_declared_keep.body(0xe8f0e8, true), 0xe8f0e8);
        assert_eq!(light_declared_keep.body(0x1d2a2b, false), 0x1d2a2b);
        assert_eq!(light_declared_keep.body(0x1d2a2b, true), 0x1d2a2b);

        // Live grid rendering: inverse video and block cursor text respond immediately to both
        // toggling the option and OSC 11 / OSC 111 without stale memoized rows.
        let mut terminal = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
            cols: 8,
            rows: 1,
            ..Default::default()
        })
        .unwrap();
        terminal
            .set_default_bg_color(Some(ghostty_vt::style::RgbColor {
                r: 0x1a,
                g: 0x21,
                b: 0x16,
            }))
            .unwrap();
        terminal
            .set_default_fg_color(Some(ghostty_vt::style::RgbColor {
                r: 0xe0,
                g: 0xe0,
                b: 0xe0,
            }))
            .unwrap();
        // Cell 0: 'C' under the block cursor; cell 1: 'I' in inverse video (fg = default bg).
        terminal.vt_write(b"\x1b]11;#1d2a2b\x1b\\C\x1b[7mI\x1b[0m\x1b[1;1H");
        let mut renderer = rex_rune::terminal::Renderer::new().unwrap();
        let osc11_frame = renderer.frame(&terminal).unwrap();
        assert!(osc11_frame.bg_from_program);
        assert_eq!(osc11_frame.bg, 0x1d2a2b);
        assert_eq!(osc11_frame.cursor, Some((0, 0)));

        let mut term = Term::new(Connection {
            model: Default::default(),
            commands: TerminalSender::new(flume::unbounded().0, "s".into(), "b".into()),
            wake: flume::unbounded().1,
        });
        term.frame = Some(Arc::new(osc11_frame));
        term.rect = [0., 0., 200., clipped_metrics().cell.1];

        struct State {
            term: Term,
            colors: TermColors,
        }
        let view = |s: &State, _: &mut Cx<State>| {
            let look = GridLook {
                cursor: Some(CursorLook::Block),
                cursor_force: None,
                ime: false,
                colors: s.colors,
            };
            el().w_full()
                .h_full()
                .motion(crate::app::theme::STILL)
                .child(grid("g", &s.term, &clipped_metrics(), look))
        };
        let mut rt = Runtime::new(
            State {
                term,
                colors: dark_declared,
            },
            view,
            (200., 60.),
            Env::default(),
        );
        let glyph_count = |rt: &Runtime<State>, color: u32| {
            let c = rgb(color);
            let want = [c.r, c.g, c.b, c.a];
            rt.instances
                .iter()
                .filter(|q| q.params[1] == rune::render::KIND_GLYPH && q.fill == want)
                .count()
        };

        // Switch off: body == 0x1a2116 == colors.background -> inverse uses 0x1a2116,
        // cursor text uses theme cursor_text (0xff00ff).
        rt.frame(1. / 60.);
        assert_eq!(rt.state.term.body(&rt.state.colors), 0x1a2116);
        assert!(glyph_count(&rt, 0x1a2116) >= 1, "inverse text in theme bg");
        assert!(glyph_count(&rt, 0xff00ff) >= 1, "cursor text uses theme's");
        assert_eq!(glyph_count(&rt, 0x1d2a2b), 0, "OSC 11 not used while off");

        // Flip switch on: memoized row redraws immediately; inverse text and block cursor text
        // both use OSC 11 (0x1d2a2b), never theme cursor_text (0xff00ff).
        rt.update(|s, tx| {
            s.colors = dark_declared_keep;
            tx.instant();
        });
        rt.frame(1. / 60.);
        assert_eq!(rt.state.term.body(&rt.state.colors), 0x1d2a2b);
        assert!(
            glyph_count(&rt, 0x1d2a2b) >= 2,
            "both inverse text and block cursor text draw in 0x1d2a2b"
        );
        assert_eq!(
            glyph_count(&rt, 0xff00ff),
            0,
            "declared cursor_text never clashes over a program background"
        );
        assert_eq!(glyph_count(&rt, 0x1a2116), 0, "no stale row from before");

        // Program resets with OSC 111 while switch stays on: row redraws back to 0x1a2116
        // and theme cursor_text (0xff00ff) returns.
        terminal.vt_write(b"\x1b]111\x1b\\");
        let reset_frame = renderer.frame(&terminal).unwrap();
        assert!(!reset_frame.bg_from_program);
        assert_eq!(reset_frame.bg, 0x1a2116);
        rt.update(|s, tx| {
            s.term.frame = Some(Arc::new(reset_frame));
            tx.instant();
        });
        rt.frame(1. / 60.);
        assert_eq!(rt.state.term.body(&rt.state.colors), 0x1a2116);
        assert!(
            glyph_count(&rt, 0x1a2116) >= 1,
            "inverse text back to theme"
        );
        assert!(glyph_count(&rt, 0xff00ff) >= 1, "cursor text back to theme");
        assert_eq!(glyph_count(&rt, 0x1d2a2b), 0, "no stale OSC 11 row");
    }

    fn ime_test_term(terminal: &ghostty_vt::Terminal, rect: [f32; 4]) -> Term {
        use rex_rune::connection::{Connection, TerminalSender};
        let frame = rex_rune::terminal::Renderer::new()
            .unwrap()
            .frame(terminal)
            .unwrap();
        let mut term = Term::new(Connection {
            model: Default::default(),
            commands: TerminalSender::new(flume::unbounded().0, "s".into(), "b".into()),
            wake: flume::unbounded().1,
        });
        term.frame = Some(Arc::new(frame));
        term.rect = rect;
        term
    }

    #[test]
    fn ime_anchor_works_when_cursor_is_hidden_visible_cropped_or_scrolled() {
        use ghostty_vt::terminal::ScrollViewport;
        struct ImeState {
            term: Term,
            ime: bool,
            cursor: Option<CursorLook>,
        }
        let view = |s: &ImeState, _: &mut Cx<ImeState>| {
            let look = GridLook {
                cursor: s.cursor,
                cursor_force: None,
                ime: s.ime,
                colors: TermColors::default(),
            };
            el().w_full()
                .h_full()
                .child(grid("g", &s.term, &clipped_metrics(), look))
        };
        let (cw, ch) = clipped_metrics().cell;
        let full_rect = [0., 0., 20. * cw, 5. * ch];

        let mut terminal = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
            cols: 20,
            rows: 5,
            max_scrollback: 1 << 16,
        })
        .unwrap();
        terminal.vt_write(b"abc\x1b[?25l\x1b[3;5H");

        // Hidden cursor + look.ime == true: ime_area aligned with cursor cell, no /cursor drawn.
        let mut rt = Runtime::new(
            ImeState {
                term: ime_test_term(&terminal, full_rect),
                ime: true,
                cursor: Some(CursorLook::Block),
            },
            view,
            (20. * cw, 5. * ch),
            Env::default(),
        );
        rt.frame(0.);
        assert_eq!(rt.ime_area, Some([4. * cw, 2. * ch, cw, ch]));
        assert!(
            !rt.elements()
                .iter()
                .any(|e| e.key.as_deref() == Some("g/cursor")),
            "hidden cursor draws no cursor element"
        );
        assert!(
            rt.elements()
                .iter()
                .any(|e| e.key.as_deref() == Some("g/ime"))
        );

        // Preedit renders at the anchor even while cursor is hidden.
        rt.update(|s, _| s.term.preedit = "你好".into());
        rt.frame(0.);
        let preedit = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("g/preedit"))
            .expect("preedit rendered at hidden cursor anchor");
        assert_eq!(preedit.text.as_deref(), Some("你好"));
        assert_eq!((preedit.rect[0], preedit.rect[1]), (4. * cw, 2. * ch));
        rt.update(|s, _| s.term.preedit.clear());

        // look.ime == false disables ime_area whether hidden or visible.
        rt.update(|s, _| s.ime = false);
        rt.frame(0.);
        assert_eq!(rt.ime_area, None);
        terminal.vt_write(b"\x1b[?25h");
        rt.update(|s, _| s.term = ime_test_term(&terminal, full_rect));
        rt.frame(0.);
        assert_eq!(rt.ime_area, None);

        // Visible bar and underline cursors keep their own shapes while ime_area uses full cell.
        rt.update(|s, _| s.ime = true);
        for (seq, expected_size) in [(&b"\x1b[6 q"[..], (2., ch)), (&b"\x1b[4 q"[..], (cw, 2.))] {
            terminal.vt_write(seq);
            rt.update(|s, _| s.term = ime_test_term(&terminal, full_rect));
            rt.frame(0.);
            let cursor = rt
                .elements()
                .into_iter()
                .find(|e| e.key.as_deref() == Some("g/cursor"))
                .expect("visible cursor drawn");
            assert_eq!((cursor.rect[2], cursor.rect[3]), expected_size);
            assert_eq!(rt.ime_area, Some([4. * cw, 2. * ch, cw, ch]));
        }

        // Rapid hidden -> visible -> hidden transitions never drop ime_area.
        for (seq, expect_cursor) in [
            (&b"\x1b[?25l"[..], false),
            (&b"\x1b[?25h"[..], true),
            (&b"\x1b[?25l"[..], false),
        ] {
            terminal.vt_write(seq);
            rt.update(|s, _| s.term = ime_test_term(&terminal, full_rect));
            rt.frame(0.);
            assert_eq!(rt.ime_area, Some([4. * cw, 2. * ch, cw, ch]));
            assert_eq!(
                rt.elements()
                    .iter()
                    .any(|e| e.key.as_deref() == Some("g/cursor")),
                expect_cursor
            );
        }

        // Cropped pane: cursor hidden on row 1, col 15; pane shows 3 bottom rows and 10 cols.
        terminal.vt_write(b"\x1b[2;16H\x1b[?25l");
        let cropped_rect = [0., 0., 10. * cw, 3. * ch];
        rt.update(|s, _| s.term = ime_test_term(&terminal, cropped_rect));
        rt.frame(0.);
        // display_row_start(3) == 2 (bottom-aligned because cursor is hidden), so row 1 clamps to
        // pane row 0 and col 15 clamps to col 9.
        assert_eq!(rt.ime_area, Some([9. * cw, 0., cw, ch]));

        // Scrolled back (cursor_at == None): anchor falls back to bottom-left visible cell.
        terminal.vt_write(b"\x1b[5;1H");
        for i in 0..10 {
            terminal.vt_write(format!("line {i}\r\n").as_bytes());
        }
        terminal.vt_write(b"\x1b[?25l");
        terminal.scroll_viewport(ScrollViewport::Delta(-6));
        rt.update(|s, _| s.term = ime_test_term(&terminal, full_rect));
        rt.frame(0.);
        assert_eq!(rt.state.term.frame.as_ref().unwrap().cursor_at, None);
        assert_eq!(rt.ime_area, Some([0., 4. * ch, cw, ch]));
    }
}
