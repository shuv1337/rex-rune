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
(`…spike*`, `…prompt-check2`) on this Mac, turned off; their bundles were removed and
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
  to the worker's tokio thread with a channel, never to the UI thread.
- **Identifiers.** Ledger ids map to request identifiers `rex-<n>`. A note that replaces
  another posts with the same identifier, so the centre replaces it instead of stacking;
  `Seen` removes the delivered identifiers. Ids are minted in the backend and never reused
  within a worker's life. The owner identity is a constant (there is one Notification
  Center), so the ledger does not reset itself between commands.
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
  `LSMinimumSystemVersion` read from the binary's `minos`), an `.icns` made with `iconutil`
  from `resources/icons` (missing sizes made with `sips`), an ad-hoc `codesign --sign -`,
  and `lsregister -f`. It prints that it is ad hoc and not a release.
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
- **No polling.** Permission is read once per command, like the D-Bus server information.
  While a prompt is unanswered, the next command reads the answer; there is no timer.
- **The permission prompt is a person's.** No script answers it, and no privacy database is
  edited. The acceptance script uses a throwaway bundle id when it wants to see the
  waiting state, so it does not consume the real app's first prompt.
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

All passed. The full suite ran **1127 tests: 1127 passed, 0 failed, 4 ignored**. No
existing test was weakened: the D-Bus error-classification and image-data tests moved
beside the D-Bus backend with their assertions unchanged, and the headless outbox tests
still cover replacement, isolation, close-on-seen, failed sends, the cap and one
navigation per click. The macOS backend adds four pure tests: identifier round-trip,
foreign identifiers refused, the bare-bundle refusal and reason, and the status-to-reason
map (authorized/provisional allowed, denied and not-determined reported differently).
Linux and Windows were not run natively here; their CI jobs compile the same boundary.

### Private native acceptance

`scripts/notification-macos-private.py` starts its own Rex server (from Rex Beta.app, read
in place) with a private socket, state directory, HOME, `ZDOTDIR` and `HISTFILE` under
`/tmp/rex-notifications-*`, creates a session whose shell is a wrapper on that HOME, and
runs the client with Rune's `--script`/`--snap`/`--tree` harness. It stops the server by
PID and checks the real `~/.zsh_history` did not change. It never contacts the daily Rex
server, settings or display.

Run against the final release build and bundle (`notification-macos-private.py`):

1. **Bare `target/release/rex-rune`:** exits 0, no crash. Settings → Notifications shows
   **Unavailable**, with the row reading **Start Rex Rune.app to allow notifications**
   (`captures-final/M11-bare-binary.png`).
2. **`Rex Rune.app`:** exits 0. This Mac's `dev.shuv.rex-rune` is currently denied, so the
   page reports **Unavailable** and **Turned off in System Settings**
   (`captures-final/M11-app-bundle.png`).
3. **Send test notification:** the row reports **Last test failed: Turned off in System
   Settings.**, with the Failed chip, and stays within the page
   (`captures-final/M11-send-test.png`).

The waiting state was observed separately with a throwaway copy of the same bundle under
`target/release/` (id `dev.shuv.rex-rune.prompt-check2`, ad-hoc signed, registered), which
raised a fresh permission prompt: while that prompt was up, Settings showed **Unavailable**
and **Waiting for permission from macOS** (`captures-waiting/M11-app-bundle.png`). The
copy was removed and unregistered afterwards. The prompt itself is a system window, not
part of the captured app window, and `screencapture` failed for this process (“could not
create image from display”), so there is no screen capture of it.

The first run against the real bundle raised that prompt and then exited at the end of the
script without answering it; macOS has since reported `dev.shuv.rex-rune` as **denied**,
which is the state the captures above record. It stays denied across a rebuild and re-sign
of the same bundle id. To use notifications now, turn **Rex Rune** on in System Settings →
Notifications.

### Hand checks (need a person, and an allowed permission)

Not verified here, because they require clicking **Allow**:

- A posted banner, replacing an activity note rather than stacking, and a click showing
  the tab (and raising the window).
- Seeing a tab removing its delivered notification.
- A banner showing while Rex is frontmost (`willPresentNotification`).
- The restart-failure notification from a bundled app.
- Turning permission back on in System Settings for `dev.shuv.rex-rune`, then repeating
  items 2 and 3 above (they should read **Available** and **Last test: sent to the
  service.**).

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
- Dismissal is mapped to forgetting the id; no delivered-notification list is polled, so a
  note the person removes in Notification Center without a response is forgotten only
  through that response or a later `Seen`.
- Windows compiles the not-available backend in CI; nobody has run it.
- Linux was not run natively for this slice; the D-Bus backend moved without assertion
  changes and its CI job remains the check.
- No release, push, PR or merge was performed by this record's author beyond opening the
  pull request.
