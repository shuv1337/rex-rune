# Rex Server API 24 compatibility

Rex Rune advertises revisions 0 through 24 and validates the revision in
WELCOME. Existing per-feature gates still apply.

The vendored Rex SDK was regenerated from verbatim `rex api list --json`
and `server.version` outputs. The capture came from a private Linux server,
`0.1.0-snapshot.2670`, commit `b070aae645f5cbfac65d65d031a8b712b2ebca36`.
The server was built on November 25, 2026 at 1:59 PM PST. The 343,142-byte
`help.json` (`help.all`) was byte-identical to snapshot 2646, so only the
version metadata changed and the generated SDK types stayed the same.

| Revision | Wire addition | Rex Rune |
| --- | --- | --- |
| 22 | `session.transfer_window` moves a live window across sessions and reuses block IDs. | Negotiates the revision. No transfer action. |
| 23 | `com.superlogical.fs` adds tar upload and download on filesystem blocks. | Negotiates the revision. No filesystem block or tar support. |
| 24 | `com.superlogical.terminal.program_resume_records` and the `program_resume_changed` block event. | Read-only Program resume popup. No launch, approval or rule-edit action. |

The catalog adds four methods to the API 21 catalog. None of the existing
request or response shapes changed.

## Program Resume on snapshot 2670

Rex verifies OSC 7577 reports, keeps up to eight records per terminal in
memory, and marks one record chosen. The built-in trust rules cover `claude`,
`codex`, and `opencode`. The host can configure `resume.trust` in `init.lua`.
Rex does not sanitize display text such as program names.

The read returns `revision_too_low` when pinned to API 21 through 23 on
2670. It returns `method_not_found` on the actual API 21 server, snapshot
2562. Snapshot 2670 sends the event even to clients pinned below 24.
Events carry the full list without the secret arguments. Observed reasons
are `report`, `clear`, `rejected`, `owner_exited`, and `rules_changed`.
Replace the list regardless of reason. A read's `seq` is the last event
sequence queued before the request. A foreground job change alone can
change the chosen record without an event.

On a clean `rex server stop` and restart with `--restore=true`, snapshot 2670
saved the trusted chosen program under `resume` in the terminal's `state.json`.
Rex logged `terminal: resuming saved program` with `program=claude rule=claude`,
and a marker file written by the private stand-in confirmed the exact
`claude --resume <fixture UUID>` invocation. The untrusted `no_rule` record
was not saved or relaunched; its pane accepted input in a restored shell.
There is no resume, approval, forget, clear, or rule-edit API for the client.
Snapshot 2646 already had the same API revision but did not persist or rerun
the programs. The restart wording describes the behavior tested on 2670 with
server restore enabled, not a guarantee based on the negotiated revision alone.

## Reader

Rex Rune gates the read and the event on the negotiated API 24 revision.
The library reads only when a caller opens a watch on the selected
foreground session. It reads each terminal once per watch, with up to two
reads in flight and an eight-second deadline. Passive events update held
views without reading or generating activity, notifications, or sounds.

Replies and events are decoded as metadata. The reader does not use the
generated response type, which describes the whole wire response including
secret arguments. The client drops those arguments and server error text,
escapes hostile display text and caps records at 16 per block and 1024 per
host. It skips records whose raw owner name exceeds 256 bytes or whose
timestamp exceeds 64 bytes. It does not evaluate trust rules or execute
anything.

Events apply above a block's last sequence. Reads apply at or above it.
Dropped events invalidate affected snapshots and only reread an open
foreground watch. A watch and its pending work are scoped to the current
connection epoch, with no retries on reconnect.

`scripts/program-resume-private.py` exercises the worker against private
2670 and 2562 servers. It checks the persisted state and actual process
invocation across a clean 2670 restart. [Popup scope and tests](parity-2026-11/program-resume.md).

## Read-only live check on a Linux workstation

On November 25, 2026 (PST), with the existing Rex process already listening,
the client probe negotiated revision 24 against the live server:

```sh
rex-probe --version-only unix:///run/user/$(id -u)/rex/server.sock
```

Result: `Negotiated Rex API 24`; `server.version` reported
`0.1.0-snapshot.2670`, commit `b070aae645f5cbfac65d65d031a8b712b2ebca36`,
platform `linux_amd64`, `dirty: false`, API revision 24.

The explicit `--version-only` mode uses the same revision negotiation as the
app but identifies as `Other` so Rex cannot select it for a script prompt.
It sends only HELLO and `server.version`, then closes. A debug trace on a
disposable 2670 server showed exactly one request (`server.version`) before
the mode was used on the live socket. It never lists, attaches to, creates
or changes sessions. Do not use the ordinary `rex-probe` mode on a live host:
that mode creates a test session.

## API 0 through 9 compatibility

Rex Rune lowers `MIN_API_REVISION` from `10` to `0` so it matches the official
`rex` CLI floor (`0..=24`) while preserving every feature gate:

- **Revision 0 split anchor (`r01`).** `session.new_split` translates
  `anchor_id` to `anchor_block_id` on revision 0 (in both `HostCommand::Request`
  and `HostCommand::NewTerminal`, and in `rex-workspace-probe`) and sends
  `anchor_id` on revision 1 and newer.
- **Stop server gate (`r04`).** `HostCommand::StopServer` requires negotiated
  revision 4 or newer. Below revision 4, the worker refuses locally without
  sending `server.stop` on the wire, records the refusal in `notice` and
  `server_stop_result`, and the Server settings page and Commands disable
  **Stop server…** with `"Requires Rex Server API revision 4 or newer."`.
- **Bundled bootstrap tickets (`r08`).** `session.bootstrap` includes
  `data_tickets` only on revision 8 and newer. Revisions 0 through 7 omit
  `data_tickets` and open terminal streams through `data.connect`.
- **Revision 0 reporting honesty.** `ProtocolStats::api_revision` is an
  `Option<u32>` (set to `Some(rev)` on WELCOME and cleared to `None` on
  disconnect), distinguishing a real negotiated revision `0` from an unreported
  or disconnected host in Settings -> Server, Settings -> About, **Copy version
  info**, and **Copy debug info**. `ServerInfo::api_revisions` accepts
  `(0, max)` ranges whenever `max != 0` so servers reporting
  `api_min_revision = 0` and `api_server_revision = 24` display `0–24` rather
  than `Not reported`.

`examples/api_compat_probe.rs` and `scripts/api-compat-private.py` verify live
down-negotiation on a private disposable `0.1.0-snapshot.2670` server across
revisions `0`, `1`, `3`, `7`, `8`, `9`, `24`, and `4` (including live output,
`session.new_split` on rev 0 and rev 1, bundled vs `data.connect` ticket usage,
local `StopServer` refusal on rev 0 and rev 3, and clean shutdown via rev 4
`server.stop`).
