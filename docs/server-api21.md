# Rex API 21 compatibility

Verified on Linux x86-64 on **2026-11-24 (PST)** on top of Rex Rune 0.11.0
(`master@ddca0370`). Rex Rune now advertises **API revisions 10 through 21**
while keeping all existing per-feature revision gates (uploads on 11, SSH
personal-server setup on 12, floating layers and downloads on 13).

## Server provenance and isolation

- Primary capture and verification target: Rex **`0.1.0-snapshot.2562`**, commit
  `9e2eef18ea487e17be2b7facf899c4b3a9b8b9ca` (Linux amd64, built
  `2026-11-24T17:40:34Z`, `api_revision: 21`, supports revisions `0..=21`).
  - Release archive SHA-256:
    `7131ec9851e1c5627e69c9c08b373fb2a7a66879d623b8773645b0cc995875a1`.
  - Extracted binary SHA-256:
    `e60fd6918b63f09346de865d3c74fba2fdeb938977cc28c7a2b3e62fbb469b99`.
- Follow-up snapshot sanity check: Rex **`0.1.0-snapshot.2563`**, commit
  `14f6d7f95cc6fe056935937de9783b8e40b416da` (Linux amd64, built
  `2026-11-24T18:34:57Z`, `api_revision: 21`, supports revisions `0..=21`).
  - Binary SHA-256:
    `b7f2d6e645e4f7e7cc2e71e825c97e780d51dedff7d3d3378105a50827c3a24f`.
  - Verified byte-for-byte identical `help.json` and `/llms.txt` to
    `0.1.0-snapshot.2562`.
- Backwards-compatibility baselines:
  - API 13 (`0.0.1794323808-g757adf`, commit `757adf99`, binary SHA-256
    `0436f853fb12f3790e1054e0018d652a0fe9f19a66495741e31ede0de211d60a`), recorded
    in [server-api13.md](server-api13.md).
  - API 12 (`0.1.0-preview.14`, commit `7e460d02`, binary SHA-256
    `17b4ddb5e7f66e47109c65fa5ce21c522e409cf2062c82be35d2e05e0a441239`), recorded
    in [server-preview14.md](server-preview14.md).
- Isolation: every live server ran exclusively on a disposable private Unix
  socket under `/mnt/data/tmp` with `--restore=false --config /dev/null` (or a
  private Lua fixture), private `HOME`, XDG directories, state directory,
  `--autostart=false`, explicit `REX_SERVER`, and a scratch-first `PATH`. No
  installer, package manager, system binary, or live `rex-server.service` was
  modified, and every throwaway server, Xvfb display, and test client was
  stopped by PID and cleaned up after verification.

## Complete protocol diff: API 13 to API 21

Baseline: the previous SDK capture (`0.0.1794323808-g757adf`, API 13). Raw
single-line captures from `0.1.0-snapshot.2562` (`rex api list --json` and
`server.version`) were copied verbatim into `vendor/rexrs/codegen/help.json` and
`vendor/rexrs/codegen/version.json` and regenerated with
`python3 codegen/generate.py` without changing the generator or local SDK
patches.

Summary of schema and protocol changes from revision 14 through revision 21:

- **Top-level methods (74 -> 76, none removed):**
  - Added `focus_history.reverse` (API 19): steps back through the calling
    principal's focus changes with optional session/window filters and cursor.
  - Added `client.undo_close` (API 20): reopens the caller's most recent
    `undoable` pane, window, or session close with its original block, split,
    layer, window, and session IDs.
- **Added optional fields on existing top-level methods:**
  - `block.close`, `session.close_window`, `session.destroy` (API 20): optional
    boolean `undoable` request field (omitted by default).
  - `session.view`, `session.attach`, `session.bootstrap` (API 21): optional
    nullable `tiled_zoomed_block_id` on `session::Window`. When a tiled pane is
    zoomed, `tiled_zoomed_block_id` names it and the pane draws behind
    non-minimized floating layers whether focus is on the zoomed tiled pane
    (`zoomed_block_id == tiled_zoomed_block_id`) or on a floating pane
    (`zoomed_block_id == null`). When a floating pane is zoomed,
    `zoomed_block_id` differs from `tiled_zoomed_block_id` and covers the entire
    window in front of every layer.
- **Block-type methods:**
  - `com.superlogical.terminal` (18 -> 19): added `ports` (API 15), reporting
    listening TCP ports, forward-compatible `status` string, optional `pid`,
    process descriptors, and optional truncation/blob `id`. Because its process
    object shape matches `com.superlogical.terminal.process`, `generate.py`
    names the shared struct `terminal::PortsResponseProcess` instead of
    `terminal::Ancestor` (nothing in `rex-rune` references `Ancestor`).
  - `com.superlogical.tunnel` (0 -> 1): added `info` (API 16), reporting tunnel
    flavor, target/address metadata, `last_owner`, and active connection count.
  - `com.superlogical.onboarding` (2 -> 2): unchanged.
- **Control events and client behavior:**
  - `client_prompt_requested` and `client_prompt_ended` (API 17): `rex.ui`
    broadcasts prompts to every connected `ClientKind::App` client and waits up
    to `timeout_ms` (default up to an hour) for answers via `client.reply`.
    Clients that cannot show prompts must reply `not_supported` immediately so
    only that client leaves the prompt without stalling scripts.
  - `event_dropped` (API 18): replaces oversized control events using the same
    `seq` instead of closing the socket with 1009. Carries `event_type`, `size`,
    and optional `session_id`, `block_id`, `creator_name`, and
    `block_event_name`.
  - No breaking changes to HELLO/WELCOME negotiation, data tickets,
    `rex.data.terminal.v1`, `rex.data.bytes.v1`, `client.keymap`, `client.key`,
    `client.action`, `client.perform`, or `client.reply`.

## Adopted in Rex Rune

1. **Vendored SDK refresh (`vendor/rexrs`):**
   - Refreshed `codegen/help.json` and `codegen/version.json` to API 21 and
     regenerated `src/api/generated/` and `tests/api_examples.rs` while
     preserving all local patches (`api_revisions` range negotiation,
     `Option<Vec<String>>` for `attach_session_ids`, and existing tests).
     `Cargo.lock` is unchanged (`--locked` passes).
   - Added hand-written SDK tests: `tests/zoom.rs`, `tests/undo.rs`,
     `tests/focus_history.rs`, and `tests/ports.rs`.
2. **Protocol negotiation and discovery probe kind (`src/transport.rs`, `src/tailnet.rs`):**
   - Raised `API_REVISION` from `13` to `21`, keeping `MIN_API_REVISION = 10`
     and keeping all per-feature revision gates (`upload = 11`, SSH setup `= 12`,
     floating layers `= 13`, downloads `= 13`).
   - Updated tailnet discovery probes (`transport::discovery_from_socket`) to
     identify as `ClientKind::Other` so short-lived probes are never selected as
     `rex.ui` prompt targets on rev 17+ servers.
3. **`rex.ui` transport and prompt model (`src/connection/prompt.rs`, `src/connection.rs`):**
   - Handles directed prompt events even during bootstrap and before
     `ControlState::includes`. With no consumer enabled, sends a single detached
     `not_supported` refusal. Hosts negotiated below API 17 send nothing.
   - The opt-in `HostCommand::Prompts(true)` enables one live offer per
     connection. Disabling it refuses the active offer. Invalid, unknown-kind
     and second offers are refused without replacing the active prompt. The
     app and existing probes do not opt in. The saved host's `prompts` preference
     defaults to true and only an explicit false is written to disk.
   - The bounded `HostModel.prompt_events` queue carries epoch-tagged offers,
     ends, rejected answers, reply outcomes and dropped-offer notices. Drop
     notices are only delivered while opted in. Local expiry closes an
     unanswered offer without sending anything; a sent answer gets its own
     reply budget. Disconnect clears the queue and active state; a stale epoch
     or call ID is never sent.
   - An accepted reply closes the offer. `bad_request` leaves it offered with a
     cleaned reason for the next answer. A reply too large for the negotiated
     message limit stays local and can be shortened. `not_found`, other API
     failures and the eight-second reply timeout close it without disconnecting.
     A transport failure disconnects. Answers are never automatically retried or logged.
   - `client.action` is detached. Before a prompt, the worker still defers
     ordinary commands and events behind its 35-second wait. A prompt allows
     ordinary work again and extends the action's wait to at least 35 seconds
     past the prompt deadline. Queued `client_actions` still apply when the
     response arrives. Disconnect discards outstanding action waits but still
     settles buffered commands before reconnect backoff, ahead of newer commands.
4. **`event_dropped` recovery (`src/connection.rs`, `src/connection/status_sweep.rs`):**
   - List-level session drops (`session_created`, `session_moved`,
     `session_label_changed`) re-list sessions via `session.attach_all` (even
     when `payload.session_id` is populated), reconcile cached view labels from
     the refreshed list (whether `session_id` is present or omitted), and
     `session_destroyed` removes the named session (or re-lists when unnamed).
   - `ControlState::includes` applies `view_seq` to `event_dropped` only when
     `event_type == "session_view_changed"` so dropped `block_event` or
     `program_status_removed` notices are not suppressed by a concurrent
     `session.view` that does not carry block metadata.
   - Other session-scoped drops cancel superseded in-flight detached
     `status_sweep::Reader` reads for the session/blocks, refresh the session
     view (when opened or selected; or clear `viewed` / mark in-flight fetches
     stale when unplaced), clear once-read block metadata marks (`queried`,
     `status_queried`, `status_failed`, `status_reading`, `status_read`), and
     downgrade existing activity records' `live` flag so a dropped
     `StatusRemoved` event is cleared by the subsequent `program_status` read.
   - Server-wide drops trigger `client.keymap` refresh for `keymap_changed`,
     `hosts.list` re-read for `hosts_changed`, and `server.status` re-read for
     `tailscale_status_changed`.
   - Known limitation: if `client_prompt_requested` or
     `client_perform_requested` is replaced by `event_dropped`, the server's
     drop notice carries no `call_id`, so the client cannot send a `client.reply`
     and the caller waits until its own timeout. A dropped prompt is surfaced
     to the prompt consumer, without replacing any existing offer.
5. **Session and block ID reuse cleanup (`src/connection.rs`):**
   - Clears per-block and per-session once-read marks (`queried`,
     `status_queried`, `status_failed`, `status_reading`, `themed`, `tickets`,
     `viewed`, `urgent_views`, `view_seq`, `fetch_stale`, `background_views`,
     `status_read`) on `block_closed`, `session_destroyed`, and sessions removed
     by `session.attach_all` (including removed sessions that never had a cached
     view) so an undone close (rev 20) re-reads layout, titles, and activity
     cleanly without reconnecting.
6. **API 21 dual zoom (`src/layout.rs`, `src/connection.rs`, `src/app/`):**
   - Added pure helpers `layout::full_window_zoom`, `layout::tiled_zoom`
     (validating that the block exists in a non-minimized tiled layer and
     falling back cleanly to the tiled layout if stale), `layout::is_zoomed`,
     `layout::any_zoomed`, and `layout::block_visible`.
   - Under API 21 tiled zoom (`tiled_zoomed_block_id` set, no differing
     `zoomed_block_id`), the zoomed tiled pane fills `[0, 0, 1, 1]` on the tiled
     layer behind all non-minimized floating layers while floating cards,
     floating split dividers, resize grips, and header drag remain active, and
     focusing a floating pane keeps the zoomed tiled pane's geometry and PTY
     size stable. Hidden tiled siblings (and blocks in minimized floating
     layers) are marked not visible by `block_visible`: no new worker is spawned
     for them while hidden, and any already-running worker keeps its data stream
     with `DataCommand::Visible(false)` so it parses output without repainting.
   - Under legacy API 10-20 zoom (`tiled_zoomed_block_id` absent) or API 21
     floating zoom (`zoomed_block_id != tiled_zoomed_block_id`), the zoomed pane
     covers the entire window in front of every layer.
   - Kept Zoom/Restore pane menu and Commands labels, Zoomed header chips,
     `balanced`, and `beside` accurate across both fields.

## Undo close (API 20+)

**Undo close** in Commands asks the active host to reopen the last eligible
close from this client. `client.undo_close` has no default shortcut, but can
be bound in Settings → Shortcuts or in the host's keymap. It is not available
to remote actions. It is disabled when the host is disconnected or negotiates
an API revision below 20.

Confirmed human pane, tab, and session closes, including Close other panes
and Close other tabs, send `undoable: true` only on API 20+. Each pane or tab
in a bulk close is a separate close. Floating-layer closes and automated
requests do not use this flag. The server can hold eligible closes for about
a minute; when the history is empty or expires, Rex Rune shows **Nothing to
reopen** and stays connected.

The client sends `client.undo_close` with `{}` once on the requested
connection epoch. It refreshes the selected session or re-lists and attaches
as needed to show the restored session with the server's focus. A held pane
is absent from the view and its previous data stream has ended even before a
control `block_closed` event arrives. The worker clears that pane's once-read
metadata and discards the old stream. On restoration it reads status, title,
and process again and opens a fresh stream for the visible pane.

Mock and headless tests cover wire shape, reconnect and revision guards,
metadata and data recovery, and the Commands and Shortcuts wiring. To check
running PTYs and both revisions against disposable servers:

```sh
cargo build --locked --no-default-features --example undo_close_probe
python3 scripts/undo-close-private.py target/debug/examples/undo_close_probe /path/to/api21/rex /path/to/api13/rex
```

On the private API 21 `0.1.0-snapshot.2562` server, the probe restored a pane,
tab, and session. A shell variable survived each close and was read through
a fresh data stream after undo. Empty history left the connection up. On
the private API 13 server, all three closes succeeded without schema errors.

## Deferred deliberately

- **`rex.ui` interactive prompt rendering (#144):** deferred to its own UI slice;
  the shipped client still refuses with `not_supported` in the meantime.
- **Focus history navigation UI (`focus_history.reverse`):** SDK types are
  generated and tested; UI commands/bindings are deferred.
- **Listening ports and tunnel inspection UI (`com.superlogical.terminal.ports`,
  `com.superlogical.tunnel.info`):** SDK types are generated and tested; UI
  surfaces are deferred.

## Verification

### Static checks and unit / headless UI suites

| Check | Result |
| --- | --- |
| `cargo +1.96.1 fmt --all -- --check` (root and `vendor/rexrs`) | Pass |
| `cargo +1.96.1 check --locked --no-default-features` | Pass |
| `cargo +1.96.1 clippy --locked --all-targets --all-features -- -D warnings` | Pass |
| `cargo +1.99.1 clippy --locked --all-targets --all-features -- -D warnings` | Pass |
| `cargo +1.96.1 test --manifest-path vendor/rexrs/Cargo.toml --all-features` | **167 passed**, 0 failed, 10 ignored (live tests) |
| `cargo +1.96.1 test --locked --all-features` | **1,422 passed**, 0 failed, 6 ignored (481 lib, 933 headless UI, 8 integration) |
| `cargo +1.96.1 build --locked --release --bin rex-rune` | Pass |
| `cargo +1.96.1 build --locked --no-default-features --bin rex-probe --bin rex-workspace-probe` | Pass |

### Live private-server verification matrix

All live checks ran against isolated disposable servers under `/mnt/data/tmp`
using the freshly built release `target/release/rex-rune` and debug probes.

| Check | API 21 (`0.1.0-snapshot.2562`) | API 13 (`rex-osc52`) | API 21 (`0.1.0-snapshot.2563`) |
| --- | --- | --- | --- |
| Live `vendor/rexrs` SDK tests (`--ignored --test-threads=1`) | Pass (10/10: 9 blocking, 1 async) | Not required | Not required |
| `rex-probe` with `REX_EXPECT_REVISION` | Pass (rev 21) | Pass (rev 13) | Pass (rev 21) |
| `rex-workspace-probe` with `REX_EXPECT_REVISION` | Pass (rev 21) | Pass (rev 13) | Pass (rev 21) |
| `scripts/keymap-acceptance.py` | Pass | Pass | Not required |
| `scripts/x11-smoke.py` | Pass | Verified in `server-api13.md` | Not required |
| `scripts/floating-layers-private.py` | Pass | Pass | Not required |
| `scripts/floating-drag-private.py` | Pass | Verified in M4 | Not required |
| `scripts/copy-mode-x11.py` | Pass | Verified in copy-mode | Not required |
| `scripts/remote-control-acceptance.py` (`REX_EXPECT_REVISION=21`) | Pass (real private SSH host B) | Verified in `server-api13.md` | Not required |
| `scripts/download-contract.py --quick --rex12` | Pass (API 21 + API 12 preview.14) | Verified in `server-api13.md` | Not required |
| Live Lua `rex.ui.input` (`timeout_ms = 5000`) with only `rex-rune` attached | Pass: returned `(nil, "unsupported")` in 0.015 s; `rex-rune` stayed connected | N/A (rev 17+ feature) | Identical API 21 |
| Live API 21 dual zoom on Xvfb (`t1`/`t2` tiled + `f1` floating) | Pass: tiled zoom grew `t1` 76 -> 138 cols while `f1` stayed 67 cols; focusing `f1` kept `t1` at 138 cols and `f1` at 67 cols; zooming `f1` grew `f1` to 138 cols; restoring `f1` returned `f1` to 67 cols with `t1` still at 138 cols | N/A (rev 21 feature) | Identical API 21 |

No Wayland, macOS, Windows, or Nix local builds are claimed here; Linux X11
native acceptance and the full Rust unit/headless UI/SDK suites were verified.

### `rex.ui` slice 1 contract (#144)

Checked the `help.json` and `llms.txt` captures and exercised snapshot.2562
on a fresh private socket with [`scripts/prompt-contract.py`](../scripts/prompt-contract.py).
Set `REX_PROMPT_PROBE` to the binary built by
`cargo build --locked --no-default-features --example prompt_probe`
to include the real Rex Rune worker. The script allocates and removes its own
HOME, XDG directories, server state, socket and synthetic session.

| Case | Observed on snapshot.2562 |
| --- | --- |
| Lua `rex.ui.input({title, placeholder, value, confirm, timeout = 5})` | The event carries those four display fields, `kind: "input"`, and top-level `timeout_ms: 5000`. Lua's option is `timeout` in seconds, not `timeout_ms`; the latter was ignored, leaving the 300000 ms default. |
| Lua `rex.ui.pick` | Takes the same fields and `items`, `multiple`, `allow_other`. Item fields are `id`, `title`, `subtitle`, `icon`, `detail`, `selected`. Item order is preserved. Unset Boolean flags and empty items were omitted on the wire. The capture also permits `items: null`. A single pick can carry `selected` on the wire, but the client only counts it for multiple. |
| `client.reply` | `{"text":""}`, `{"ids":["a"]}`, `{"ids":["z","a"]}`, and `{"cancelled":true}` each received `{}` on matching offers. `allow_other` accepts `{"text":"custom"}`; Lua receives `{text="custom"}` rather than an item ID. An invalid ID returns `bad_request`; the same call ID then accepts a valid answer. |
| Same-client `client.action` | The action's response waits for the answer. An ordered `client.key` release and unordered `client.reply` both completed while the action was pending; after the answer the action returned its queued `client_actions`. |
| Direct Lua `client.key` binding | The binding's `rex.ui.input` returned `(nil, "failed")` without a prompt. `client.key` returned `handled: true` from the binding and did not wait for a person. No key-path change was needed. |
| Second app answers first | Each app gets a different `call_id`. The other app's answer produced `client_prompt_ended` with `reason: "answered"` on the first app; the first app's late reply got `not_found`. |

Display text has control and formatting characters stripped and whitespace
folded. Action, title, placeholder and confirm are capped at 60, 120, 80 and
24 characters; a blank title becomes `Question`, and a blank or case-insensitive
`Cancel` confirmation is omitted. Item titles/subtitles are capped at 200
characters and details at 32. Unknown icon names use a generic icon. The
client refuses prefills over 4096 characters and lists over 2000 items rather
than cutting them down; item IDs are opaque, unique, nonempty and at most
1024 bytes. The display deadline clamps `timeout_ms` to 1000 through 3660000.
