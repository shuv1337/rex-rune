# Terminal Listening Ports

Slice 1 implements the UI-independent foundation for inspecting listening TCP ports reported by
Rex Server API 15 (`com.superlogical.terminal.ports`) and API 16 (`ports_changed`).
[Slice 2](#slice-2-ui) adds a Rune UI over these types and commands.

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
  - Zero eager reads at attach and zero reads for background sessions: only the host's
    selected foreground session (never `background_views` prefetched by `status_sweep`) is
    eligible for `WatchPorts` or `ReadPorts`, and switching away to another session cancels
    in-flight/queued reads and drops the watch for the previous session.
  - `HostModel::ports: HashMap<String, PortSnapshot>`,
    `HostModel::ports_reading: HashSet<String>`, and `HostModel::ports_read_completions: u64`.
  - Epoch-scoped commands `HostCommand::WatchPorts { epoch, session: Option<String> }` and
    `HostCommand::ReadPorts { epoch, session: String }`, strictly gated on `api_revision >= 15`.
  - Detached `ports::Reader` with max 3 concurrent in-flight reads, 8 s deadline per read, at
    most one in-flight read per block, immediate queue pumping whenever lifecycle cancellation
    frees a slot, and clean `method_not_found` / `revision_too_low` / `not_supported` ->
    `PortsStatus::Unsupported` handling via shared `transport::detached_result_with_seq`.
  - Rex combining rule (`ControlState::ports_seq`):
    - Reply applies when `reply.seq >= ports_seq[block]` (tie goes to reply).
    - `ports_changed` event applies only when `api_revision >= 16` and
      `event.seq > ports_seq[block]` (strictly greater); below API 16, `ports_changed` is
      still intercepted and silently discarded before `activity::parse_block_event`.
  - Passive `ports_changed` events never touch `urgent_views`, activity, bells, or notices,
    are exempted from `view_seq` in `ControlState::includes` beside `clipboard::is_copy`,
    store snapshots only for blocks currently in a held view, and wake the UI only when the
    snapshot or reading state actually changes.
  - Full lifecycle cleanup across `block_closed`, undoable close (`replace_view` gone blocks),
    `remove_session`, `session.attach_all` session removal, disconnect (`ports` and
    `ports_reading` cleared immediately so stale addresses/PIDs/paths never linger), and
    reconnect (`ports`, `ports_reading`, `ports_read_completions`, `ports_seq`, watched
    session, and `ports_reads` reset). When `replace_view` adds new terminal blocks to the
    currently watched foreground session, those new blocks are queued and pumped cleanly.
  - Targeted `event_dropped` arm for `event_type == "block_event"` +
    `block_event_name == Some("ports_changed")`: verifies held-view membership and matching
    `session_id` (unheld blocks and mismatched pairs never insert into `ports_seq`), skips
    invalidation when a stored snapshot already covers `drop_seq` (`ports_seq >= drop_seq`),
    drops stale snapshots, advances `ports_seq`, cancels affected reads, re-reads only if
    watched and eligible, and pumps queued reads into freed slots without triggering unrelated
    `session.view` or activity refreshes. Generic drops with missing `event_type` or
    `block_event` without `block_event_name` apply the same guarded recovery, while unrelated
    specific `block_event_name`s leave ports untouched.

## Locality and Reach Security Matrix

`HostLocality::from_endpoint` classifies a host as `Local` **only** when the saved endpoint
starts with `unix://` (matching `host_icon` in `src/app/panel.rs`). `ssh://` maps to `Ssh`;
all other schemes (`https://`, `http://`, `wss://`, `ws://`, even for `localhost` or
`127.0.0.1`) map to `Remote`. URLs and `ssh -L` commands are never constructed from raw
unparsed wire strings.

| Bind address | Local (`unix://`) `open_url` | `Ssh` (`ssh://`) `ssh_forward_command` | `Remote` (`http/s`, `ws/s`) | `copy_address` (all hosts) |
| --- | --- | --- | --- | --- |
| `127.0.0.0/8` (loopback v4) | `http://<addr>:<port>` | `ssh -N -L <local>:<addr>:<port> [-p P] -- <dest>` | `None` (no Open / no `ssh -L`) | `<addr>:<port>` |
| `::1` (loopback v6) | `http://[::1]:<port>` | `ssh -N -L '<local>:[::1]:<port>' [-p P] -- <dest>` | `None` | `[::1]:<port>` |
| `0.0.0.0` (wildcard v4) | `http://127.0.0.1:<port>` | `ssh -N -L <local>:127.0.0.1:<port> [-p P] -- <dest>` | `None` | `0.0.0.0:<port>` |
| `::` (wildcard v6) | `http://[::1]:<port>` | `ssh -N -L '<local>:[::1]:<port>' [-p P] -- <dest>` | `None` | `[::]:<port>` |
| Specific v4 (no zone) | `http://<addr>:<port>` | `ssh -N -L <local>:<addr>:<port> [-p P] -- <dest>` | `None` | `<addr>:<port>` |
| Specific v6 (no zone) | `http://[<addr>]:<port>` | `ssh -N -L '<local>:[<addr>]:<port>' [-p P] -- <dest>` | `None` | `[<addr>]:<port>` |
| Link-local with `%zone` | `None` (copy only) | `None` (copy only) | `None` (copy only) | `[<addr>%<zone>]:<port>` |
| Unparsable / invalid zone | `None` (copy port only) | `None` (copy port only) | `None` (copy port only) | `<port>` |

Additional SSH forward rules:
- Privileged remote ports (`port < 1024`) map to local port `10000 + port` (for example,
  remote port 80 forwards to local port 10080).
- Bracketed IPv6 `-L` forwarding specs are single-quoted (`'<local>:[<addr>]:<port>'`) so
  shells such as zsh do not attempt glob expansion.
- `--` terminates SSH options before the destination, and `ssh::Target::destination()` is
  POSIX shell-quoted (`'...'` with `'\''` escaping) when containing shell metacharacters;
  control and bidi characters are rejected.

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
- Send `HostCommand::WatchPorts { epoch, session: Some(sid) }` when opening the foreground
  session's ports surface and `WatchPorts { epoch, session: None }` when closing it; use
  `ReadPorts` for an explicit refresh action (especially useful on API 15 servers where
  `api_revision < PORTS_EVENTS_MIN_API_REVISION` and passive `ports_changed` events are gated
  off).
- View-change and reconnect contract:
  - While `WatchPorts` is active on the selected foreground session, `replace_view`
    automatically queues and pumps reads for any newly added terminal blocks (for example, a
    new split or undo-restored pane).
  - Switching away to another session automatically cancels queued/in-flight reads and clears
    the old watch; Slice 2 should send a new `WatchPorts` if the surface follows the new
    foreground session.
  - Disconnecting clears `HostModel::ports` and `HostModel::ports_reading` immediately and
    resets the watch; if the Ports surface stays open across a reconnect, Slice 2 must send
    `WatchPorts { epoch: new_epoch, session: Some(sid) }` once the new `connection_epoch` is
    connected.
- Distinguish UI empty/loading/error states directly from `HostModel`:
  - `m.protocol.api_revision < PORTS_MIN_API_REVISION`: API below 15.
  - `m.ports_reading.contains(block)` without a cached snapshot: reading ports.
  - `m.ports.get(block)` with `PortsStatus::Unsupported`: unsupported terminal.
  - `m.ports.get(block)` with `ports.is_empty()`: no listening ports.
  - `!snap.is_complete()`: show partial/truncated/capped caveat.
- Drive row actions exclusively through `ports::HostLocality::from_endpoint`, `ports::open_url`,
  `ports::copy_address`, and `ports::ssh_forward_command`.

## Slice 2 UI

In Commands, choose **Show listening ports…** for the current session. The pane menu's
**Listening ports…** puts that pane's group first without focusing it. Both paths share the
`client.ports` shortcut reference in Settings; it is unbound by default and not remotely
invocable. There is no pane badge and no background read. Results group by tab and pane.
Type a port number, process, address, tab or pane to filter. Up and Down select a row.
If an update moves the highlighted port, the selection follows it; if that port disappears,
Enter does nothing until another port is selected.

| Host attachment | Enter or Copy | Ctrl+Enter (Cmd+Enter on macOS) |
| --- | --- | --- |
| Unix socket | Copy the reported bind address and port | Open an HTTP URL in the desktop handler, with wildcard addresses mapped to loopback |
| SSH | Copy the bind address and port, with a notice naming the host | Copy an `ssh -N -L` command; never run it |
| Other remote endpoints, including loopback URLs | Copy the bind address and port, with a notice naming the host | No action |

Zoned IPv6 and invalid addresses never open or offer a forwarding command. An invalid
address copies only the numeric port. A remote-host banner names where the listeners are.
Locality comes only from the saved endpoint: a manually forwarded Unix socket or a Rex
server in another network namespace still looks local. Rex Rune does not probe a port,
check whether it speaks HTTP, or infer which remote sockets are reachable.

Escape, an outside press, a panel change or leaving the session stops the watch. Reconnect
starts one watch in the new connection epoch if the list is still open on the same session.
On API 15, reopen the list to refresh; API 16 and later deliver change events. Coverage
messages distinguish unavailable or disconnected hosts, loading, unsupported terminals,
empty or filtered lists, partial scans and report limits.

### Verification

Headless coverage lives in `src/app/ports_tests.rs`. Run `ports_local_draw`,
`ports_remote_ssh_draw` and `ports_empty_api14_draw` individually with `REX_TEST_SNAP_DIR`
set; repeat with `REX_PORTS_MINIMUM=1` for 640×480. The cases cover the local action buttons
and keycaps, the SSH banner and partial warning, and the API 14 refusal.
Visually inspected all six real-font PNGs at 1024×700 and 640×480. Both populated lists
fit the four fixture rows and footer at minimum size without clipping or overlap.

`scripts/ports-x11.py --api21 <rex>` uses `REX_BIN`, private Rex servers and sshd, real X11
input, an X11 clipboard, and an opener stub. It checks the exact copied address, local URL,
and SSH command, and verifies that only the markers before and after the popup reach the
PTY. The forwarding command is never executed.

Both native cases passed with Rex `0.1.0-snapshot.2562` (API 21). Only the local case invoked
the opener, exactly once, and both PTYs received exactly the two markers.
