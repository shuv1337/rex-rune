//! Getting around Settings: its pages, the history between them and Search Settings.
//! `settings_shell` draws the sidebar and the page header on this state.
//!
//! - A [`Page`] is a section's own page or a [`SubPage`] pushed over it (Interface → Panes, a
//!   notification state). The sidebar keeps the section selected while a sub-page shows.
//! - [`Workspace::navigate`] is a visit the user makes: it is added to the history, and the
//!   page opens at its top. Back and Forward ([`Workspace::settings_history`]) return to a
//!   page where it was scrolled. `Workspace::open_section` only shows a section, as opening
//!   Settings on About does, and leaves no stop of its own.
//! - Search Settings has no list of its own to keep in step with the pages: it builds every
//!   page and reads the rows `settings_kit` makes ([`row`], [`heading`]). A hit that is a row
//!   opens its page and marks that row for a moment. The actions of the Shortcuts page are
//!   found by name too, after the settings, so a search for a key binding ends on its row.
use super::{
    panel::{PanelKind, SettingsSection},
    theme::*,
    workspace::Workspace,
};
use rune::{El, Key, KeyEvent, SpringPair, TextEdit, Transition, Tx, el};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

/// The page's header row, above its scroll region; the sidebar's search field sits in the
/// same band.
pub const HEADER: f32 = 56.;
/// The page's inset from the section list and from the dialog's trailing edge.
pub const PAGE_INSET: f32 = 20.;
/// How long the row a search landed on stays marked.
const MARKED_FOR: Duration = Duration::from_millis(2400);
/// The most hits the sidebar lists.
const HITS: usize = 40;
/// The most stops Back can return through.
const STOPS: usize = 64;

/// A program state the Notifications page gives a page of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Blocked,
    Error,
    Done,
    Working,
}

impl Status {
    pub const ALL: [Status; 4] = [
        Status::Blocked,
        Status::Error,
        Status::Done,
        Status::Working,
    ];

    /// What the program does, after "Notify when a program".
    pub fn title(self) -> &'static str {
        match self {
            Status::Blocked => "Needs you",
            Status::Error => "Fails",
            Status::Done => "Finishes",
            Status::Working => "Starts working",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Status::Blocked => "blocked",
            Status::Error => "error",
            Status::Done => "done",
            Status::Working => "working",
        }
    }
}

/// A page pushed over a section's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubPage {
    /// Interface → Customize panes.
    Panes,
    /// Interface → Customize tabs.
    Tabs,
    /// Terminal → Advanced font settings.
    AdvancedFont,
    /// Notifications → one program state.
    Status(Status),
}

impl SubPage {
    pub const ALL: [SubPage; 7] = [
        SubPage::Panes,
        SubPage::Tabs,
        SubPage::AdvancedFont,
        SubPage::Status(Status::Blocked),
        SubPage::Status(Status::Error),
        SubPage::Status(Status::Done),
        SubPage::Status(Status::Working),
    ];

    /// The section whose page it was pushed from, which stays selected in the sidebar.
    pub fn section(self) -> SettingsSection {
        match self {
            SubPage::Panes | SubPage::Tabs => SettingsSection::Interface,
            SubPage::AdvancedFont => SettingsSection::Terminal,
            SubPage::Status(_) => SettingsSection::Notifications,
        }
    }

    /// The key of the row on its section's page that opens it, where the keyboard returns
    /// when the page is left.
    pub fn opener(self) -> String {
        match self {
            SubPage::Panes => "customize-panes".into(),
            SubPage::Tabs => "customize-tabs".into(),
            SubPage::AdvancedFont => "open-advanced-font".into(),
            SubPage::Status(status) => format!("notify-status:{}", status.key()),
        }
    }
}

/// What Settings shows beside its sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Section(SettingsSection),
    Sub(SubPage),
}

impl Page {
    /// Every page, sections first, in the sidebar's order.
    pub fn all() -> impl Iterator<Item = Page> {
        SettingsSection::ALL
            .into_iter()
            .map(Page::Section)
            .chain(SubPage::ALL.into_iter().map(Page::Sub))
    }

    pub fn section(self) -> SettingsSection {
        match self {
            Page::Section(section) => section,
            Page::Sub(sub) => sub.section(),
        }
    }

    fn sub(self) -> Option<SubPage> {
        match self {
            Page::Section(_) => None,
            Page::Sub(sub) => Some(sub),
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Page::Section(section) => section.title(),
            Page::Sub(SubPage::Panes) => "Panes",
            Page::Sub(SubPage::Tabs) => "Tabs",
            Page::Sub(SubPage::AdvancedFont) => "Advanced font settings",
            Page::Sub(SubPage::Status(status)) => status.title(),
        }
    }

    /// Names the page's scroll region (`settings-scroll:<key>`).
    pub fn key(self) -> String {
        match self {
            Page::Section(section) => section.key().into(),
            Page::Sub(SubPage::Panes) => "panes".into(),
            Page::Sub(SubPage::Tabs) => "tabs".into(),
            Page::Sub(SubPage::AdvancedFont) => "advanced-font".into(),
            Page::Sub(SubPage::Status(status)) => format!("status-{}", status.key()),
        }
    }

    /// Where the page is, for a search hit: `Interface › Panes`.
    fn place(self) -> String {
        match self {
            Page::Section(section) => section.title().into(),
            Page::Sub(_) => format!("{} › {}", self.section().title(), self.title()),
        }
    }

    /// Words someone looking for this page may type that its title and rows don't hold.
    fn keywords(self) -> &'static str {
        match self {
            Page::Section(section) => match section {
                SettingsSection::Interface => "appearance layout window scale zoom sidebar tabs",
                SettingsSection::Themes => "colors colours palette appearance dark light chrome",
                SettingsSection::Terminal => "font shell text",
                SettingsSection::Shortcuts => "keys keyboard bindings keymap hotkeys",
                SettingsSection::Integrations => "agents skill hooks claude codex opencode pi",
                SettingsSection::Notifications => "alerts sound bell desktop activity status",
                SettingsSection::Hosts => "servers ssh tailscale tailnet endpoint remote control",
                SettingsSection::Updates => "upgrade version release",
                SettingsSection::About => "version build license licence credits",
            },
            Page::Sub(SubPage::Panes) => "splits padding space customize highlight fade hover",
            Page::Sub(SubPage::AdvancedFont) => "leading tracking kerning glyphs",
            Page::Sub(SubPage::Tabs) => "icons color programs width stretch customize",
            Page::Sub(SubPage::Status(status)) => match status {
                Status::Blocked => "blocked approval question sign-in waiting",
                Status::Error => "error failed",
                Status::Done => "done finished complete",
                Status::Working => "working busy progress",
            },
        }
    }
}

/// A page Back can return to, and how far down it was left.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Stop {
    page: Page,
    scroll: f32,
}

/// A setting row, as Search Settings knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Indexed {
    page: Page,
    /// The heading of the section the row's card is under; empty for a group with no name.
    heading: String,
    label: String,
    helper: String,
    /// An action's row on Shortcuts, listed after every setting found.
    action: bool,
}

/// One result of a search: a page, or a row on one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub page: Page,
    pub label: String,
    /// Where a row is (`Interface › Panes`), or the section a sub-page belongs to.
    pub place: String,
    /// The hit is a row of `page`, to mark once the page shows.
    pub row: bool,
}

/// The row a search landed on, marked until `until`.
#[derive(Clone, Debug)]
struct Found {
    page: Page,
    label: String,
    until: Instant,
}

/// Where Settings is, where it has been, and its search.
#[derive(Default)]
pub struct Nav {
    /// The page pushed over the selected section's own.
    sub: Option<SubPage>,
    stops: Vec<Stop>,
    /// The stop that is on screen, once `record` has run for it.
    at: usize,
    /// How far the page on screen is scrolled, as its viewport last told.
    offset: Rc<Cell<f32>>,
    pub search: TextEdit,
    /// Every page's rows, read when a search starts and dropped when it ends.
    index: Option<Vec<Indexed>>,
    hits: Vec<Hit>,
    selected: usize,
    found: Option<Found>,
}

impl Nav {
    /// Settings just opened: no history yet and no search, on the page it was left on.
    pub fn opened(&mut self) {
        self.stops.clear();
        self.at = 0;
        self.offset.set(0.);
        self.search = TextEdit::default();
        self.index = None;
        self.hits.clear();
        self.selected = 0;
        self.found = None;
    }

    /// A section's own page is shown: nothing is pushed over it.
    pub fn show_section(&mut self) {
        self.sub = None;
    }

    pub fn query(&self) -> &str {
        self.search.text.trim()
    }

    pub fn hits(&self) -> &[Hit] {
        &self.hits
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Light hit `index`, as the pointer over it does.
    pub fn select(&mut self, index: usize) {
        if index < self.hits.len() {
            self.selected = index;
        }
    }

    /// Move the lit hit down (1) or up (-1), around the ends. Returns the hit now lit.
    pub fn step_selected(&mut self, direction: isize) -> Option<usize> {
        let count = self.hits.len() as isize;
        if count == 0 {
            return None;
        }
        self.selected = (self.selected as isize + direction).rem_euclid(count) as usize;
        Some(self.selected)
    }

    /// The cell the page's viewport reports its scroll offset into.
    pub fn offset(&self) -> Rc<Cell<f32>> {
        self.offset.clone()
    }

    /// Whether `page` is the stop on screen.
    fn current(&self, page: Page) -> bool {
        self.stops
            .get(self.at)
            .is_some_and(|stop| stop.page == page)
    }

    /// Note `page`, the one on screen, and how far down it is, before leaving it. A page that
    /// was shown without a visit (Settings opening on a section) becomes a stop here, after
    /// the one Settings was on, and ends what Forward could reach.
    fn record(&mut self, page: Page) {
        let scroll = self.offset.get();
        if self.current(page) {
            self.stops[self.at].scroll = scroll;
            return;
        }
        if !self.stops.is_empty() {
            self.stops.truncate(self.at + 1);
        }
        self.stops.push(Stop { page, scroll });
        self.at = self.stops.len() - 1;
    }

    /// A visit to `page` from the recorded stop: what was ahead is forgotten.
    fn push(&mut self, page: Page) {
        self.stops.truncate(self.at + 1);
        self.stops.push(Stop { page, scroll: 0. });
        if self.stops.len() > STOPS {
            self.stops.remove(0);
        }
        self.at = self.stops.len() - 1;
        self.offset.set(0.);
    }

    /// The page Back would show from `page`, the one on screen.
    fn back_of(&self, page: Page) -> Option<Page> {
        let at = if self.current(page) {
            self.at.checked_sub(1)?
        } else {
            self.at
        };
        self.stops.get(at).map(|stop| stop.page)
    }

    /// Whether Back (-1) or Forward (1) has somewhere to go from `page`, the one on screen.
    fn can_go(&self, page: Page, direction: isize) -> bool {
        if !self.current(page) {
            // Shown without a visit: it would follow the stop Settings was on.
            return direction < 0 && !self.stops.is_empty();
        }
        self.at
            .checked_add_signed(direction)
            .is_some_and(|next| next < self.stops.len())
    }

    fn step(&mut self, direction: isize) -> Option<Stop> {
        let next = self.at.checked_add_signed(direction)?;
        let stop = *self.stops.get(next)?;
        self.at = next;
        self.offset.set(stop.scroll);
        Some(stop)
    }
}

thread_local! {
    /// What the page being built is built for.
    static PASS: RefCell<Pass> = const { RefCell::new(Pass::Draw(None)) };
}

enum Pass {
    /// To be drawn, with the label of the row to mark and whether it has been.
    Draw(Option<(String, bool)>),
    /// To be read for the search index: its rows so far, and where the rows under the next
    /// heading start.
    Read(Vec<(String, String, String)>, usize),
}

/// Every labelled row of a Settings page passes through here as it is built (`setting` and
/// `disclosure` in `settings_kit`): it is noted for the search index, or marked when it is
/// the row a search landed on.
pub fn row(label: &str, helper: &str, frame: El) -> El {
    PASS.with_borrow_mut(|pass| match pass {
        Pass::Read(rows, _) => {
            if !label.is_empty() {
                rows.push((String::new(), label.to_owned(), helper.to_owned()));
            }
            frame
        }
        Pass::Draw(Some((found, marked))) if !*marked && found.as_str() == label => {
            *marked = true;
            frame.child(mark())
        }
        Pass::Draw(_) => frame,
    })
}

/// A row Search Settings lists from somewhere other than the page's build (an action's row
/// on Shortcuts): marked when a search landed on it, and never read.
pub fn marked(label: &str, frame: El) -> El {
    PASS.with_borrow_mut(|pass| match pass {
        Pass::Draw(Some((found, marked))) if !*marked && found.as_str() == label => {
            *marked = true;
            frame.child(mark())
        }
        _ => frame,
    })
}

/// A section's heading, told once its rows are built: the rows since the last one are under it.
pub fn heading(title: &str) {
    PASS.with_borrow_mut(|pass| {
        if let Pass::Read(rows, open) = pass {
            for row in &mut rows[*open..] {
                row.0 = title.to_owned();
            }
            *open = rows.len();
        }
    });
}

/// The mark on the row a search landed on: `SELECTION`, the tint and its outline, inside the
/// row's edges. It fades in and out and takes no input.
fn mark() -> El {
    let (fill, outline) = selection(false);
    el().key("settings-found")
        .pin(Some(3.), Some(3.), Some(3.), Some(3.))
        .radius(CARD_RADIUS - 3.)
        .continuous_corners()
        .fill(fill)
        .border(1., outline)
        .inert()
        .transition(Transition::opacity().springs(SpringPair::new(SNAPPY, POP)))
}

/// `page`'s rows, by building it.
fn read(s: &Workspace, page: Page) -> Vec<Indexed> {
    PASS.set(Pass::Read(Vec::new(), 0));
    drop(super::settings_view::body(s, page));
    let Pass::Read(rows, _) = PASS.replace(Pass::Draw(None)) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    rows.into_iter()
        // A label a page repeats (one per host) is found once, at its first row.
        .filter(|(_, label, _)| seen.insert(label.clone()))
        .map(|(heading, label, helper)| Indexed {
            page,
            heading,
            label,
            helper,
            action: false,
        })
        .collect()
}

/// Every action of the Shortcuts page, by the name its row has, whatever its filter holds.
fn actions(s: &Workspace) -> Vec<Indexed> {
    let mut seen = std::collections::HashSet::new();
    s.shortcut_rows()
        .into_iter()
        .filter(|row| seen.insert(row.title.clone()))
        .map(|row| Indexed {
            page: Page::Section(SettingsSection::Shortcuts),
            heading: String::new(),
            label: row.title,
            helper: String::new(),
            action: true,
        })
        .collect()
}

/// `page`, built to be drawn, with the row a search landed on marked.
pub fn draw(s: &Workspace, page: Page) -> El {
    let found = s
        .settings_nav
        .found
        .as_ref()
        .filter(|found| found.page == page)
        .map(|found| (found.label.clone(), false));
    PASS.set(Pass::Draw(found));
    let body = super::settings_view::body(s, page);
    PASS.set(Pass::Draw(None));
    body
}

/// How well `query` (lower case; `words` are its parts) finds something named `label` that
/// also carries `more`: lower is better, None is no match.
fn rank(query: &str, words: &[&str], label: &str, more: &str) -> Option<u8> {
    let label = label.to_lowercase();
    let named: Vec<&str> = label
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    // The label begins with the query, or holds its words whole and in order: `font` finds
    // `Terminal font` as well as it finds `Font size`.
    if label.starts_with(query)
        || named
            .windows(words.len())
            .any(|run| run.iter().zip(words).all(|(word, typed)| word == typed))
    {
        return Some(0);
    }
    if named.iter().any(|word| word.starts_with(query)) {
        return Some(1);
    }
    if label.contains(query) {
        return Some(2);
    }
    if words.iter().all(|word| label.contains(word)) {
        return Some(3);
    }
    let more = more.to_lowercase();
    words
        .iter()
        .all(|word| label.contains(word) || more.contains(word))
        .then_some(4)
}

/// What `query` finds: pages by their title and keywords, rows by their label, helper and
/// heading, then the actions of Shortcuts by their name. Best match first, then in the
/// sidebar's order and each page's own. A page that a row of its section opens under the
/// page's own name is one hit, where the row is, that opens the page.
fn search(index: &[Indexed], query: &str) -> Vec<Hit> {
    let query = query.trim().to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    // Settings before actions, then by rank.
    let mut pages: Vec<((bool, u8), Hit)> = Vec::new();
    for page in Page::all() {
        if let Some(rank) = rank(&query, &words, page.title(), page.keywords()) {
            let place = match page {
                Page::Section(_) => String::new(),
                Page::Sub(sub) => sub.section().title().to_owned(),
            };
            pages.push((
                (false, rank),
                Hit {
                    page,
                    label: page.title().to_owned(),
                    place,
                    row: false,
                },
            ));
        }
    }
    let mut rows: Vec<((bool, u8), Hit)> = Vec::new();
    for found in index {
        let more = format!("{} {}", found.helper, found.heading);
        let Some(rank) = rank(&query, &words, &found.label, &more) else {
            continue;
        };
        let row = Hit {
            page: found.page,
            label: found.label.clone(),
            place: found.page.place(),
            row: true,
        };
        // The row that opens a page of its own name: the page's hit, in the row's place.
        let opened = pages.iter().position(|(_, page)| {
            page.page.sub().is_some() && page.label == row.label && page.place == row.place
        });
        rows.push(match opened {
            Some(at) => {
                let ((_, of_page), page) = pages.remove(at);
                ((found.action, rank.min(of_page)), page)
            }
            None => ((found.action, rank), row),
        });
    }
    // Stable: equal ranks keep pages before rows and the pages' order.
    pages.extend(rows);
    pages.sort_by_key(|(key, _)| *key);
    pages.into_iter().map(|(_, hit)| hit).take(HITS).collect()
}

impl Workspace {
    fn settings_open(&self) -> bool {
        self.panel
            .as_ref()
            .is_some_and(|p| matches!(p.kind, PanelKind::Settings))
    }

    /// The page Settings shows: the selected section's own, or the one pushed over it.
    pub fn settings_page(&self) -> Page {
        match self.settings_nav.sub {
            Some(sub) if sub.section() == self.settings_section => Page::Sub(sub),
            _ => Page::Section(self.settings_section),
        }
    }

    fn show_page(&mut self, page: Page) {
        self.open_section(page.section());
        self.settings_nav.sub = page.sub();
    }

    /// Go to `page` as the user does from the sidebar, a row or a search: a new stop in the
    /// history, opened at its top.
    pub fn navigate(&mut self, page: Page) {
        let from = self.settings_page();
        if page == from {
            return;
        }
        self.settings_nav.record(from);
        self.settings_nav.push(page);
        self.settings_nav.found = None;
        self.show_page(page);
    }

    /// Open the page a section's row leads to. The row goes with its page, so the keyboard
    /// goes to the way back.
    pub fn open_sub_page(&mut self, sub: SubPage, tx: &mut Tx) {
        self.navigate(Page::Sub(sub));
        tx.focus("settings-back", tx.focus_visible());
    }

    /// Escape, or the crumb, on a page opened from a section's row: back to that section,
    /// where it was scrolled when the row was pressed, or at its top when the page was
    /// reached another way (a search). False when Settings isn't on such a page (or isn't
    /// what is open).
    pub fn leave_settings_page(&mut self, tx: &mut Tx) -> bool {
        if !self.settings_open() {
            return false;
        }
        let Page::Sub(sub) = self.settings_page() else {
            return false;
        };
        let section = Page::Section(sub.section());
        if self.settings_nav.back_of(Page::Sub(sub)) == Some(section) {
            self.settings_history(-1, tx);
        } else {
            self.navigate(section);
        }
        true
    }

    /// Whether Back (-1) or Forward (1) has a page to show.
    pub fn settings_can_go(&self, direction: isize) -> bool {
        self.settings_nav.can_go(self.settings_page(), direction)
    }

    /// Back (-1) or Forward (1) through the pages visited, each where it was scrolled.
    /// Returns whether Settings is what is open, and so whether the step was Settings' to take.
    pub fn settings_history(&mut self, direction: isize, tx: &mut Tx) -> bool {
        if !self.settings_open() {
            return false;
        }
        let from = self.settings_page();
        if !self.settings_nav.can_go(from, direction) {
            return true;
        }
        self.settings_nav.record(from);
        if let Some(stop) = self.settings_nav.step(direction) {
            self.settings_nav.found = None;
            self.show_page(stop.page);
            tx.scroll_to_y(format!("settings-scroll:{}", stop.page.key()), stop.scroll);
        }
        true
    }

    /// The search field's text changed: find again, from the first hit.
    pub fn settings_search_changed(&mut self) {
        if self.settings_nav.query().is_empty() {
            self.settings_nav.index = None;
            self.settings_nav.hits.clear();
            self.settings_nav.selected = 0;
            return;
        }
        if self.settings_nav.index.is_none() {
            let mut index: Vec<Indexed> = Page::all().flat_map(|page| read(self, page)).collect();
            index.extend(actions(self));
            self.settings_nav.index = Some(index);
        }
        let nav = &mut self.settings_nav;
        nav.hits = search(
            nav.index.as_deref().unwrap_or_default(),
            nav.search.text.trim(),
        );
        nav.selected = 0;
    }

    /// End the search: the sidebar lists the sections again.
    pub fn clear_settings_search(&mut self) {
        let focused = self.settings_nav.search.focused;
        self.settings_nav.search = TextEdit::default();
        self.settings_nav.search.focused = focused;
        self.settings_search_changed();
    }

    /// Open hit `index` of the search: its page, and for a row, the row in view and marked.
    pub fn open_settings_hit(&mut self, index: usize, tx: &mut Tx) {
        let Some(hit) = self.settings_nav.hits.get(index).cloned() else {
            return;
        };
        self.settings_nav.selected = index;
        // An action's row is on the page only while the Shortcuts filter leaves it there.
        if hit.row && hit.page == Page::Section(SettingsSection::Shortcuts) {
            self.shortcut_filter.clear();
        }
        self.navigate(hit.page);
        self.settings_nav.found = hit.row.then(|| Found {
            page: hit.page,
            label: hit.label,
            until: Instant::now() + MARKED_FOR,
        });
        if hit.row {
            tx.scroll_center("settings-found");
            tx.wake_after(MARKED_FOR);
        }
    }

    /// Take the mark off the row a search landed on once its time is up.
    pub fn poll_settings(&mut self, now: Instant, tx: &mut Tx) -> bool {
        let Some(found) = &self.settings_nav.found else {
            return false;
        };
        if now < found.until && self.settings_open() {
            tx.wake_after(found.until - now);
            return false;
        }
        self.settings_nav.found = None;
        true
    }

    /// Alt Left and Alt Right step through Settings' history, wherever focus is in it.
    pub fn settings_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if !ev.mods.alt || ev.mods.ctrl || ev.mods.cmd || ev.mods.shift {
            return false;
        }
        let direction = match ev.key {
            Key::Left => -1,
            Key::Right => 1,
            _ => return false,
        };
        self.settings_history(direction, tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERFACE: Page = Page::Section(SettingsSection::Interface);
    const TERMINAL: Page = Page::Section(SettingsSection::Terminal);
    const ABOUT: Page = Page::Section(SettingsSection::About);
    const PANES: Page = Page::Sub(SubPage::Panes);

    fn visit(nav: &mut Nav, from: Page, to: Page, scrolled: f32) {
        nav.offset.set(scrolled);
        nav.record(from);
        nav.push(to);
    }

    #[test]
    fn history_branches_and_keeps_each_stops_scroll() {
        let mut nav = Nav::default();
        assert!(!nav.can_go(INTERFACE, -1) && !nav.can_go(INTERFACE, 1));
        visit(&mut nav, INTERFACE, PANES, 120.);
        visit(&mut nav, PANES, TERMINAL, 40.);
        assert!(nav.can_go(TERMINAL, -1) && !nav.can_go(TERMINAL, 1));

        nav.offset.set(300.);
        nav.record(TERMINAL);
        assert_eq!(
            nav.step(-1),
            Some(Stop {
                page: PANES,
                scroll: 40.
            })
        );
        assert_eq!(nav.offset.get(), 40., "the page comes back where it was");
        nav.record(PANES);
        assert_eq!(
            nav.step(-1).map(|s| (s.page, s.scroll)),
            Some((INTERFACE, 120.))
        );
        assert!(nav.step(-1).is_none());
        assert!(nav.can_go(INTERFACE, 1));
        nav.record(INTERFACE);
        assert_eq!(nav.step(1).map(|s| s.page), Some(PANES));
        nav.record(PANES);
        assert_eq!(
            nav.step(1).map(|s| (s.page, s.scroll)),
            Some((TERMINAL, 300.))
        );

        // A visit from the middle forgets what was ahead.
        nav.record(TERMINAL);
        nav.step(-1);
        visit(&mut nav, PANES, ABOUT, 0.);
        assert!(!nav.can_go(ABOUT, 1));
        assert_eq!(
            nav.stops.iter().map(|s| s.page).collect::<Vec<_>>(),
            [INTERFACE, PANES, ABOUT]
        );
    }

    #[test]
    fn a_page_shown_without_a_visit_becomes_a_stop_when_it_is_left() {
        let mut nav = Nav::default();
        // Settings opened on About: nothing to go back to.
        assert!(!nav.can_go(ABOUT, -1));
        visit(&mut nav, ABOUT, TERMINAL, 0.);
        assert_eq!(
            nav.stops.iter().map(|s| s.page).collect::<Vec<_>>(),
            [ABOUT, TERMINAL]
        );
        // Shown again without a visit while there is history: Back returns to where it was.
        assert!(nav.can_go(INTERFACE, -1) && !nav.can_go(INTERFACE, 1));
        nav.record(INTERFACE);
        assert_eq!(nav.step(-1).map(|s| s.page), Some(TERMINAL));

        nav.opened();
        assert!(nav.stops.is_empty() && !nav.can_go(TERMINAL, -1));
    }

    #[test]
    fn history_is_bounded() {
        let mut nav = Nav::default();
        for i in 0..200 {
            let (from, to) = if i % 2 == 0 {
                (INTERFACE, TERMINAL)
            } else {
                (TERMINAL, INTERFACE)
            };
            visit(&mut nav, from, to, 0.);
        }
        assert_eq!(nav.stops.len(), STOPS);
        assert_eq!(nav.at, STOPS - 1);
    }

    fn indexed(page: Page, heading: &str, label: &str, helper: &str) -> Indexed {
        Indexed {
            page,
            heading: heading.into(),
            label: label.into(),
            helper: helper.into(),
            action: false,
        }
    }

    #[test]
    fn search_ranks_labels_over_helpers_and_finds_pages_by_keyword() {
        let index = [
            indexed(
                INTERFACE,
                "Panes",
                "Pane density",
                "Compact joins panes, with no padding.",
            ),
            indexed(
                PANES,
                "Padding",
                "Pane padding",
                "Space around each terminal's content.",
            ),
            indexed(TERMINAL, "Typography", "Font size", "From 8 to 48 pt."),
        ];
        let labels = |query: &str| -> Vec<(String, String)> {
            search(&index, query)
                .into_iter()
                .map(|hit| (hit.label, hit.place))
                .collect()
        };
        // A word of the label first, then a row whose helper holds it.
        assert_eq!(
            labels("padding"),
            [
                ("Pane padding".to_owned(), "Interface › Panes".to_owned()),
                ("Panes".to_owned(), "Interface".to_owned()),
                ("Pane density".to_owned(), "Interface".to_owned()),
            ]
        );
        // A page by a keyword none of its rows holds.
        assert_eq!(
            labels("sound"),
            [("Notifications".to_owned(), String::new())]
        );
        // Case and spare spaces don't matter; every word must be found.
        assert_eq!(labels("  FONT  size ")[0].0, "Font size");
        assert!(labels("font zebra").is_empty());
        assert!(labels("   ").is_empty());
        // A heading finds the rows under it.
        assert_eq!(labels("typography")[0].0, "Font size");
        // A page's own title leads its rows.
        assert_eq!(
            labels("pane")[0],
            ("Panes".to_owned(), "Interface".to_owned())
        );
        let hit = &search(&index, "font size")[0];
        assert!(hit.row && hit.page == TERMINAL);
        assert!(!search(&index, "sound")[0].row);
    }

    #[test]
    fn a_whole_word_ranks_with_a_prefix_and_a_page_and_the_row_that_opens_it_are_one_hit() {
        const ADVANCED: Page = Page::Sub(SubPage::AdvancedFont);
        let mut split = indexed(
            Page::Section(SettingsSection::Shortcuts),
            "",
            "Split right",
            "",
        );
        split.action = true;
        let index = [
            indexed(TERMINAL, "Typography", "Terminal font", ""),
            indexed(TERMINAL, "Typography", "Font style", ""),
            indexed(TERMINAL, "Typography", "Font size", ""),
            indexed(TERMINAL, "Typography", "Advanced font settings", ""),
            indexed(ADVANCED, "Metrics", "Line height", ""),
            split,
            indexed(INTERFACE, "Panes", "Pane density", "Splits side by side."),
        ];
        let found = search(&index, "font");
        assert_eq!(
            found
                .iter()
                .map(|hit| hit.label.as_str())
                .collect::<Vec<_>>(),
            [
                "Terminal font",
                "Font style",
                "Font size",
                "Advanced font settings",
                // The section, by a keyword of its own.
                "Terminal"
            ],
            "in the page's order, the sub-page once"
        );
        // The one hit opens the page itself, and says where it is as the row would.
        let advanced = &found[3];
        assert_eq!(
            (advanced.page, advanced.place.as_str(), advanced.row),
            (ADVANCED, "Terminal", false)
        );
        // A keyword of the page alone still finds the page.
        assert_eq!(search(&index, "kerning")[0].page, ADVANCED);
        // An action is found by name, after every setting, however well it matches.
        let found = search(&index, "split");
        assert_eq!(
            found
                .iter()
                .map(|hit| (hit.label.as_str(), hit.place.as_str()))
                .collect::<Vec<_>>(),
            [
                ("Panes", "Interface"),
                ("Pane density", "Interface"),
                ("Split right", "Shortcuts")
            ]
        );
        assert!(found[2].row);
    }
}
