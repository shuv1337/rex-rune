# Pane header program icon

PN02 in [gaps/d1-workspace-chrome.md](gaps/d1-workspace-chrome.md): the official Rex for macOS
draws each pane's program icon before its title in every pane header (`>_` + title 13 pt
regular, 22 pt buttons). Rex Rune drew the program glyph in toolbar tabs, in sidebar tab rows
and in the macOS title row, which is a lone pane's header there, but a pane header drawn by the
client (a split pane's card, a lone pane's Auto row, and a floating header) showed only its
name.

| Split panes, 1024×700 | A lone pane, edge to edge |
| --- | --- |
| ![Both split headers with their own glyphs](images/pane-header-icon-split.png) | ![A lone pane's header row](images/pane-header-icon-single.png) |

![The On hover header, revealed over the terminal](images/pane-header-icon-floating.png)

![Show tab icons at 0 removes the header glyph](images/pane-header-icon-off.png)

![The 640×480 minimum size](images/pane-header-icon-minimum-split.png)

![A reconnecting pane's header: no glyph, and Retry clear of the buttons](images/pane-header-icon-reconnect.png)

The first four are 1024×700 and the last two 640×480, default settings, in the built headless
client with Inter and IBM Plex Mono; in the last, the left pane is reconnecting, at the 31%
width the strip's check below uses. The two panes are `nvim` (Neovim's N) and `git` (its own
diamond), so a header showing its neighbour's glyph would be visible; the lone pane runs a
shell, whose glyph is the `>_` the official client draws for it too.

## What changed

- Every pane header draws that pane's own program glyph before its name: split panes, a carded
  single pane (Pane headers → Always), a lone pane's Auto row, the floating On hover header and
  one brought out by Reveal pane headers. It is `pane_header`'s placement that decides, so a
  header never gains or loses its glyph with anything but its own row.
- It uses the tabs' own path: `PaneProgram::of` for that pane's block (its deciding status
  record's app, then its foreground program), resolved by `program_icons`, and
  `tab_icons::append_restoring` for the cell, so the mapping, dedup, 15 pt slot at 13 pt and
  8 pt gap are the title row's. The cell carries the program's name as its tooltip, as a tab's
  does, and its glyph mixes back to its own resting colour while the header is hovered, as the
  name beside it does.
- **Show tab icons** governs it: `interface.tab_icon_count` 0 draws no glyph and leaves no gap;
  any count above 0 draws one, because a header has one pane to speak for rather than the tab's
  count. This is exactly what the macOS title row does (`…min(1)`).
- **Use colour for tab icons** applies with the tabs' rule. A header's glyph is lit with its
  pane (its pane has the keyboard: `INK`, otherwise `MUTED`, as its name is), so Active pane and
  Active tab tint a header only where its pane has the keyboard. See the limits below.
- Header fading multiplies only the glyph's own colour alpha, with the name: 0.85 when the pane
  is not focused (Fade inactive pane headers) or the window is not focused (Fade headers in
  inactive windows). Hovering the header, or the glyph itself, restores both, as it does the
  buttons' icons. The buttons keep their existing cues and the row itself is never faded.
- The glyph is given only the room the row will actually have. `pane_header::glyph_fits` adds
  up the header's leading pad and trailing pad, the 8 pt gap between every child, the 15 pt
  glyph, room for the name's first letters, the buttons the header draws (Split right and Split
  down join Zoom and Close from 200 pt wide) and what cannot shrink beside them: the activity
  word and, when a strip is drawn, its Retry label, each measured with Rune's own text
  measurement (`rune::text::measure`, which rounds widths up) at the size it is drawn; the
  `Zoomed` chip's 56 pt; and beside a reconnecting strip the pane name's room as well, because
  the name and the strip share what the row has left while Retry keeps its width. The progress
  bar is pinned across the row out of its flow and takes none. When that does not fit, the
  header keeps its name and buttons. So at 640×480 a 15% pane (about 89 pt) drops it, and so
  does a Blocked pane drawing “Needs input” at 25% (151 pt, two buttons) or 33% (200 pt, four
  buttons), where the row without the glyph still fits; a reconnecting pane drops it until its
  row is about 278 pt wide (more for a longer name), which covers the widths where Retry would
  otherwise fall on Zoom and Close or Split right. The glyph never pushes Close past a pane's
  edge or Retry over the buttons.
- Unchanged: the 30 pt row, the four buttons, the activity word, the Zoomed chip, the progress
  bar, the reconnect strip, name truncation (a long name still gives way first and keeps its
  whole text as the tooltip), the header's appearance rules, and the macOS title row, which
  already drew the glyph.

The terminal does not move: the header row's height and the pane rectangles are the same with
the glyph on or off. Nothing is sent to the server for any of this.

## Checks

- `cargo fmt --all -- --check`; `cargo clippy --locked --all-targets --all-features -- -D
  warnings`; `cargo test --locked --all-features` **1163 passed, 0 failed, 6 ignored**. This is
  shared code: it is compiled on Linux here, and macOS and Windows are left to CI; no OS `cfg`
  was added.
- `pane_header_icon_tests.rs` (new):
  - `split_headers_draw_their_own_panes_program_glyph`: each header's glyph names its own
    pane's program (Neovim against Git), is one 15 pt slot before the name, centred with it and
    inside its row, is `INK` on the pane with the keyboard and `MUTED` on the other, and under
    Active pane only the focused pane's is tinted.
  - `every_header_placement_draws_the_glyph_and_hidden_ones_do_not`: Auto and Always rows, a
    floating On hover header and one brought out by Reveal pane headers (on a Hidden pane too)
    draw it inside their own row; Hidden with nothing revealed draws no header and no glyph,
    and Compact's Auto is unchanged.
  - `tab_icon_count_governs_the_glyph_and_never_moves_the_grid`: counts 1, 0, 3, 0 and 1 leave
    every grid and header rectangle identical; 0 removes the glyph and its gap (the name moves
    back to the card's edge), and a header draws exactly one glyph cell, never the tab's count.
  - `a_narrow_header_keeps_its_buttons_and_drops_the_glyph`: a 15% pane at 640×480 has no
    glyph and its Close still ends inside the pane, while the wide pane keeps its glyph.
  - `an_activity_word_drops_the_glyph_before_it_pushes_close_out`: a Blocked record drawing
    “Needs input” on a 25% and a 33% pane of a 640×480 split (about 151 pt, two buttons, and
    200 pt, four) drops the glyph, keeps the whole word, and leaves Close inside the pane; with
    Show tab icons at 0 the same headers keep Close inside too, so the glyph never makes the row
    worse. Before this rule the glyph put Close's edge 23 pt outside the 151 pt pane.
  - `the_longest_activity_words_drop_the_glyph_and_leave_close_where_it_was`: “Needs sign-in”
    at 29% (about 170 pt, two buttons) and “Paused 100%” at 36% (about 219 pt, four), the
    longest words a header draws, drop the glyph, keep Close inside the pane, and leave Close
    and the row exactly where the same header without a glyph has them.
  - `the_reconnecting_strip_drops_the_glyph_before_retry_meets_the_buttons`: on a 31% and a 40%
    pane (about 188 pt, two buttons, and 244 pt, four) the reconnecting strip drops the glyph
    and its Retry pill ends before Zoom or Split right, as it does with Show tab icons at 0.
    Before this rule the glyph drew Retry over Zoom and Close at about 185–197 pt and over
    Split right at about 232–275 pt, where the same header without the glyph cleared them.
  - A sweep of 177 split widths from 8% to 52% at 640×480, for every activity word, the strip
    and the strip with a word, in rows, cards and floating headers, with short and long names:
    no width drew the glyph where it put Close further outside the pane than the same row with
    Show tab icons at 0 (the worst difference was 0), and every header that drew the glyph kept
    Close at least 8 pt inside the pane (9 pt on a floating header). A sweep of the reconnecting
    strip's own squeeze found the strip and its Retry pill in exactly the same place with or
    without a glyph. Both sweeps were temporary review tests and are not committed.
  - `a_long_pane_name_truncates_between_its_glyph_and_its_buttons`: a 536-character name leaves
    the terminal grid where it was, is cut before Split right, keeps its whole text as the
    tooltip, and all four buttons and the glyph stay.
  - `pane_header_icon_draw_large`, `pane_header_icon_draw_minimum` and
    `pane_header_icon_draw_reconnect`: the captures above, run individually with
    `REX_TEST_SNAP_DIR`. The five older captures are pixel-identical after this change.
- Updated: `header_fading_tests.rs` treats the header glyph as a faded icon (with the name and
  the buttons' icons) rather than an unchangeable cue, and its hover test moves the pointer onto
  the glyph itself and expects it back at its resting colour; `pane_header_tests.rs` pins the lone
  pane's glyph at the terminal padding and its name 23 pt after, and
  `tab_icon_colour_tests.rs` excludes the header's glyph from the elements the colour choice
  must leave alone.

## Limits

- Only the focused pane's glyph is tinted under Active pane and Active tab. In tabs those modes
  ask whether the tab is on screen, and a header is always on screen; its tab-like "active"
  flag is the pane's own focus, which is also what decides `INK` or `MUTED`, as in the macOS
  title row. Nothing renders shell-provided icon themes; that remains deferred.
- The glyph's room is a client-side rule of ours, worked out from the row's own pads, gaps and
  buttons rather than something the official header was seen to do; the official evidence has
  one pane per window and no narrow split. Rune can measure text while the row is built, so the
  rule uses the runtime's own measurement (`rune::text::measure`, rounded up to whole points)
  for the activity word at 11 pt (Needs approval, Question, Needs sign-in, Needs input, Error,
  Paused or Paused 100%) and the strip's Retry label at 11.5 pt, and reserves the room they
  draw. Beside a reconnecting strip it is deliberately generous: the whole pane name's room is
  reserved, so such a header can drop the glyph where the row would in fact have held it. The
  glyph never takes room from Close or the strip's Retry. What a reconnecting strip does with
  no glyph is unchanged: on a row narrower than about 185 pt the strip itself is squeezed.
- The glyph is the Shell `>_` for an unnamed or unknown program, as on its tab. A pane whose
  program has not been read yet therefore looks like a shell.
- Not verified: Wayland, a HiDPI display, a screen reader, macOS or Windows by hand. No native
  pointer script was re-run: they pin Pane headers → Hidden, so their grids are the terminal's,
  and this change moves no terminal.
