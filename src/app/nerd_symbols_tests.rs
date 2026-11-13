//! Real-font evidence for the bundled Symbols Nerd Font: a Powerline prompt, and each prompt
//! glyph measured against the cells Rex's grid gives it. Run individually with
//! `REX_TEST_SNAP_DIR` (fonts are process-global); without it nothing is drawn.
use super::pane_verb_tests::{Camera, framed_terminal, rect};
use super::*;

/// The glyphs a prompt draws, how many cells each may cover (Nerd Fonts draws its wide icons
/// for a space after them) and how far, in cells, its outline may overlap the cells either
/// side: the Powerline arrow's does by design, so adjoining segments leave no seam.
const GLYPHS: [(char, usize, f32); 4] = [
    ('\u{e0a0}', 1, 0.),   // branch
    ('\u{e0b0}', 1, 0.12), // Powerline arrow
    ('\u{f07b}', 2, 0.),   // folder
    ('\u{f113}', 2, 0.),   // GitHub
];
/// The columns of the measured row each glyph starts at.
const STRIDE: usize = 5;

fn screen() -> Vec<u8> {
    let mut out = String::from("\r\n");
    for (ch, span, _) in GLYPHS {
        // White on a red box as wide as the glyph may draw, with blank cells between.
        out.push_str(&format!(" \x1b[97;41m{ch}{}\x1b[0m", " ".repeat(span - 1)));
        out.push_str(&" ".repeat(STRIDE - 1 - span));
    }
    out.push_str("\r\n\r\n");
    out.push_str(
        "\x1b[97;44m \u{f07b} ~/rex \x1b[34;42m\u{e0b0}\x1b[30;42m \u{e0a0} main \
         \x1b[32;49m\u{e0b0}\x1b[0m \u{f113} $ ",
    );
    out.into_bytes()
}

#[test]
fn nerd_symbols_draw() {
    let Some(mut camera) = Camera::new() else {
        return;
    };
    let fonts = crate::app::fonts::load("IBM Plex Mono", &[]);
    let _families = rune::text::with_families("Inter", fonts.families.1);
    let (mut ws, _host) = tabbed_workspace("single");
    let (sender, _terminal) = flume::unbounded();
    framed_terminal(&mut ws, "block:a", &sender, &screen());
    let size = (720., 360.);
    ws.viewport = [size.0, size.1];
    let scale = 2.;
    let mut rt = Runtime::with_text(
        ws,
        view,
        size,
        Env::new(true, scale),
        rune::text::TextSystem::new_with_paths(fonts.files, fonts.paths, fonts.families),
    );
    for _ in 0..90 {
        rt.frame(1. / 60.);
    }
    let pixels = camera.snap(&mut rt, "nerd-symbols");
    let width = (size.0 * scale) as usize;
    let grid = rect(&rt, "grid:block:a");
    let (cw, ch) = crate::app::term::metrics(rt.state.settings.font_size).cell;
    let px = |points: f32| (points * scale).round() as usize;
    // Rows 0 to 2 hold only the measured glyphs, so all white ink there is theirs.
    let (top, bottom) = (px(grid[1]), px(grid[1] + 3. * ch));
    let (mut report, mut outside) = (Vec::new(), Vec::new());
    for (i, (glyph, span, overlap)) in GLYPHS.into_iter().enumerate() {
        let col = 1 + i * STRIDE;
        let (left, right) = (
            grid[0] + col as f32 * cw,
            grid[0] + (col + span) as f32 * cw,
        );
        let (mut x0, mut x1, mut y0, mut y1) = (usize::MAX, 0, usize::MAX, 0);
        for y in top..bottom {
            for x in px(left - cw)..px(right + cw) {
                let p = &pixels[(y * width + x) * 4..][..3];
                if p.iter().all(|c| *c > 200) {
                    (x0, x1, y0, y1) = (x0.min(x), x1.max(x + 1), y0.min(y), y1.max(y + 1));
                }
            }
        }
        assert!(x1 > 0, "U+{:04X} drew nothing", glyph as u32);
        // In cells from its first cell's left edge, and from the top of its row.
        let cells = |x: usize| (x as f32 / scale - left) / cw;
        let rows = |y: usize| (y as f32 / scale - grid[1]) / ch - 1.;
        let ink = (cells(x0), cells(x1), rows(y0), rows(y1));
        report.push(format!(
            "U+{:04X}: x {:.3}..{:.3} of {span} cell(s), y {:.3}..{:.3} of its row",
            glyph as u32, ink.0, ink.1, ink.2, ink.3
        ));
        // And a device pixel of antialiasing.
        let slack = overlap + 1. / scale / cw;
        if ink.0 < -slack || ink.1 > span as f32 + slack || ink.2 < 0. || ink.3 > 1. {
            outside.push(glyph);
        }
    }
    eprintln!("cell {cw}×{ch} pt at {scale}×\n{}", report.join("\n"));
    assert!(outside.is_empty(), "leave their cells: {outside:?}");
    std::fs::remove_file(&rt.state.settings_path).ok();
}
