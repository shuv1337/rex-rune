# Shared Rex Host List — Slice 1 (Read-Only Sync, Deduplication & Diagnostics)

Part of [shuv1337/rex-rune#145](https://github.com/shuv1337/rex-rune/issues/145) (requested by `@oriori1703`).

## Scope

Synchronizes hosts read-only from this machine's Rex server (`hosts.list` + `hosts_changed` push events) into Rex Rune's sidebar, host switcher, session switcher, and **Settings → Remote Hosts**:

- Hosts added via `rex hosts add` or declared in `init.lua` (`rex.host({ ... })`) appear live under their Rex labels.
- Pre-existing local Rex Rune hosts matching a shared host's canonical expanded endpoint are linked without duplicating rows or losing local preferences (`shell`, `collapsed`, `sessions`, `recent_paths`, `muted_tabs`, `remote_control`).
- Unknown fields on each `hosts.list` host JSON object are preserved verbatim in `SharedHost::raw` (`serde_json::Map<String, Value>`) as the foundation for Slice 2 writes.
- Config `diagnostics` and broken hosts-file `conflict` errors are surfaced cleanly in **Settings → Remote Hosts** without clobbering last-known rows.
- A default-on local toggle (**Use shared Rex host list**, `sync_shared_hosts`, omitted from `workspace.json` when default `true`) lets the user opt out: turning it off unlinks all shared links locally with zero server commands, and turning it back on reconciles from the cached list immediately.
- In Slice 1, linked hosts show `"Shared via rex hosts"` (and `"Declared in <source>"` when `source` is present). Local preferences (`shell`, `remote_control`) remain editable, while renaming, endpoint edits, and removing a linked host are clearly disabled/guarded as `"Managed by rex hosts (read-only)"` so they do not fight the next `hosts_changed` refresh before Slice 2 adds `hosts.put` / `hosts.remove`.

### Out of Scope (Slice 2 / Deferred)

- Zero `hosts.put` or `hosts.remove` calls in Slice 1 (Slice 2 owns shared writes from Add/Edit/Remove dialogs).
- No reading CLI held-changes queues, no shelling out to `rex hosts`, no non-local `REX_HOME`/`home.json` reading (Slice 3), no auto-publishing local Rune hosts, no multi-endpoint failover, no `API_REVISION` bump, and no `Spec::remote()` changes (the remotely allowed set stays at 8 actions).

## Live Contract Observations (Step 0)

Observed against `/usr/bin/rex` (`rex 0.1.0-snapshot.2488`, commit `4ad2c24ded916ee8cde94dc8052dbe5415a2513f`, API revision 19) on a disposable private Unix socket with isolated `HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME`/`XDG_RUNTIME_DIR`/`TMPDIR` and `--restore=false`:

1. **`hosts.list` response payload**:
   ```json
   {
     "hosts": [
       {
         "id": "host:0199abcd-1234-7abc-8def-0123456789ab",
         "label": "Lab",
         "endpoints": ["lab.ts.net", "http://10.0.0.5:7297"],
         "icon": "server",
         "added_from": "probe",
         "host_id": "11111111-2222-3333-4444-555555555555",
         "updated_at": "2026-11-24T07:09:11.533708251Z"
       },
       {
         "id": "host:2419b086-b186-5244-92c6-056bc79e2982",
         "label": "Studio",
         "endpoints": ["studio.tail.ts.net", "ssh://me@studio"],
         "icon": "desktop",
         "source": "init.lua:2"
       }
     ],
     "hash": "da7256a5e895f798090e6ef655327ccff27b773a64571c10efe60686d26fbfbe",
     "diagnostics": [
       {
         "severity": "warning",
         "source": "init.lua:7",
         "message": "rex.host: member \"endpoints\" is empty, but a host needs at least one endpoint, so this host is ignored"
       }
     ]
   }
   ```
   - `hosts` is sorted by label (case-insensitive) then id; empty list is `[]` (schema allows `null` or array).
   - `diagnostics` is omitted when empty; schema notes `hash` does not cover `diagnostics`, and `severity` is `"warning"` or `"error"` (unknown values shown as warning).
   - `id` format is `host:<uuid>`.
   - `init.lua` declares hosts via `rex.host({ label = ..., endpoints = { ... }, icon = ... })`, which sets `source` to `init.lua:<line>`.

2. **`hosts_changed` push event wire payload**:
   ```json
   {"type": "hosts_changed", "payload": {"hash": "da7256a5e895f798090e6ef655327ccff27b773a64571c10efe60686d26fbfbe"}}
   ```
   - Emitted on both `hosts.put` and `hosts.remove` with the exact new list `hash`.

3. **Error responses**:
   - Corrupt hosts overlay file (`<state_dir>/overlay/hosts/everywhere.json` containing invalid JSON): `hosts.list` fails with `status=409`, `code="conflict"`, message `"hosts/everywhere.json is not a JSON object"`.
   - Older servers without `hosts.list`: `method_not_found` or `not_supported` -> quiet `SharedHostState::Unsupported`.

## Reconciliation Rules (`src/hosts.rs`)

Applied in strict order against a successfully read `SharedHostList`:

1. **Self-suppression**: Any shared record whose `host_id` matches the local server's `server.status.host_id` is suppressed.
2. **Existing linked row (`shared.id` from `source_endpoint`)**: Label follows Rex (`record.label`). If the saved `endpoint` is no longer in the record's expanded endpoints, pick the first endpoint accepted by `validate_host` and mark `endpoint_changed = true` — the workspace applies the `edit_host` safety policy (drop `sessions` and `recent_paths`, reset `remote_control = false`, reconnect; preserve `shell` and `collapsed`).
3. **Unlinked local row matching canonical expanded endpoint**: Link (`shared = Some(SharedLink { id, from })`), adopt Rex's `label`, preserve all local preferences (`shell`, `collapsed`, `sessions`, `recent_paths`, `muted_tabs`, `remote_control`). Never links the source server row itself.
4. **Unmatched shared record**: Append a new linked host row using the first valid expanded endpoint accepted by `validate_host`. If no endpoint is usable, surface it as an unconnectable entry in **Settings → Remote Hosts** only without spawning a worker.
5. **Departed linked row**: If a linked row's `shared.id` is absent from a successfully read list, unlink (`shared = None`) and keep it as a local host with a clear notice — never silently delete it.

## Verification

- `src/hosts.rs`: unit tests covering all 5 reconcile rules, `-S` endpoint expansion, canonicalization, self-suppression by `host_id`, unknown-field preservation in `raw`, and idempotence.
- `src/connection/hosts.rs` & `src/connection/hosts_tests.rs`: fake WebSocket tests proving one detached `hosts.list` per connection, same-hash `hosts_changed` no-op, changed-hash re-read, `conflict` handling, quiet `method_not_found` -> `Unsupported`, and zero `hosts.put`/`hosts.remove` requests.
- `src/settings.rs`: tests for `SharedLink` and `sync_shared_hosts` defaults, omission when default, and round-trip.
- `src/app/shared_hosts_tests.rs`: headless UI tests for preference preservation on link/rename, endpoint-change safety reset, conflict preservation, unlink-and-keep with notice, opt-out toggle (zero commands on off, cached reconcile on), Slice 1 read-only guards with local shell editing intact, Settings search, and 640×480 layout fit.
- Full verification under Rust 1.96.1 (`fmt`, `--no-default-features` check, `clippy -D warnings`, full `--all-features` test suite) and Rust 1.99.1 (`clippy -D warnings`).
