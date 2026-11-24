//! Searching a terminal's screen and scrollback with Ghostty's search, which keeps up with
//! new output and scrolls the viewport to the selected match.
use anyhow::Result;
use ghostty_vt::{
    Terminal,
    search::{Search, Status},
    selection::Selection,
    terminal::{PointCoordinate, PointSpace},
};

/// One match on screen, from `start` to `end` inclusive, as viewport (column, row) cells.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct Match {
    pub start: (u16, u16),
    pub end: (u16, u16),
}

/// What a frame shows of a search.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Results {
    /// Matches on screen.
    pub matches: Vec<Match>,
    /// The selected match, when it is on screen.
    pub selected: Option<Match>,
    /// The selected match's place among all matches, newest first.
    pub index: Option<usize>,
    pub total: usize,
    rows: Vec<Vec<(usize, usize, bool)>>,
}

impl Results {
    fn new(
        matches: Vec<Match>,
        selected: Option<Match>,
        index: Option<usize>,
        total: usize,
        cols: usize,
        rows: usize,
    ) -> Self {
        let mut by_row = vec![Vec::new(); rows];
        for found in &matches {
            let first_row = usize::from(found.start.1).min(rows);
            let last_row = usize::from(found.end.1).min(rows.saturating_sub(1));
            for (y, row) in by_row
                .iter_mut()
                .enumerate()
                .take(last_row.saturating_add(1))
                .skip(first_row)
            {
                let first = if usize::from(found.start.1) == y {
                    usize::from(found.start.0)
                } else {
                    0
                };
                let last = if usize::from(found.end.1) == y {
                    usize::from(found.end.0)
                } else {
                    cols.saturating_sub(1)
                };
                row.push((first, last, selected == Some(*found)));
            }
        }
        Self {
            matches,
            selected,
            index,
            total,
            rows: by_row,
        }
    }

    /// The inclusive columns each match covers on viewport row `y`, and whether it is selected.
    pub fn on_row(&self, y: usize) -> &[(usize, usize, bool)] {
        self.rows.get(y).map(Vec::as_slice).unwrap_or_default()
    }
}

/// One terminal's search, if the user is searching it. The needle outlives the terminal it
/// was typed for, so a pane that reconnects to a fresh snapshot is searched again.
#[derive(Default)]
pub struct Controller {
    needle: String,
    search: Option<Search>,
}

impl Controller {
    /// Searches for `needle` (empty stops searching) in `terminal`, or in the terminal a
    /// snapshot brings later.
    pub fn set_needle(&mut self, terminal: Option<&Terminal>, needle: String) -> Result<()> {
        self.needle = needle;
        match (terminal, &mut self.search) {
            (Some(terminal), Some(search)) if !self.needle.is_empty() => {
                search.set_needle(terminal, Some(&self.needle))?;
            }
            _ => self.search = None,
        }
        Ok(())
    }

    /// Selects the next older match (`older`) or the next newer one, wrapping around.
    pub fn step(&mut self, terminal: &Terminal, older: bool) -> Result<()> {
        if let Some(search) = &mut self.search {
            if older {
                search.select_next(terminal)?;
            } else {
                search.select_prev(terminal)?;
            }
        }
        Ok(())
    }

    /// Makes one bounded unit of progress, feeding terminal changes when needed. Returns whether
    /// more work remains, so the data worker schedules another tick without blocking I/O.
    pub fn update(&mut self, terminal: &Terminal) -> Result<bool> {
        if self.needle.is_empty() {
            return Ok(false);
        }
        let search = match &mut self.search {
            Some(search) => search,
            None => {
                let mut search = Search::new(terminal)?;
                search.set_needle(terminal, Some(&self.needle))?;
                self.search.insert(search)
            }
        };
        let status = match search.status()? {
            Status::Running => search.tick()?,
            Status::FeedRequired | Status::Complete => {
                search.feed(terminal)?;
                search.status()?
            }
        };
        if search.selected_index()?.is_none() && search.total_matches()? > 0 {
            search.select_next(terminal)?;
        }
        Ok(status != Status::Complete)
    }

    /// The search no longer applies to the terminal it ran on: a fresh snapshot replaced it.
    pub fn forget_terminal(&mut self) {
        self.search = None;
    }

    /// The selected match's starting cell in full-screen (scrollback + active) coordinates,
    /// or `None` when no search match is selected.
    pub fn selected_match_screen(&self, terminal: &Terminal) -> Result<Option<PointCoordinate>> {
        let Some(search) = &self.search else {
            return Ok(None);
        };
        let Some(found) = search.selected_match(terminal)? else {
            return Ok(None);
        };
        Ok(terminal.point_from_grid_ref(&found.start(), PointSpace::Screen)?)
    }

    pub fn results(&self, terminal: &Terminal) -> Result<Option<Results>> {
        let Some(search) = &self.search else {
            return Ok(None);
        };
        let mut matches = Vec::new();
        for found in search.viewport_matches(terminal)? {
            matches.extend(on_screen(terminal, &found)?);
        }
        let selected = match search.selected_match(terminal)? {
            Some(found) => on_screen(terminal, &found)?,
            None => None,
        };
        Ok(Some(Results::new(
            matches,
            selected,
            search.selected_index()?,
            search.total_matches()?,
            usize::from(terminal.cols()?),
            usize::from(terminal.rows()?),
        )))
    }
}

/// A match's cells on screen, cut to the viewport; none when it is off screen.
fn on_screen(terminal: &Terminal, found: &Selection<'_>) -> Result<Option<Match>> {
    let last = (
        terminal.cols()?.saturating_sub(1),
        terminal.rows()?.saturating_sub(1),
    );
    // A cell below a scrolled-up viewport still gets a viewport row, past the last one.
    let point = |at| -> Result<Option<(u16, u16)>> {
        Ok(terminal
            .point_from_grid_ref(&at, PointSpace::Viewport)?
            .and_then(|p| Some((p.x, u16::try_from(p.y).ok()?)))
            .filter(|p| p.1 <= last.1))
    };
    Ok(match (point(found.start())?, point(found.end())?) {
        (None, None) => None,
        (start, end) => Some(Match {
            start: start.unwrap_or((0, 0)),
            end: end.unwrap_or(last),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finish(search: &mut Controller, terminal: &Terminal) {
        for _ in 0..10_000 {
            if !search.update(terminal).unwrap() {
                return;
            }
        }
        panic!("search did not finish");
    }

    #[test]
    fn a_search_selects_the_newest_match_and_steps_through_the_rest() {
        let mut terminal = Terminal::new(ghostty_vt::TerminalOptions {
            cols: 20,
            rows: 3,
            max_scrollback: 1 << 20,
        })
        .unwrap();
        terminal.vt_write(b"needle one\r\nother\r\nneedle two\r\nlast");
        let mut search = Controller::default();
        assert_eq!(search.results(&terminal).unwrap(), None);
        search.set_needle(Some(&terminal), "needle".into()).unwrap();
        finish(&mut search, &terminal);
        let results = search.results(&terminal).unwrap().unwrap();
        assert_eq!(results.total, 2);
        assert_eq!(results.index, Some(0), "the newest match first");
        let newest = Match {
            start: (0, 1),
            end: (5, 1),
        };
        assert_eq!(results.selected, Some(newest));
        assert_eq!(results.on_row(1), [(0, 5, true)]);

        // The older match is in scrollback: selecting it scrolls it into view.
        search.step(&terminal, true).unwrap();
        let results = search.results(&terminal).unwrap().unwrap();
        assert_eq!(results.index, Some(1));
        let selected = results.selected.expect("scrolled into view");
        assert_eq!((selected.start.0, selected.end.0), (0, 5));
        search.step(&terminal, false).unwrap();
        assert_eq!(search.results(&terminal).unwrap().unwrap().index, Some(0));

        // New output is searched too.
        terminal.vt_write(b"\r\nneedle three");
        finish(&mut search, &terminal);
        assert_eq!(search.results(&terminal).unwrap().unwrap().total, 3);

        search.set_needle(Some(&terminal), String::new()).unwrap();
        assert_eq!(search.results(&terminal).unwrap(), None);
    }

    #[test]
    fn a_fresh_terminal_is_searched_for_the_same_needle() {
        let options = ghostty_vt::TerminalOptions {
            cols: 20,
            rows: 3,
            ..Default::default()
        };
        let mut search = Controller::default();
        let first = Terminal::new(options).unwrap();
        search.set_needle(None, "abc".into()).unwrap();
        assert_eq!(search.results(&first).unwrap(), None, "no terminal yet");
        finish(&mut search, &first);
        assert_eq!(search.results(&first).unwrap().unwrap().total, 0);
        search.forget_terminal();
        let mut second = Terminal::new(options).unwrap();
        second.vt_write(b"xx abc");
        finish(&mut search, &second);
        let results = search.results(&second).unwrap().unwrap();
        assert_eq!(results.total, 1);
        assert_eq!(
            results.matches,
            [Match {
                start: (3, 0),
                end: (5, 0)
            }]
        );
    }

    #[test]
    fn a_match_below_a_scrolled_up_viewport_is_off_screen() {
        let mut terminal = Terminal::new(ghostty_vt::TerminalOptions {
            cols: 20,
            rows: 5,
            max_scrollback: 1 << 20,
        })
        .unwrap();
        terminal.vt_write(b"needle top\r\n");
        for row in 0..12 {
            terminal.vt_write(format!("row {row}\r\n").as_bytes());
        }
        terminal.vt_write(b"needle bottom\r\n");
        let mut search = Controller::default();
        search.set_needle(Some(&terminal), "needle".into()).unwrap();
        finish(&mut search, &terminal);
        search.step(&terminal, true).unwrap();
        let results = search.results(&terminal).unwrap().unwrap();
        assert_eq!(results.total, 2);
        let older = Match {
            start: (0, 0),
            end: (5, 0),
        };
        assert_eq!(results.selected, Some(older));
        assert_eq!(
            results.matches,
            [older],
            "the newer match is below the screen"
        );
    }

    /// Ghostty's search picks an older match first, and then walks toward newer ones, once a
    /// terminal restored from a snapshot has been resized. Every match is still reached. Its
    /// internals decide this (`terminal/search/screen.zig`), and a pane this client owns meets
    /// it whenever it resizes after attaching, so it wants an upstream fix.
    #[test]
    #[ignore = "Ghostty's search order on a restored, resized terminal; see the comment"]
    fn a_restored_and_resized_terminal_selects_the_newest_match_first() {
        let mut restored = Terminal::new(ghostty_vt::TerminalOptions {
            cols: 80,
            rows: 24,
            max_scrollback: 1 << 20,
        })
        .unwrap();
        restored.vt_write(b"mise WARN x\r\n\r\n$ ");
        let snapshot = ghostty_vt::snapshot::encode(&restored).unwrap();
        let mut stream = crate::terminal::TerminalStream::new(1);
        stream.feed(&snapshot).unwrap();
        let terminal = stream.terminal.as_mut().unwrap();
        terminal.resize(120, 29, 0, 0).unwrap();
        terminal.vt_write(b"\x1b[H\x1b[2J\x1b[3J      needle-top\r\n\r\n\r\n");
        for row in 1..=28 {
            terminal.vt_write(format!("row {row}\r\n").as_bytes());
        }
        terminal.vt_write(b"needle-bottom\r\n$ ");
        let terminal = stream.terminal.as_ref().unwrap();
        let mut search = Controller::default();
        for needle in ["n", "ne", "nee", "need", "needl", "needle"] {
            search.set_needle(Some(terminal), needle.into()).unwrap();
            search.update(terminal).unwrap();
        }
        finish(&mut search, terminal);
        let selected = search.results(terminal).unwrap().unwrap().selected.unwrap();
        assert_eq!(selected.start.0, 0, "needle-bottom, not needle-top");
    }
}
