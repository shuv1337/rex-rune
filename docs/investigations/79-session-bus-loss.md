# #79: optional desktop bus loss must not close terminals

## Scope and source

Base: `a8fc9e736620372a2f1e42045881abe44a9a5cd6` (`origin/master`).
Rune: vendored `d70929a4dae263d16d2e8088e7bfe53f58f619ad` plus documented patches.
No dependency bump, signal handler, notification-worker change or platform-wide loader shim.

Desktop preferences are best-effort reads, not terminal state. GSettings owns the
GNOME animation preference, Rune samples it at window creation/focus, and Rune's
separate zbus portal worker caches the colour preference. Rex owns terminal state
on the server; the client must not turn losing an optional bus into process death.
The notification worker has its own connection/recovery and health reporting.
There is no new durable operation, replay or recovery queue in this fix.

## Investigation notebook — original attempt (superseded lifetime claim)

The original native successes below were real but **not lifetime-complete proof**:
extra focus reads masked the first-read/deferred-acquisition gap. See the follow-up
for the single-read failure, corrected ownership, and explicitly degraded appearance.

Evidence root: `~/.cache/agent-ws/rex-issue79/`. Only private HOME/XDG, bus,
Xvfb/openbox and Rex sockets; no owner's services stopped. Times are Pacific.

1. Rebuilt the unmodified base with
   `CARGO_TARGET_DIR=/home/shuv/repos/rex-rune/target cargo build --locked --bin rex-rune`:
   exit 0 (`before-build.log`). Reused the existing shared build cache, not a shared
   source checkout.
2. Ran the original private reproducer from the #78 evidence with this binary:
   `REX_BIN=/home/shuv/repos/rex-rune/target/debug/rex-rune python3 ~/.cache/agent-ws/rex-m34-notify/parent-bus-sigterm.py "$PWD"`.
   Result: **one bus loss, one SIGTERM (-15)**. `before-native.log` and
   `rex-m34-notify/parent-sigterm-fshdbvnf/result.json`. No `rex-notifications`
   thread. Rules out the notification worker as a necessary cause.
3. GDB pending breakpoints on `g_bus_get_sync` and
   `g_dbus_connection_set_exit_on_close` identify the creator, not just nearby
   threads. `rex-m34-notify/parent-sigterm-og4t4bha/gdb.log` records:

   ```text
   g_bus_get_sync
   libgvfsdbus.so
   ... g_vfs_get_default
   g_file_new_for_path
   libdconfsettings.so
   ... g_settings_backend_get_default
   ... g_settings_new_full
   rune::platform::preferences::reduced_motion
   rune::window::Host::new (window.rs:541)
   ```

   The setter breakpoint records its second argument **1**, from the same GVfs
   connection construction. Thus Rune's GNOME reduced-motion read loads GSettings,
   dconf initializes GFile/GVfs, and GVfs acquires GIO's shared session connection
   with exit-on-close enabled. The truncated `rune-appearance` thread name is
   correlation, not the source of this GIO call. These loader-tracing runs stopped
   early under batch GDB and are evidence of construction only, not bus-loss runs.

4. A separate GDB run with only `catch signal SIGTERM` completes the causal
   chain: `rex-m34-notify/parent-sigterm-xo8yzv3b/gdb.log` shows `raise` inside
   GObject signal delivery from GIO's closed-connection handling on the dconf
   worker, then `Program terminated with signal SIGTERM`. Command:
   `REX_BIN=/home/shuv/repos/rex-rune/target/debug/rex-rune python3 ~/.cache/agent-ws/rex-issue79/trace-private.py "$PWD"`.
   The scratch wrapper runs GDB with `trace-signal.gdb`, while its private parent
   fixture terminates only its own bus. GDB's batch exit 0 is not the app's exit;
   the debugger's terminal-signal record is the evidence here.
5. Added the private native regression before changing Rune. It fails on the
   rebuilt base at its first bus loss: **SIGTERM -15**, exit 1
   (`regression-before.log`, `native-c9ikfue4/`). The independently repeated
   original reproducer and this regression give **2/2 baseline process exits**.
6. First attempted fix at the existing GSettings boundary: after validating schema/key and
   resolving GIO's public functions, acquire the shared session connection,
   clear exit-on-close, and hold the reference through backend construction.
   All references are released; missing bus/library/schema/symbols return the
   existing `None` preference outcome. No signal disposition is changed.
7. The original local patch made the separate zbus portal subscription report its end once to stderr:
   `Rune: desktop appearance updates unavailable; keeping the current appearance.`
   Its cached colour remains. This is a diagnostic, **not an in-window toast**,
   not notification-service success, and not a claim of automatic portal recovery.
   The Rune upgrade to `983e33b9` dropped this local log override and kept upstream portal logging.
8. Regression passes (`regression-after.log`, `native-bv5lywu0/`). Strengthened
   focus coverage to actually minimize/map/refocus the private window between
   checks; it passes again (`regression-focus-after.log`, `native-x6s6fdi0/`).
9. Planted a targeted violation, changing only `exit_on_close(bus, 0)` to `1`:
   rebuilt and ran the final fixture. It fails at the first bus loss with **-15**
   (`mutation-{build,test}.log`, `native-fi58634n/`). Restored `0`, rebuilt and
   reran: pass (`final-build.log`, `regression-final.log`, `native-825b58ur/`).
   This proves the test catches the connection policy, not merely the diagnostic.
10. Final fixture also pins `XDG_DATA_HOME` and clears inherited `XAUTHORITY`;
    rerun passes (`regression-final2.log`, `native-5qw_g60w/`).

## Original verification recipe and results (historical)

All below ran on Linux on **2026-11-14 PST**, from this worktree, with
`CARGO_TARGET_DIR=/home/shuv/repos/rex-rune/target` for Cargo commands:

```sh
cargo build --locked --bin rex-rune
cargo fmt --all -- --check
cargo test --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
REX_BIN=/home/shuv/repos/rex-rune/target/debug/rex-rune \
  python3 tests/private/session-bus-loss-x11.py
```

- Build, formatting, full tests and strict Clippy: **exit 0**. Full suite:
  **1,163 passed, 0 failed, 6 pre-existing ignores** (379 library, 776 app,
  8 integration). Logs and `.exit` receipts: `final-build`, `fmt`, `tests`,
  `clippy` under the evidence root. Rune's six pre-existing dependency warnings
  remain; no lint settings or hooks changed.
- Original native fixture: **4/4 runs passed**, each with two bus losses/restarts,
  native X11 keys producing independently asserted PTY files before, while
  absent, and after restart, plus a fresh process launched with no bus. **0/8
  process exits on bus loss**. The last three runs include real focus loss/gain.
  GIO/dconf/GVfs must be mapped or the fixture refuses to claim coverage.
  No notification worker starts. Each run records exactly **one** appearance
  diagnostic; SIGTERM still kills the app normally when explicitly sent at cleanup.
- Independently reran the existing opt-in notification-worker test, directly from
  the test executable built by Cargo, with private HOME/XDG, no display and fresh
  matching `REX_M34_PRIVATE_BUS` / `DBUS_SESSION_BUS_ADDRESS`:
  `rex_rune-d2bc2526e73a8a3e notification_worker_reconnects_after_a_private_bus_restart --ignored`.
  **1 passed**, no replay of the note lost with the bus (`notification-worker.log`,
  `worker-7vF7C6/bus-fixture/calls.jsonl`). This is separate from whole-app survival.
- `python3 -m py_compile tests/private/session-bus-loss-x11.py` and
  `git diff --check`: exit 0.

The fixture is in `tests/private/`, not `scripts/`: the latter path triggers an
unrelated existing Arch-helper workflow that runs a privileged/systemd container.
That is forbidden for this task. No CI definitions or gates changed; ordinary
Linux/macOS/Windows CI remains applicable to the PR. No container was run.

## Follow-up: deferred backend lifetime (2026-11-14 PST)

Starting PR head: `357ee7a173cc745174aefbb6359728b71ccbcea0`, fetched and
verified before detaching the provided implementation worktree. No other worktree
was changed. `origin/master` is `db182c8` (#133); no merge was needed because the
full suite passed without it. No unrelated test changes were copied.

### Hypothesis, proof, and design

Astra's complete review and exact-function probe are in
`~/.cache/agent-ws/rex79-abi-review-357ee7a/REVIEW.md` and its adjacent
`probe.rs`, `run.py`, `refocus.py`, `gdb-identity.log`, `gdb-policy.log`, `gdb.log`.
The first `gnome_animations` returns `Some(true)`, then dconf asynchronously calls
`g_bus_get_sync` **after our reference was unref'd**. GDB recorded protected object
`0x5555555c8010` being replaced by `0x7fffec000970`, whose exit-on-close was **1**.
The untraced probe dies at first loss in **4/4 reviewer attempts**, including a
one-second wait. A second sample masks the first loss, but not the next loss after
restart/one sample. GIO 2.90.1 ABI/header checks were otherwise sound; actual native
UI occurrence rate remains unknown.

New durable regression: `tests/private/session-bus-single-read.py` extracts the
**exact current production function**, including its local owner, and compiles it
in a disk-backed private Cargo fixture with the repo lockfile's libloading version.
Its oracle is process survival/actual output, not copied lifetime logic. No focus,
extra sample, GLib pumping, tracing or application runtime is needed for the
first-read gap. Before touching production, **4/4 new runs on 357ee7a died -15**
at first loss (`before-{1,2,3,4}.{log,exit}`).

Repair: one static `Mutex<Option<SessionBus>>` owns the current protected shared
connection beyond the synchronous settings read. Every successful acquisition
replaces/unrefs the previous reference, even if it is the same pointer; references
do not accumulate per sample. `Drop` releases one reference. The existing static
GIO library keeps its function pointer and backend callbacks valid. GDBusConnection
methods and GObject ref/unref are thread-safe (installed GIR and GIO headers checked).
The mutex covers only ownership exchange and a local `is_closed` flag read, never
connection I/O, settings construction or unref. There is no generic manager,
signal handler, loader shim, timer, worker, retry or process-wide runtime change.

The retained connection exposed a second, concrete boundary: GIO may return a
**closed singleton**, held by backend/queued callbacks. Four intermediate guard
runs survived the first loss but incorrectly returned cached `Some(true)` while
absent (`after-{1,2,3,4}`); simply releasing our closed reference was also insufficient
(`release-closed-1`). These were **failed acceptance runs**, not fixed passes.
GIO 2.90.1 `schedule_closed_unlocked` retains the object until its closed-signal
idle source runs in the construction thread's GLib context. Rune does not run that
context. A scratch-only callback-drain command made fresh acquisition possible
(`diagnose-context.py/.log/.exit`, exit 0). This is causal diagnosis **only**, not
production code or acceptance evidence. No pumping was added to either regression.

Lead explicitly selected bounded degradation instead of adding GLib dispatch:
release our closed reference before the next acquisition; if GIO still returns a
closed object, release that result and return **None promptly**, without settings
construction/cached success. A genuinely live returned object is protected and
retained as before. Sampling may remain unavailable until application restart.
The last current reference, if any, has process lifetime; closing the app reclaims
it. Portal colour stays cached and its subscription remains stopped. Notifications
have their own recovery worker. This trades same-process optional GNOME preference
recovery for a small ownership fix; it does not disable appearance at startup.

### Completed acceptance and mutation

New receipts: `~/.cache/agent-ws/rex79-lifetime-fix/`. Each probe/native log names
its private artifact directory under `~/.cache/agent-ws/rex-issue79/`; completed
`result.json` records `passed: true`. All processes were reaped.

- **4/4 final single-read runs pass; 0/8 process exits on the two bus losses**
  (`fixed-{1,2,3,4}.{log,exit}`). First/live read is `Some(true)`; absent and exactly
  one post-restart sample are `None` on this retained-closed-singleton path. Both
  losses strictly assert the same process is alive. These runs prove survival and
  honest degradation, **not a fresh live appearance connection after restart**.
  Explicit SIGTERM still yields **-15**. Fresh-process live returns `Some(true)`;
  fresh absent, missing schema with a live bus, and missing library with a live bus
  return `None` and exit 0. Absent/post-restart samples take sub-millisecond time.
- Independent lifetime mutation: inserted only
  `drop(SESSION_BUS.lock().unwrap().take())` before the function returned. Regression
  **fails at first bus loss with -15**, exit 1 (`mutation-drop.{log,exit}`). Restored
  the guard, then final runs 2–4 pass. This catches the lifetime defect independently
  of the earlier policy mutation; no expected output was changed to hide death.
- Native fixture no longer refocuses inside every PTY assertion. There is no added
  focus before first loss, then one requested focus gain after the restart and no
  extra focus before second loss. Window creation can itself focus; the standalone
  probe, not this native launch, is the exact-one-read proof.
  **4/4 final native runs pass, 0/8 process exits** (`native-final-{1,2,3,4}`). Real
  X11 keys produce independently checked PTY files before loss, during each absent
  period, after restart, and on fresh absent startup. Mapped GIO/dconf/GVfs required;
  no notification worker starts. Exactly **one** appearance-degradation diagnostic
  per process with the original local log override; ordinary SIGTERM remains **-15**.
  The current fixture does not require that diagnostic. An earlier native guard-only run
  also passed (`native-1`), excluded from these final counts.
- Independent opt-in notification-worker fixture: **1 passed**, only two fake
  service Notify calls (before/after restart), no replay of the lost note
  (`notification-worker.{log,exit}`, private `worker-*/bus-fixture/calls.jsonl`).

Commands completed on Linux, with `RUSTUP_TOOLCHAIN=1.96.0` and
`CARGO_TARGET_DIR=/home/shuv/repos/rex-rune/target` for repo Cargo checks:

```sh
python3 tests/private/session-bus-single-read.py
cargo build --locked --bin rex-rune
cargo build --locked --release --bin rex-rune
cargo fmt --all -- --check
cargo test --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
REX_BIN=/home/shuv/repos/rex-rune/target/debug/rex-rune \
  python3 tests/private/session-bus-loss-x11.py
# Run the just-built app test executable, private HOME/XDG/no display,
# REX_M34_PRIVATE_BUS=<fresh path>, DBUS_SESSION_BUS_ADDRESS=unix:path=<same>/bus:
/home/shuv/repos/rex-rune/target/debug/deps/rex_rune-c5c1ae3dc855e778 \
  notification_worker_reconnects_after_a_private_bus_restart --ignored
```

All final commands exit 0 (`final-build`, `release-build`, `fmt`, `tests`, `clippy`,
`notification-worker` receipts). Full suite **1,163 passed, 0 failed, 6 existing
ignores** (379 library, 776 app, 8 integration). Full tests/Clippy used absent private
bus addresses and no DISPLAY/WAYLAND_DISPLAY/XAUTHORITY. The same six pre-existing
Rune dependency warnings remain. Both Python fixtures compile and `git diff --check`
passes. Native fixture's fresh absent startup and the probe's fresh live/schema/lib
checks run only against owned fixtures; no owner service is used or restarted.

Both regressions remain manual in `tests/private/`; no CI definitions, path fences,
hooks or gates were changed. This diff does not match the unrelated Arch privileged
workflow's path filters. No workflow was dispatched. macOS/Windows code paths remain
unchanged; CI checks on those paths belong to the lead/Terra, not native validation
claims here. No long CI wait, merge, tag, release or container run.

Additional rat-stack pages searched/read before production edits:
`ratstack://skills/investigate-a-failure`, `ratstack://skills/durability`,
`ratstack://skills/test-behaviour`, `ratstack://lore/tests-that-earn-their-place`,
`ratstack://lore/error-model`, `ratstack://systems/fence`,
`ratstack://lore/the-fence`, `ratstack://repo/AGENTS.md`,
`ratstack://lore/scopes-own-resources`, plus the existing language-neutral principle
leaves listed below. Applied resource ownership/dependency-fault isolation,
independent behavioral oracle + planted bug, honest unknown/unavailable outcomes,
and intact checks. No TS imports, Effect framework or speculative race hardening.

## Limits

- Actual acceptance is **X11 on this machine's GIO/dconf/GVfs**, not native
  Wayland, other GSettings modules/desktops, macOS or Windows. macOS/Windows code
  paths are unchanged; their CI results are for the lead to collect, not a claim
  of native checks here.
- The portal colour subscription remains stopped after bus loss and retains its
  last value; automatic appearance resubscription is not added. GNOME animation
  sampling is unavailable while the bus is absent and **may stay unavailable after
  the bus returns until app restart**, because GIO/backend callbacks can retain the
  closed singleton. It uses Rune's existing unavailable-preference fallback; cached
  data is not reported as a successful sample. Notification recovery retains its own owner.
- No real notification daemon, desktop audio, personal bus/display/Rex server,
  host package install/upgrade, personal client restart, merge, tag or release was used.

## Rat-stack guidance followed

Search/read used the provided `ratstack.sh` helper. Language-neutral guidance:

- `ratstack://repo/AGENTS.md`, `ratstack://skills/rat-stack-mode`: respect the
  product's own Rust/platform boundaries; do not import the TS/Effect stack.
- `ratstack://skills/investigate-a-failure`: reproduce, rebuild, prove the creator
  before changing policy, and retain the notebook and command receipts.
- `ratstack://lore/testing-ladder`, `ratstack://skills/test-behaviour`,
  `ratstack://lore/tests-that-earn-their-place`: a real native process/bus seam
  test catches SIGTERM; a mock or source-string assertion cannot. No new domain
  rule or lifecycle machine warrants generated properties/models here.
- `ratstack://lore/the-fence`: keep fmt/clippy/test gates and hooks intact.
- `ratstack://skills/durability`, `ratstack://lore/error-model`: isolate optional dependency failure, no replay;
  distinguish missing desktop service from service success or a program defect.
- Principle leaves read from `joelhooks/rat-stack` because helper leaf IDs return
  HTTP 422: `find-the-cause-first`, `rebuild-what-you-run`, `measure-real-limits`,
  `keep-execution-receipts`, `keep-unknown-distinct`,
  `tests-detect-broken-behavior` (under `skills/rat-stack-mode/principles/`), and
  `skills/durability/principles/fail-open-on-liveness.md`.

Helper HTTP 429/422 papercuts already exist as `pc_147c28c817cf` and
`pc_f7689c96f265`; no duplicate filed.
