# Shared Rex Host List — Slice 2 (Shared Host Writes: `hosts.put` & `hosts.remove`)

Completes [shuv1337/rex-rune#145](https://github.com/shuv1337/rex-rune/issues/145) (requested by `@oriori1703`) on top of [Slice 1](shared-hosts-slice1.md).

## Scope

Adds epoch-scoped, server-authoritative writes (`hosts.put` and `hosts.remove`) to this machine's Rex server shared host list while keeping all Slice 1 reconciliation, deduplication, and opt-out invariants intact:

- **Pure core (`src/hosts.rs`):**
  - `SharedHost::is_declared(&self) -> bool` (`self.source.is_some()`). Declared `init.lua` hosts remain strictly read-only across Rex Rune.
  - `build_put_update` starts from `record.raw.clone()` so `id`, `icon`, `added_from`, `host_id`, and all unknown JSON fields survive untouched. When the edited endpoint is unchanged canonically, `record.endpoints` is kept verbatim (preserving bare hostnames such as `studio.ts.net`); when it changes, only the matching endpoint entry is replaced in place, unrelated sibling endpoints stay intact, and canonical duplicates are deduplicated cleanly.
  - `build_put_create` constructs a clean `hosts.put` host object (`label`, `endpoints: [endpoint]`, optional `added_from`), omitting `id` so the server generates its canonical `host:<uuidv7>` identifier.
  - `build_remove_payload` constructs `{"id": host_id}`.
  - `UnconnectableHost::duplicate` distinguishes skipped duplicate-endpoint entries (`Skipped` chip) from records with no valid endpoint (`Unconnectable` chip).
- **Connection layer (`src/connection/hosts.rs`, `src/connection.rs`):**
  - Adds epoch-scoped `HostCommand::PutSharedHost { id, epoch, host }` and `HostCommand::RemoveSharedHost { id, epoch, host_id }` (modelled on `StopServer` and `ResizeLayer`: single-attempt, never auto-retried, never queued across reconnects).
  - Delivers structured outcomes (`WriteResult` / `WriteOutcome::{Put, Removed, Failed}`) via a bounded `HostModel::shared_writes` queue that wakes the UI.
  - Stale epoch, disconnected/backoff state, server errors (`conflict`, `not_supported`, `method_not_found`, `revision_too_low`, `bad_request`, `too_large`, etc.), timeouts, and transport drops all report `WriteOutcome::Failed` without retrying or mutating cached `SharedHostState::Loaded`.
  - On write success, if the returned `hash` differs from the cached `Loaded` list hash, the reader starts a `hosts.list` refresh for that expected hash so `SharedHostState::Loaded` stays authoritative even if `hosts_changed` is delayed, while deduplicating the matching `hosts_changed` notification.
- **Workspace & UI (`src/app/shared_hosts.rs`, `workspace.rs`, `dialogs.rs`, `panel.rs`, `settings_view.rs`, `commands.rs`, `ssh_setup.rs`):**
  - **Declared `init.lua` hosts (`source.is_some()`):** strictly read-only across Settings, sidebar host/session menus, Commands, `edit_host`, `save_ssh_socket`, `remove_host`, and stale open forms, naming `"Declared in <source> (read-only)"`. Local `shell` and `remote_control` stay editable.
  - **Stored shared hosts (`source.is_none()`, source connected and `Loaded`):**
    - **Edit:** Settings, sidebar, and Commands open `Form::EditHost`. Shell-only edits remain local and immediate with zero server writes. Label/endpoint edits validate locally (including canonical duplicate-endpoint checks), send epoch-scoped `PutSharedHost` with preserved `raw` fields, and wait for server confirmation before updating `settings.hosts` (applying `edit_host`'s safety reset on endpoint change: `remote_control = false`, clearing `sessions`/`recent_paths`, reconnecting; keeping `collapsed` and applying any local `shell` edit). Concurrent writes and stale open forms whose host changed/became declared/went offline are refused cleanly.
    - **`ssh_setup::save_ssh_socket`:** refused on declared `init.lua` hosts and unavailable sources; on connected stored shared hosts, routes through `PutSharedHost` and updates/clears SSH review state only after server confirmation.
    - **Remove:** opens a distinct confirmation dialog warning that the host is removed from Rex's shared host list for every client (`Remove from shared list`). Sends epoch-scoped `RemoveSharedHost` only on confirmation, never deletes ahead of server reply, tracks self-initiated removal IDs so the subsequent `hosts_changed` / `hosts.list` removes the local row cleanly without firing Rule 5's `"kept as a local host"` notice, and keeps Settings open when triggered from Settings.
    - **Add host (`Form::AddHost`):** when `sync_shared_hosts` is enabled and the local source is connected and `Loaded`, offers a **Save to shared Rex host list** switch (`save-shared-host`). When enabled, validates locally (including canonical duplicate endpoint), sends epoch-scoped `PutSharedHost`, and links/adds cleanly on reply without duplicating rows when `hosts_changed` reconciles. Turning the switch off keeps local-only adding with zero `PutSharedHost`. Never auto-publishes existing local-only hosts.
  - **Settings → Remote Hosts polish:** distinguishes editable stored shared hosts (`Shared via rex hosts`) from read-only declared hosts (`Declared in <source> (read-only)`) and offline/conflict states, and labels skipped duplicate-endpoint entries with a `Skipped` chip distinct from `Unconnectable`.

### Non-Regressions & Out of Scope

- `Spec::remote()` remains untouched (8 actions).
- Default `workspace.json` serialization is unchanged (no new serialized fields when default).
- All Slice 1 invariants remain intact (5-rule reconcile, duplicate-endpoint protection, `sync_shared_hosts` toggle off = zero server commands, `conflict` preservation, orphaned-link cleanup).
- Multi-endpoint editing UI, non-local `REX_HOME`/`home.json` reading, and overlaying declared `init.lua` hosts remain out of scope.

## Live Contract Observations (Step 0)

Observed against `/usr/bin/rex` (`rex 0.1.0-snapshot.2531`, commit `c67cc2d0c88ecc86f5d751079b809dd9e9047636`, API revision 19) on a disposable private Unix socket under `/mnt/data/tmp/` with isolated `HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME`/`XDG_RUNTIME_DIR`/`TMPDIR`, `--restore=false`, and a private `init.lua` declaring one host:

1. **`hosts.put` create (without `id`):**
   - Request:
     ```json
     {"host": {"label": "Lab", "endpoints": ["https://lab.invalid"], "added_from": "probe"}}
     ```
   - Response:
     ```json
     {
       "host": {
         "label": "Lab",
         "endpoints": ["https://lab.invalid"],
         "added_from": "probe",
         "id": "host:01a20d00-0cf1-7131-8402-8a0dc2531519",
         "updated_at": "2026-11-24T10:00:36.849235113Z"
       },
       "hash": "42a939f72e3f01dda649d896c80b2cd04df4bcc513cce66649b4b5ee2287b95a"
     }
     ```
   - Server generates a `host:<uuidv7>` ID, sets RFC 3339 `updated_at`, persists into `<state_dir>/overlay/hosts/everywhere.json`, and emits `{"type": "hosts_changed", "payload": {"hash": "<same hash>"}}`.
   - Duplicate label (case-insensitive) fails with `status=409`, `code="conflict"`, message `"the label \"Lab\" is already used by the host at https://lab.invalid"`, and emits zero events.

2. **`hosts.put` update (preserving unknown fields and multiple endpoints):**
   - Request with existing `id`, updated `label` and `endpoints`, plus `icon`, `added_from`, `host_id`, and unknown `"custom_extra": {"nested": [1, 2, 3]}`:
   - Server stores and returns every unknown field and all endpoints verbatim in both the `hosts.put` reply and subsequent `hosts.list`, updating only `updated_at`, and emits `hosts_changed` with the matching `hash`.

3. **`hosts.put` and `hosts.remove` against a declared `init.lua` host (`source: "init.lua:1"`):**
   - On the wire, the Rex server does not reject writes targeting a declared host's `id`: `hosts.put` writes an overlay entry in `everywhere.json` (dropping `source`) and `hosts.remove` records a tombstone under `"removals": [{"id": "..."}]` without editing `init.lua`.
   - **Rex Rune policy:** Rex Rune intentionally keeps declared `init.lua` hosts (`source.is_some()`) strictly read-only in the client and never sends `hosts.put` or `hosts.remove` for them, avoiding silent overlay shadowing of configuration-managed hosts.

4. **`hosts.remove`:**
   - Request: `{"id": "host:0199abcd-1234-7abc-8def-0123456789ab"}`.
   - Existing stored host: responds `{"removed": true, "hash": "<new hash>"}` and emits `hosts_changed` with `<new hash>`.
   - Non-existent ID: responds `{"removed": false, "hash": "<unchanged hash>"}` (not an error) and emits zero events.

5. **Corrupt `<state_dir>/overlay/hosts/everywhere.json`:**
   - Both `hosts.put` and `hosts.remove` fail with `status=409`, `code="conflict"`, message `"hosts/everywhere.json is not a JSON object"`, emitting zero events.

## Verification

- `src/hosts.rs`: unit tests for `SharedHost::is_declared`, `build_put_update` (unknown-field preservation, bare-hostname retention on rename-only, in-place endpoint replacement with unrelated endpoints intact, canonical deduplication), `build_put_create`, `build_remove_payload`, and `UnconnectableHost::duplicate`.
- `src/connection/hosts_tests.rs`: fake-WebSocket tests (loopback TCP, deterministic FIFO fences, zero sleep polling) for `PutSharedHost` and `RemoveSharedHost` success + refresh deduplication, stale epoch local refusal, disconnected refusal without replay on reconnect, `conflict`/`not_supported`/drop preservation of cached `SharedHostState::Loaded`.
- `src/app/shared_hosts_tests.rs`: headless UI tests covering stored shared host edit (preserving `raw`, waiting for server, safety reset on endpoint change, failure preservation, shell-only local edit), `save_ssh_socket` routing through `PutSharedHost`, shared removal warning dialog + confirmation + no Rule 5 notice + Settings staying open, `Form::AddHost` shared toggle + duplicate check + local opt-out + zero duplicate rows on reconcile, declared `init.lua` read-only enforcement, offline/`Conflict`/`Unsupported`/stale-form refusals, `Skipped` vs `Unconnectable` chips, and 640×480 layout fit across Settings and all three dialogs.
- Full verification under Rust 1.96.1 (`fmt`, `--no-default-features` check, `clippy -D warnings`, full `--all-features` test suite) and Rust 1.99.1 (`clippy -D warnings`).
