# In-app install.sh updates

Settings → Updates offers **Update now** for the install.sh/tarball channel.
**Copy command** remains a manual fallback. Progress and failures stay on that
page; a successful install offers **Restart to finish**. Restart saves the
workspace and re-execs the installed binary with the same settings-file path,
without replaying CLI commands or session-creation arguments. Rex server sessions
keep running and the normal saved-workspace attachment behavior applies.

## Eligibility and trust

- Only a release install whose schema-1 receipt matches the running binary and
  version can update itself. Source, pacman (`rex-linux-bin`), deb, Nix and other
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
- The archive URL comes only from that verified manifest. Initial and redirected
  requests must be HTTPS. Connection timeout is 20 seconds, total request timeout
  300 seconds. Downloads must match the signed size exactly (also capped at
  256 MiB) and SHA-256 before any extraction or execution.
- Extraction is into a private directory beside the installed binary. Paths must
  stay below the version/architecture package root; absolute paths, `..`, `.`,
  duplicate paths, all symlinks/hardlinks and special files are refused. Expanded
  payload is capped at 1 GiB and 20,000 entries. No archive installer script runs.
  The verified new binary must successfully report the expected `--version`.

## Files and rollback

The file layout matches `scripts/install-desktop.sh --copy --receipt-version`:

- `$XDG_BIN_HOME/rex-linux` (default `~/.local/bin/rex-linux`), mode 0755;
- `$XDG_DATA_HOME/applications/dev.shuv.rex-linux.desktop`;
- `$XDG_DATA_HOME/icons/hicolor/{32,128,256}x{32,128,256}/apps/dev.shuv.rex-linux.png`;
- `$XDG_DATA_HOME/rex-linux/install.json`, mode 0600, containing schema, canonical
  installed path, version and binary SHA-256. Data defaults to `~/.local/share`.

`desktop-file-install` and `desktop-file-validate` are required, just as for the
shell installer. Launcher Exec/TryExec rewriting uses the same quoting. Desktop
and icon caches are refreshed best-effort after committing the payload.

All files are prepared before replacement. A lock prevents simultaneous in-app
updates. A journal and hard-linked backups are synced before replacing files via
same-filesystem atomic renames; the old binary stays backed up until the whole
installation, including its receipt, commits. A failed replacement restores all
previous files, including files originally absent. Preferences and the Rex server
are never touched.

Individual files are atomically replaced, not the entire multi-directory tree in
one filesystem operation. An interrupted transaction is recovered before the UI
starts on the next launch. If restoration fails (for example, storage becomes
unavailable), the app reports the error and retains `.rex-update-transaction`
next to the binary for another recovery attempt; do not delete its backups.
Completed updates remove the backups. There is no user-requested rollback button
or `--rollback` command; deliberately installing an older release remains the
shell installer's explicit `--version` operation.

## Automatic installation

**Install updates automatically** defaults **off**, including existing settings
files. When enabled (`updates.install_automatically: true`), a successful scheduled
or manual check can install an eligible release in the background and offer
restart. It never restarts without a click. Automatic checks must also be on;
`REX_LINUX_NO_UPDATE_CHECK=1` disables automatic installation too. The daily check
cadence is unchanged. An installation error stays visible for manual retry; it
does not create a rapid background retry loop.

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
read-only files, rollback at every replacement and interrupted recovery.

Real-font screenshots can be captured by running this test alone:

```sh
mkdir -p /path/to/captures
REX_TEST_SNAP_DIR=/path/to/captures cargo test --locked --all-features \
  updater_settings_render_install_states -- --test-threads=1
# Add REX_UPDATE_MINIMUM=1 for the 640×480 layout.
```
