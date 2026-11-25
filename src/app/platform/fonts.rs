//! Font families from Rune's native catalog: fontconfig, Core Text, or DirectWrite.

/// The platform's own monospaced families, tried in order before bundled IBM Plex Mono.
#[cfg(target_os = "macos")]
pub const STAND_INS: &[&str] = &["SF Mono", "Menlo"];
#[cfg(windows)]
pub const STAND_INS: &[&str] = &["Cascadia Mono", "Consolas"];
#[cfg(not(any(target_os = "macos", windows)))]
pub const STAND_INS: &[&str] = &[];

pub const NOT_FOUND: &str = "No installed font family has this name";

/// Core Text hides SF Mono's system family. Rune exposes it as `ui-monospace`, but
/// an installed family actually called SF Mono still takes precedence.
const SYSTEM_MONO_NAME: Option<&str> = if cfg!(target_os = "macos") {
    Some("SF Mono")
} else {
    None
};

fn system_mono_name() -> Option<&'static str> {
    SYSTEM_MONO_NAME.filter(|name| {
        !rune::text::installed_families()
            .iter()
            .any(|family| family.name().eq_ignore_ascii_case(name))
    })
}

pub fn uses_system_mono(name: &str) -> bool {
    SYSTEM_MONO_NAME.is_some_and(|system| system.eq_ignore_ascii_case(name))
        && system_mono_name().is_some()
}

/// The canonical display name and the name Rune should use in the terminal stack.
pub fn resolve(name: &str) -> Option<(&'static str, &'static str)> {
    let listed = rune::text::installed_families()
        .iter()
        .find(|family| family.name().eq_ignore_ascii_case(name));
    if let Some(family) = listed {
        Some((family.name(), family.name()))
    } else if let Some(system) = system_mono_name().filter(|s| s.eq_ignore_ascii_case(name)) {
        Some((system, "ui-monospace"))
    } else {
        ["monospace", "sans-serif", "serif"]
            .into_iter()
            .find(|generic| generic.eq_ignore_ascii_case(name))
            .map(|generic| (generic, generic))
    }
}

pub fn installed() -> impl Iterator<Item = (&'static str, bool)> {
    rune::text::installed_families()
        .iter()
        .map(|family| (family.name(), family.monospace()))
        .chain(system_mono_name().map(|name| (name, true)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_family_is_not_resolved_to_an_unrelated_face() {
        assert_eq!(resolve("No Such Family 7f3a"), None);
    }

    #[test]
    fn resolves_installed_families_case_insensitively() {
        if let Some((name, _)) = installed().next() {
            let upper = name.to_uppercase();
            if name.eq_ignore_ascii_case(&upper) {
                assert_eq!(resolve(&upper).map(|(name, _)| name), Some(name));
            }
        }
    }

    #[test]
    fn generic_families_are_css_keywords() {
        for name in ["monospace", "sans-serif", "serif"] {
            assert_eq!(resolve(name), Some((name, name)));
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn core_text_lists_menlo_helvetica_and_the_system_mono_family() {
        let find = |name| installed().find(|(listed, _)| *listed == name);
        assert_eq!(find("Menlo"), Some(("Menlo", true)));
        assert_eq!(find("Helvetica"), Some(("Helvetica", false)));
        if uses_system_mono("SF Mono") {
            assert_eq!(find("SF Mono"), Some(("SF Mono", true)));
            assert_eq!(resolve("SF Mono"), Some(("SF Mono", "ui-monospace")));
        } else {
            assert_eq!(resolve("SF Mono"), Some(("SF Mono", "SF Mono")));
        }
    }

    #[test]
    #[cfg(windows)]
    fn direct_write_lists_consolas_and_arial() {
        let find = |name| installed().find(|(listed, _)| *listed == name);
        assert_eq!(find("Consolas"), Some(("Consolas", true)));
        assert_eq!(find("Arial"), Some(("Arial", false)));
        assert_eq!(resolve("consolas"), Some(("Consolas", "Consolas")));
    }
}
