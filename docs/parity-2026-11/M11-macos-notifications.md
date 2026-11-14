# M11 — desktop notifications on macOS

Base: `6499ff1` (v0.9.0). Verification: 2026-11-13, Pacific Standard Time.
Scope: the desktop-notification channel only. The worker's replacement, close-on-seen,
click-once and 64-note memory rules are unchanged and still shared by every platform.

## Spike: what macOS requires

A Swift probe (`spike.swift` in the evidence root; bundle id `dev.shuv.rex-rune.spike*`)
measured the framework before any client code was written.

| Attempt | Result |
| --- | --- |
| Bare executable, `Bundle.main.bundleIdentifier` read | `nil`; no crash |
| Bare executable, `+[UNUserNotificationCenter currentNotificationCenter]` | **NSInternalInconsistencyException**, `bundleProxyForCurrentProcess is nil: mainBundle.bundleURL file:///…`, process aborted (exit 134). The exception comes from `currentNotificationCenter` itself, not from adding a request |
| `.app` with a `CFBundleIdentifier`, unsigned and unregistered, under `$TMPDIR` | `requestAuthorization` answered immediately with `UNErrorDomain` 1, “Notifications are not allowed for this application”; no prompt |
| `.app` ad-hoc signed (`codesign --force --sign -`) and registered (`lsregister -f`), still under `$TMPDIR` | same error 1; no prompt |
| `.app` ad-hoc signed and registered, under `~/Applications` | `getNotificationSettings` = NotDetermined (0); `requestAuthorization` left the system prompt up for the whole 8 s probe, unanswered by script |
| The same id after that unanswered request | Denied (1); `add(request)` returned **no error** while denied, i.e. “sent” cannot mean “displayed” |

So four things are needed, and each fails differently: an app bundle with an identifier
(without it, a crash), a code signature (ad-hoc is enough), LaunchServices registration,
and a location that is not under `/var/folders`. The client therefore checks the bundle
**before** it touches the framework (the crash is in `currentNotificationCenter`), and
`scripts/macos-app.py` both signs and registers the bundle.

The probe and the acceptance bundle left notification entries for their own test ids
(`…spike*`, `…prompt-check2`, `…acceptance.*`) on this Mac; their bundles were removed and
unregistered afterwards. Nothing under `TCC.db`, `com.apple.ncprefs` or any notification
database was read or written, and no system prompt was clicked by script.

## Shipped

- **The service is chosen at the platform boundary.** `src/app/platform/notify.rs` selects
  the freedesktop backend over D-Bus on Linux and the BSDs (`notify/dbus.rs`, the previous
  code moved unchanged in behaviour), the notification centre on macOS
  (`notify/user_notifications.rs`) or an explicit not-available backend elsewhere
  (`notify/none.rs`, Windows). `notify.rs` and `notify/worker.rs` keep `Backend`, `Ledger`,
  `Command`, replacement, close-on-seen, click-once and the 64-note cap. A new
  `Session::event` reports what the desktop does (activation token, click, dismissal, lost
  service); the driver loop is the old loop, parameterised by the backend.
- **macOS requests.** The first need reads `getNotificationSettings`; NotDetermined asks
  for authorization with **alert only** (no badge, no sound: Rex has sounds of its own).
  Authorized, Provisional and Ephemeral post; Denied reports **Turned off in System
  Settings**; a request still waiting for a person reports **Waiting for permission from
  macOS**; a request that fails reports that macOS did not allow notifications for this
  app. Every call has the existing three-second bound, and a completion handler is bridged
  to the worker's tokio thread with a one-slot channel and `try_send`, never to the UI
  thread, so a handler that fires twice cannot block the framework's callback thread.
- **One prompt at a time.** A request still unanswered after its bound is kept on the
  backend. While it is, `ready` checks it without waiting and reports **Waiting for
  permission from macOS** at once: later notes, checks and Send test neither ask macOS
  again nor hold the worker for another three seconds. Once the prompt's answer arrives,
  that command uses it and the next one reads the notification settings again.
- **Identifiers.** Ledger ids map to request identifiers `rex-<launch>-<n>`, where
  `<launch>` is 16 hex digits chosen when the backend starts (a randomly seeded hash of
  the start time and process id). Notes in Notification Center outlive the process and
  ledger ids start again at 1, so without it a new note would silently replace an earlier
  run's note about another tab, and clicking that stale note would open whichever tab has
  its number now. A note that replaces another posts with the same identifier, so the
  centre replaces it instead of stacking; `Seen` removes the delivered identifiers. The
  delegate accepts only this launch's prefix: an earlier launch's click or dismissal is
  ignored. The owner identity is `macos-notification-centre:<prefix>`, constant for the
  process, so the ledger does not reset itself between commands.
- **Clicks.** A `UNUserNotificationCenter` delegate (retained by the backend; the centre's
  reference is weak) reports the default action as a click, which takes the same path as
  Linux: `Ledger::click`, then `open_note(target, None)`. A dismissal forgets the id. The
  delegate also implements `willPresentNotification`, answering Banner + List so a banner
  still shows while Rex is frontmost; the client's own eligibility rules decide which tabs
  notify and are unchanged.
- **A bare executable refuses safely.** `Desktop::new` reads the main bundle identifier and
  never creates the notification centre when it is absent, so a bare
  `target/release/rex-rune` cannot reach the aborting call. `ready` returns the platform's
  own reason, “Start Rex Rune.app to allow notifications”, which Settings shows in the row.
- **One Settings page, platform words.** Settings → Notifications takes the section title,
  the service row's detail and the footnote from `platform::notify`: “Desktop service”,
  “Last check, not a delivery receipt.” on Linux and the BSDs (unchanged), “macOS
  notifications” / “What macOS lets this app do.” on macOS, and not-available words
  elsewhere. A new `Service::Unavailable(reason)` state shows the chip **Unavailable** and
  puts the reason in the row itself rather than in a tooltip. Linux never produces it.
- **`scripts/macos-app.py`** builds `target/release/Rex Rune.app`: `Contents/MacOS/rex-rune`,
  an `Info.plist` (`dev.shuv.rex-rune`, Rex Rune, the Cargo.toml version, `NSHighResolutionCapable`,
  and `LSMinimumSystemVersion`: the later of the binary's own `minos` and 11.0, because
  `UNUserNotificationCenter` needs 10.14 and the banner presentation used while Rex is
  frontmost needs 11.0; an Intel build's `minos` of 10.12 is not claimed), an `.icns` made
  with `iconutil` from `resources/icons` at 16–256 px (only ever scaled down with `sips`;
  the largest source is 256 px and iconutil takes a subset of sizes), an ad-hoc
  `codesign --sign -`, and `lsregister -f`. It prints that it is ad hoc and not a release.
  `--unregister` runs `lsregister -u` on the bundle and removes it.
- **Restart failure.** `notify_restart_failure` now delegates to the platform. D-Bus is
  unchanged. macOS posts “Rex restart failed” through the same backend when bundled, and
  only if notifications are already allowed: there is no window left to explain a prompt.
  A bare executable reports unavailable, and stderr already carries the message.
- **zbus is now a Linux/BSD-only dependency**, so Windows no longer builds it. The macOS
  dependencies (`objc2` 0.5.3, `objc2-foundation` 0.2.2, `objc2-user-notifications` 0.2.2,
  `block2` 0.5.1) were already in `Cargo.lock` through Rune; `Cargo.lock` changed only in
  `rex-rune`'s dependency list and `cargo build --locked` succeeds.

## Decisions and boundaries

- **The app bundle is local, not a release.** No notarization, Developer ID, CI publishing
  or installer. `macos-app.py` is a source-build convenience; the README says so.
- **No polling.** Permission is read once per command, like the D-Bus server information,
  and there is no timer. While a prompt is unanswered, each command checks the kept
  request without waiting and reports waiting; the command after the answer arrives uses
  it. Health in Settings changes only when a command runs (opening the page is one).
- **The permission prompt is a person's.** No script answers it, and no privacy database is
  edited. The acceptance script never launches the real `dev.shuv.rex-rune` bundle: it
  runs a copy under a throwaway bundle id, so the real app's first prompt stays the
  person's.
- **Out of scope:** notification actions beyond the default click (no categories are
  registered), badges, critical alerts (no entitlement), Windows notifications, and macOS
  sound settings. Rex's own local activity sounds already play on macOS with `afplay`
  (`sounds/player.rs`) and were not touched; the notification request asks for no sound.

## Verification

Evidence root: `~/.cache/agent-ws/rex-m11-notify/`.

### Automated checks

All on this Mac (Apple Silicon, macOS 27), with Rust 1.96.1:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo build --locked --release --bin rex-rune
python3 scripts/macos-app.py
```

All passed. The full suite ran **1133 tests: 1129 passed, 0 failed, 4 ignored**. No
existing test was weakened: the D-Bus error-classification, image-data and urgency tests
live beside the D-Bus backend with their assertions unchanged (urgency is now a private
function there, and the Settings test asserts that its test note is `Kind::Test`), and
the headless outbox tests still cover replacement, isolation, close-on-seen, failed sends,
the cap and one navigation per click. The macOS backend has seven pure tests: identifier
round-trip; identifiers of the form `rex-<16 hex digits>-<n>`, with an earlier launch's
identifier for the same ledger id refused and the launch in the owner identity; foreign
identifiers refused; an unanswered prompt reported as waiting on every command without
being replaced, and dropped once answered (or once its handler is gone); each prompt
answer's reason; the bare-bundle refusal; and the status-to-reason map. Linux and Windows
were not built here; their CI jobs compile the D-Bus and not-available backends, and the
edits to them (moving `urgency` into `dbus.rs`, comments in `none.rs`) were checked only
by reading.

### Private native acceptance

`scripts/notification-macos-private.py` starts its own Rex server (from Rex Beta.app, read
in place) with a private socket, state directory, HOME, `ZDOTDIR` and `HISTFILE` under
`/tmp/rex-notifications-*`, creates a session whose shell is a wrapper on that HOME, and
runs the client with Rune's `--script`/`--snap`/`--tree` harness. It stops the server by
process group (SIGTERM, then SIGKILL after 10 s) and asserts the real `~/.zsh_history`
gained nothing from the private session. It never contacts the daily Rex server,
settings or display.

It never launches the real `dev.shuv.rex-rune` bundle. It copies `target/release/Rex
Rune.app` to `target/notification-acceptance/<random>/Rex Rune Acceptance.app`, sets the
copy's identifier to `dev.shuv.rex-rune.acceptance.<random>` (and its name to Rex Rune
Acceptance), re-signs it ad hoc, asserts the signature's identifier is the throwaway one,
registers it with `lsregister -f`, runs only that copy, and in a `finally` block runs
`lsregister -u` and deletes it. The copy is under the repository's `target/`, not
`/var/folders`, where macOS refuses notification requests.

Run against the final release build and bundle (2026-11-13, captures in
`captures-review/`):

1. **Bare `target/release/rex-rune`:** exits 0, no crash. Settings → Notifications shows
   **Unavailable**, with the row reading **Start Rex Rune.app to allow notifications**
   (`M11-bare-binary.png`). Asserted.
2. **Throwaway bundle** (`dev.shuv.rex-rune.acceptance.27443e7531f2`): exits 0. Opening
   the page raised the system permission prompt for the new id (not answered), and after
   the request's bound the page shows **Unavailable** and **Waiting for permission from
   macOS**. Asserted.
3. **Send test notification while that prompt is still up:** pressed at 6.0 s and read at
   6.8 s, the row says **Last test failed: Waiting for permission from macOS.** with the
   Failed chip, and no **Sending test…** remains (`M11-waiting-and-send-test.png`).
   Asserted. The previous code asked macOS again for this command and would still have
   been sending for three seconds; the unified log showed no `usernoted` entry naming the
   throwaway id to count requests directly, so this timing and the unit test are the
   evidence that no second request was made.

The copy was then unregistered and removed, and the server stopped. macOS keeps a
notification entry for each throwaway id it has seen, in System Settings →
Notifications; nothing here reads or edits it. A request identifier with the launch
prefix was not observed natively, because nothing is posted without permission; the unit
tests cover its form.

Earlier evidence, from the first version of this script, which did launch the real
bundle: that run raised the real `dev.shuv.rex-rune` prompt and exited without answering
it, and macOS has since reported that id as **denied**; it stays denied across a rebuild
and re-sign. In that state the page reported **Unavailable** / **Turned off in System
Settings**, and Send test **Last test failed: Turned off in System Settings.**
(`captures-final/`). That is why the script now uses a throwaway id. To use
notifications, turn **Rex Rune** on in System Settings → Notifications. The prompt itself
is a system window, not part of the captured app window, and `screencapture` fails for
this process (“could not create image from display”), so there is no capture of it.

### Hand checks (need a person, and an allowed permission)

Not verified here, because they require clicking **Allow**:

- A posted banner, replacing an activity note rather than stacking, and a click showing
  the tab (and raising the window).
- Seeing a tab removing its delivered notification.
- A banner showing while Rex is frontmost (`willPresentNotification`).
- The restart-failure notification from a bundled app.
- Turning permission back on in System Settings for `dev.shuv.rex-rune`, then opening
  Settings → Notifications in `Rex Rune.app` by hand: it should read **Available**, and
  Send test **Last test: sent to the service.**
- A note left in Notification Center by an earlier launch: a new note does not replace
  it, and clicking it shows no tab.

## Not verified / limits

- Real delivery, replacement, close-on-seen, click navigation and foreground presentation
  were not exercised: the permission prompt was never answered by this worker, and macOS
  does not deliver while denied. The request-building and completion bridge are covered by
  reading and by the accepted `add` seen in the spike, not by a native post.
- The permission prompt cannot be answered without a person; no automation was added for
  it. An unanswered prompt that goes away with the app leaves notifications off for that
  bundle id (observed), which is why the README says to re-enable it in System Settings.
- Whether macOS keeps an allowed permission across a rebuild was not checked (it kept the
  denied state across a re-sign here).
- The delegate is installed when the notification worker starts (the first command), on
  that thread, not at launch. A click on a note that launches Rex is therefore not
  delivered; since identifiers are per launch, that note would not be this launch's
  anyway.
- While a prompt is unanswered, this process reports waiting until that request's answer
  arrives. A change made in System Settings before the prompt is answered was not
  checked.
- Dismissal is mapped to forgetting the id; no delivered-notification list is polled, so a
  note the person removes in Notification Center without a response is forgotten only
  through that response or a later `Seen`.
- Windows compiles the not-available backend in CI; nobody has run it.
- Linux was not run natively for this slice; the D-Bus backend moved without assertion
  changes and its CI job remains the check.
- No release or merge was performed by this record's authors.
