# Vim copy mode (UI integration — #149 slice 2)

Completes **#149**, building on slice 1 (`#164`, `src/terminal/copy_mode.rs`, `Frame::copy`,
`Frame::display_row_start`, `Command::CopyMode`, `Model::copy_yank`, `Model::copy_ended_epoch`).
This slice wires keyboard-only Vim navigation, visual selection, scrollback search handoff,
race-free clipboard/PRIMARY yank, zero-PTY-leak input guards, and copy-cursor / mode-chip
rendering into the Rune front end. Attribution: requested and originally contributed by
`@MrSloth-dev`. Verification date: 2026-11-24 (Pacific Standard Time), branched from
`c0d583d`.

## Shipped

- **Single registered action (`src/app/actions.rs`, `src/keymap.rs`, `src/app/commands.rs`):**
  - Registers one action `pane.copy_mode` (`Remote::Never`, keeping `Spec::remote()`'s exact 8
    remotely allowed actions untouched), one Commands row (`Copy mode`), and one recordable
    `Settings → Shortcuts` row (`Copy mode` under `Text and view`).
  - Default binding: `Ctrl+Shift+X` (`Cmd+Shift+X` on macOS), identical on both Control and
    Command sets. No server-side `copy` key mode is declared or sent in `client.keymap`.
  - Entering copy mode requires a focused connected pane with a frame (informs cleanly when
    unavailable), is idempotent on an already-active pane, cancels any active floating-layer
    drag, clears IME preedit/sequences/modes, allocates a monotonically increasing non-zero
    epoch (`>= 1` and strictly `> model.copy_ended_epoch`), and sends `CopyModeOp::Begin`.
- **Layout-independent copy-mode dispatcher (`src/app/copy_mode.rs`, `src/app/input.rs`):**
  - Dispatches logical characters (supporting shifted, Option, and AltGr symbols such as `$`,
    `^`, `/`, `?`, `0`, `G`, `H`, `M`, `L`, `W`, `B`, `E`, `V`, `Y`, `N` across non-US keyboard
    layouts), special keys (`Arrows`, `Home`, `End`, `PageUp`, `PageDown`, `Backspace`, `Enter`,
    `Escape`), and `Ctrl+C`/`U`/`D`/`B`/`F`/`V` chords.
  - Preserves pending repeat counts (`1..=9999`, clamped to `MAX_COUNT`) and `g`/`y` prefixes
    across modifier-only key presses (`Key::Modifier`), swallows dead keys (`Key::Dead`), and
    suppresses post-exit key repeats and releases (including repeats of held motion keys after
    automatic non-keyboard exits) while still draining releases for keys originally pressed
    before entering copy mode.
  - Escape / `Ctrl+C` hierarchy: pending count/prefix → clear visual selection → exit copy mode.
- **Zero-PTY-leak guards (`src/app/input.rs`, `src/app/panes.rs`, `src/app/upload.rs`,
  `src/app/select_copy.rs`, `src/app/term.rs`):**
  - While copy mode is active, unbound keys, `client.key.forward`, server-owned keybindings,
    `pane.paste`, `pane.send_key`, `pane.clear`, `pane.reset`, `pane.insert_path`,
    `pane.go_to_directory`, `pane.select_all`, file drops (`drop_files` and drop-hover
    highlights), IME `Preedit`/`Commit`, middle-click PRIMARY paste, and mouse-tracking
    pointer/Shift+right-click/wheel reports are blocked or routed locally (`CopyModeOp::Scroll`).
    Left-clicking exits copy mode without leaking mouse-tracking press events.
  - `Term::try_send` defense-in-depth ends copy mode before any mutating PTY command (excluding
    previously-forwarded key releases) if `copy_mode_active` is still set.
- **Search handoff and yank delivery (`src/app/copy_mode.rs`, `src/app/input.rs`,
  `src/connection.rs`, `src/app/select_copy.rs`, `src/app/term.rs`):**
  - `/` (forward) and `?` (backward) open the pane's find bar and clear stale highlights;
    `Escape` closes the find bar and returns to copy mode; `Enter` blurs the find bar, keeps
    search highlights visible, and jumps the copy cursor straight to Ghostty's newest selected
    match (`CopyModeOp::ToMatch`, waiting for an in-flight search scan to settle in `run_data`
    if `Enter` arrives before the 16 ms tick). `n` / `N` step matches in the chosen search
    direction (respecting `Command::SearchStep(older)` sign semantics, including re-opening
    `find.last` with count and direction). Exiting copy mode clears any search opened from copy
    mode.
  - `CopyYank` payloads are delivered once per `(epoch, revision)` on `terminal_woke` to the
    clipboard (and PRIMARY selection when `terminal.primary_selection` is enabled), never
    triggered by `copy_on_selection` during visual motions. `Ctrl+Shift+C` (`pane.copy`) yanks
    the active visual copy-mode selection and never falls back to stale pointer selection text.
- **Immediate lifecycle auto-exit (`src/app/workspace.rs`, `src/app/navigation.rs`,
  `src/app/compatibility.rs`, `src/app/floating.rs`, `src/app/panes.rs`):**
  - Copy mode exits immediately on pane/tab/session/host change, panel/dialog open,
    split/new-tab/new-session, `PromptStep`, floating layer minimize/focus change, host
    disconnect/forget, terminal `restore_generation` bump, or worker auto-exit
    (`copy_ended_epoch >= active.epoch`).
- **Copy cursor and toolbar mode chip (`src/app/term.rs`, `src/app/toolbar.rs`,
  `src/app/activity_chip.rs`, `src/app/downloads.rs`, `src/app/downloads_view.rs`):**
  - Renders `gblock/copy-cursor` using `ACCENT` (`colors.search_match`) fill and `ON_ACCENT`
    (`colors.search_text`) glyph/sprite ink when focused (`CursorLook::Block`) and a 1 pt hollow
    outline when unfocused (`CursorLook::Hollow`), automatically 2 cells wide on wide CJK/emoji
    cells, hiding the hardware cursor while active.
  - Reuses `toolbar::mode_chip` (`copy`, `copy · visual`, `copy · line`, `copy · block`, plus
    pending count/prefix), floating cleanly when the toolbar is hidden and integrating into the
    unified title row on macOS without shifting terminal grid geometry.

## Evidence

- **Headless UI suite (`src/app/copy_mode_tests.rs`, 8 tests):**
  - Single action registration, `Remote::Never` + exact 8-action `Spec::remote()` invariant,
    Commands row, `Settings → Shortcuts` row, and offline/unfocused entry guards.
  - Zero-PTY-leak sweep across all 8 copy-mode sub-states (Normal, Char, Line, Block, pending
    count, pending `g`, pending `y`, open-but-unfocused find bar) covering unbound keys,
    `client.key.forward`, `pane.paste`, `pane.send_key`, dead keys, modifier-only keys, IME
    `Preedit`/`Commit`, middle-click PRIMARY paste, wheel/motion/right-click mouse tracking,
    left-click exit, and post-exit key repeats/releases.
  - Layout-independent symbol dispatch (`$`, `^`, `?`, `/`, `G`, `H`, `M`, `L`, including macOS
    Option and Windows/Linux AltGr symbols) and modifier-only preservation across pending
    counts/prefixes.
  - Full Vim workflow: `gg`, count clamping (`99999` → `9999`), Escape hierarchy, visual
    `v`/`V`/`Ctrl+V`/`o`/`y`/`Y`/`2yy`/`Enter`/`Ctrl+Shift+C`, `copy_on_selection` isolation,
    clipboard + PRIMARY delivery, and duplicate-wake deduplication.
  - Search handoff (`/` and `?`, `Escape`, `Enter` → `ToMatch`, `n`/`N` direction reversal,
    exit search cleanup).
  - Lifecycle auto-exit across non-focus-leaving shortcuts (kept active), tab/pane switch,
    `PromptStep`, worker auto-exit (`copy_ended_epoch`), `restore_generation` bump, server-side
    layer minimization, and panel open.
  - Cursor rendering (narrow + 2-cell CJK wide + hollow unfocused), zero grid shift at 640×480,
    unclipped toolbar controls, hidden toolbar floating chip, unified title row chip, and
    `Settings → Shortcuts` row.
  - Real-font visual captures (`copy_mode_draw` via `REX_TEST_SNAP_DIR`): inspected at 2× scale
    across `copy-mode-1024-block-cjk.png` (block selection over CJK/ASCII + `copy · block` chip),
    `copy-mode-640-narrow.png` (640×480 narrow toolbar + visual line selection),
    `copy-mode-cropped.png` (cropped viewport where frame rows exceed visible rows), and
    `copy-mode-light-osc11.png` (light `#f5f5f0` OSC 11 background with `ACCENT`/`ON_ACCENT`
    cursor legibility).
- **Private native X11 acceptance (`scripts/copy-mode-x11.py`, 4/4 passing checks, disposable
  Rex server `--restore=false` on a private unix socket, private HOME/XDG/TMPDIR/Xvfb/openbox,
  real `xdotool` input and independent `tkinter` X11 clipboard client):**
  - `zero-pty-leak`: raw byte dumper confirmed 0 bytes leaked across copy-mode navigation,
    unbound keys, dead/modifier sequences, and `q` exit.
  - `scrollback-visual-yank`: `Ctrl+Shift+X` → `gg` → `V` → `j` → `y` copied exact multi-line
    scrollback text (`SCROLL_ROW_01 ... \nSCROLL_ROW_02 ...`) to the X11 clipboard (and visual
    capture `x11-copy-mode-scrollback-visual.png` inspected).
  - `clean-exit-resumes-pty`: typing immediately after yank exit reached the live PTY cleanly.
  - `search-and-yy`: `?` search jumped to scrollback match `SCROLL_ROW_15` and `yy` copied
    the exact line to the X11 clipboard.
