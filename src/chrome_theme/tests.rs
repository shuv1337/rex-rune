use super::{derive::*, *};
use crate::{
    color::{contrast, over},
    settings::Settings,
    terminal_theme::MAX_THEME_BYTES,
};
use std::fs;

pub(super) fn assert_legible(p: PaletteSpec) {
    for surface in [
        p.ground,
        p.card,
        p.sidebar,
        p.popover,
        p.surface_raised.unwrap(),
    ] {
        for text in [p.ink, p.muted] {
            assert!(
                contrast(text, surface) >= 4.5,
                "{text:06x} on {surface:06x}"
            );
        }
        for tone in [p.accent, p.accent2, p.live, p.warning, p.danger] {
            assert!(
                contrast(tone, surface) >= 3.0,
                "{tone:06x} on {surface:06x}"
            );
        }
    }
    assert!(contrast(p.on_accent, p.accent) >= 4.5);
    assert!(contrast(p.ink, over(p.accent, p.popover, 0.18)) >= 4.5);
}

#[test]
fn three_colour_seeds_and_dark_and_light_ghostty_themes_are_legible() {
    for (dark, ground, ink, accent) in [
        (true, 0x101824, 0xbfcbd6, 0x7cc4ff),
        (false, 0xf2efe8, 0x45454c, 0x203cbc),
    ] {
        let seed = derive_from_seed(&ChromeOverrides {
            ground: Some(ground),
            ink: Some(ink),
            accent: Some(accent),
            ..Default::default()
        });
        assert_eq!(
            (seed.dark, seed.ground, seed.card, seed.ink, seed.accent),
            (dark, ground, ground, ink, accent)
        );
        assert_legible(seed);

        let ghostty = TerminalTheme::parse(
            "Sample",
            &format!("background = #{ground:06x}\nforeground = #{ink:06x}\ncursor-color = #aaaaaa\n\
                      selection-background = #123456\npalette = 4=#{accent:06x}\npalette = 3=#ffc000"),
        )
        .unwrap();
        let p = derive_from_terminal(&ghostty, &ChromeOverrides::default());
        assert_eq!(p.dark, dark);
        assert_eq!(p.accent, accent, "achromatic cursor yields to ANSI blue");
        assert_eq!(p.terminal_selection, 0x123456);
        assert_eq!(p.ground, p.card);
        assert!(contrast(p.ink, p.ground) >= 7.0);
        assert_legible(p);
    }
    let p = derive_from_seed(&ChromeOverrides {
        ground: Some(0x767676),
        ink: Some(0x767676),
        card: Some(0xffffff),
        popover: Some(0xffffff),
        sidebar: Some(0xaaaaaa),
        on_accent: Some(0x777777),
        muted: Some(0x767676),
        ..Default::default()
    });
    for surface in [p.ground, p.card, p.sidebar, p.popover] {
        assert!(contrast(p.ink, surface) >= 4.5);
        assert!(contrast(p.muted, surface) >= 4.5);
        assert!(contrast(p.accent, surface) >= 3.0);
    }
    assert!(contrast(p.on_accent, p.accent) >= 4.5);
    assert!(
        !derive_from_seed(&ChromeOverrides {
            appearance: Some(Appearance::Light),
            ..Default::default()
        })
        .dark
    );
}

fn dirs() -> ThemeDirs {
    ThemeDirs {
        user: None,
        system: vec![],
    }
}

fn resolve<'a>(lib: &'a ThemeLibrary, id: &str) -> &'a ResolvedTheme {
    lib.resolve(&ChromeTheme::from_id(id)).1
}

#[test]
fn custom_toml_json_and_ghostty_keep_their_paired_colours() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path();
    fs::write(
        path.join("midnight.toml"),
        r##"
        name = "Night #1" # inline note
        appearance = "dark"
        ground = "#172030"
        ink = "#eeeeee"
        accent = '#7cc4ff'
        [terminal]
        foreground = "#abcdef"
        cursor_text = "#111111"
        ansi = [
            "#000000", "#ff0000", # a trailing comment
            "#00ff00",
        ]
        palette.1 = "#e87495"
        palette.255 = "#ffffff"
    "##,
    )
    .unwrap();
    fs::write(
        path.join("desk.json"),
        r##"{
        "name": "Desk", "appearance": "light", "ground": "#f0f4f6",
        "border_subtle": "#dde2e6",
        "terminal": {
            "background": "#e3e8ea", "foreground": "#234",
            "cursor": "#296979", "selection_foreground": "#101010",
            "selection_background": "#fc0", "ansi": ["#111", "#222"],
            "palette": {"1": "#933", "7": "#aaa"}
        }
    }"##,
    )
    .unwrap();
    fs::write(
        path.join("Minimal"),
        "background = #f6f7f9\npalette = 4=#003399\n",
    )
    .unwrap();
    fs::write(
        path.join("three.toml"),
        "ground = '#123'\nink = '#fff'\naccent = '#def'\n",
    )
    .unwrap();

    let lib = ThemeLibrary::scan(path, &dirs());
    let dark = resolve(&lib, "custom:midnight");
    assert_eq!(dark.name, "Night #1");
    assert_eq!(dark.palette.ground, 0x172030);
    assert_eq!(dark.terminal.foreground, Some(0xabcdef));
    assert_eq!(dark.terminal.cursor_text, Some(0x111111));
    assert_eq!(
        dark.terminal.palette,
        [(0, 0), (1, 0xe87495), (2, 0x00ff00), (255, 0xffffff)]
    );
    assert_eq!(dark.terminal.scheme, Some("dark"));
    let light = resolve(&lib, "custom:desk");
    assert_eq!(light.name, "Desk");
    assert_eq!(light.palette.border_subtle, Some(0xdde2e6));
    assert_eq!(light.palette.terminal_selection, 0xffcc00);
    assert_eq!(light.terminal.background, Some(0xe3e8ea));
    assert_eq!(light.terminal.selection_foreground, Some(0x101010));
    assert_eq!(
        light.terminal.palette,
        [(0, 0x111111), (1, 0x993333), (7, 0xaaaaaa)]
    );
    let ghostty = resolve(&lib, "custom:Minimal");
    assert_eq!(ghostty.name, "Minimal");
    assert_eq!(ghostty.terminal.scheme, Some("light"));
    assert_eq!(ghostty.palette.accent, 0x003399);
    let three = resolve(&lib, "custom:three");
    assert_eq!(three.terminal.background, Some(three.palette.ground));
    assert_eq!(three.terminal.foreground, Some(three.palette.ink));
    for theme in [dark, light, ghostty, three] {
        assert_legible(theme.palette);
    }
}

#[test]
fn iterm_plist_reads_real_components_and_all_terminal_roles() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path();
    fn rgb(key: &str, [r, g, b]: [f64; 3]) -> String {
        format!(
            "<key>{key}</key><dict>\
                <key>Alpha Component</key><real>1</real>\
                <key>Blue Component</key><real>{b}</real>\
                <key>Color Space</key><string>sRGB</string>\
                <key>Red Component</key><real>{r}</real>\
                <key>Green Component</key><real>{g}</real>\
            </dict>"
        )
    }
    for (id, bg, fg) in [("dark", 0.06, 0.96), ("light", 0.96, 0.06)] {
        let mut body = r#"<?xml version="1.0" encoding="UTF-8"?>
            <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
            <plist version="1.0"><dict><!-- sample -->
            <key>Unused metadata</key><array><dict><key>Flag</key><true/></dict></array>"#.to_owned();
        for (key, values) in [
            ("Background Color", [bg; 3]),
            ("Foreground Color", [fg; 3]),
            ("Cursor Color", [0.5; 3]),
            ("Cursor Text Color", [0.; 3]),
            ("Selection Color", [-0.1, 0.5, 1.1]),
            ("Selected Text Color", [1.; 3]),
        ] {
            body.push_str(&rgb(key, values));
        }
        for i in 0..16 {
            body.push_str(&rgb(&format!("Ansi {i} Color"), [0.2, 0.4, 0.7]));
        }
        body.push_str("</dict></plist>");
        fs::write(path.join(format!("{id}.itermcolors")), body).unwrap();
    }
    let lib = ThemeLibrary::scan(path, &dirs());
    for id in ["dark", "light"] {
        let theme = resolve(&lib, &format!("custom:{id}"));
        assert_eq!(theme.name, id);
        assert_eq!(theme.palette.dark, id == "dark");
        assert_eq!(theme.terminal.selection_background, Some(0x0080ff));
        assert_eq!(theme.terminal.selection_foreground, Some(0xffffff));
        assert_eq!(theme.terminal.cursor, Some(0x808080));
        assert_eq!(theme.terminal.cursor_text, Some(0));
        assert_eq!(theme.terminal.palette.len(), 16);
        assert_eq!(theme.terminal.palette[15], (15, 0x3366b3));
        assert_legible(theme.palette);
    }
}

#[test]
fn theme_reads_are_bounded_and_invalid_imports_fall_back_without_rewriting_settings() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path();
    for (file, body) in [
        (
            "bad.toml",
            "ground = '#123'\n[terminal]\nansi = ['#fff' '#ddd']",
        ),
        ("bad2.toml", "appearance = 'twilight'\nground = '#123'"),
        ("bad.json", r#"{"accent":"wat"}"#),
        (
            "bad.itermcolors",
            "<plist><dict><key>Cursor Color</key><array/></dict></plist>",
        ),
        ("empty", "background = nonsense"),
        (".hidden.conf", "background = #ffffff"),
        ("not..a.theme", "background = #ffffff"),
        ("duplicate.json", r##"{"ground":"#123456"}"##),
        ("duplicate.toml", "ground = '#ffffff'"),
    ] {
        fs::write(path.join(file), body).unwrap();
    }
    let oversized = path.join("oversized");
    let mut giant = b"background = #ffffff\n#".to_vec();
    giant.resize(MAX_THEME_BYTES as usize + 1, b'x');
    fs::write(&oversized, giant).unwrap();
    fs::create_dir(path.join("directory.toml")).unwrap();
    let lib = ThemeLibrary::scan(path, &dirs());
    assert_eq!(lib.entries().count(), 51);
    let id = ChromeTheme::from_id("custom:duplicate");
    assert_eq!(lib.resolve(&id).1.palette.ground, 0x123456);
    for input in [
        "custom:../other",
        "ghostty:.hidden",
        "ghostty:dir\\file",
        "custom:a\0b",
        "custom:",
        "ghostty:dir/file",
        "custom:a..b",
    ] {
        assert_eq!(
            ChromeTheme::from_id(input),
            ChromeTheme::Midnight,
            "{input:?}"
        );
    }
    fs::remove_file(path.join("duplicate.json")).unwrap();
    fs::remove_file(path.join("duplicate.toml")).unwrap();
    let settings: Settings = serde_json::from_value(serde_json::json!({
        "chrome_theme": id,
        "light_chrome_theme": "ghostty:Paper White",
        "match_system_appearance": true,
    }))
    .unwrap();
    let saved = serde_json::to_value(&settings).unwrap();
    assert_eq!(saved["chrome_theme"], "custom:duplicate");
    assert_eq!(saved["light_chrome_theme"], "ghostty:Paper White");
    assert!(saved.get("dark_chrome_theme").is_none());
    let lib = ThemeLibrary::selected(
        path,
        &dirs(),
        &[&settings.chrome_theme, &settings.light_chrome_theme],
    );
    for id in [&settings.chrome_theme, &settings.light_chrome_theme] {
        let (resolved, theme) = lib.resolve(id);
        assert_eq!(*resolved, ChromeTheme::Midnight);
        assert_eq!(theme.terminal, TerminalTheme::midnight());
        assert_eq!(id.preset().name, "Midnight");
    }
}
