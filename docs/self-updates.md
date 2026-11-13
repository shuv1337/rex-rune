# In-app install.sh updates

Settings → Updates offers **Update now** for the install.sh/tarball channel. (A pacman
install has its own, smaller path: see [Pacman installs](#pacman-installs).)
**Copy command** remains a manual fallback. Progress and failures stay on that
page; a successful install offers **Restart to finish**. Restart saves the
workspace, closes the window, and waits for host shutdown (up to two seconds) so
in-flight downloads can remove partial files before re-exec. Original launch
arguments, including `--config`, `--font`, `--font-size` and `--window-size`, are
preserved, except `--new`: session creation is never replayed. Rex server sessions
keep running. If re-exec fails after the window closes, Rex sends a critical desktop
notification through the session notification service (and logs any delivery failure
to stderr); reopen Rex or repair the installation.

## Eligibility and trust

- Only a release install whose schema-1 receipt matches the running binary and
  version can update itself. Source, pacman (`rex-rune-bin`), deb, Nix and other
  package channels keep their existing instructions. No sudo or root operations.
- The binary, launcher, three icons and receipt must be user-writable regular
  files in existing directories under `~/.local`, on one filesystem. Custom
  `XDG_BIN_HOME` / `XDG_DATA_HOME` paths are supported within that boundary.
  Symlinked install files, missing directories and read-only installs are refused
  with a reason. Use the manual installer to repair an invalid/missing receipt.
- Clicking Update now fetches and verifies the manifest again, using only the two
  embedded Minisign keys. It must still name the release shown when clicked, be
  strictly newer than the running/receipt version, satisfy `update_from`, and not
  be older than the highest previously verified release. If latest changed, use
  Check now and retry. Cached update-state JSON is never an artifact trust source.
  The signed manifest has **no expiry**: a valid older manifest at or above the
  local high-water mark is accepted. The high-water mark prevents rollback below
  an observed release, not freezing updates at an already observed release.
- The archive URL comes only from that verified manifest. Initial and redirected
  requests must be HTTPS. Connection timeout is 20 seconds; each transport read
  has a 30-second idle timeout, with a one-hour overall cap including redirects.
  Timeouts are reported as “timed out”. Downloads must match the signed size
  exactly (also capped at 256 MiB) and SHA-256 before extraction or execution.
  The signed target's minimum glibc version is checked before downloading.
- Extraction is into a private directory beside the installed binary. Paths must
  stay below the version/architecture package root; absolute paths, `..`, `.`,
  duplicate paths, all symlinks/hardlinks and special files are refused. Expanded
  payload is capped at 1 GiB and 20,000 entries. No archive installer script runs.
  The verified new binary must report the expected `--version` within 10 seconds;
  otherwise it is killed and the existing installation is left untouched.

## Files and rollback

The file layout matches `scripts/install-desktop.sh --copy --receipt-version`:

- `$XDG_BIN_HOME/rex-rune` (default `~/.local/bin/rex-rune`), mode 0755;
- `$XDG_DATA_HOME/applications/dev.shuv.rex-rune.desktop`;
- `$XDG_DATA_HOME/icons/hicolor/{32,128,256}x{32,128,256}/apps/dev.shuv.rex-rune.png`;
- `$XDG_DATA_HOME/rex-rune/install.json`, mode 0600, containing schema, canonical
  installed path, version and binary SHA-256. Data defaults to `~/.local/share`.

`desktop-file-install` and `desktop-file-validate` are required, just as for the
shell installer. Launcher Exec/TryExec rewriting uses the same quoting. Desktop
and icon caches are refreshed best-effort after committing the payload.

All files are prepared before replacement. A lock prevents simultaneous in-app
updates. A journal and hard-linked backups are synced before replacing files via
same-filesystem atomic renames; the old binary stays backed up until the whole
installation, including its receipt, commits. The journal records each prepared
file's device/inode identity, size and nanosecond modification time. Recovery
requires all of these to match, so inode reuse alone cannot identify a later repair
as this transaction's file. Recovery restores only destinations still belonging
to this transaction, skipping files already restored or not yet replaced. If any
destination belongs to a later installer/repair, the entire journal is discarded
without changing any destination, and Rex shows an informational notice that the
obsolete journal and backups were discarded. Files absent before are removed only
if they still have this transaction's full file identity. Preferences and the Rex server are never
touched.

Individual files are atomically replaced, not the entire multi-directory tree in
one filesystem operation. An interrupted transaction is recovered before the UI
starts on the next launch, using the journal's original install paths even if the
launch's XDG environment changed. Recovery re-execs the restored binary before
any workspace actions run. Only when a transaction journal exists does recovery
wait up to two seconds for the install lock. Without a journal, housekeeping tries
the lock once and silently skips a busy lock, an unwritable directory or a root
launch. If journal recovery fails or the lock remains busy, Rex logs the error to stderr and
**continues opening**, with a persistent (dismissible) UI notice. A failed recovery
retains `.rex-update-transaction` next to the binary for another attempt. Committed
journals and abandoned `.rex-update-*` / `.rex-update-cleanup-*` directories are
swept best-effort under the same lock, after recovery, including when no transaction
was started. Sweep or post-rollback cleanup failures never suppress a successful
rollback's required re-exec.

### Manual recovery

First close Rex windows and allow any updater to finish; do not run recovery as
root. A transient lock/storage failure can be retried by reopening Rex. Persistent
errors (a legacy journal without full file identity records, mismatched binary paths, or data
folders symlinked outside `~/.local`) require manual repair. There is no rollback
button or `--rollback` command.

Keep the journal and its `old-*` backups until a complete manual installation has
been verified. Do **not** copy individual backups over files from a later repair.
For a default install, retire the journal under the install lock before repairing:

```sh
bin="${XDG_BIN_HOME:-$HOME/.local/bin}"
(
  flock -w 10 9 || exit 1
  if [ -d "$bin/.rex-update-transaction" ]; then
    mv "$bin/.rex-update-transaction" "$bin/.rex-recovery-backup-$(date +%s)"
  fi
  curl --proto '=https' --tlsv1.2 -fsSL \
    https://github.com/shuv1337/rex-rune/releases/latest/download/install.sh | sh
) 9>"$bin/.rex-update.lock"
"$bin/rex-rune" --version
```

Use the original installation's `XDG_BIN_HOME` and `XDG_DATA_HOME` if customized
(the journal's `binary` and `data` fields record them). Check that the launcher
opens and the expected version is installed before deleting the retained
`.rex-recovery-backup-*` directory. If repair fails, retain it for diagnosis.
Directories outside the supported boundary can still be managed by the manual
installer, but remain ineligible for in-app updates. Deliberately installing an
older release remains the shell installer's explicit `--version` operation.

## Updating from Rex Linux 0.7.x

The client was `rex-linux` through 0.7.x, and the updater inside those clients cannot
change. It checks that `latest.json` has `product: "rex-linux"` and a signature whose
trusted comment is `rex-linux vX.Y.Z latest.json`, downloads the target's `tarball`, and
installs it only if the archive's root is `rex-linux-<version>-<target>`, `bin/rex-linux`
prints exactly `rex-linux <version>`, and the launcher and icons are named
`dev.shuv.rex-linux`. A renamed release keeps all of that true:

- **Signed metadata keeps the product id `rex-linux`**, in `latest.json` and in every
  trusted comment. Current clients require the same id.
- **Each target has two archives.** `tarball` is the bridge,
  `rex-linux-<version>-<target>.tar.gz`: this release's binary as `bin/rex-linux` with
  the old launcher and icon names, and an `install.sh` that only refuses and names the
  current installer (a kept copy of the 0.7.x `install.sh` would run it). `archive` is
  `rex-rune-<version>-<target>.tar.gz`, which current clients and `install.sh` install.
- **Started as `rex-linux`, the client names itself that way** in `--version`, through
  the bridge's binary or the compatibility link.

The 0.7.x updater therefore installs the new release under the old names. At the next
start, the client finds a `rex-linux` receipt that matches its own binary and finishes
the rename (`update::installer::migrate_legacy_install`):

1. writes `dev.shuv.rex-rune.desktop` and the three `dev.shuv.rex-rune.png` icons;
2. renames `bin/rex-linux` to `bin/rex-rune` and leaves `rex-linux` as a link to it;
3. writes `$XDG_DATA_HOME/rex-rune/install.json`;
4. removes the old launcher and icons, then the old receipt.

The old receipt is what triggers the move and is removed last, so a start interrupted
part-way is finished by the next one. Nothing is moved unless the receipt names this
directory and matches this binary's SHA-256, while an update is in progress, or as
root. If `desktop-file-install` fails, a notice says so and the next start tries again;
until then the old launcher keeps working.

An update from 0.7.x that is cut off part-way is undone as any other: 0.7.5 journals
the files under their old names, the journal's format is unchanged, and a client
started as `rex-linux` looks for those names. Recovery runs before settings move, and
nothing is moved by a process whose installation was just rolled back, so the restored
0.7.x client finds its settings and receipt where it left them. If `install.sh`
replaces the installation before the client starts again, the journal that update
left is cleared as superseded.

One limit: a start with a different `XDG_DATA_HOME` from the install's does not find
the receipt, so the rename waits for a start that has it.

Settings and update state are moved separately, for every install method: see
[Renamed from Rex Linux](../README.md#renamed-from-rex-linux).

## Pacman installs

A pacman install cannot replace its own files: they are root's. The `rex-rune-bin`
package therefore carries a small root helper and a polkit action for it, and
Settings → Updates offers **Update now** when all of these hold:

- the install is Rex's own pacman package (its channel marker names no other updater);
- `/usr/lib/rex-rune/rex-rune-update` and `/usr/bin/pkexec` exist (the package from
  the first release that ships the helper, and `polkit`);
- a newer release has been verified, with no manual-upgrade floor.

Otherwise the page shows the instructions it always has (`omarchy update` on Omarchy,
`sudo pacman -Syu rex-rune-bin` elsewhere), and they stay on the page as the fallback
either way. On Omarchy the text still recommends `omarchy update`, which updates Rex
Rune with the rest of the system.

**Update now** runs `pkexec --disable-internal-agent /usr/lib/rex-rune/rex-rune-update
<version>` on a worker thread and waits. Your desktop's polkit agent asks for an
administrator's password (on a one-person Arch or Omarchy machine, your own); nothing
runs before that. The client passes the version and
reads an exit code, and that is all it does: the helper downloads the package itself
from the Rex package site, checks it against the Rex package key shipped in the
package and against this computer's pacman keyring, refuses anything but a newer
`rex-rune-bin`, and installs it as a systemd service so that closing Rex does not stop
it half-way. [The design note](plans/pacman-update-now.md#as-built) has the reasoning
and what was tested.

![Update now offered on an Omarchy pacman install](images/pacman-update-available.png)

![Waiting for the password and the update](images/pacman-update-waiting.png)

Real-font captures of each state come from `pacman_update_draw`, run alone with
`REX_TEST_SNAP_DIR` (and `REX_UPDATE_MINIMUM=1` for 640×480).

What the page says afterwards:

| Ending | Shown |
| --- | --- |
| Upgraded | **Restart to finish**, as for an install.sh update. It starts `/usr/bin/rex-rune`, the client the package installs |
| The dialog was closed | "Update cancelled. Nothing was installed."; **Update now** is back |
| Not authorized, or no polkit agent running | Why, with the command to copy |
| Any refusal by the helper (held back by `IgnorePkg`, another pacman running, a download or signature problem, a key this computer does not trust, other packages would change, pacman failed, no systemd) | The reason in a sentence, with the command to copy. `journalctl -t rex-rune-update` has the helper's own words |

Limits, as built:

- The waiting line has no time limit of its own. It ends when `pkexec` does: the helper
  bounds its downloads, but a dialog nobody answers stays, and the page waits with it.
  Restarting Rex clears it.
- Quitting Rex after the password was given does not stop the update. Quitting while
  the dialog is still open leaves that to polkit: the dialog may stay until dismissed,
  and nothing is installed unless it is answered.
- The helper is looked for once, at start, so a package that gains it while Rex runs
  offers Update now after a restart. If the package was already upgraded another way
  while Rex runs, Update now says the installed package is already this version or
  newer; restart Rex to use it.
- A release that needs a manual upgrade first (a floor) shows instructions only, as
  for install.sh installs.

It never starts on its own: **Install updates automatically** applies to install.sh
installs only, since a password dialog nobody asked for would be worse than a notice.
The helper reports nothing until it ends, so the page shows one waiting line and no
percentage.

Not verified: the polkit dialog on a real desktop, and an upgrade against the
production package site (that needs a release newer than an installed `rex-rune-bin`
that has the helper). The headless tests never start `pkexec`.

## Automatic installation

**Install updates automatically** defaults **off**, including existing settings
files. When enabled (`updates.install_automatically: true`), a successful scheduled
or manual check can install an eligible release in the background and offer
restart. It never restarts without a click. Automatic checks must also be on;
`REX_RUNE_NO_UPDATE_CHECK=1` (or its old name, `REX_LINUX_NO_UPDATE_CHECK=1`) disables automatic installation too. The daily check
cadence is unchanged. An installation error stays visible for manual retry; it
does not create a rapid background retry loop. Checks and installation cannot run
concurrently in one workspace. Across processes, the update-state lock covers only
load/apply/save, not network requests; each fetch reloads the latest state before
applying its manifest. Dismissal and announcement saves change only their notice
fields under that same lock, preserving the verified-version high-water mark.

## Validation

Requires the normal Rust/Zig toolchains and `desktop-file-utils`:

```sh
cargo test --locked --no-default-features update::installer
cargo test --locked --all-features updater_
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
```

`signed_fixture_end_to_end` serves a manifest, Minisign signature and tarball from
loopback using a deterministic **test-only** key, then exercises the real hash,
extraction, desktop integration, replacement and receipt code against a temporary
prefix. Only the artifact transport is substituted to map the signed production
URL to the local fixture. It executes the installed fixture binary and checks its
version, permissions and receipt. The shipped app has no alternate-key or HTTP
artifact option. Additional tests cover tampered/unknown signatures, size/hash
failures, downgrade/floor/changed-release refusals, unsafe tar paths and links,
read-only files, rollback at every replacement, partial rollback failures, lock
contention, stale-stage cleanup, actual-executable recovery, and a real
`install-desktop.sh` repair after interruption. The signed fixture includes a PAX
long path, matching Python tarfile releases; archive tests exercise size bombs
and more than 20,000 entries. Cases requiring non-root installation are skipped
when running tests as root.

The rename has its own checks. Unit tests move a `rex-linux` installation to the new
names, finish a move cut short after the binary was renamed, refuse an unverified
one, and roll back an update from the old client that was cut off after it replaced
the binary. `scripts/test-bridge.py` holds a bridge tarball to the 0.7.x updater's rules, and
the release workflow runs it on the real build before taking over real 0.7.5 tarball
and `.deb` installs. `scripts/bridge-e2e.sh` runs the whole path in a scratch home: the
updater built from the v0.7.5 tag, with only its download pointed at a loopback
fixture, installs this checkout's release build from the bridge tarball, and the
client's first start then renames the installation and moves its settings. Run it
before the release that carries the rename.

Real-font screenshots can be captured by running this test alone:

```sh
mkdir -p /path/to/captures
REX_TEST_SNAP_DIR=/path/to/captures cargo test --locked --all-features \
  updater_settings_render_install_states -- --test-threads=1
# Add REX_UPDATE_MINIMUM=1 for the 640×480 layout.
```
