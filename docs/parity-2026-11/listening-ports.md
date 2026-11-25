# Terminal Listening Ports - Slice 1 (Protocol, Model, Sync, and Reach Policy)

Slice 1 implements the UI-independent foundation for inspecting listening TCP ports reported by
Rex Server API 15 (`com.superlogical.terminal.ports`) and API 16 (`ports_changed`). No Rune UI
changes under `src/app/` are included in this slice; Slice 2 builds the UI surfaces on top of
these types and connection commands.

## Scope

- **Pure model, wire parser, caps, and reach policy (`src/ports.rs`, `src/ports/tests.rs`):**
  - Revision gates: `PORTS_MIN_API_REVISION = 15` (`com.superlogical.terminal.ports`) and
    `PORTS_EVENTS_MIN_API_REVISION = 16` (`ports_changed`).
  - Raw-safe `PortSnapshot::parse(&Value)` that never fails on unknown statuses, unknown
    protocols, out-of-range ports, malformed addresses, or hostile process strings.
  - `PortsStatus`: `Ok`, `Partial`, `Unsupported` (unknown strings map to `Partial`;
    `Unsupported` always forces empty `ports` and `processes`).
  - Filtering, deduplication, sorting, and caps:
    - Keeps only `protocol == "tcp"` and `port` in `1..=65535`.
    - Validates `address` by splitting optional `%<zone>` (`[A-Za-z0-9._-]`, 1..=16 ASCII
      chars) and parsing the IP with `std::net::IpAddr` (no brackets; unzoned IPv4-mapped IPv6
      normalized to IPv4; IPv4 with zone or invalid zone becomes `PortAddress::Unparsable`).
    - Validates `first_seen_at` as strict RFC 3339 capped at 40 chars (via shared
      `connection::activity::time::timestamp`); invalid strings become `None`.
    - Keeps positive `pid` (`> 0`) only; sanitizes all process fields through the shared
      `connection::perform::sanitize_text` helper with strict character caps (`label` 48,
      `name` 64, `user` 64, `argv0` 128, `path`/`invoked_path`/`script`/`cwd` 1024, `zone` 16).
    - Derives `PortProcess::label` as the last path component (splitting on `/` and `\`,
      trimming trailing separators) of the first non-empty of `script`, `argv0`, `name`.
    - Deduplicates identical `(port, canonical address)` deterministically (retaining richer
      `pid` / `first_seen_at`), sorts by `(port, canonical address)`, caps at
      `MAX_PORTS_PER_BLOCK = 256` (`capped = true` if trimmed), and prunes unreferenced
      processes.
    - Host-wide cap `MAX_PORTS_PER_HOST = 4096` enforced across all blocks on a `HostModel`
      (`apply_host_budget` trims from the end, sets `capped = true`, and prunes unreferenced
      processes).
    - `PortSnapshot::is_complete()` is `status == Ok && !truncated && !capped`. Does not call
      `blob_read` in Slice 1/2.
- **Connection sync and lifecycle (`src/connection/ports.rs`, `src/connection.rs`,
  `src/connection/ports_tests.rs`):**
  - Zero eager reads at attach and zero reads for background sessions.
  - `HostModel::ports: HashMap<String, PortSnapshot>` and
    `HostModel::ports_reading: HashSet<String>`.
  - Epoch-scoped commands `HostCommand::WatchPorts { epoch, session: Option<String> }` and
    `HostCommand::ReadPorts { epoch, session: String }`, strictly gated on `api_revision >= 15`.
  - Detached `ports::Reader` with max 3 concurrent in-flight reads, 8 s deadline per read, at
    most one in-flight read per block, and clean `method_not_found` / `revision_too_low` /
    `not_supported` -> `PortsStatus::Unsupported` handling without disconnecting.
  - Rex combining rule (`ControlState::ports_seq`):
    - Reply applies when `reply.seq >= ports_seq[block]` (tie goes to reply).
    - `ports_changed` event applies when `event.seq > ports_seq[block]` (strictly greater).
  - Passive `ports_changed` events are intercepted in `block_event` before
    `activity::parse_block_event` (never touching `urgent_views`, activity, bells, or notices),
    exempted from `view_seq` in `ControlState::includes` beside `clipboard::is_copy`, stored
    only for blocks currently in a held view, and wake the UI only when the snapshot or
    reading state actually changes.
  - Full lifecycle cleanup across `block_closed`, undoable close (`replace_view` gone blocks),
    `remove_session`, `session.attach_all` session removal, disconnect (`ports_reading` cleared),
    and reconnect (`ports`, `ports_reading`, `ports_seq`, watched session, and `ports_reads`
    reset).
  - Targeted `event_dropped` arm for `event_type == "block_event"` +
    `block_event_name == Some("ports_changed")`: drops affected snapshot(s), advances
    `ports_seq` to at least `drop_seq`, cancels affected in-flight reads, and re-reads only if
    watched without triggering unrelated `session.view` or activity refreshes. Generic drops
    with missing `event_type` or `block_event` without `block_event_name` also clear/re-read
    ports if watched, while unrelated specific `block_event_name`s leave ports untouched.

## Locality and Reach Security Matrix

`HostLocality::from_endpoint` classifies a host as `Local` **only** when the saved endpoint
starts with `unix://` (matching `host_icon` in `src/app/panel.rs`). `ssh://` maps to `Ssh`;
all other schemes (`https://`, `http://`, `wss://`, `ws://`, even for `localhost` or
`127.0.0.1`) map to `Remote`. URLs and `ssh -L` commands are never constructed from raw
unparsed wire strings.

| Bind address | Local (`unix://`) `open_url` | `Ssh` (`ssh://`) `ssh_forward_command` | `Remote` (`http/s`, `ws/s`) | `copy_address` (all hosts) |
| --- | --- | --- | --- | --- |
| `127.0.0.0/8` (loopback v4) | `http://<addr>:<port>` | `ssh -N -L <local>:<addr>:<port> [-p P] <dest>` | `None` (no Open / no `ssh -L`) | `<addr>:<port>` |
| `::1` (loopback v6) | `http://[::1]:<port>` | `ssh -N -L <local>:[::1]:<port> [-p P] <dest>` | `None` | `[::1]:<port>` |
| `0.0.0.0` (wildcard v4) | `http://127.0.0.1:<port>` | `ssh -N -L <local>:127.0.0.1:<port> [-p P] <dest>` | `None` | `0.0.0.0:<port>` |
| `::` (wildcard v6) | `http://[::1]:<port>` | `ssh -N -L <local>:[::1]:<port> [-p P] <dest>` | `None` | `[::]:<port>` |
| Specific v4 (no zone) | `http://<addr>:<port>` | `ssh -N -L <local>:<addr>:<port> [-p P] <dest>` | `None` | `<addr>:<port>` |
| Specific v6 (no zone) | `http://[<addr>]:<port>` | `ssh -N -L <local>:[<addr>]:<port> [-p P] <dest>` | `None` | `[<addr>]:<port>` |
| Link-local with `%zone` | `None` (copy only) | `None` (copy only) | `None` (copy only) | `[<addr>%<zone>]:<port>` |
| Unparsable / invalid zone | `None` (copy port only) | `None` (copy port only) | `None` (copy port only) | `<port>` |

Additional SSH forward rules:
- Privileged remote ports (`port < 1024`) map to local port `10000 + port` (for example,
  remote port 80 forwards to local port 10080).
- `ssh::Target::destination()` is always POSIX shell-quoted (`'...'` with `'\''` escaping)
  when containing shell metacharacters, and control/bidi characters are rejected.

## Live Wire Contract and Verification

Verified against disposable isolated servers via `examples/ports_probe.rs` (built with
`--no-default-features`) and `scripts/ports-private.py`:

- **API 21 (`0.1.0-snapshot.2562`):**
  1. `block_creator.list` confirmed `com.superlogical.terminal` declares both `ports` method
     and `ports_changed` event schema (identical payload shape).
  2. Live Python TCP listener in a terminal pane was reported with exact port, PID, and
     derived label `listen.py` via both passive `ports_changed` and explicit `ReadPorts`.
  3. Closing the listener socket cleared the snapshot via `ports_changed` (`status: "ok"`,
     `ports: []`).
  4. Exiting the terminal process (`exit.on_completion = false`) delivered a final
     `ports_changed` event with `status: "unsupported"` and `ports: []`.
  5. Pinned client max API revision 15 negotiated revision 15 and observed open/closed ports
     cleanly via `ReadPorts`.
  6. Pinned client max API revision 14 negotiated revision 14 and sent zero `ports` requests.
- **API 14 (`0.1.0-preview.19`):**
  - Negotiated revision 14, confirmed `com.superlogical.tunnel.info` exists on API 14, sent
    zero `com.superlogical.terminal.ports` requests (verified against server log), and stayed
    connected with no notice.

## Slice 2 UI Handoff

Slice 2 can build directly on Slice 1 without touching protocol or worker code:
- Send `HostCommand::WatchPorts { epoch, session: Some(sid) }` when opening a session's ports
  surface and `WatchPorts { epoch, session: None }` when closing it; use `ReadPorts` for an
  explicit refresh action (especially useful on genuine API 15 servers where `api_revision <
  PORTS_EVENTS_MIN_API_REVISION`).
- Distinguish UI empty/loading/error states directly from `HostModel`:
  - `m.protocol.api_revision < PORTS_MIN_API_REVISION`: API below 15.
  - `m.ports_reading.contains(block)` without a cached snapshot: reading ports.
  - `m.ports.get(block)` with `PortsStatus::Unsupported`: unsupported terminal.
  - `m.ports.get(block)` with `ports.is_empty()`: no listening ports.
  - `!snap.is_complete()`: show partial/truncated/capped caveat.
- Drive row actions exclusively through `ports::HostLocality::from_endpoint`, `ports::open_url`,
  `ports::copy_address`, and `ports::ssh_forward_command`.
