//! Bundled chrome fonts, the terminal family and the user's ordered fallbacks as the
//! platform resolves them, and a bundled symbols font after them; and the installed
//! families, for the font pickers in Settings. Where families are found is
//! [`super::platform::fonts`].
use super::platform::fonts as system;
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::OnceLock,
};

pub struct Fonts {
    pub files: &'static [&'static [u8]],
    /// System font files Rune registers by path, without copying them into heap buffers.
    pub paths: &'static [PathBuf],
    pub families: (&'static str, &'static str),
    /// The resolved primary family, for Settings; the grid uses the mono slot's full stack.
    pub terminal: Option<&'static str>,
    /// The platform's monospaced family the grid uses when the primary family is not
    /// installed ([`system::STAND_INS`]); None leaves bundled IBM Plex Mono.
    pub stand_in: Option<&'static str>,
}

/// Nerd Fonts' symbols (Powerline, Font Awesome, Devicons, Codicons, Material Design and
/// more), so a prompt drawn for a Nerd Font has its glyphs whatever the terminal family.
/// Provenance and licence: `resources/fonts/README.md`.
const SYMBOLS: &[u8] = include_bytes!("../../resources/fonts/SymbolsNerdFont-Regular.ttf");
pub const SYMBOLS_FAMILY: &str = "Symbols Nerd Font";

/// Rune's bundled faces and the symbols font: what the app registers from memory.
fn files() -> &'static [&'static [u8]] {
    static FILES: OnceLock<Vec<&'static [u8]>> = OnceLock::new();
    FILES.get_or_init(|| {
        rune_theme_opencode::FONTS
            .iter()
            .copied()
            .chain([SYMBOLS])
            .collect()
    })
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn css_family(family: &str) -> String {
    format!("'{}'", family.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn bundled(family: &str) -> Option<&'static str> {
    [
        rune_theme_opencode::FAMILIES.0,
        rune_theme_opencode::FAMILIES.1,
        SYMBOLS_FAMILY,
    ]
    .into_iter()
    .find(|name| name.eq_ignore_ascii_case(family))
}

fn add_name(names: &mut Vec<String>, name: &str) {
    if !names
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(name))
    {
        names.push(name.to_owned());
    }
}

/// A font family a picker lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Family {
    pub name: String,
    /// The name in lower case, which a search is matched against.
    pub lower: String,
    /// Every character is one width, or one of two (as CJK terminal fonts are): what a
    /// terminal's grid needs.
    pub mono: bool,
}
impl Family {
    pub fn new(name: &str, mono: bool) -> Self {
        Self {
            name: name.to_owned(),
            lower: name.to_lowercase(),
            mono,
        }
    }
}

/// The families of `listed` (a family or one of its faces, and whether it is fixed-pitch)
/// and the bundled ones, in alphabetical order whatever their case. A family is monospaced
/// only when everything listed of it is; mixed families stay in All fonts. Names the
/// settings can't hold (with a comma or a control character) are left out.
pub fn catalog<'a>(listed: impl IntoIterator<Item = (&'a str, bool)>) -> Vec<Family> {
    // Keyed by the name in lower case, so "Hack" and "hack" from two files are one family.
    let mut families: BTreeMap<String, (String, bool)> = BTreeMap::new();
    let mut add = |name: &str, mono: bool| {
        let Ok(name) = rex_rune::settings::parse_font_family(name) else {
            return;
        };
        let entry = families.entry(name.to_lowercase()).or_insert((name, true));
        entry.1 &= mono;
    };
    for (name, mono) in listed {
        add(name, mono);
    }
    add(rune_theme_opencode::FAMILIES.0, false);
    add(rune_theme_opencode::FAMILIES.1, true);
    add(SYMBOLS_FAMILY, false);
    families
        .into_values()
        .map(|(name, mono)| Family::new(&name, mono))
        .collect()
}

static CATALOG: OnceLock<Vec<Family>> = OnceLock::new();

/// The families installed on this machine and the bundled ones, read once per run. Without
/// the platform's list (no `fc-list` on Linux) only the bundled families are listed.
pub fn read_installed() -> &'static [Family] {
    CATALOG.get_or_init(|| {
        let listed = system::installed();
        catalog(listed.iter().map(|(name, mono)| (name.as_str(), *mono)))
    })
}

/// [`read_installed`] once it has been read. The first call starts reading it on a thread of
/// its own, which calls `ready` when it has: a first scan after boot takes a quarter of a
/// second on macOS.
pub fn installed(ready: impl FnOnce() + Send + 'static) -> Option<&'static [Family]> {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("rex-fonts".into())
            .spawn(move || {
                read_installed();
                ready();
            });
        if let Err(error) = spawned {
            eprintln!("Could not list installed fonts: {error}");
        }
    });
    CATALOG.get().map(Vec::as_slice)
}

/// `family` for a text style ([`rune::Family::Named`]), which loads an installed family the
/// first time it is drawn: a name can be shown in its own face. A family name has no comma,
/// so Rune reads it as one literal name. Each name is kept for the life of the process, once.
pub fn named(family: &str) -> rune::Family {
    thread_local! {
        static NAMES: std::cell::RefCell<std::collections::HashMap<String, &'static str>> =
            Default::default();
    }
    NAMES.with_borrow_mut(|names| {
        rune::Family::Named(names.entry(family.to_owned()).or_insert_with(|| {
            leak(match bundled(family) {
                Some(name) => name.to_owned(),
                None => system::stack_name(family),
            })
        }))
    })
}

/// What the grid uses when the terminal family is not installed, for Settings.
pub fn stand_in_words() -> String {
    match system::STAND_INS {
        [] => "bundled IBM Plex Mono".into(),
        [first, rest @ ..] => {
            let mut words = (*first).to_owned();
            for name in rest {
                words.push_str(&format!(" or {name}"));
            }
            words + ", else bundled IBM Plex Mono"
        }
    }
}

pub fn load(terminal_family: &str, fallbacks: &[String]) -> Fonts {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    let mut terminal = None;
    let mut stand_in = None;
    let mut resolve = |family: &str, paths: &mut Vec<PathBuf>| {
        if let Some(name) = bundled(family) {
            return Some((name.to_owned(), name.to_owned()));
        }
        let found = system::resolve(family)?;
        for file in found.files {
            if seen.insert(file.clone()) {
                paths.push(file);
            }
        }
        Some((found.name, found.stack))
    };
    for (index, family) in std::iter::once(terminal_family)
        .chain(fallbacks.iter().map(String::as_str))
        .enumerate()
    {
        match resolve(family, &mut paths) {
            Some((name, stack)) => {
                if index == 0 {
                    terminal = Some(leak(name));
                }
                add_name(&mut names, &stack);
            }
            None if index == 0 => {
                let found = system::STAND_INS
                    .iter()
                    .find_map(|family| resolve(family, &mut paths));
                let using = match found {
                    Some((name, stack)) => {
                        add_name(&mut names, &stack);
                        let name = leak(name);
                        stand_in = Some(name);
                        name
                    }
                    None => {
                        add_name(&mut names, "IBM Plex Mono");
                        "IBM Plex Mono"
                    }
                };
                eprintln!("Terminal font {family:?} not found; using {using}.");
            }
            None => eprintln!("Terminal font {family:?} not found; skipping this fallback."),
        }
    }
    add_name(&mut names, "IBM Plex Mono");
    add_name(&mut names, SYMBOLS_FAMILY);
    let mono = names
        .iter()
        .map(|name| css_family(name))
        .collect::<Vec<_>>()
        .join(", ");
    Fonts {
        files: files(),
        paths: Box::leak(paths.into_boxed_slice()),
        families: (rune_theme_opencode::FAMILIES.0, leak(mono)),
        terminal,
        stand_in,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_stack_keeps_chrome_separate_and_preserves_fallback_order() {
        let fonts = load("IBM Plex Mono", &["Inter".into(), "ibm plex mono".into()]);
        assert_eq!(fonts.families.0, rune_theme_opencode::FAMILIES.0);
        assert_eq!(
            fonts.families.1,
            "'IBM Plex Mono', 'Inter', 'Symbols Nerd Font'"
        );
        assert_eq!(fonts.terminal, Some("IBM Plex Mono"));
        assert_eq!(fonts.stand_in, None);
        assert_eq!(
            &fonts.files[..fonts.files.len() - 1],
            rune_theme_opencode::FONTS
        );
        assert!(fonts.paths.is_empty(), "bundled faces are not re-read");
    }

    #[test]
    fn the_symbols_font_follows_named_fallbacks_and_plex_once() {
        let fonts = load(
            "IBM Plex Mono",
            &["symbols nerd font".into(), "Inter".into()],
        );
        assert_eq!(
            fonts.families.1, "'IBM Plex Mono', 'Symbols Nerd Font', 'Inter'",
            "a named place is kept; the bundled copy is not added twice"
        );
        assert!(fonts.paths.is_empty());
    }

    #[test]
    fn the_catalog_merges_faces_sorts_names_and_marks_fixed_pitch_families() {
        let listed = [
            ("Zed Mono", true),
            ("Noto Sans", false),
            ("hack", false),
            ("Hack", true),
            ("Noto Sans", false),
            ("Sarasa Term", true),
            ("Odd, Name", true),
            ("Fira Code", false),
            ("Fira Code", true),
            ("Bad\u{7}Name", true),
        ];
        let catalog = catalog(listed);
        let names: Vec<(&str, bool)> = catalog.iter().map(|f| (f.name.as_str(), f.mono)).collect();
        assert_eq!(
            names,
            [
                ("Fira Code", false),
                // One family for both spellings, excluded because one face is proportional.
                ("hack", false),
                ("IBM Plex Mono", true),
                ("Inter", false),
                ("Noto Sans", false),
                ("Sarasa Term", true),
                ("Symbols Nerd Font", false),
                ("Zed Mono", true),
            ]
        );
        assert!(catalog.iter().all(|f| f.lower == f.name.to_lowercase()));
        // With no installed list at all the bundled families are still there to choose.
        let bundled: Vec<String> = super::catalog([]).into_iter().map(|f| f.name).collect();
        assert_eq!(bundled, ["IBM Plex Mono", "Inter", "Symbols Nerd Font"]);
    }

    #[test]
    fn the_installed_families_always_include_the_bundled_ones_in_order() {
        // Whatever this machine has, or with no list of installed fonts at all.
        let catalog = read_installed();
        assert!(catalog.iter().any(|f| f.name == "IBM Plex Mono" && f.mono));
        assert!(catalog.iter().any(|f| f.name == "Inter"));
        assert!(catalog.iter().any(|f| f.name == SYMBOLS_FAMILY));
        assert!(catalog.windows(2).all(|pair| pair[0].lower < pair[1].lower));
        assert!(std::ptr::eq(catalog, read_installed()), "read once");
    }

    #[test]
    fn the_catalog_is_read_off_the_calling_thread_and_says_when() {
        let (done, ready) = std::sync::mpsc::channel();
        let caller = std::thread::current().id();
        let first = installed(move || done.send(std::thread::current().id()).unwrap());
        match ready.recv_timeout(std::time::Duration::from_secs(60)) {
            Ok(reader) => assert_ne!(reader, caller, "read on a thread of its own"),
            // Another caller started the read, so this `ready` was dropped unused.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                read_installed();
            }
            Err(error) => panic!("the catalog was not read: {error} ({first:?})"),
        }
        let read = installed(|| unreachable!("started once")).expect("read by now");
        assert!(std::ptr::eq(read, read_installed()));
    }

    #[test]
    fn a_family_name_is_interned_once_as_a_literal_name() {
        let (a, b) = (named("Joe's Mono"), named("Joe's Mono"));
        let rune::Family::Named(first) = a else {
            panic!("a named family")
        };
        let rune::Family::Named(second) = b else {
            panic!("a named family")
        };
        assert_eq!(first, "Joe's Mono");
        assert!(std::ptr::eq(first, second), "kept once");
    }

    #[test]
    fn the_stand_ins_are_named_in_order_before_the_bundled_font() {
        let words = stand_in_words();
        assert!(words.ends_with("bundled IBM Plex Mono"), "{words}");
        let mut at = 0;
        for name in system::STAND_INS {
            at += words[at..].find(name).expect("named in order");
        }
    }

    #[test]
    fn named_family_samples_use_the_requested_bundled_face_not_chrome_fallback() {
        let _families = rune::text::with_families("Inter", "Inter");
        let mut ts =
            rune::text::TextSystem::new(rune_theme_opencode::FONTS, rune_theme_opencode::FAMILIES);
        let sample = rune::text::Run::new("Aa 0O 1il");
        let plex = ts.run_layout(
            &sample,
            rune::TextStyle::new(16., 400.).family(named("IBM Plex Mono")),
            1.,
        );
        let chrome = ts.run_layout(
            &sample,
            rune::TextStyle::new(16., 400.).family(named("Inter")),
            1.,
        );
        let bytes = |layout: &rune::text::RunLayout| {
            layout
                .layout
                .lines()
                .next()
                .unwrap()
                .runs()
                .next()
                .unwrap()
                .font()
                .data
                .as_ref()
                .to_vec()
        };
        assert_ne!(
            bytes(&plex),
            bytes(&chrome),
            "a named sample is not silently the chrome face"
        );
        assert_eq!(
            bytes(&plex),
            include_bytes!("../../vendor/rune/crates/rune/assets/IBMPlexMono-Regular.ttf")
        );
    }

    #[test]
    fn css_names_are_escaped_as_names_not_syntax() {
        assert_eq!(css_family("Joe's Mono"), "'Joe\\'s Mono'");
        assert_eq!(css_family(r"A\B"), r"'A\\B'");
    }

    /// The glyph ids, and the font each came from, that `text` shapes to in `style`.
    fn shaped(
        ts: &mut rune::text::TextSystem,
        text: &str,
        style: rune::TextStyle,
    ) -> Vec<(char, u32, Vec<u8>)> {
        let run = rune::text::Run::new(text);
        let layout = ts.run_layout(&run, style, 1.);
        let mut glyphs = Vec::new();
        for line in layout.layout.lines() {
            for run in line.runs() {
                let font = run.font().data.as_ref().to_vec();
                for cluster in run.visual_clusters() {
                    let ch = text[cluster.text_range()].chars().next().unwrap();
                    for glyph in cluster.glyphs() {
                        glyphs.push((ch, glyph.id, font.clone()));
                    }
                }
            }
        }
        glyphs
    }

    #[test]
    fn nerd_glyphs_come_from_the_bundled_symbols_font_without_changing_ascii_cells() {
        let fonts = load("IBM Plex Mono", &[]);
        let _families = rune::text::with_families("Inter", fonts.families.1);
        let mut ts =
            rune::text::TextSystem::new_with_paths(fonts.files, fonts.paths, fonts.families);
        let style = rune::TextStyle::new(15., 400.).mono();
        // Branch, Powerline arrow, folder and GitHub: what prompts draw.
        let glyphs = shaped(&mut ts, "\u{e0a0}\u{e0b0}\u{f07b}\u{f113}", style);
        assert_eq!(glyphs.len(), 4);
        for (ch, id, font) in &glyphs {
            assert_ne!(*id, 0, "U+{:04X} has a glyph", *ch as u32);
            assert_eq!(font, SYMBOLS, "U+{:04X} from the symbols font", *ch as u32);
        }
        // ASCII stays on IBM Plex Mono, with its cells.
        let plex = include_bytes!("../../vendor/rune/crates/rune/assets/IBMPlexMono-Regular.ttf");
        assert!(shaped(&mut ts, "M0", style).iter().all(|g| g.2 == plex));
        let mut without =
            rune::text::TextSystem::new(rune_theme_opencode::FONTS, ("Inter", "'IBM Plex Mono'"));
        assert_eq!(
            ts.measure("MMMM 1234", style, 1.),
            without.measure("MMMM 1234", style, 1.)
        );
        let plain = {
            let _plex = rune::text::with_families("Inter", "'IBM Plex Mono'");
            crate::app::term::metrics(15.).cell
        };
        assert_eq!(
            crate::app::term::metrics(15.).cell,
            plain,
            "the grid's cell"
        );
    }

    #[test]
    fn native_font_paths_shape_from_the_file_without_an_embedded_copy() {
        let _families = rune::text::with_families("Inter", "Inter");
        const PLEX: &[u8] =
            include_bytes!("../../vendor/rune/crates/rune/assets/IBMPlexMono-Regular.ttf");
        let path = std::env::temp_dir().join(format!(
            "rex-font-path-{}-{}.ttf",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, PLEX).unwrap();
        let style = rune::TextStyle::new(13., 400.).mono();
        let mut without_path =
            rune::text::TextSystem::new_with_paths(&[], &[], ("Inter", "IBM Plex Mono"));
        let fallback_width = without_path
            .layout_rich("MMMM", &[], style, 1., None)
            .full_width();
        let mut with_path = rune::text::TextSystem::new_with_paths(
            &[],
            std::slice::from_ref(&path),
            ("Inter", "IBM Plex Mono"),
        );
        let plex_width = with_path
            .layout_rich("MMMM", &[], style, 1., None)
            .full_width();
        std::fs::remove_file(path).unwrap();
        assert!(
            (plex_width - fallback_width).abs() > 0.5,
            "path font was not selected: {plex_width} vs {fallback_width}"
        );
    }

    #[test]
    #[ignore = "requires fontconfig and a CJK font; exercised by the Nix package check"]
    fn fontconfig_cjk_fallback_has_real_glyphs_and_keeps_ascii_metrics() {
        let _families = rune::text::with_families("Inter", "Inter");
        let fonts = load("IBM Plex Mono", &[]);
        let mut ts =
            rune::text::TextSystem::new_with_paths(fonts.files, fonts.paths, fonts.families);
        let style = rune::TextStyle::new(15., 400.).mono();
        let text = rune::text::Run::new("世界 日本語 한글");
        let shaped = ts.run_layout(&text, style, 1.);
        let mut count = 0;
        for line in shaped.layout.lines() {
            for run in line.runs() {
                for cluster in run.visual_clusters() {
                    for glyph in cluster.glyphs() {
                        assert_ne!(
                            glyph.id,
                            0,
                            "missing CJK glyph in {:?}",
                            cluster.text_range()
                        );
                        count += 1;
                    }
                }
            }
        }
        assert!(count >= 9);
        let mut bundled =
            rune::text::TextSystem::new(rune_theme_opencode::FONTS, rune_theme_opencode::FAMILIES);
        assert_eq!(
            ts.measure("MMMM 1234", style, 1.),
            bundled.measure("MMMM 1234", style, 1.)
        );
    }
}
