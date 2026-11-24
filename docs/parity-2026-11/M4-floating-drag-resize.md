# M4 — floating panes, slice 2: pointer drag and 8-way edge resize

Completes **#136**, building on [slice 1](M4-floating-layers.md) (server actions and lifecycle).
This slice adds pointer-driven **drag-move** from a floating pane's header and **8-way
edge/corner resize** on API revision 13 hosts via `session.resize_layer`, fixes per-layer
tiled-divider z-order under floating cards, preserves the server-authoritative model, API 13
gate (`layers::MIN_API_REVISION = 13`) and strict host/epoch/layer-identity validation, and
proves quantitative two-client geometry convergence natively on X11. Verification date:
2026-11-23 (Pacific Standard Time), branched from `87028e9`.

## Live API-13 contract observations (Step 0)

Checked directly against a disposable API-13 Rex server (`0.0.1794323808-g757adf`, commit
`757adf99`, `api_revision = 13`) on a private unix socket before implementation:

1. **Unit-square bounds check is exact in `f64`:** `x + w == 1.0` and `y + h == 1.0` (including
   `[0.3, 0.2, 0.7, 0.8]` and `[0.0, 0.0, 1.0, 1.0]`) are accepted. Exceeding `1.0` by even
   1 ulp of `1.0` (`1.0000000000000002`, as well as `1e-15`..`1e-6`, negative `x`/`y`, or
   zero `w`/`h`) is rejected with `status=400 code=bad_request`:
   `bounds must be a finite normalized rectangle with positive size contained within the unit square`.
2. **Small sizes are accepted:** widths and heights down to `0.05`, `0.01`, `0.001` and `1e-6`
   are accepted by the server; the client's `160×96` pt minimum (`layers::MIN_FLOATING_PT`) is
   a client ergonomics guard that never forces a smaller layer to grow.
3. **`session.view` echoes sent bounds bit-identically:** non-dyadic values
   (`[0.1234567890123456, 0.2345678901234567, 0.4567890123456789, 0.543210987654321]`) came
   back bit-identical in `session.view`.
4. **Identical bounds are a no-op:** sending the layer's existing bounds leaves `revision`
   unchanged (`rev_before == rev_after`).
5. **Second attached client receives `session_view_changed`:** calling `session.resize_layer`
   from one client broadcasts `session_event` → `session_view_changed` with the new revision
   over `/api/control` (`rex.control.v1`) to other attached clients; an identical-bounds call
   emits no view-change event.
6. **Focus, z-order and zoom stay unchanged:** calling `session.resize_layer` on a
   non-frontmost layer leaves `focused_block_id` and floating-layer stacking order unchanged;
   calling it while a block is zoomed succeeds and leaves `zoomed_block_id` unchanged. Because
   `session.resize_layer` never raises, dragging or resizing a non-frontmost layer sends
   `session.raise_layer` immediately before `session.resize_layer` on commit.

## Shipped

- **Per-layer z-order (`src/layout.rs`, `src/app/panes.rs`):** `layout::Placement` and
  `layout::Divider` carry `layer_index: usize`. `panes::area` emits each visible layer back to
  front: that layer's cards, then that layer's own split dividers, then (for floating layers on
  API >= 13 drawn at their bounds) its outer 8-way resize grips, and finally the active ghost
  outline on top. A tiled split divider beneath a floating card no longer steals pointer input
  through the card, while multi-pane floating layers keep their inner `session.resize_pane`
  dividers alongside their single outer layer resize grips.
- **Pure floating geometry and strict `sendable` clamping (`src/layers.rs`):** `Grip` (Move +
  8 resize edges/corners), `bounds`, `min_fraction` (`MIN_FLOATING_PT = (160, 96)` pt, `None`
  for areas `< 1` pt), `dragged` (Move keeps width/height bit-identical; all 8 resize grips
  hold the opposite edge fixed, clamp to `[0, 1]`, and respect `min` without forcing a layer
  already smaller than `min` to grow), `sendable` (guarantees finite positive bounds with
  `x + w <= 1.0` and `y + h <= 1.0` strictly in `f64`, stepping down via `f64::next_down()` if
  needed), and `Blocked::ChangedDuringDrag` (keeping slice 1's `Blocked::Stale` untouched).
- **Epoch- and revision-checked wire dispatch (`src/connection.rs`):**
  `HostCommand::ResizeLayer { epoch, session, layer, bounds, raise_first }` re-checks the live
  `connection_epoch`, `control.welcome().api_revision >= layers::MIN_API_REVISION`, and
  `layers::sendable(bounds)` in the host worker right before sending optional
  `session.raise_layer` and `session.resize_layer`. It is never replayed across disconnect or
  reconnect.
- **Header drag-move and 8-way edge/corner resize (`src/app/floating_drag.rs`, `panes.rs`,
  `pane_header.rs`, `workspace.rs`, `input.rs`, `mod.rs`):**
  - Header drag (`Grip::Move`) attaches on floating pane headers drawn at their bounds (API >=
    13, not `full_bleed`, not zoomed, no panel open), with a 4 pt drag slop so shorter clicks
    keep existing click-to-focus and send no layer request. Header buttons (Split right, Split
    down, Zoom, Close), the Retry pill, and right-click keep their presses/menu and start no
    drag. `pane_header::placement` keeps an `OnHover` header mounted while grabbed.
  - 8-way resize grips (`n`, `s`, `e`, `w`, and L-shaped `nw`, `ne`, `sw`, `se`) wrap each
    floating layer's outer `card_rect` (4 pt outside, 3 pt inside; L-shaped 12 pt corner arms
    so top-right NE never overlaps the Close button) with `Cursor::ResizeEdge(WindowEdge::...)`.
  - **Ghost outline preview and commit-on-release:** while active, draws an inert accent ghost
    outline (`1.5 pt` accent border, card radius, square in Compact) at the proposed rectangle
    without resizing the live card or sending speculative PTY resizes. `Phase::Down` is the
    only phase that starts a drag; cancellation (Escape without leaking to PTY, pointer cancel,
    `window_focus(false)`, panel open, host/session switch, disconnect, epoch change, layer
    removed/minimized/zoomed/bounds-changed remotely, or degenerate area) clears `layer_drag`,
    sends zero layer requests, and never re-arms on later motion.
  - **API 10–12 and disconnected hosts:** emit zero resize grips, attach no header drag, and
    send zero layer requests. No new shortcuts, Commands rows, `Spec::remote()` changes (remote
    allowed set stays 8), or Settings controls.

## Evidence

- **Pure geometry and layout tests:** 11 tests in `src/layers.rs` (including deterministic
  sweeps across starts, areas, all 9 grips and extreme deltas verifying `sendable` exact
  `x + w <= 1.0` & `y + h <= 1.0`, bit-exact move size, opposite-edge invariance, smaller-than-
  min preservation, and `next_down()` overshoot stepping) and 7 tests in `src/layout.rs`
  (including `layer_index` tagging across tiled, minimized, and floating layers).
- **Worker wire tests (`src/connection/server_controls_tests.rs`):** verifies `raise_first`
  ordering (`session.raise_layer` then `session.resize_layer`), single `session.resize_layer`
  when frontmost, stale-epoch blocking before the wire, and API-12 worker-side refusal before
  the wire.
- **Headless Rune UI tests (`src/app/floating_tests/drag.rs`, 13 tests alongside slice 1's 51
  tests):** header move ghost + exact command + decoy host isolation; non-frontmost
  `raise_first`; click `< 4 pt` slop focusing without layer requests; header buttons, Retry
  pill, right-click, and disjoint NE L-corner vs Close button; all 8 resize grips + cursors +
  exact bounds; 10-case cancellation matrix (Escape without PTY leak, pointer cancel, window
  focus loss, panel open, disconnect, epoch change, layer gone, layer minimized, tab zoomed,
  remote bounds change) with post-cancel motion never re-arming; API 10–12 + disconnected zero
  grips / zero commands; tiled divider under floating card blocked from stealing input;
  multi-pane floating layer outer grips + inner split divider coexistence; `OnHover` header
  retention while grabbed; 640×480 Compact extremes; lone full-bleed zero grips; real-font
  ghost captures (`floating_drag_ghost_draw`, `floating_resize_ghost_draw`).
- **Native X11 two-client acceptance (`scripts/floating-drag-private.py`, 21/21 passing checks,
  disposable API-13 Rex server on a private unix socket, private HOME/XDG/TMPDIR/Xvfb/openbox,
  two `1024×700` clients, real `xdotool` pointer/keyboard input):**
  - Baseline: Client A `[165, 133, 544, 342]`, Client B `[165, 133, 544, 342]`, `|A - B| = 0 px`,
    within `<= 0.9 px` of authoritative bounds.
  - Client A header move (`+120, +60` px): server bounds matched to floating-point precision
    with size bit-identical (`0.55 × 0.55`); Client A & B converged at `[285, 193, 544, 342]`
    (`|A - B| = 0 px`).
  - Client A East edge resize (`-140` px): width shrank `0.55 → 0.410558`, left/top/height
    unchanged, PTY columns shrank `58 → 43`, Client A & B converged at `[285, 193, 404, 342]`
    (`|A - B| = 0 px`).
  - Client B SouthEast corner resize (`+110, +75` px): top-left unchanged, width/height grew to
    `0.520120 × 0.667555`, PTY grew to `55×17` matching `stty size` `(17, 55)`, Client A & B
    converged at `[285, 193, 514, 417]` (`|A - B| = 0 px`).
  - Escape mid-drag: server revision (`19`) and bounds unchanged; raw TTY probe confirmed zero
    bytes (`ESC_BYTES=`) leaked to the PTY.
  - Raise-on-grab: dragging a non-frontmost floating layer raised it to front and applied the
    moved bounds.

## Documented limits

- **Lone full-bleed floating pane:** when a tab has a single visible pane that happens to be
  floating (`full_bleed == true`, e.g., empty tiled layer with `PaneHeader::Auto`), it continues
  to draw edge to edge as before with no floating resize grips or header drag.
- **No snapping, magnetic docking, drag-to-dock, drag-between-tabs, or double-click-to-zoom**
  on floating headers.
- **API 10–12 hosts** remain attach-and-render only for floating layers (zero grips, zero
  header drag, zero layer requests); API 14 is not claimed.
- **macOS and Windows** native pointer drag/resize were not manually run; their builds and the
  full headless UI suite are covered by CI.
