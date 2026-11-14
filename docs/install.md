# Installing Rex Rune

Rex Rune is a desktop client, not a Rex server. Every install method here installs
only the `rex-rune` client binary, its `.desktop` launcher and icons. It never
installs, replaces or shadows the `rex` server.

Stable binary releases are Linux-only, for **x86_64** (amd64) and **aarch64**
(arm64). The current release is **0.10.0**. macOS and Windows are source builds:
see [Building and development](building.md#macos) and
[Building and development](building.md#windows).

On distributions where `/bin` is a symlink to `/usr/bin` (`usrmerge`),
`which -a rex` can list both `/usr/bin/rex` and `/bin/rex`; that is one file, not
a second server.

## Arch Linux and Omarchy (pacman)

Install `rex-rune-bin` from the Rex package repository:

```sh
sudo pacman -Syu rex-rune-bin
```

The repository section is still named `[rex-linux]` so existing `pacman.conf`
files keep working. Add it once to `/etc/pacman.conf`:

```ini
[rex-linux]
SigLevel = Required DatabaseRequired
Server = https://pkg.shuv.dev/arch/$arch
```

Trust the archive key first:

```sh
curl --proto '=https' --tlsv1.2 -fsSLo rex-linux-archive-key.asc https://pkg.shuv.dev/rex-linux-archive-key.asc
sudo pacman-key --add rex-linux-archive-key.asc
```

Both package and database signatures are mandatory. On Omarchy, `omarchy update`
updates Rex Rune with the rest of the system. After installing,
`command -v rex-rune` should print `/usr/bin/rex-rune`, and `pacman -Qi
rex-rune-bin` should say `Validated By : Signature`.

A `rex-linux-bin` install needs no change to `pacman.conf`: its next
`sudo pacman -Syu` asks to replace it with `rex-rune-bin`. See
[Renamed from Rex Linux](legacy-rex-linux.md).

## One-command user install or upgrade

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/shuv1337/rex-rune/releases/latest/download/install.sh | sh
# Explicit upgrade, check, pin (including deliberate downgrade), or uninstall:
# ... | sh -s -- --upgrade
# ... | sh -s -- --check
# ... | sh -s -- --version X.Y.Z
# ... | sh -s -- --uninstall
```

The signed, immutable release asset detects x86_64/aarch64 Linux and requires
**glibc 2.39+**, `curl`, `tar`, coreutils, `bash`, `desktop-file-utils`, and
either **minisign or OpenSSL 3+**. It verifies the signed checksums with embedded
CI and backup keys, verifies the tarball, then installs `~/.local/bin/rex-rune`
with a launcher and icons. `XDG_BIN_HOME`, `XDG_DATA_HOME` and `XDG_CACHE_HOME`
are respected. No root, server changes or automatic downgrade. Uninstall keeps
preferences.

NixOS users should use [Nix](nix.md); musl is unsupported.

**Pipe-to-shell is trust on first use:** HTTPS and GitHub deliver the script you
execute. Its detached `install.sh.minisig` can be verified with a separately
trusted key before running a saved copy; retain that reviewed copy for upgrades.
The script's body runs only after the complete function has downloaded. Package
verification is mandatory; there is no unsigned mode. A saved script with old
keys fails closed after rotation. **Only** explicit `--trust-repo-keys` expands
trust to keys at the requested release's protected tag (never `master`), printing
the key ID and fingerprint. Prefer fetching and reviewing the current script.

`/usr/bin/rex-rune` (or `/usr/bin/rex-linux`) or a mismatching install receipt
blocks replacement; `--force` deliberately overrides those guards, **not
signature checks or downgrade protection**. A receipt requires Python 3 to
validate; pre-receipt installs such as 0.5.1 upgrade without it. An install made
as `rex-linux` is checked the same way and replaced: its launcher and icons are
removed, and `~/.local/bin/rex-linux` becomes a link to `rex-rune`. Avoid mixing
user installs and package managers.

## Manual download

Download a package and `SHA256SUMS` from
[Releases](https://github.com/shuv1337/rex-rune/releases). Choose **x86_64 /
amd64** for Intel/AMD or **aarch64 / arm64** for ARM64. For signed releases,
[verify the signature](#verify-a-download) before installing.

```sh
sha256sum --ignore-missing -c SHA256SUMS

# Debian/Ubuntu: install the one .deb matching your architecture.
sudo apt install ./rex-rune_X.Y.Z_amd64.deb

# Or extract the matching rex-rune tarball and install for your user (no root).
tar -xzf rex-rune-X.Y.Z-x86_64-linux-gnu.tar.gz
cd rex-rune-X.Y.Z-x86_64-linux-gnu
bash install.sh
```

The tarball installs into `~/.local/bin` with a desktop launcher and icons,
independently of any source checkout. It needs `desktop-file-utils`; uninstall
with `bash scripts/install-desktop.sh --uninstall`. Uninstall a `.deb` with
`sudo apt remove rex-rune`. Neither removes preferences or changes Rex servers.
Avoid mixing both installation methods: a user installation can shadow
`/usr/bin/rex-rune`.

Prebuilt packages are built on Ubuntu 24.04 and require **glibc 2.39+**. They are
dynamically linked, not self-contained AppImages. Install your distribution's
graphics drivers, fontconfig (including `fc-match`), xkbcommon, Wayland/X11,
EGL/OpenGL/Vulkan libraries, CA certificates and `xdg-utils`. On Debian/Ubuntu
the `.deb` declares these dependencies. Rust and Zig are **not** needed to run a
binary. A running Rex server with API revision 10, 11, 12 or 13 is required.

## Verify a download

Releases from 0.5.0 include `SHA256SUMS.minisig`, `latest.json`, and
`latest.json.minisig`; from 0.5.2, `install.sh` and `install.sh.minisig` too.
Both release keys are configured; v0.4.2 and earlier remain unsigned. The CI
public key is committed as
[`packaging/keys/release.pub`](../packaging/keys/release.pub):

- Minisign key ID: `A1FD78C27652816E`.
- SHA-256 fingerprint (decoded 42-byte minisign public-key record):
  `9e779605311a46a5287d6c9b25eb2083fb17447454239c7d66f1ecebe336d0c6`.

The offline recovery public key is committed as
[`packaging/keys/release-backup.pub`](../packaging/keys/release-backup.pub):

- Minisign key ID: `368EA1A12E2721F3`.
- SHA-256 fingerprint (decoded 42-byte minisign public-key record):
  `d9fe424ca8b20725e11d6039c39445d38a33023a8d366c69e0612a1c3528de76`.

See the [public-key registry](../packaging/keys/README.md). Both public trust
roots are embedded in the client for signed release metadata checks
(`latest.json`); tag publication still fails closed on missing secrets or invalid
signatures. In-app installer updates use those same trust roots and the signed
manifest's tarball hash and size; other channels never self-update.

For a signed release, obtain and retain a trusted copy of those public keys
separately from the downloaded assets. With `minisign` installed, and the
package, `SHA256SUMS` and `SHA256SUMS.minisig` from the **same release** in your
directory:

```sh
# Stop if verification fails. For a recovery release, use release-backup.pub instead.
minisign -Vm SHA256SUMS -p /path/to/trusted/release.pub &&
  sha256sum --ignore-missing -c SHA256SUMS
```

Require both commands to succeed before installing. A checksum alone detects
corruption, not a replaced download. First obtaining the public keys still
relies on trusting the repository; retain them for later verification. The
signed manifest is at
`https://github.com/shuv1337/rex-rune/releases/latest/download/latest.json`
with its `.minisig` beside it. Verify its exact bytes before parsing it; it has
no expiry.

Release builds also produce GitHub provenance attestations for both package
formats:

```sh
gh attestation verify <downloaded-package> --repo shuv1337/rex-rune
```

Attestations identify the build workflow; they do not replace minisign
verification. Older releases do not gain signatures or attestations
retroactively.

## Desktop launcher from a checkout

```sh
bash scripts/install-desktop.sh [--copy] [path/to/rex-rune]
bash scripts/install-desktop.sh --uninstall
```

This installs `dev.shuv.rex-rune.desktop` under `$XDG_DATA_HOME/applications`
(normally `~/.local/share/applications`) pointing at the release binary of this
checkout (or the binary you pass), and the app icon at 32, 128 and 256 px into
the user's `hicolor` theme. Without `--copy`, re-run it if you move the
checkout. With `--copy`, the binary is copied to `$XDG_BIN_HOME` (normally
`~/.local/bin`) and the launcher points there, so it no longer depends on the
checkout. `--uninstall` removes the launcher, the icons and the copied binary.
It needs `desktop-file-install`, `desktop-file-validate` and
`update-desktop-database`; no root access. See
[icon provenance](../resources/icons/README.md).

With mise, `mise run install` builds the release binary and runs the script with
`--copy`, and `mise run uninstall` runs it with `--uninstall`.

## Updates and uninstalling

Settings → Updates checks the signed release manifest and offers channel-specific
guidance. **Update now** is available for verified, user-writable install.sh
installs and for pacman installs that ship the update helper. See
[self-updates](self-updates.md) for eligibility, rollback and pacman details.

Uninstalling does not remove preferences or change any Rex server:

- `install.sh --uninstall` removes the user install and keeps preferences.
- `sudo apt remove rex-rune` removes the `.deb`.
- `sudo pacman -R rex-rune-bin` removes the package.

## Releases, package builds and signatures

[docs/releases.md](releases.md) documents the release workflow, its eight
packages, attestations, signing keys, manual/offline signing and recovery.
The [install-script review](install-script-review.md) records the independent
supply-chain review and the findings applied to the installer.

Packaging includes the Nix flake, Linux tarballs, Debian/Ubuntu packages and the
pacman package. RPM, Flatpak and AUR packages are not provided.
