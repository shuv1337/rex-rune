# Program resume

**Show program resume…** in Commands opens a read-only list of the current
session's reported programs. The pane menu's **Program resume…** puts that
pane's records first; the menu itself shows only a cached chosen name, when
one is already known. `client.program_resume` is unbound, available in
Settings → Shortcuts, and local-only. Hosts below API 24 cannot open the list.

The view uses the existing 460 pt filter popup. It groups by pane and preserves
the server's record order inside each group. The primary line names the
program and verdict; the second line shows the folder, report age, PID and
trust rule or reason. Verdicts use secondary text without an icon or alert.
Keyboard selection follows the same record across live reordering.

The client reads on open and listens for updates while the popup is open. It
does not ask about other sessions, poll, or send a read when merely opening
the pane menu. A watch waits for the foreground view and ends on close,
host or session switch, or a new connection epoch.

The restart wording refers to Rex snapshot 2670 with server restore enabled.
Rex owns persistence and execution. Rex Rune uses only escaped metadata and
does not show, store, copy, type or run launch arguments. Enter and row clicks
do not activate anything, and the popup does not send input to the terminal.
No pane cue, Activity entry, notification, sound, toolbar badge or setting
is added. See [the API record](../server-api24.md) for the difference between
snapshots 2646 and 2670.

## Verification

- `program_resume` tests cover local wiring, version and epoch fences,
  delayed view arrival, stable row identity, filter, hostile text, trust
  wording, footer and terminal isolation.
- `program_resume_populated_draw` and `program_resume_empty_draw` capture
  synthetic rows with real fonts. Run the ignored tests individually with
  `REX_TEST_SNAP_DIR`; set `REX_RESUME_MINIMUM=1` for 640×480. The populated
  and empty renders were checked at 640×480 and 1024×700.
- `scripts/program-resume-private.py` exercises the real connection and
  private server restart; all seven checks passed with snapshots 2670 and
  2562. `scripts/program-resume-x11.py /usr/bin/rex` passed at 640×480 on
  Xvfb with private snapshot 2670: no eager reads, a passive report appeared
  in the open popup, reopening read again, and the raw PTY saw only the
  before-open and after-close markers.
