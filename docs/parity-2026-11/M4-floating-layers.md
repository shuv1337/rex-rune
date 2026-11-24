# M4 — floating panes, slice 1: server actions and lifecycle (partial)

Part of **#136**, option B (a usable single-pane MVP). This record covers **slice 1 only**:
the layer verbs, their Commands and pane-menu rows, the cached floating-layer list and the
close confirmation. Pointer drag and edge resize are **slice 2** and are not here; #136 is not
finished by this slice. Verification: 2026-11-15, Pacific Standard Time, in a worktree based on
`fcc1974` with `2253e7e` and the review corrections on top. Every native receipt records the
source and binary it came from (`identity.json`: commit, whether the tree was dirty and which
files, the client binary's SHA-256, the `rex` binary's SHA-256, the server's version, commit and
API revision, and the commands run), so an exact head is never inferred from a file's
timestamp.

## Shipped

- **Float pane** (`pane.float`) moves the focused pane's block into a new floating layer of its
  own tab with `session.float_block`. The request states a centred rectangle
  (`[0.15, 0.12, 0.7, 0.7]` of the window); the server reports the real bounds, and the client
  never draws its own. The block, its program, its scrollback and its stream are the same one;
  focus does not change and any zoom ends (the server's rule).
- **New floating pane** (`pane.new_floating`) starts **one** terminal in a new floating layer
  of the tab on screen with `session.new_layer`, through the same creation path as Split: the
  host's shell, this client's declared theme, and the focused pane's folder. It names the
  active tab's `window_id` explicitly, so another client switching tabs cannot move it. From a
  pane menu it names **that menu's** tab and folder instead, so the pane the menu was opened on
  decides where the terminal starts even while the keyboard is elsewhere; a menu whose pane,
  tab, session or connection epoch has changed since it was opened sends nothing and says the
  layer changed. It never creates a session, tab or server, and with no tab open it is
  unavailable.
- **Minimize floating layer** (`pane.minimize_layer`) sends one `session.set_layer_state` with
  `minimized`. When the layer holds the tab's **only visible panes** the row is disabled and
  says so ("Only visible panes"), because Rex keeps a window visible; nothing is sent and the
  view is not edited locally. The count uses every visible layer, not the zoom projection.
- **Restore and focus floating layer** (`pane.restore_layer`) sends one
  `session.set_layer_state` with `normal` and `focus: true`: the server restores, focuses,
  raises and activates the window in one revision. It works on an already-normal layer too, and
  is never composed from two requests.
- **Bring floating layer to front** (`pane.raise_layer`) sends `session.raise_layer` alone. It
  never focuses, and is named for raising only; on the frontmost layer it is disabled ("Already
  in front").
- **Return pane to tiles** (`pane.return_to_tiles`) sends `session.move_block` with
  `direction: horizontal`, `side: after`, `ratio: 0.5` and the anchor the client chose:
  **the first block of the tab's tiled layer in layout order, never the moving pane**. The
  server accepts an anchor in any layer; this client's policy is a tiled one in the same tab,
  so it will not detach a pane beside another floating one. With no tiled pane it is disabled
  ("No tiled pane beside it") and sends nothing — no placeholder pane, no restart, and #44's
  move-to-new-tab gap is not claimed as solved.
- **Close floating layer…** (`pane.close_layer`) asks first, naming the layer, every pane in it
  and the programs that end (a layer another client made with several panes is named pane by
  pane). **Keep running is the default**; only the confirmation sends one
  `session.close_layer`, and only while the captured layer is still the one the view shows. A
  layer that gained or lost a pane, disappeared, or belongs to a previous connection epoch
  closes nothing and says the layer changed — it is never retargeted. Closing a pane or tab is
  unchanged and still closes a pane, not a layer.
- **Floating layers list** (Commands, Pane group): the tab on screen's floating layers, back to
  front, named by their first pane (`alpha`, `beta +1`), and marked `Minimized, still running`
  when hidden. `Restore <name>` / `Show <name>` runs the atomic restore-and-focus for that
  captured layer. It is read from the cached view at open time — no background prefetch and no
  polling — and it is the way to bring back a minimized layer, whose pane has no menu of its
  own. Each row keeps its own identity (its command ID, not its position): a host wake that
  reorders the layers, or adds or removes one, leaves the keyboard on the row it chose, and a
  row that is gone runs nothing — Enter never sends the command that moved into its index.
- **Pane menu**: one `Floating layer…` row (with the existing chevron) opens a small menu with
  the verbs that apply to that pane's layer. Six more rows in the pane menu itself pushed the
  destructive Close below the fold at 640×480, so they live one level in; the layer menu fits
  there, close last, while the longer pane menu scrolls with its overflow cue. Each layer-menu
  row runs through the same `shortcut_details` formatter as Commands, so a bound verb shows its
  effective keys and an unbound row keeps its own words (including a refusal reason). On a host
  before API revision 13 the row is absent and the pane menu is exactly as it was.
- Every verb is **local only**: `Remote::Never`, no new default binding, and the remote
  allowed set is still the same eight view actions. Each has a Commands row, a title, a
  Settings → Shortcuts heading and, when bound, its effective hint (the unbound case keeps the
  row's own words in the hint column).

## The API 13 gate

The layer methods and `session.view`'s layers were verified live at **API revision 13 only**
(read-only contract check, `~/.cache/agent-ws/issue136-gate/RESULT.md`). Revisions 10–12 are
unproven, so `src/layers.rs` holds `MIN_API_REVISION = 13` and decides from the negotiated
revision:

- a connected host at revision 10–12 lists every layer row **disabled**, with `Needs Rex API
  13, not <revision>` in its hint column, and every verb reports "This host's Rex is API
  revision N; floating panes need revision 13. Nothing was changed.";
- no layer request is sent to such a host, and no local view is edited on its behalf;
- attachment, tiled rendering and every pre-existing action are unchanged there.

`MIN_API_REVISION` (10) and `API_REVISION` (13) are untouched: this is a capability gate on the
feature, not a new minimum for the client, and API 14 is not claimed (the client would refuse it
at negotiation). No generic capability manager was added; the check is
`layers::unsupported(connected, revision)`, a pure function used by every row and verb.

## What the client relies on from the server

Quoted from the live contract (identical to the pinned SDK):

- `float_block`: the new layer is in the block's own window, in front of its floating layers;
  focus and the active window do not change; a zoom ends; a pane already floating is
  `bad_request`.
- `new_layer`: `layout` always starts new blocks; the whole layout is created or none of it;
  the layer starts in front; `window_id` may be named, and with no window it is `conflict`.
- `set_layer_state`: `state` is `normal` or `minimized`; minimizing a window's last visible
  panes is `bad_request`; `focus: true` also raises, focuses the layer's last focused block and
  makes the window active in one revision; `focus` with `minimized` fails. There is no enum on
  `state`, so the client only ever sends the two documented values.
- `raise_layer`: never changes focus or bounds; the frontmost layer is a no-op.
- `move_block`: the anchor must be placed in a window; an emptied floating layer is removed and
  an emptied window closes; other splits keep their IDs; defaults are horizontal/after/0.5.
- `close_layer`: only a floating layer; it closes **every** block in the layer and returns once
  they have closed; closing the window's last blocks closes the window.
- `session.view`: layers back to front, the first tiled; `bounds` are window fractions; an
  unknown `kind` draws as floating and an unknown `state` as normal.

## Evidence

**Behavior tests** (51 tests in `src/app/floating_tests.rs` and `src/app/floating_tests/history.rs`;
one confirmation copy test in `src/app/floating.rs`; five pure view tests in `src/layers.rs`).
The [floating-menu ownership matrix](../plans/floating-menu-ownership.md#history-matrix-results)
names the Runtime histories, which check both the captured and decoy host queues:

- every verb is local-only, titled, under the Panes heading, with a Commands row; the remote
  allowed set stays exactly eight;
- revisions 10, 11 and 12: every verb refused with the revision and **zero** host commands,
  rows disabled with the reason, pane menu unchanged;
- float: one `session.float_block` for the same block with exactly `{block_id, bounds,
  session_id}`, the centred rectangle, no new block/tab/session, view revision unchanged; an
  already-floating pane refused before any request;
- new floating: one `session.new_layer` with `{bounds, focus, layout, session_id, window_id}`,
  one terminal shell block, the focused pane's `cwd_from`, no split/tab; no tab → no row and no
  request;
- minimize: one `set_layer_state {layer_id, state}` without `focus`; the last visible layer
  disabled with "Only visible panes", nothing sent, cached state not changed;
- restore: one `set_layer_state {focus, layer_id, state}` with `normal` and `focus: true`, no
  second raise/focus call;
- raise: one `raise_layer {layer_id}`; the frontmost layer refused with "Already in front";
- return: one `move_block` anchored on the tab's first tiled pane with the documented defaults;
  no tiled pane → disabled, nothing sent, no invented anchor;
- close: the dialog names both panes of a two-pane layer, Keep running sends nothing, confirm
  sends one `close_layer`; a layer that gained a pane, disappeared or arrived on a new epoch
  closes nothing (and a disappeared/reconnected one refuses a restore too);
- a layer verb sends its one request and no other command: the pane's terminal worker receives
  nothing, its selection state is unchanged, and focus stays where the server has it;
- the list: this tab's layers only, back to front, `Restore beta`/`Show alpha` with the
  minimized detail, and `Restore beta` sends one atomic restore; the pane menu has one layer
  row, and its menu lists the verbs for the pane it names with the destructive close last;
- a pane menu opened on one pane while the keyboard is on another starts `New floating pane` in
  **the menu's** tab and folder (`cwd_from` names that pane), and still does after another
  client shows a different tab; a pane, tab or connection epoch that changed since the menu
  opened sends **nothing** and reports the change; the focused Commands/shortcut path still uses
  the focused pane's folder;
- a keyboard-chosen Commands row keeps its identity across a host wake: with the layers listed
  `[alpha, beta, gamma]` and `Show beta` highlighted, another client raising alpha reorders them
  to `[beta, gamma, alpha]`, and Enter still sends `set_layer_state` for **beta** (the old index
  named gamma); removing the highlighted layer leaves Enter sending nothing with a notice; and a
  row outside the inventory (`Focus next pane`) keeps its own command when a layer row above it
  disappears (`session.focus_next_pane`, not the row that shifted into its index);
- the layer menu shows each bound verb's effective hint through the same formatter Commands
  uses: unbound rows have no keys, binding `pane.minimize_layer` shows `Ctrl Shift M` in both,
  rebinding follows, and removing the binding restores the refusal words on a disabled row
  (`No tiled pane beside it`) while still naming the menu's own pane.

**Native acceptance at `51a0b4d`** (`scripts/floating-layers-private.py`, 32 passing checks;
own disposable API-13 Rex
server on a private unix socket, private HOME/XDG/TMPDIR and no desktop, bus, agent or audio
variables, private Xvfb, two clients of the exact-head debug build, real xdotool input): float
preserved the block ID, the shell's scrollback and the same shell process — asked again with
`echo shell-pid-after=$$` **after** the float, and equal to the line printed before it — typing
still reached the PTY, and the second client rendered the same floating pane (OCR of terminal
text; the two clients' rectangles are not compared); minimizing handed focus to a visible pane
and the pane's loop printed every second while hidden (7 s); the list named `Restore alpha —
Minimized, still running` while the action row said `Not floating`; restore came back focused and
in front; New floating from the pane menu started exactly one block in the captured window
and shell directory at the requested rectangle;
`Show <name>` focused and raised a layer; focusing a lower layer did not raise it and Bring to
front then changed only the order (bounds and focus unchanged); the close confirmation named
`beta`, Keep running left the layer, and confirming closed only that layer's block; Return
docked beside the real tiled pane with the same block and removed the emptied layer; with no
tiled pane the row read `No tiled pane beside it` and confirming a match sent nothing (server
revision unchanged); and a two-pane layer another client made (a split beside a floating pane)
was named pane by pane — `Close gamma +1? … gamma, delta` — and confirming ended both while the
other floating layer stayed.

Each run writes its own receipt directory (`REX_FLOAT_OUT/<timestamp>-<unique>`, created
exclusively) under `~/.cache/agent-ws/`, so no later run overwrites it: `identity.json` plus the
view records, terminal texts, OCR words, captures and both clients' logs. The run root, its
private HOME/XDG/TMPDIR, its socket and every process the run started are gone afterwards:
`scripts/floating-layers-private.py` allocates its root inside a `try` whose `finally` removes it,
and through `Desktop.close`/`stop_owned` terminates **and waits for** each client, openbox and
Xvfb (bounded, then a kill and a wait) before any file is removed, including when a receipt copy
fails. `scripts/test-fixture-cleanup.py` exercises the cleanup paths with process doubles:
a missing tool leaves no run root, `Desktop.close` waits for its children (kills then waits
on timeout), and a partial `Desktop.__init__` releases the Xvfb it had started.

**Renders** (real fonts, `REX_TEST_SNAP_DIR`, one test per run, `floating_pane_menu_draw`,
`floating_layer_menu_draw`, `floating_commands_draw`, rendered at this worktree's build in
1024×700 Comfortable and in 640×480 Comfortable, Compact and toolbar-hidden): the 640×480
Comfortable layer menu and its toolbar-hidden variant, the 640×480 Comfortable pane menu and the
640×480 Compact Commands list were read here. The six-row layer menu fits with
`Close floating layer…` last at the minimum size; the layer verbs are unbound by default, so
those rows show no keycaps, as their Commands rows show none. The pane menu at that size scrolls
with its own overflow cue (`More results below`), which is why six rows moved one level in. The
Commands list shows `Float pane — Already floating`, the other layer verbs, `Show f` and the
selected `Restore g — Minimized, still running` without clipping.

## Corrections after independent review

An independent review of `2253e7e` found four defects. Each is fixed here with a counterexample
that fails on `2253e7e` — for the three client defects at real dispatch, for the fixture through
its own process-double probe — and each failure was seen before the fix.

- **A captured menu discarded its target.** `Intent::Layer(target, NewFloating)` returned before
  checking the target, and dispatch called the targetless `new_floating_pane`, so the new pane
  inherited the *focused* pane's folder and the *active* tab. `layer_blocked` now validates a
  captured target (host ID, connection epoch, session view, window and the pane itself) for
  every verb, `NewFloating` included, and dispatch creates the pane in that tab and folder. The
  Commands and shortcut path keeps using the focused pane. Test: `2253e7e` sent
  `cwd_from: block:a` and then `window_id: other`; now `block:f` and `dev`, with zero requests
  for an epoch, pane or tab that is gone.
- **A layer reorder retargeted the keyboard's Commands row.** `Panel.selected` was only an index,
  so a server reorder made Enter run whatever row moved into it. A Commands row now carries its
  command ID (`Entry::id`) and the keyboard records which row it is on; a host wake follows that
  row (`restore_row_selection`), and Enter resolves the ID, sending **nothing** if the row is
  gone. Rows chosen by pointer or without an ID keep index behaviour. Tests: Enter after the
  reorder sent gamma on `2253e7e` and sends beta now; a removed row sent a command on `2253e7e`
  and sends none now, with a notice; `Focus next pane` keeps its command when a layer row above
  it disappears.
- **The layer submenu had no shortcut hints.** Its rows were built with empty `keys` and
  returned before the shared formatter. They now go through `shortcut_details`, so a bound verb
  shows the same hint as its Commands row and an unbound row keeps its words. Test: binding
  `pane.minimize_layer` showed no keys on `2253e7e` and shows `Ctrl Shift M` now, on rebind and
  removal too.
- **The native fixture did not own its resources for their whole lifetime.** Its run root was
  allocated before the `try` (a missing tool leaked it), `Desktop.close` only sent terminate and
  never waited or reaped, and a failed receipt copy skipped the removal. The root is now
  allocated inside the owning `try`; `Desktop.close` and its `stop_owned` helper terminate,
  boundedly wait, then kill and wait for each owned child before files are removed; the
  borrowed `Desktop.__init__` also releases a partial start; receipts copy failures are
  recorded and never skip the removal; each run gets its own exclusive receipt directory and an
  `identity.json`. The review's process-double probe reproduced the leak, zero waits and
  leaked Xvfb on `2253e7e`; see `scripts/test-fixture-cleanup.py` for the checked-in coverage.

Two smaller corrections: the last-visible and no-anchor refusals no longer advise something that
does not help (minimizing another layer, or splitting a tab to make a tiled anchor — Split adds
to the floating layer), and the changelog no longer says the layer row keeps the pane menu from
scrolling: it moves six rows one level in, the layer menu fits at 640×480, and the longer pane
menu still scrolls there.

### Second review, of `d168709`

- **`Desktop.close` broke the five scripts that wait on its handles.** It reset `apps`, `wm`
  and `server` after reaping them, so their own `finally` raised `AttributeError` before the
  private server, proxy or run root was removed. It now leaves the reaped handles in place: a
  repeated `wait` returns at once and a second close terminates nothing again.
  `scripts/test-fixture-cleanup.py` runs the exact `finally` of `status-sweep-private.py`,
  `process-cwd-contract.py`, `server-settings-x11.py`, `readme-gallery.py` and
  `insert-path-x11.py` against process doubles and checks each remaining cleanup. It also
  covers repeated close, the timeout kill and wait, and a constructor that fails after starting
  Xvfb. The same file checks that `floating-layers-private.py` gives itself an `XDG_DATA_HOME`
  instead of inheriting the caller's, and still removes its run root when a tool is missing. It
  runs in Linux CI and starts no real fixture process.
- **The open layer submenu rebuilt its target on every rebuild and on Enter.** It kept only a
  `PaneTarget`, so a pane another client moved to another tab was started there, and a new
  connection epoch was adopted. The submenu now captures its `LayerTarget` once, in
  `Panel.layer`; every rebuild and Enter validate that snapshot. Tests drive the real pane menu
  → layer submenu → layout wake → Enter for a pane moved to another window with its tab
  deleted, an epoch change, a pane or tab that disappeared, and the allowed case where another
  tab is raised while the pane stays where the menu named it.
- **Pointer selection could revive a stale keyboard row.** Hovering changed only `selected`, so
  a later hover at the same index after a reorder or removal made Enter run the row the
  keyboard had named. Hover now retires that identity (`Panel.row = None`), including when it
  lands on the row the keyboard already had. Pointer and keyboard histories cover reorder,
  same-index hover and removal; the keyboard-only identity tests are unchanged.

## Limits, and slice 2 follow-up

- **Pointer drag and edge resize (shipped in slice 2):** slice 1 intentionally shipped verbs
  only; header drag-move, 8-way edge/corner resize via `session.resize_layer`, per-layer
  tiled-divider z-order, and quantitative two-client geometry convergence are now shipped in
  [M4-floating-drag-resize.md](M4-floating-drag-resize.md), completing #136.
- **No multi-pane creation or group editing.** New floating layers have one pane; a multi-pane
  layer made by another client (here: a split beside a floating pane) is listed, focused, raised
  and closed with all its panes named, natively for one such layer and in the headless suite for
  membership changes mid-confirmation.
- **Slice 1's two-client check was terminal text; slice 2 proves geometry convergence.**
  Quantitative two-client rectangle convergence is verified in
  `scripts/floating-drag-private.py` ([M4-floating-drag-resize.md](M4-floating-drag-resize.md)).
- Revisions 10–12 are unproven and therefore disabled; proving them needs older server binaries.
  The server's own refusals (last-visible minimize, tiled-layer close, out-of-window bounds) are
  documented rules; the client refuses the ones it can decide first and reports the rest as
  they arrive, without retrying.
- macOS and Windows were not run natively; their builds and the headless suite are covered by
  CI. No native OS windows, snapping, tile policy, #44 move-to-new-tab or #117.
- The client adds no layout cache, no optimistic edit, no replay after a connection failure and
  no new settings page or toolbar state.
