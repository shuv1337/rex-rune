//! The theme library on Settings → Themes (its cards, search, keys and the rows it builds)
//! and the strips of colour in the switcher's rows.
use super::*;
use crate::app::{
    panel::SettingsSection,
    theme::rgb,
    themes_view::{CARD_HEIGHT, CARD_WIDTH, Picture},
};
use rex_linux::settings::ChromeTheme;
use rune::ElementInfo;

const MOCHA: &str = "\
background = #1e1e2e
foreground = #cdd6f4
cursor-color = #f5e0dc
cursor-text = #11111b
selection-background = #585b70
selection-foreground = #cdd6f4
palette = 0=#45475a
palette = 1=#f38ba8
palette = 2=#a6e3a1
palette = 3=#f9e2af
palette = 4=#89b4fa
palette = 5=#f5c2e7
palette = 6=#94e2d5
palette = 7=#bac2de
";

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..40 {
        rt.frame(1. / 60.);
    }
}

/// A user and a Ghostty theme directory under one root, removed by the caller.
fn dirs(tag: &str) -> (std::path::PathBuf, ThemeDirs) {
    let root = std::env::temp_dir().join(format!("rex-linux-{tag}-{}", std::process::id()));
    let (user, system) = (root.join("user"), root.join("system"));
    std::fs::create_dir_all(&user).unwrap();
    std::fs::create_dir_all(&system).unwrap();
    let dirs = ThemeDirs {
        user: Some(user),
        system: vec![system],
    };
    (root, dirs)
}

fn write(dirs: &ThemeDirs, custom: bool, name: &str, text: &str) {
    let dir = if custom {
        dirs.user.as_ref().unwrap()
    } else {
        &dirs.system[0]
    };
    std::fs::write(dir.join(name), text).unwrap();
}

/// Settings on Themes, at rest, listing the themes in `dirs`.
fn library(dirs: ThemeDirs, hosts: usize, size: (f32, f32)) -> Runtime<Workspace> {
    let mut ws = workspace_hosts(hosts);
    ws.theme_dirs = dirs;
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(SettingsSection::Themes);
    let mut rt = runtime_at(ws, size);
    settle(&mut rt);
    rt
}

fn find(rt: &Runtime<Workspace>, key: &str) -> Option<ElementInfo> {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
}

fn rect(rt: &Runtime<Workspace>, key: &str) -> [f32; 4] {
    find(rt, key)
        .unwrap_or_else(|| panic!("missing {key}"))
        .rect
}

/// The elements inside the one keyed `key`, itself first.
fn inside(rt: &Runtime<Workspace>, key: &str) -> Vec<ElementInfo> {
    let elements = rt.elements();
    let Some(root) = elements
        .iter()
        .position(|e| !e.exiting && e.key.as_deref() == Some(key))
    else {
        return Vec::new();
    };
    let mut kept = vec![root];
    for (index, e) in elements.iter().enumerate().skip(root + 1) {
        if !e.exiting && e.parent.is_some_and(|parent| kept.contains(&parent)) {
            kept.push(index);
        }
    }
    kept.into_iter().map(|i| elements[i].clone()).collect()
}

fn part<'a>(parts: &'a [ElementInfo], key: &str) -> &'a ElementInfo {
    parts
        .iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("the card has no {key}"))
}

/// The ids of the cards that are built, in the order they are drawn.
fn cards(rt: &Runtime<Workspace>) -> Vec<String> {
    rt.elements()
        .into_iter()
        .filter(|e| !e.exiting)
        .filter_map(|e| e.key?.strip_prefix("theme-card:").map(str::to_owned))
        .collect()
}

/// The grid's headings as drawn: a group's name and how many themes it lists.
fn headings(rt: &Runtime<Workspace>) -> Vec<String> {
    rt.elements()
        .into_iter()
        .filter(|e| {
            !e.exiting
                && e.key
                    .as_deref()
                    .is_some_and(|key| key.starts_with("theme-group:"))
        })
        .map(|e| element_text(rt, e.key.as_deref().unwrap()).join(" "))
        .collect()
}

fn declared(host: &flume::Receiver<HostCommand>) -> Vec<Option<rexrs::api::terminal::SetTheme>> {
    host.try_iter()
        .filter_map(|c| match c {
            HostCommand::Theme(args) => Some(args),
            _ => None,
        })
        .collect()
}

fn type_text(rt: &mut Runtime<Workspace>, words: &str) {
    for c in words.chars() {
        press(rt, Key::Text(c.to_string()));
    }
}

fn focused_key(rt: &Runtime<Workspace>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused)?.key
}

/// Whether the keyboard is in the library's search field.
fn searching(rt: &Runtime<Workspace>) -> bool {
    focused(rt).is_some_and(|path| path.contains("theme-search"))
}

/// Bring the card of theme `id` into view and press it.
fn choose(rt: &mut Runtime<Workspace>, id: &str) {
    let key = format!("theme-card:{id}");
    rt.update(|_, tx| {
        tx.scroll_into_view(key.clone());
        tx.instant();
    });
    settle(rt);
    click(rt, &key);
}

#[test]
fn a_card_shows_its_themes_text_selection_cursor_and_eight_palette_colours() {
    let (root, dirs) = dirs("library-card");
    write(&dirs, false, "Mocha", MOCHA);
    let rt = library(dirs, 1, (1440., 1000.));

    let card = inside(&rt, "theme-card:named:Mocha");
    let face = part(&card, "theme-swatch");
    assert_eq!(
        [face.rect[2], face.rect[3]],
        [CARD_WIDTH, CARD_HEIGHT],
        "112×88"
    );
    assert_eq!(face.fill, rgb(0x1e1e2e), "on the theme's background");
    let words = part(&card, "theme-text");
    assert_eq!(words.text.as_deref(), Some("Aa"));
    assert_eq!(
        words.text_color,
        Some(rgb(0xcdd6f4)),
        "in the theme's text colour"
    );
    let chip = part(&card, "theme-selection");
    assert_eq!(chip.fill, rgb(0x585b70), "the selection's background");
    let home = card
        .iter()
        .find(|e| e.text.as_deref() == Some("~/"))
        .expect("the chip's words");
    assert_eq!(home.text_color, Some(rgb(0xcdd6f4)), "and its text");
    assert_eq!(part(&card, "theme-cursor").fill, rgb(0xf5e0dc));
    let dots: Vec<_> = (0..8)
        .map(|index| part(&card, &format!("theme-dot:{index}")).clone())
        .collect();
    assert_eq!(
        dots.iter().map(|dot| dot.fill).collect::<Vec<_>>(),
        [
            0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de
        ]
        .map(rgb),
        "palette colours 0 to 7, in order"
    );
    // Every part is inside the card, and each dot overlaps the one before it.
    for e in card.iter().filter(|e| {
        e.key.as_deref().is_some_and(|k| {
            k.starts_with("theme-") && !k.starts_with("theme-card:") && k != "theme-name"
        })
    }) {
        assert!(
            e.rect[0] >= face.rect[0] - 0.5
                && e.rect[1] >= face.rect[1] - 0.5
                && e.rect[0] + e.rect[2] <= face.rect[0] + face.rect[2] + 0.5
                && e.rect[1] + e.rect[3] <= face.rect[1] + face.rect[3] + 0.5,
            "{:?} is inside the card",
            e.key
        );
    }
    for pair in dots.windows(2) {
        assert!(pair[1].rect[0] < pair[0].rect[0] + pair[0].rect[2]);
    }
    assert_eq!(part(&card, "theme-name").text.as_deref(), Some("Mocha"));

    // A theme that sets its background only is drawn as a pane shows it: Rex's own text
    // and palette. Follow server has no colours to show, and no dots.
    let midnight = inside(&rt, "theme-card:named:rex/midnight");
    assert_eq!(part(&midnight, "theme-swatch").fill, rgb(0x001528));
    assert_eq!(
        part(&midnight, "theme-text").text_color,
        Some(rgb(0xffffff))
    );
    let server = inside(&rt, "theme-card:follow_server");
    assert!(
        !server
            .iter()
            .any(|e| e.key.as_deref().is_some_and(|k| k.starts_with("theme-dot")))
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn the_library_groups_every_theme_and_marks_the_one_in_use() {
    let (root, dirs) = dirs("library-groups");
    write(&dirs, true, "Mine", "background = #101418\n");
    write(&dirs, false, "Mocha", MOCHA);
    write(&dirs, false, "Abyss", "background = #000c18\n");
    write(
        &dirs,
        false,
        "Paper",
        "background = #f2eede\nforeground = #222222\n",
    );
    let rt = library(dirs, 1, (1440., 1000.));

    assert_eq!(
        headings(&rt),
        [
            "Default 3",
            "Built in dark 17",
            "Built in light 10",
            "Custom 1",
            "Ghostty dark 2",
            "Ghostty light 1"
        ]
    );
    // The cards of every group, in order: the built-in themes dark first, Rex's own first
    // of each.
    let mut listed: Vec<String> = ["follow_server", "builtin_default", "follow_chrome"]
        .map(str::to_owned)
        .into();
    listed.extend(
        rex_linux::terminal_theme::BUILTINS
            .iter()
            .map(|builtin| format!("named:{}", builtin.choice_id())),
    );
    listed.extend(["named:Mine", "named:Abyss", "named:Mocha", "named:Paper"].map(str::to_owned));
    assert_eq!(cards(&rt), listed);
    assert_eq!(
        listed[3..8],
        [
            "named:rex/merino-dark",
            "named:rex/midnight",
            "named:rex/rose-pine",
            "named:rex/rose-pine-moon",
            "named:rex/gruvbox-dark",
        ]
    );
    assert_eq!(listed[20], "named:rex/merino-light", "the first light one");
    assert_eq!(element_text(&rt, "theme-count"), ["34 themes"]);
    // Four cards to a row at the dialog's full width: the first on the cards' text edge
    // and the fourth ending on it.
    let search = rect(&rt, "theme-search-box");
    let first = rect(&rt, "theme-card:named:rex/merino-dark");
    let last = rect(&rt, "theme-card:named:rex/rose-pine-moon");
    assert_eq!(first[1], last[1], "one row");
    assert!((first[0] - (search[0] + 12.)).abs() < 0.5);
    assert!((last[0] + last[2] - (search[0] + search[2] - 12.)).abs() < 0.5);

    // Follow server is the default, and the only card that is marked.
    let marked = |rt: &Runtime<Workspace>| -> Vec<String> {
        cards(rt)
            .into_iter()
            .filter(|id| {
                inside(rt, &format!("theme-card:{id}"))
                    .iter()
                    .any(|e| e.key.as_deref() == Some("theme-current"))
            })
            .collect()
    };
    assert_eq!(marked(&rt), ["follow_server"]);
    // `SELECTION`: the tint under its outline, around the card and clear of it.
    let (tint, _) = crate::app::theme::selection(false);
    let server = inside(&rt, "theme-card:follow_server");
    let (halo, face) = (
        part(&server, "theme-current"),
        part(&server, "theme-swatch"),
    );
    assert_eq!(halo.fill, tint);
    assert!(halo.rect[0] < face.rect[0] - 3. && halo.rect[1] < face.rect[1] - 3.);
    assert_eq!(
        halo.rect[2],
        face.rect[2] + 2. * (face.rect[0] - halo.rect[0])
    );

    // A narrow window drops a column before cards touch.
    let narrow = {
        let (_, dirs) = self::dirs("library-groups");
        library(dirs, 1, (700., 700.))
    };
    let first = rect(&narrow, "theme-card:named:rex/merino-dark");
    let fourth = rect(&narrow, "theme-card:named:rex/rose-pine-moon");
    assert!(
        fourth[1] > first[1],
        "the fourth card wraps to a second row"
    );
    assert!((fourth[0] - first[0]).abs() < 0.5);
    std::fs::remove_file(&narrow.state.settings_path).ok();
    std::fs::remove_file(&rt.state.settings_path).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn choosing_a_card_declares_its_theme_to_every_host_and_stays_in_settings() {
    let (root, dirs) = dirs("library-choose");
    write(&dirs, false, "Mocha", MOCHA);
    write(&dirs, false, "Gone", "background = #202020\n");
    let gone = dirs.system[0].join("Gone");
    let mut ws = workspace_hosts(2);
    ws.theme_dirs = dirs;
    let hosts = [capture_host(&mut ws, 0), capture_host(&mut ws, 1)];
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    let mut rt = runtime_at(ws, (1440., 1000.));
    click(&mut rt, "settings-section:themes");
    settle(&mut rt);
    assert!(
        hosts.iter().all(|host| host.is_empty()),
        "looking declares nothing"
    );

    choose(&mut rt, "named:Mocha");
    assert!(
        matches!(
            rt.state.panel.as_ref().map(|p| &p.kind),
            Some(PanelKind::Settings)
        ),
        "Settings stays open on the library"
    );
    assert_eq!(rt.state.settings_section, SettingsSection::Themes);
    let theme = rt.state.term_theme.clone().expect("a declared theme");
    assert_eq!(theme.background, Some(0x1e1e2e));
    for host in &hosts {
        assert_eq!(
            declared(host),
            [Some(theme.args())],
            "every host is told once"
        );
    }
    let saved = Settings::load(&rt.state.settings_path).unwrap();
    assert_eq!(
        saved.terminal_theme,
        TerminalThemeChoice::Named("Mocha".into())
    );
    assert!(saved.terminal_theme_chosen);
    // The mark moved to the chosen card, and the row above names it.
    let marked = |rt: &Runtime<Workspace>, id: &str| {
        inside(rt, &format!("theme-card:{id}"))
            .iter()
            .any(|e| e.key.as_deref() == Some("theme-current"))
    };
    assert!(marked(&rt, "named:Mocha") && !marked(&rt, "follow_server"));
    assert_eq!(
        find(&rt, "terminal-theme").unwrap().label.as_deref(),
        Some("Mocha")
    );

    // A theme whose file went away since the page listed it: said once, nothing changes.
    std::fs::remove_file(gone).unwrap();
    let notices = rt.state.notices.len();
    choose(&mut rt, "named:Gone");
    assert_eq!(rt.state.notices.len(), notices + 1);
    assert_eq!(
        rt.state.settings.terminal_theme,
        TerminalThemeChoice::Named("Mocha".into())
    );
    assert!(hosts.iter().all(|host| declared(host).is_empty()));

    // Follow server stops declaring, as it does from the switcher.
    choose(&mut rt, "follow_server");
    for host in &hosts {
        assert_eq!(declared(host), [None], "Follow server stops declaring");
    }
    assert_eq!(rt.state.term_theme, None);
    std::fs::remove_file(&rt.state.settings_path).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn the_library_search_folds_accents_marks_matches_and_says_when_nothing_is_found() {
    let (root, dirs) = dirs("library-search");
    for name in ["Rose Pine", "Catppuccin Mocha", "Tokyo Night Moon"] {
        write(&dirs, false, name, "background = #1e1e2e\n");
    }
    write(&dirs, false, "Rose Pine Dawn", "background = #faf4ed\n");
    let mut rt = library(dirs, 1, (1440., 1000.));
    click(&mut rt, "theme-search");
    type_text(&mut rt, "rose");
    assert_eq!(
        cards(&rt),
        [
            "named:rex/rose-pine",
            "named:rex/rose-pine-moon",
            "named:rex/rose-pine-dawn",
            "named:Rose Pine",
            "named:Rose Pine Dawn"
        ],
        "the built-in Rosé Pine beside Ghostty's Rose Pine"
    );
    assert_eq!(
        headings(&rt),
        [
            "Built in dark 2",
            "Built in light 1",
            "Ghostty dark 1",
            "Ghostty light 1"
        ],
        "a group with nothing found has no heading"
    );
    assert_eq!(element_text(&rt, "theme-count"), ["5 of 34"]);
    // Words in any order, every one of them in the name.
    type_text(&mut rt, " moon");
    assert_eq!(cards(&rt), ["named:rex/rose-pine-moon"]);
    assert_eq!(rt.state.theme_library.query(), "rose moon");

    // Down goes to the first card found, and Return chooses it.
    press(&mut rt, Key::Down);
    assert_eq!(
        focused_key(&rt).as_deref(),
        Some("theme-card:named:rex/rose-pine-moon")
    );
    press(&mut rt, Key::Enter);
    assert_eq!(
        rt.state.settings.terminal_theme,
        TerminalThemeChoice::Named("rex/rose-pine-moon".into())
    );
    // Escape on a card ends the search and leaves Settings open.
    press(&mut rt, Key::Escape);
    assert!(rt.state.theme_library.query().is_empty());
    assert!(rt.state.panel.is_some());
    assert_eq!(element_text(&rt, "theme-count"), ["34 themes"]);

    type_text(&mut rt, "zzz");
    assert!(cards(&rt).is_empty() && find(&rt, "theme-grid").is_none());
    assert_eq!(
        element_text(&rt, "theme-none"),
        ["No theme is named “zzz”."]
    );
    assert_eq!(element_text(&rt, "theme-count"), ["0 of 34"]);
    // Escape in the field empties it first, and closes Settings only when it is empty.
    press(&mut rt, Key::Escape);
    assert!(rt.state.theme_library.query().is_empty() && rt.state.panel.is_some());
    assert!(searching(&rt));
    press(&mut rt, Key::Escape);
    assert!(rt.state.panel.is_none());

    // Opened again, the library is not searched any more.
    rt.update(|s, tx| s.perform(Intent::Settings, tx));
    settle(&mut rt);
    assert_eq!(rt.state.settings_section, SettingsSection::Themes);
    assert_eq!(element_text(&rt, "theme-count"), ["34 themes"]);
    assert_eq!(cards(&rt)[0], "follow_server");
    std::fs::remove_file(&rt.state.settings_path).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn the_arrows_move_among_the_cards_and_a_letter_starts_a_search() {
    let (root, dirs) = dirs("library-keys");
    write(&dirs, false, "Mocha", MOCHA);
    write(&dirs, false, "Abyss", "background = #000c18\n");
    let mut rt = library(dirs, 1, (1440., 1000.));
    let at = |rt: &Runtime<Workspace>| {
        focused_key(rt)
            .and_then(|key| key.strip_prefix("theme-card:").map(str::to_owned))
            .unwrap_or_default()
    };
    // Tab reaches the grid once, at its first card; the other cards are not Tab stops.
    click(&mut rt, "theme-search");
    press(&mut rt, Key::Tab);
    assert_eq!(at(&rt), "follow_server");
    press(&mut rt, Key::Tab);
    assert!(
        !focused_key(&rt).is_some_and(|key| key.starts_with("theme-card:")),
        "Tab leaves the grid"
    );
    // From the search field, Down or Return goes to the first card.
    click(&mut rt, "theme-search");
    press(&mut rt, Key::Enter);
    assert_eq!(at(&rt), "follow_server");
    assert_eq!(
        rt.state.settings.terminal_theme,
        TerminalThemeChoice::FollowServer
    );
    click(&mut rt, "theme-search");
    press(&mut rt, Key::Down);
    assert_eq!(at(&rt), "follow_server");

    // Right and Left walk the cards in order, through the groups; an end stays put.
    press(&mut rt, Key::Left);
    assert_eq!(at(&rt), "follow_server");
    for expected in [
        "builtin_default",
        "follow_chrome",
        "named:rex/merino-dark",
        "named:rex/midnight",
    ] {
        press(&mut rt, Key::Right);
        assert_eq!(at(&rt), expected);
    }
    // Down and Up keep the column, through the groups, and enter a shorter row at its last
    // card: five rows of dark built-in themes, the last of one card, three rows of light
    // ones, then Ghostty's two.
    press(&mut rt, Key::Down);
    assert_eq!(
        at(&rt),
        "named:rex/ayu-dark",
        "the second card of the next row"
    );
    for _ in 0..3 {
        press(&mut rt, Key::Down);
    }
    assert_eq!(
        at(&rt),
        "named:rex/solarized-dark",
        "the one card of the last dark row"
    );
    for _ in 0..4 {
        press(&mut rt, Key::Down);
    }
    assert_eq!(at(&rt), "named:Abyss", "the first card of Ghostty dark");
    press(&mut rt, Key::Down);
    assert_eq!(at(&rt), "named:Abyss", "the last row");
    for _ in 0..8 {
        press(&mut rt, Key::Up);
    }
    assert_eq!(at(&rt), "named:rex/merino-dark");
    for _ in 0..3 {
        press(&mut rt, Key::Right);
    }
    assert_eq!(at(&rt), "named:rex/rose-pine-moon");
    press(&mut rt, Key::Up);
    assert_eq!(at(&rt), "follow_chrome", "Default has three cards");
    assert_eq!(
        rt.state.settings.terminal_theme,
        TerminalThemeChoice::FollowServer,
        "moving chooses nothing"
    );
    // Return chooses, as Space does.
    press(&mut rt, Key::Enter);
    assert_eq!(
        rt.state.settings.terminal_theme,
        TerminalThemeChoice::FollowChrome
    );
    press(&mut rt, Key::Left);
    press(&mut rt, Key::Text(" ".into()));
    assert_eq!(
        rt.state.settings.terminal_theme,
        TerminalThemeChoice::RexDefault
    );
    assert!(
        rt.state.theme_library.query().is_empty(),
        "Space is no search"
    );

    // Up from the first row is the search field; a letter on a card searches for it.
    press(&mut rt, Key::Up);
    assert!(searching(&rt));
    press(&mut rt, Key::Down);
    type_text(&mut rt, "aby");
    assert_eq!(rt.state.theme_library.query(), "aby");
    assert!(searching(&rt));
    assert_eq!(cards(&rt), ["named:Abyss"]);
    std::fs::remove_file(&rt.state.settings_path).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn six_hundred_themes_build_only_the_rows_near_the_viewport() {
    let (root, dirs) = dirs("library-many");
    for index in 0..600 {
        write(
            &dirs,
            false,
            &format!("Theme {index:03}"),
            // Every one dark, so the last by name is the last card.
            &format!(
                "background = #{:02x}{:02x}28\nforeground = #e0e0e0\n",
                index % 64,
                index / 10
            ),
        );
    }
    let mut rt = library(dirs, 1, (1440., 1000.));
    assert_eq!(element_text(&rt, "theme-count"), ["630 themes"]);
    let built = cards(&rt).len();
    assert!(
        (8..=120).contains(&built),
        "a few screens of cards are built, not 630: {built}"
    );
    let elements = rt.elements().len();
    assert!(elements < 4000, "{elements} elements for 630 themes");
    assert!(find(&rt, "theme-card:named:Theme 599").is_none());

    // Scrolling builds the rows that come into view and lets go of the ones that left.
    let page = rect(&rt, "settings-scroll:themes");
    let (x, y) = (page[0] + page[2] / 2., page[1] + page[3] / 2.);
    for step in 0..400 {
        rt.wheel(x, y, -60.);
        rt.frame(1. / 60.);
        if step % 50 == 0 {
            let built = cards(&rt).len();
            assert!(built <= 160, "the built rows stay a few screens: {built}");
        }
    }
    settle(&mut rt);
    let last = find(&rt, "theme-card:named:Theme 599").expect("the last theme is reached");
    assert!(
        last.visible[3] > 80.,
        "and its card is on screen: {:?}",
        last.visible
    );
    assert!(
        find(&rt, "theme-card:follow_server").is_none(),
        "the first row is no longer built"
    );
    // The current theme is found by searching, wherever the page is scrolled.
    rt.update(|s, tx| {
        s.theme_library.search = rune::TextEdit::new("theme 314");
        tx.instant();
    });
    settle(&mut rt);
    assert_eq!(cards(&rt), ["named:Theme 314"]);
    let card = find(&rt, "theme-card:named:Theme 314").unwrap();
    assert!(card.visible[3] > 80., "what was found is in view");
    std::fs::remove_file(&rt.state.settings_path).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn switcher_rows_end_with_their_colours_as_a_strip() {
    let (root, dirs) = dirs("library-strips");
    write(&dirs, false, "Mocha", MOCHA);
    let mut ws = workspace();
    ws.theme_dirs = dirs;
    ws.settings.chrome_theme = ChromeTheme::RosePine;
    ws.perform(Intent::ChooseTheme, &mut rune::Tx::default());
    let pictures: Vec<(String, Option<Picture>)> = ws
        .entries()
        .into_iter()
        .map(|e| (e.label, e.picture))
        .collect();
    assert_eq!(
        pictures.len(),
        31,
        "three choices, twenty-seven built-in themes and Mocha"
    );
    assert_eq!(
        pictures[0],
        ("Follow server".to_owned(), Some(Picture::Server))
    );
    let colours = |label: &str| match pictures.iter().find(|(l, _)| l == label) {
        Some((_, Some(Picture::Theme(swatch)))) => *swatch,
        other => panic!("{label} has no colours: {other:?}"),
    };
    // What a theme leaves out is the window's own, as a pane draws it.
    assert_eq!(colours("Rex default colours").background, 0x191724);
    assert_eq!(colours("Follow chrome"), colours("Rosé Pine"));
    let mocha = colours("Mocha");
    assert_eq!(
        (
            mocha.background,
            mocha.foreground,
            mocha.ansi[1],
            mocha.ansi[4]
        ),
        (0x1e1e2e, 0xcdd6f4, 0xf38ba8, 0x89b4fa)
    );

    // Each row draws its strip before the check's column: four dots on the theme's
    // background, or for Follow server the outline alone.
    let mut rt = runtime_at(ws, (1440., 1000.));
    settle(&mut rt);
    let strip = |rt: &Runtime<Workspace>, row: usize| -> Vec<ElementInfo> {
        let parts = inside(rt, &format!("entry:{row}"));
        let at = parts
            .iter()
            .position(|e| e.key.as_deref() == Some("theme-strip"))
            .unwrap_or_else(|| panic!("row {row} has no strip"));
        let rect = parts[at].rect;
        parts
            .into_iter()
            .skip(at)
            .filter(|e| e.rect[0] >= rect[0] && e.rect[0] + e.rect[2] <= rect[0] + rect[2])
            .collect()
    };
    assert_eq!(strip(&rt, 0).len(), 1, "Follow server: no dots");
    let mocha_row = strip(&rt, 30);
    assert_eq!(mocha_row[0].fill, rgb(0x1e1e2e));
    assert_eq!(
        mocha_row[1..]
            .iter()
            .map(|dot| dot.fill)
            .collect::<Vec<_>>(),
        [0xcdd6f4, 0xf38ba8, 0xa6e3a1, 0x89b4fa].map(rgb),
        "text, red, green and blue"
    );
    let edges: Vec<f32> = (0..8)
        .map(|row| {
            let rect = strip(&rt, row)[0].rect;
            rect[0] + rect[2]
        })
        .collect();
    assert!(
        edges.iter().all(|edge| (edge - edges[0]).abs() < 0.5),
        "the strips end on one line: {edges:?}"
    );
    let check = inside(&rt, "entry:0")
        .into_iter()
        .find(|e| e.key.as_deref() == Some("check"))
        .expect("the theme in use is checked");
    assert!(
        check.rect[0] >= edges[0],
        "the check keeps its column after the strip"
    );

    // Window colours carry their palette's ground, ink and accents the same way.
    rt.update(|s, tx| {
        s.close_panel(tx);
        s.perform(Intent::ChooseChrome, tx);
    });
    settle(&mut rt);
    let chromes: Vec<Option<Picture>> = rt.state.entries().into_iter().map(|e| e.picture).collect();
    assert_eq!(
        chromes,
        ChromeTheme::ALL.map(|chrome| Some(Picture::Chrome(chrome)))
    );
    let gruvbox = strip(&rt, 3);
    assert_eq!(gruvbox[0].fill, rgb(0x282828));
    assert_eq!(gruvbox.len(), 5);
    std::fs::remove_file(&rt.state.settings_path).ok();
    std::fs::remove_dir_all(root).ok();
}
