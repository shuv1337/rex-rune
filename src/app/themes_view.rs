//! Pictures of terminal themes, and the theme library on Settings → Themes.
//!
//! - [`theme_swatch`] is a theme as a card: its background, `Aa` in its text colour, a `~/`
//!   chip in its selection colours, its cursor, and its palette colours 0 to 7 as dots.
//!   [`swatch_strip`] is the same theme in the width of a word, for a row of the switcher,
//!   and a chrome palette the same way, for a row of the Window colours list.
//! - [`library`] lists every theme this client can declare as a grid of those cards under
//!   Default, Built in, Custom and Ghostty headings, with a search field over it. Only the
//!   rows near the viewport are built (`El::virtual_rows`), so a Ghostty install's six
//!   hundred themes cost what a screenful does.
//!
//! A picture is drawn from the theme's own colours (`TerminalTheme::swatch`) and never
//! touches a terminal: a theme still reaches panes only through the server, when its card is
//! chosen, exactly as the switcher declares it.
use super::{
    fold::{Name, fold},
    keycaps::keycap,
    settings_kit::{INSET, section},
    settings_nav::PAGE_INSET,
    theme::*,
    widgets::{glyph, matched},
    workspace::Workspace,
};
use rex_linux::{
    settings::{ChromeTheme, TerminalThemeChoice},
    terminal_theme::{BUILTIN_PREFIX, BUILTINS, Swatch, TerminalTheme, paired},
};
use rune::{
    Cursor, El, GrowthSpec, Key, KeyEvent, TextEdit, TextStyle, Tx, col, edge, el, memo, row, text,
};
use rune_kit::TextInput;
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    ops::Range,
    rc::Rc,
};

/// A theme card.
pub const CARD_WIDTH: f32 = 112.;
pub const CARD_HEIGHT: f32 = 88.;
const CARD_CORNER: f32 = 12.;
/// A palette dot on a card, and how far apart the dots start: each overlaps the one before.
const DOT: f32 = 14.;
const DOT_PITCH: f32 = 12.;
/// How far the hover and the selection reach past a card's edges.
const HALO: f32 = 5.;
/// The least room between two cards of a row.
const MIN_GAP: f32 = 16.;
/// A row of cards: the halo above, the card, its name and the room under it.
const ROW: f32 = HALO + CARD_HEIGHT + NAME_GAP + NAME + 12.;
const NAME_GAP: f32 = 9.;
const NAME: f32 = 16.;
/// A group's heading in the grid; the first sits right under the search field.
const HEADING: f32 = 34.;
const FIRST_HEADING: f32 = 26.;

/// What the strip at the end of a list row pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Picture {
    /// A terminal theme, in its own colours.
    Theme(Swatch),
    /// Follow server: the colours are whatever the server has, so there are none to show.
    Server,
    /// Window colours: a chrome palette.
    Chrome(ChromeTheme),
}

/// The line a theme's edge is drawn in: its background, a step towards its text.
fn edge_of(c: &Swatch) -> rune::Color {
    rgb(mix(c.foreground, c.background, 0.16))
}

/// Whether two colours are too alike to tell a dot of one from a ground of the other.
fn near(a: u32, b: u32) -> bool {
    [16, 8, 0]
        .into_iter()
        .all(|shift| ((a >> shift) & 0xff).abs_diff((b >> shift) & 0xff) < 20)
}

/// A terminal theme as a 112×88 pt card: `Aa` in its text colour, `~/` on its selection, its
/// cursor with a mark in the cursor-text colour, and palette colours 0 to 7 as overlapping
/// dots. A picture: it takes no input.
pub fn theme_swatch(c: &Swatch) -> El {
    let ground = rgb(c.background);
    let mut dots = row()
        .key("theme-dots")
        .absolute((CARD_WIDTH - (7. * DOT_PITCH + DOT)) / 2., 59.)
        .h(DOT);
    for (index, color) in c.ansi.iter().enumerate() {
        let mut dot = el()
            .key(format!("theme-dot:{index}"))
            .size(DOT, DOT)
            .ml(if index == 0 { 0. } else { DOT_PITCH - DOT })
            .radius(DOT / 2.)
            .fill(rgb(*color))
            // Each dot is cut out of the one before it, in the theme's background.
            .outline(1.5, 0., ground);
        if near(*color, c.background) {
            // A colour the background hides (black on a black theme) keeps its place.
            dot = dot.border(1., edge_of(c));
        }
        dots = dots.child(dot);
    }
    el().key("theme-swatch")
        .role("img")
        .size(CARD_WIDTH, CARD_HEIGHT)
        .radius(CARD_CORNER)
        .continuous_corners()
        .fill(ground)
        .border(1., edge_of(c))
        .child(
            row()
                .absolute(7., 7.)
                .size(CARD_WIDTH - 14., 44.)
                .pl(10.)
                .pr(8.)
                .gap(7.)
                .center_y()
                .radius(7.)
                .continuous_corners()
                .fill(rgb(mix(c.foreground, c.background, 0.06)))
                .child(
                    text("Aa")
                        .key("theme-text")
                        .font(TextStyle::new(20., 400.))
                        .color(rgb(c.foreground))
                        .grow(1.),
                )
                .child(
                    row()
                        .key("theme-selection")
                        .h(20.)
                        .px(5.)
                        .center()
                        .radius(4.)
                        .fill(rgb(c.selection_background))
                        .child(
                            text("~/")
                                .font(TextStyle::new(12., 560.).mono())
                                .color(rgb(c.selection_foreground)),
                        ),
                )
                .child(
                    el().key("theme-cursor")
                        .size(12., 28.)
                        .radius(5.)
                        .fill(rgb(c.cursor))
                        .child(
                            el().absolute(3., 21.)
                                .size(6., 2.)
                                .radius(1.)
                                .fill(rgb(c.cursor_text)),
                        ),
                ),
        )
        .child(dots)
}

/// Follow server's card: no colours to show, so the server's glyph on an outlined card.
fn server_card() -> El {
    el().key("theme-swatch")
        .role("img")
        .size(CARD_WIDTH, CARD_HEIGHT)
        .radius(CARD_CORNER)
        .continuous_corners()
        .fill(white(0.03))
        .border(1., white(0.12))
        .center()
        .child(glyph(&SERVER, 24., rgb(muted())))
}

/// The width a [`swatch_strip`] takes in a row.
pub const STRIP_WIDTH: f32 = 44.;

/// Colours in a list row: a capsule in the ground's colour holding four dots. A terminal
/// theme shows its text colour and its red, green and blue on its background; a chrome
/// palette its ink, its two accents and its live colour on its ground. Follow server, whose
/// colours are the server's, is the capsule's outline alone, so the strips of a list keep
/// one column.
pub fn swatch_strip(picture: &Picture) -> El {
    let strip = row()
        .key("theme-strip")
        .role("img")
        .size(STRIP_WIDTH, 14.)
        .gap(3.)
        .center()
        .radius(7.);
    let (ground, text, dots) = match picture {
        Picture::Server => return strip.border(1., white(0.14)),
        Picture::Theme(c) => (
            c.background,
            c.foreground,
            [c.foreground, c.ansi[1], c.ansi[2], c.ansi[4]],
        ),
        Picture::Chrome(chrome) => {
            let p = palette_of(*chrome);
            (p.ground, p.ink, [p.ink, p.accent, p.accent2, p.live])
        }
    };
    strip
        .fill(rgb(ground))
        .border(1., rgb(mix(text, ground, 0.16)))
        .children(dots.map(|color| el().size(6., 6.).radius(3.).fill(rgb(color))))
}

/// The library's own state: what its search field holds.
#[derive(Default)]
pub struct Library {
    pub search: TextEdit,
}

/// One card of the library.
#[derive(Hash)]
struct Item {
    /// The choice as it is saved (`named:Dracula`): names the card and its row.
    id: String,
    name: String,
    /// Its colours; none for Follow server, whose colours are the server's.
    swatch: Option<Swatch>,
    /// What the choice does, where its name doesn't say.
    help: String,
    /// The parts of the name the search matched.
    marks: Vec<Range<usize>>,
    current: bool,
}

impl Item {
    fn choice(&self) -> TerminalThemeChoice {
        TerminalThemeChoice::from(self.id.clone())
    }
}

enum Line {
    /// A group's name, and how many of its themes are listed.
    Heading(&'static str, usize),
    /// The cards of one row, as a range of the shelf's items.
    Cards(Range<usize>),
}

/// The library as it is laid out now: the themes the search leaves, in rows of `cols`.
struct Shelf {
    items: Vec<Item>,
    lines: Vec<Line>,
    /// The room between two cards.
    gap: f32,
    /// Every theme there is, whatever is typed.
    total: usize,
    /// What the rows are drawn in besides their items: the chrome palette.
    look: u64,
}

fn card_key(id: &str) -> String {
    format!("theme-card:{id}")
}

impl Shelf {
    fn line_key(&self, index: usize) -> String {
        match &self.lines[index] {
            Line::Heading(title, _) => format!("theme-group:{title}"),
            Line::Cards(cards) => format!("theme-row:{}", self.items[cards.start].id),
        }
    }

    /// The line item `index` is on, and its place in it.
    fn place(&self, index: usize) -> Option<(usize, usize)> {
        self.lines
            .iter()
            .enumerate()
            .find_map(|(line, cards)| match cards {
                Line::Cards(cards) if cards.contains(&index) => Some((line, index - cards.start)),
                _ => None,
            })
    }

    /// The nearest row of cards before (-1) or after (1) line `from`.
    fn cards_from(&self, from: usize, direction: isize) -> Option<&Range<usize>> {
        let mut at = from;
        loop {
            at = at.checked_add_signed(direction)?;
            match self.lines.get(at)? {
                Line::Cards(cards) => return Some(cards),
                Line::Heading(..) => {}
            }
        }
    }

    fn line(&self, index: usize) -> El {
        let key = self.line_key(index);
        match &self.lines[index] {
            Line::Heading(title, count) => row()
                .key(key)
                .w_full()
                .h(if index == 0 { FIRST_HEADING } else { HEADING })
                .px(INSET)
                .pb(8.)
                .gap(7.)
                .items_end()
                .child(
                    text(*title)
                        .role("heading")
                        .font(medium(13.))
                        .color(rgb(ink())),
                )
                .child(
                    text(count.to_string())
                        .font(ui(11.5))
                        .color(rgb(muted()))
                        .pb(0.5),
                ),
            Line::Cards(cards) => {
                let items = &self.items[cards.clone()];
                let mut version = DefaultHasher::new();
                (items, self.gap.to_bits(), self.look, cards.start == 0).hash(&mut version);
                memo(key.clone(), version.finish(), || {
                    row()
                        .key(key)
                        .w_full()
                        .h(ROW)
                        .px(INSET)
                        .pt(HALO)
                        .gap(self.gap)
                        .children(items.iter().enumerate().map(|(column, item)| {
                            // Tab enters the grid at its first card; the arrows go on from there.
                            cell(item, cards.start + column == 0, self.gap)
                        }))
                })
            }
        }
    }
}

/// One theme of the grid: its card, with the hover and the selection behind it, over its
/// name. The whole cell is the control; the keyboard's ring is drawn on the card.
fn cell(item: &Item, entry: bool, gap: f32) -> El {
    let (choice, id) = (item.choice(), item.id.clone());
    // A name may reach into the room beside its card, short of its neighbour's.
    let reach = ((gap - 8.) / 2.).clamp(0., 14.);
    let halo = el()
        .absolute(-HALO, -HALO)
        .size(CARD_WIDTH + 2. * HALO, CARD_HEIGHT + 2. * HALO)
        .radius(CARD_CORNER + HALO)
        .continuous_corners();
    let halo = if item.current {
        // `SELECTION`: the tint under its outline, around the card of the theme in use.
        let (fill, outline) = selection(false);
        halo.key("theme-current").fill(fill).border(1., outline)
    } else {
        halo.fill(white(0.07)).group_opacity(0., 1.)
    };
    let mut name = text(item.name.clone())
        .key("theme-name")
        .font(if item.current { medium(11.5) } else { ui(11.5) })
        .color(rgb(if item.current { ink() } else { muted() }))
        .truncate()
        .shrink()
        .help_when_truncated(item.name.clone());
    if !item.current {
        name = name.group_color(rgb(ink()));
    }
    if !item.marks.is_empty() {
        name = name.spans(matched(&item.marks, false));
    }
    let picture = match &item.swatch {
        Some(colours) => theme_swatch(colours),
        None => server_card(),
    };
    let mut cell = col()
        .key(card_key(&item.id))
        .role("radio")
        .label(item.name.clone())
        .checked(item.current)
        .w(CARD_WIDTH)
        .cursor(Cursor::Pointer)
        .hover_group()
        .focusable(true)
        .on_press(move |s: &mut Workspace| s.pick_theme(choice.clone()))
        .on_key(move |s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| s.library_key(&id, ev, tx))
        .child(
            el().size(CARD_WIDTH, CARD_HEIGHT).child(halo).child(
                picture
                    .radius(CARD_CORNER)
                    .focus_ring(2., 1., rgb(accent())),
            ),
        )
        .child(
            row()
                .w(CARD_WIDTH + 2. * reach)
                .h(NAME)
                .ml(-reach)
                .mt(NAME_GAP)
                .center()
                .child(name),
        );
    if !entry {
        cell = cell.tab_index(-1);
    }
    if !item.help.is_empty() {
        cell = cell.help(item.help.clone());
    }
    cell
}

/// How many cards a row holds in a page `width` wide, and the room between two of them: the
/// first card starts on the cards' text edge and the last ends on it.
fn columns(width: f32) -> (usize, f32) {
    let room = (width - 2. * INSET).max(CARD_WIDTH);
    let cols = (((room + MIN_GAP) / (CARD_WIDTH + MIN_GAP)).floor() as usize).max(1);
    let gap = if cols > 1 {
        (room - cols as f32 * CARD_WIDTH) / (cols - 1) as f32
    } else {
        0.
    };
    (cols, gap)
}

impl Library {
    pub fn query(&self) -> &str {
        self.search.text.trim()
    }
}

impl Workspace {
    /// `theme` as a picture shows it in this window: what it leaves out is the chrome's own.
    pub(super) fn swatch(&self, theme: &TerminalTheme) -> Swatch {
        let chrome = palette_of(self.settings.chrome_theme);
        theme.swatch(chrome.card, chrome.terminal_selection)
    }

    /// The library for what its search field holds, laid out for the Settings page as wide
    /// as it is now.
    fn shelf(&self) -> Shelf {
        let page = super::settings_view::page_width(self.viewport[0] / self.ui_scale());
        let (cols, gap) = columns(page - 2. * PAGE_INSET);
        let query = fold(self.theme_library.query());
        let current = &self.settings.terminal_theme;
        let mut total = 0;
        let mut groups: Vec<(&'static str, Vec<Item>)> = ["Default", "Built in", "Custom"]
            .into_iter()
            .chain(["Ghostty dark", "Ghostty light"])
            .map(|title| (title, Vec::new()))
            .collect();
        let mut add =
            |group: usize, name: &str, help: &str, choice: TerminalThemeChoice, swatch| {
                total += 1;
                if let Some(marks) = Name::new(name).find(&query, false) {
                    groups[group].1.push(Item {
                        current: *current == choice,
                        id: String::from(choice),
                        name: name.to_owned(),
                        swatch,
                        help: help.to_owned(),
                        marks,
                    });
                }
            };
        add(
            0,
            "Follow server",
            if self.settings.terminal_theme_declared {
                "Resets panes to Rex's own colours."
            } else {
                "Declares nothing: panes show the colours the server has."
            },
            TerminalThemeChoice::FollowServer,
            None,
        );
        add(
            0,
            "Rex default colours",
            "Rex's own text and palette, on this window's background.",
            TerminalThemeChoice::RexDefault,
            Some(self.swatch(&TerminalTheme::default())),
        );
        let chrome = paired(self.settings.chrome_theme);
        add(
            0,
            "Follow chrome",
            &format!("The theme of the window colours: {}.", chrome.name),
            TerminalThemeChoice::FollowChrome,
            Some(self.swatch(&(chrome.theme)())),
        );
        for builtin in BUILTINS {
            add(
                1,
                builtin.name,
                "",
                TerminalThemeChoice::Named(format!("{BUILTIN_PREFIX}{}", builtin.id)),
                Some(self.swatch(&(builtin.theme)())),
            );
        }
        for theme in &self.themes {
            let group = match (theme.custom, theme.light) {
                (true, _) => 2,
                (false, false) => 3,
                (false, true) => 4,
            };
            add(
                group,
                &theme.name,
                "",
                TerminalThemeChoice::Named(theme.name.clone()),
                Some(self.swatch(&theme.theme)),
            );
        }
        let (mut items, mut lines) = (Vec::new(), Vec::new());
        for (title, group) in groups {
            if group.is_empty() {
                continue;
            }
            lines.push(Line::Heading(title, group.len()));
            let start = items.len();
            items.extend(group);
            let mut at = start;
            while at < items.len() {
                let end = (at + cols).min(items.len());
                lines.push(Line::Cards(at..end));
                at = end;
            }
        }
        let mut look = DefaultHasher::new();
        self.settings.chrome_theme.hash(&mut look);
        Shelf {
            items,
            lines,
            gap,
            total,
            look: look.finish(),
        }
    }

    /// The Themes page is about to show: list the themes installed now.
    pub(super) fn show_theme_library(&mut self) {
        self.read_themes();
    }

    /// Declare the theme of the card that was chosen, as the switcher does. Settings stays
    /// where it is.
    fn pick_theme(&mut self, choice: TerminalThemeChoice) {
        if let Err(error) = self.apply_theme(choice) {
            self.notify(format!("{error:#}"));
        }
    }

    /// Keys on the card `id`: the arrows move among the cards (Up from the first row goes to
    /// the search field), Return chooses as Space does, and a letter starts a search.
    fn library_key(&mut self, id: &str, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if ev.mods.ctrl || ev.mods.alt || ev.mods.cmd {
            return false;
        }
        let shelf = self.shelf();
        let Some(index) = shelf.items.iter().position(|item| item.id == id) else {
            return false;
        };
        let Some((line, column)) = shelf.place(index) else {
            return false;
        };
        let target = match &ev.key {
            Key::Left => index.checked_sub(1),
            Key::Right => Some(index + 1).filter(|next| *next < shelf.items.len()),
            Key::Up | Key::Down => {
                let direction = if ev.key == Key::Up { -1 } else { 1 };
                match shelf.cards_from(line, direction) {
                    // A shorter row is entered at its last card.
                    Some(cards) => Some((cards.start + column).min(cards.end - 1)),
                    None if direction < 0 => {
                        tx.scroll_into_view("theme-search-box");
                        tx.focus("theme-search", true);
                        return true;
                    }
                    None => None,
                }
            }
            Key::Enter => {
                self.pick_theme(shelf.items[index].choice());
                return true;
            }
            Key::Escape if !self.theme_library.query().is_empty() => {
                self.search_themes("", tx);
                return true;
            }
            Key::Text(typed) if !typed.trim().is_empty() => {
                self.search_themes(typed, tx);
                return true;
            }
            _ => return false,
        };
        if let Some(target) = target
            && let Some((line, _)) = shelf.place(target)
        {
            tx.scroll_into_view(shelf.line_key(line));
            tx.focus(card_key(&shelf.items[target].id), true);
        }
        // An arrow at an end of the grid stays put, and never scrolls the page instead.
        true
    }

    /// Put `typed` in the library's search field and the keyboard with it.
    fn search_themes(&mut self, typed: &str, tx: &mut Tx) {
        self.theme_library.search = TextEdit::new(typed);
        self.theme_library.search.focused = true;
        tx.scroll_into_view("theme-search-box");
        tx.instant();
    }
}

/// Keys in the library's search field: Escape empties it (and closes Settings only when it
/// is empty already), Down and Return go to the first card found.
fn search_key(s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.settings_key(ev, tx) || super::input::global_key(s, ev, tx) {
        return true;
    }
    match ev.key {
        Key::Escape if !s.theme_library.search.text.is_empty() => {
            s.theme_library.search = TextEdit::default();
            s.theme_library.search.focused = true;
            tx.instant();
        }
        Key::Down | Key::Enter => {
            let shelf = s.shelf();
            let Some(first) = shelf.items.first() else {
                return true;
            };
            tx.scroll_into_view(shelf.line_key(0));
            tx.focus(card_key(&first.id), true);
        }
        _ => return false,
    }
    true
}

fn search_field(s: &Workspace, shelf: &Shelf) -> El {
    let library = &s.theme_library;
    let searching = !library.query().is_empty();
    let count = if searching {
        format!("{} of {}", shelf.items.len(), shelf.total)
    } else {
        format!("{} themes", shelf.total)
    };
    let mut trailing = row().gap(8.).center_y().child(
        text(count)
            .key("theme-count")
            .font(ui(11.5))
            .color(rgb(muted())),
    );
    if searching {
        trailing = trailing.child(keycap("Esc", false));
    }
    let input = TextInput::new(&library.search)
        .placeholder("Search terminal themes…")
        .label("Search terminal themes")
        .leading_icon(&SEARCH)
        .leading_icon_size(14.)
        .text_style(ui(13.))
        .height(30.)
        .plain()
        .on_edit(|s: &mut Workspace| &mut s.theme_library.search)
        // The grid is replaced by what was found, at once: nothing slides.
        .on_change(|_: &mut Workspace, tx: &mut Tx| tx.instant())
        .on_key(search_key)
        .trailing(trailing)
        .width(None);
    row()
        .key("theme-search-box")
        .w_full()
        .h(30.)
        .center_y()
        .radius(8.)
        .continuous_corners()
        .fill(white(0.055))
        .focus_within_ring(1.5, 0., rgb(accent()).alpha(0.7))
        .child(input.build(kit()).key("theme-search").w_full())
}

/// The theme library: a search field over every theme this client can declare, as cards in
/// groups. The card of the theme in use is marked; choosing another declares it at once.
pub fn library(s: &Workspace) -> El {
    let shelf = Rc::new(s.shelf());
    let field = search_field(s, &shelf);
    if shelf.items.is_empty() {
        return section(
            "",
            [
                field,
                text(format!("No theme is named “{}”.", s.theme_library.query()))
                    .key("theme-none")
                    .font(ui(13.))
                    .color(rgb(muted()))
                    .wrap()
                    .px(INSET)
                    .py(18.),
            ],
        );
    }
    let (keys, rows) = (shelf.clone(), shelf.clone());
    let grid = col()
        .key("theme-grid")
        .role("radiogroup")
        .label("Terminal themes")
        .w_full()
        // Rows never ease or fade in: the grid is a still list.
        .growth(GrowthSpec {
            spring: STILL,
            edge: edge::NO_MASK,
        })
        .virtual_rows(
            shelf.lines.len(),
            ROW,
            move |_: &Workspace, index| keys.line_key(index).into(),
            move |_: &Workspace, index| rows.line(index),
        );
    section("", [field, grid])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_hold_as_many_cards_as_fit_with_the_ends_on_the_text_edge() {
        // The Settings page at its full size: 816 - 204 - 1, less its 20 pt insets.
        let (cols, gap) = columns(571.);
        assert_eq!(cols, 4);
        assert!((4. * CARD_WIDTH + 3. * gap - (571. - 2. * INSET)).abs() < 0.01);
        assert!(gap >= MIN_GAP);
        // A narrower dialog drops a column before cards touch; never fewer than one.
        assert_eq!(columns(400.).0, 3);
        assert_eq!(columns(300.).0, 2);
        assert_eq!(columns(140.), (1, 0.));
        assert_eq!(columns(40.), (1, 0.));
    }
}
