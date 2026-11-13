//! Where the terminal's font families are found.
//!
//! On Linux and the BSDs that is fontconfig: `fc-match` resolves a family and `fc-list` lists
//! them, as the distributions' packages and the Nix package arrange. macOS and Windows have
//! no fontconfig (a Mac app started from Finder has no Homebrew on `PATH`), so there the
//! system's own list of fonts answers instead, Core Text or DirectWrite, through Rune.
//!
//! A family a platform does not have is not found, and [`STAND_INS`] names what the grid
//! uses in its place before the bundled IBM Plex Mono.
use std::path::PathBuf;

pub use system::{NOT_FOUND, STAND_INS, installed, resolve, stack_name};

/// An installed family, resolved for the terminal's font stack.
#[derive(Debug, PartialEq, Eq)]
pub struct Found {
    /// Its name, to show.
    pub name: String,
    /// The name the font stack asks for it by: the same, except where the platform hides
    /// the family's own name (SF Mono's is `.SF NS Mono`).
    pub stack: String,
    /// The files of its faces.
    pub files: Vec<PathBuf>,
}

#[cfg(not(any(target_os = "macos", windows)))]
mod system {
    use super::Found;
    use std::{path::PathBuf, process::Command};

    /// Fontconfig has no stand-ins of its own: its answer for `monospace` is one of Rune's
    /// automatic fallbacks already.
    pub const STAND_INS: &[&str] = &[];

    /// Why Settings marks a family as not installed.
    pub const NOT_FOUND: &str = "fontconfig has no family of this name";

    pub fn stack_name(family: &str) -> String {
        family.to_owned()
    }

    fn pattern(family: &str) -> String {
        family
            .replace('\\', "\\\\")
            .replace(':', "\\:")
            .replace(',', "\\,")
    }

    /// The available faces fontconfig resolves for one requested family, in four styles,
    /// without accepting an unrelated substitute. Rune synthesizes a style the family does
    /// not provide.
    pub fn resolve(family: &str) -> Option<Found> {
        let generic = ["monospace", "sans-serif", "serif"]
            .iter()
            .any(|name| family.eq_ignore_ascii_case(name));
        let mut name = None;
        let mut files = Vec::new();
        for style in ["", ":bold", ":italic", ":bold:italic"] {
            let out = Command::new("fc-match")
                .args([
                    "-f",
                    "%{family[0]}\n%{file}",
                    "--",
                    &format!("{}{style}", pattern(family)),
                ])
                .output()
                .ok()?;
            if !out.status.success() {
                continue;
            }
            let out = String::from_utf8_lossy(&out.stdout);
            let Some((found, file)) = out.split_once('\n') else {
                continue;
            };
            if !generic && !found.eq_ignore_ascii_case(family) {
                continue;
            }
            let file = PathBuf::from(file.trim());
            if file.is_file() && !files.contains(&file) {
                name.get_or_insert_with(|| found.to_owned());
                files.push(file);
            }
        }
        let name = name?;
        Some(Found {
            stack: name.clone(),
            name,
            files,
        })
    }

    /// What `fc-list` is asked to print for each face: its first family name (the one
    /// [`resolve`] accepts) and its spacing, tab-separated.
    const FORMAT: &str = "%{family[0]}\t%{spacing}\n";

    /// Each face in `fc-list`'s output in [`FORMAT`]: its family and whether it is
    /// fixed-pitch.
    pub fn parse_listing(listing: &str) -> Vec<(String, bool)> {
        listing
            .lines()
            .map(|line| {
                let (name, spacing) = line.split_once('\t').unwrap_or((line, ""));
                // FC_DUAL, FC_MONO and FC_CHARCELL; a proportional face has no spacing at all.
                let mono = matches!(spacing.trim().parse::<u32>(), Ok(90 | 100 | 110));
                (name.to_owned(), mono)
            })
            .collect()
    }

    /// Every installed face's family, and whether that face is fixed-pitch. Without
    /// `fc-list`, none.
    pub fn installed() -> Vec<(String, bool)> {
        let listing = Command::new("fc-list")
            .args(["--format", FORMAT])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
            .unwrap_or_default();
        parse_listing(&listing)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn fontconfig_names_are_escaped_as_patterns_not_syntax() {
            assert_eq!(pattern(r"A:B,C\D"), r"A\:B\,C\\D");
        }

        #[test]
        fn fontconfig_spacing_marks_mono_dual_and_charcell_faces_fixed_pitch() {
            let listing = "Zed Mono\t100\nNoto Sans\t\nSarasa Term\t90\nCell Font\t110\nPropo\t0\n";
            let fixed: Vec<bool> = parse_listing(listing).into_iter().map(|f| f.1).collect();
            assert_eq!(fixed, [true, false, true, true, false]);
        }

        #[test]
        fn missing_fontconfig_still_lists_bundled_families() {
            const CHILD: &str = "REX_TEST_NO_FONTCONFIG";
            if std::env::var_os(CHILD).is_some() {
                let catalog = crate::app::fonts::read_installed();
                let names: Vec<_> = catalog.iter().map(|f| f.name.as_str()).collect();
                assert_eq!(names, ["IBM Plex Mono", "Inter", "Symbols Nerd Font"]);
                assert!(resolve("DejaVu Sans Mono").is_none());
                return;
            }
            super::super::in_child(
                CHILD,
                "app::platform::fonts::system::tests::missing_fontconfig_still_lists_bundled_families",
            );
        }
    }
}

#[cfg(any(target_os = "macos", windows))]
mod system {
    use super::Found;

    /// The platform's own monospaced families, tried in order when the requested one is not
    /// installed.
    #[cfg(target_os = "macos")]
    pub const STAND_INS: &[&str] = &["SF Mono", "Menlo"];
    #[cfg(windows)]
    pub const STAND_INS: &[&str] = &["Cascadia Mono", "Consolas"];

    /// Why Settings marks a family as not installed.
    pub const NOT_FOUND: &str = "No installed font family has this name";

    /// Families the system has under a hidden name: the name people know them by, the
    /// hidden name and the file names (without extension) to leave out. All are monospaced.
    /// SF Mono is the system's monospaced font; Terminal's copy of it is not installed for
    /// other apps. Its italic file says it is upright, so with it in the family the grid's
    /// upright text could be drawn from either file.
    #[cfg(target_os = "macos")]
    const HIDDEN: &[Hidden] = &[("SF Mono", ".SF NS Mono", &["SFNSMonoItalic"])];
    #[cfg(windows)]
    const HIDDEN: &[Hidden] = &[];

    type Hidden = (&'static str, &'static str, &'static [&'static str]);

    fn hidden(family: &str) -> Option<&'static Hidden> {
        HIDDEN
            .iter()
            .find(|(shown, ..)| shown.eq_ignore_ascii_case(family))
            .filter(|_| rune::text::installed_family_files(family).is_empty())
    }

    /// The name to ask the system for `family` by: its hidden name, when only that is
    /// installed.
    pub fn stack_name(family: &str) -> String {
        hidden(family)
            .map_or(family, |(_, name, _)| name)
            .to_owned()
    }

    /// The faces of the installed family `family`, by any of its names, case aside.
    pub fn resolve(family: &str) -> Option<Found> {
        let stack = stack_name(family);
        let mut files = rune::text::installed_family_files(&stack);
        if let Some((.., left_out)) = hidden(family) {
            files.retain(|file| {
                !file
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| left_out.contains(&stem))
            });
        }
        (!files.is_empty()).then(|| Found {
            name: family.to_owned(),
            stack,
            files,
        })
    }

    /// Every installed family, and whether it is fixed-pitch, with the hidden families
    /// under the names they are known by.
    pub fn installed() -> Vec<(String, bool)> {
        let mut families: Vec<(String, bool)> = rune::text::installed_families()
            .into_iter()
            .map(|family| (family.name, family.fixed_pitch))
            .collect();
        for (shown, hidden, _) in HIDDEN {
            if !rune::text::installed_family_files(hidden).is_empty() {
                families.push(((*shown).to_owned(), true));
            }
        }
        families
    }

    #[cfg(all(test, target_os = "macos"))]
    mod tests {
        use super::*;

        #[test]
        fn sf_mono_is_the_system_font_under_its_hidden_name() {
            let sf = resolve("SF Mono").expect("macOS has SF Mono");
            assert_eq!(sf.name, "SF Mono");
            assert_eq!(sf.stack, ".SF NS Mono");
            assert!(sf.files.iter().all(|f| f.is_file()));
            assert!(
                sf.files.iter().any(|f| f.ends_with("SFNSMono.ttf")),
                "{:?}",
                sf.files
            );
            assert!(
                !sf.files.iter().any(|f| f.ends_with("SFNSMonoItalic.ttf")),
                "{:?}",
                sf.files
            );
            let menlo = resolve("menlo").expect("macOS has Menlo");
            assert_eq!(menlo.stack, "menlo");
            assert!(menlo.files.iter().any(|f| f.ends_with("Menlo.ttc")));
            assert!(resolve("No Such Family 7f3a").is_none());
        }

        #[test]
        fn core_text_lists_and_resolves_families_without_fontconfig_on_path() {
            const CHILD: &str = "REX_TEST_NO_FONTCONFIG";
            if std::env::var_os(CHILD).is_some() {
                assert!(std::process::Command::new("fc-list").output().is_err());
                let catalog = crate::app::fonts::read_installed();
                let mono = |name: &str| catalog.iter().find(|f| f.name == name).map(|f| f.mono);
                assert_eq!(mono("Menlo"), Some(true));
                assert_eq!(mono("SF Mono"), Some(true));
                assert_eq!(mono("Helvetica"), Some(false));
                assert_eq!(mono("IBM Plex Mono"), Some(true));
                assert!(catalog.iter().all(|f| !f.name.starts_with('.')), "hidden");
                let menlo = resolve("Menlo").expect("Menlo resolves");
                assert!(menlo.files.iter().any(|f| f.ends_with("Menlo.ttc")));
                return;
            }
            super::super::in_child(
                CHILD,
                "app::platform::fonts::system::tests::core_text_lists_and_resolves_families_without_fontconfig_on_path",
            );
        }
    }

    #[cfg(all(test, windows))]
    mod tests {
        use super::*;

        #[test]
        fn direct_write_lists_and_resolves_the_system_families() {
            let catalog = crate::app::fonts::read_installed();
            let mono = |name: &str| catalog.iter().find(|f| f.name == name).map(|f| f.mono);
            assert_eq!(mono("Consolas"), Some(true));
            assert_eq!(mono("Arial"), Some(false));
            assert_eq!(mono("IBM Plex Mono"), Some(true));
            let consolas = resolve("consolas").expect("Windows has Consolas");
            assert_eq!(consolas.stack, "consolas");
            assert!(
                consolas.files.iter().any(|f| f
                    .file_name()
                    .is_some_and(|n| n.eq_ignore_ascii_case("consola.ttf"))),
                "{:?}",
                consolas.files
            );
            assert!(resolve("No Such Family 7f3a").is_none());
            assert!(STAND_INS.iter().any(|family| resolve(family).is_some()));
        }
    }
}

/// Runs the test `name` again in a process of its own with `PATH` empty and `marker` set, so
/// no fontconfig program can be found and the font list is read afresh.
#[cfg(test)]
#[allow(dead_code)]
fn in_child(marker: &str, name: &str) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--test-threads=1"])
        .env(marker, "1")
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "the child ran the test"
    );
}
