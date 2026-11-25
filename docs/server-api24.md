# Rex Server API 24 compatibility

Rex Rune advertises revisions 10 through 24 and validates the revision in
WELCOME. Existing per-feature gates still apply.

The vendored Rex SDK was regenerated from verbatim `rex api list --json`
and `server.version` outputs. The capture came from a private Linux server,
`0.1.0-snapshot.2646`, commit `3dda44da999b669ea56a0fce4f16b86d4d55d2cb`.
The server was built on November 25, 2026 at 11:17 AM PST.

| Revision | Wire addition | Rex Rune |
| --- | --- | --- |
| 22 | `session.transfer_window` moves a live window across sessions and reuses block IDs. | Negotiates the revision. No transfer action. |
| 23 | `com.superlogical.fs` adds tar upload and download on filesystem blocks. | Negotiates the revision. No filesystem block or tar support. |
| 24 | `com.superlogical.terminal.program_resume_records` and the `program_resume_changed` block event. | Reads record metadata and follows changes. No UI or actions. |

The catalog adds four methods to the API 21 catalog. None of the existing
request or response shapes changed.

## Program Resume on snapshot 2646

Rex verifies OSC 7577 reports, keeps up to eight records per terminal in
memory, and marks one record chosen. The built-in trust rules cover `claude`,
`codex`, and `opencode`. The host can configure `resume.trust` in `init.lua`.
Rex does not sanitize display text such as program names.

The read returns `revision_too_low` when pinned to API 21 through 23 on
2646. It returns `method_not_found` on the actual API 21 server, snapshot
2562. Snapshot 2646 sends the event even to clients pinned below 24.
Events carry the full list without the secret arguments. Observed reasons
are `report`, `clear`, `rejected`, `owner_exited`, and `rules_changed`.
Replace the list regardless of reason. A read's `seq` is the last event
sequence queued before the request. A foreground job change alone can
change the chosen record without an event.

On a clean restart with restore enabled, 2646 restored the terminals and
returned empty record lists. It did not persist records, rerun a trusted
record, ask for confirmation, or emit a resume event. Terminal state had
only `version`, `process_cwd`, and `size`. There is no resume, approval,
forget, clear, or rule-edit API for the client. This is evidence about
2646, not a guarantee about future Rex builds.

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
2646 and 2562 servers. Its clean-restart tripwire checks for saved records,
program execution, prompts and resume events. All seven checks passed on
these snapshots. Both `rex-probe` and `rex-workspace-probe` also passed
against each snapshot with `REX_EXPECT_REVISION` set to its revision.
