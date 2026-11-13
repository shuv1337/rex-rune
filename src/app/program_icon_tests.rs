//! Program icons: the official grouping, a script's interpreter, every icon drawable at 1x
//! and 2x, and the tiles beside sessions.
use super::pane_verb_tests::Camera;
use super::tab_icon_tests::{element, settle};
use super::*;
use crate::app::{
    activity::{PaneProgram, tab_activity},
    program_icons::{all, by_slug, program_icon, shell},
    theme::*,
    widgets::{SESSION_TILE, program_glyph, program_tile},
};

fn slug(program: Option<&str>, executable: Option<&str>) -> &'static str {
    program_icon(program, None, executable).slug
}

#[test]
fn programs_get_the_official_clients_icons() {
    for (program, expected) in [
        ("nvim", "neovim"),
        ("vim", "vim"),
        ("vi", "vim"),
        ("claude", "claude-code"),
        ("claude-code", "claude-code"),
        ("codex", "codex"),
        ("git", "git"),
        ("docker", "docker"),
        ("top", "shell-top"),
        ("htop", "htop"),
        ("btop", "btop"),
        ("sleep", "shell-sleep"),
        ("caffeinate", "shell-caffeinate"),
        ("log", "shell-log"),
        ("head", "shell-log"),
        // Names from the official client's own table.
        ("claude code", "claude-code"),
        ("pool", "poolside"),
        ("grok-build", "grok"),
        ("agent", "cursor"),
        ("agy", "antigravity"),
        ("vibe-rs", "mistral"),
        ("glab-tui", "gitlab"),
        ("pub", "dart"),
        ("fvm", "flutter"),
        ("pass-cli", "proton-pass"),
        ("./proton-drive", "proton-drive"),
        ("./gradlew", "gradle"),
        ("UE4Editor", "unreal-engine"),
        ("python2.7", "python"),
        ("python3.15", "python"),
        ("zsh", "shell"),
        ("bash", "shell"),
        ("fish", "shell"),
        ("ssh", "shell"),
        ("mystery", "shell"),
        ("", "shell"),
        // Case, paths, `.exe`, a login shell's dash and a version don't change the program.
        ("NVIM", "neovim"),
        ("/usr/bin/git", "git"),
        ("C:\\Program Files\\Git\\git.exe", "git"),
        ("-zsh", "shell"),
        ("python3.12", "python"),
        ("node22", "node"),
        // A name that only looks like a version keeps its own icon.
        ("k9s", "k9s"),
        ("d2", "d2"),
        // And a version doesn't make an unknown name a known one.
        ("x264", "shell"),
        ("ue5", "shell"),
    ] {
        assert_eq!(slug(Some(program), None), expected, "{program}");
    }
    assert_eq!(slug(None, None), "shell");
    // Shell and its variants are only ever chosen, never a command's own name.
    assert_eq!(slug(Some("shell"), None), "shell");
    assert_eq!(slug(Some("shell-top"), None), "shell");
}

#[test]
fn a_script_without_an_icon_shows_its_interpreter() {
    assert_eq!(slug(Some("sq"), Some("node")), "node");
    assert_eq!(slug(Some("tool"), Some("python3")), "python");
    // The program's own icon wins over its interpreter's.
    assert_eq!(slug(Some("pi"), Some("node")), "pi");
    assert_eq!(slug(Some("sq"), Some("mystery")), "shell");
    // Scripts known by where they live, as the official client knows them, before their
    // interpreter; their own name still comes first.
    let script = |path: &str| program_icon(Some("cli.js"), Some(path), Some("node")).slug;
    assert_eq!(
        script("/opt/homebrew/lib/node_modules/@github/copilot/npm-loader.js"),
        "copilot"
    );
    assert_eq!(script("/u/.npm/pi-coding-agent/dist/cli.js"), "pi");
    assert_eq!(script("pi-coding-agent/dist/bundle/cli.js"), "pi");
    assert_eq!(script("/u/.local/share/vibe/_bin/vibe-rs"), "mistral");
    assert_eq!(
        script("/u/not-copilot/npm-loader.js"),
        "node",
        "only at a /"
    );
    assert_eq!(script("/u/app/dist/cli.js"), "node");
    assert_eq!(
        program_icon(Some("claude"), Some("/x/copilot/npm-loader.js"), None).slug,
        "claude-code"
    );

    let (mut ws, _host) = tabbed_workspace("single");
    let model = &mut ws.hosts[0].model;
    model.programs.insert("block:a".into(), "sq".into());
    model.executables.insert("block:a".into(), "node".into());
    assert_eq!(PaneProgram::of(model, "block:a").icon().slug, "node");
    model
        .programs
        .insert("block:a".into(), "npm-loader.js".into());
    model
        .scripts
        .insert("block:a".into(), "/n/@github/copilot/npm-loader.js".into());
    assert_eq!(PaneProgram::of(model, "block:a").icon().slug, "copilot");
    let tab = model.views["s"].windows[0].clone();
    assert_eq!(tab_activity(model, &tab).icon().slug, "copilot");
    model.scripts.remove("block:a");
    model.programs.insert("block:a".into(), "sq".into());
    let tab = model.views["s"].windows[0].clone();
    let activity = tab_activity(model, &tab);
    assert_eq!(
        activity.program.as_deref(),
        Some("sq"),
        "the name shown stays"
    );
    assert_eq!(activity.icon().slug, "node");
    assert_eq!(ws.session_icon(0, "s").slug, "node");
    std::fs::remove_file(&ws.settings_path).ok();
}

#[test]
fn every_slug_is_unique_and_every_icon_paints_at_1x_and_2x() {
    let slugs: std::collections::HashSet<_> = all().iter().map(|i| i.slug).collect();
    assert_eq!(slugs.len(), all().len());
    assert!(all().len() >= 150);
    assert!(std::ptr::eq(shell(), by_slug("shell").unwrap()));
    let (ws, _host) = tabbed_workspace("single");
    let path = ws.settings_path.clone();
    let mut rt = runtime_at(ws, (640., 480.));
    for icon in all() {
        for px in [15, 18, 30, 36] {
            let white = rgb(0xffffff);
            for (what, drawn, one) in [("glyph", &icon.glyph, true), ("tile", &icon.tile, false)] {
                let image = rt.text.color_icon(drawn, px, white, one);
                let size = image.map(|p| p.size());
                assert!(
                    size.is_some_and(|(w, h)| w == px && h == px),
                    "{} {what} at {px} px: {size:?}",
                    icon.slug
                );
                // A lost mask or an empty trace still makes an image of the right size.
                let rgba = image.unwrap().rgba();
                let painted = rgba.chunks(4).filter(|p| p[3] > 0).count();
                assert!(
                    painted * 20 >= (px * px) as usize,
                    "{} {what} at {px} px paints {painted} of {} pixels",
                    icon.slug,
                    px * px
                );
            }
        }
    }
    std::fs::remove_file(&path).ok();
}

#[test]
fn sessions_show_their_programs_tile_in_the_sidebar_and_switcher() {
    let (mut ws, _host) = tabbed_workspace("split");
    let model = &mut ws.hosts[0].model;
    let tab = model.views["s"].windows[1].clone();
    let focused = tab.focused_block_id.clone().unwrap();
    model.programs.insert(focused, "nvim".into());
    let view = model.views.get_mut("s").unwrap();
    view.active_window_id = tab.window_id.clone();
    assert_eq!(ws.session_icon(0, "s").slug, "neovim");
    assert!(
        std::ptr::eq(ws.session_icon(0, "missing"), shell()),
        "no layout, no program"
    );
    if !ws.sidebar.is_open() {
        ws.sidebar.toggle();
    }
    let mut rt = runtime_at(ws, (1024., 700.));
    settle(&mut rt);
    let row = element(&rt, "session:0:s");
    let tile = rt.elements().into_iter().find(|e| {
        !e.exiting
            && e.path.starts_with(&format!("{}/", row.path))
            && e.rect[2] == SESSION_TILE
            && e.rect[3] == SESSION_TILE
    });
    assert!(tile.is_some(), "a tile before the session's name");
    rt.update(|s, tx| s.perform(Intent::Switcher, tx));
    settle(&mut rt);
    let entry = rt
        .state
        .entries()
        .into_iter()
        .find(|e| matches!(&e.action, Intent::Select(0, id) if id == "s"))
        .expect("the session's row");
    assert_eq!(entry.program.map(|p| p.slug), Some("neovim"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// Every icon as Rex draws it: tile, ink glyph on the toolbar ground and tinted glyph, at 1x
/// and 2x. Run individually with `REX_TEST_SNAP_DIR`.
#[test]
fn program_icons_sheet_draw() {
    let Some(mut camera) = Camera::new() else {
        return;
    };
    fn sheet(_: &Workspace, _: &mut rune::Cx<Workspace>) -> rune::El {
        let mut grid = rune::col().padding(16.).gap(6.).fill(rgb(ground()));
        for chunk in all().chunks(12) {
            let mut line = rune::row().gap(6.);
            for icon in chunk {
                line = line.child(
                    rune::col()
                        .w(92.)
                        .h(78.)
                        .gap(4.)
                        .center()
                        .radius(8.)
                        .fill(white(0.04))
                        .child(program_tile(icon, 36.))
                        .child(
                            rune::row()
                                .gap(6.)
                                .child(program_glyph(icon, 15., rgb(ink())))
                                .child(program_glyph(icon, 15., rgb(muted())))
                                .child(program_glyph(icon, 15., rgb(icon.tint))),
                        )
                        .child(
                            rune::text(icon.name)
                                .font(ui(10.))
                                .color(rgb(muted()))
                                .truncate(),
                        ),
                );
            }
            grid = grid.child(line);
        }
        grid
    }
    let _families = rune::text::with_families("Inter", "Inter");
    let rows = all().len().div_ceil(12) as f32;
    let size = (
        32. + 12. * 92. + 11. * 6.,
        32. + rows * 78. + (rows - 1.) * 6.,
    );
    for scale in [1., 2.] {
        let (ws, _host) = tabbed_workspace("single");
        let path = ws.settings_path.clone();
        let fonts = crate::app::fonts::load("IBM Plex Mono", &[]);
        let mut rt = Runtime::with_text(
            ws,
            sheet,
            size,
            Env::new(true, scale),
            rune::text::TextSystem::new(fonts.files, fonts.families),
        );
        settle(&mut rt);
        camera.snap(&mut rt, &format!("program-icons-{scale}x"));
        std::fs::remove_file(&path).ok();
    }
}
