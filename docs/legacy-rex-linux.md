# Renamed from Rex Linux

This client was **Rex Linux** (`rex-linux`) through 0.7.x. It is now **Rex Rune**
(`rex-rune`), after the toolkit it is built on and the repository it lives in. The
binary, packages, launcher, icons and settings folder all took the new name. An
existing install moves over when it updates:

| Installed with | Update with | Result |
| --- | --- | --- |
| `install.sh` or the tarball | **Update now** in Settings → Updates, or `install.sh` again | `~/.local/bin/rex-rune`, with the launcher and icons renamed. After **Update now** the rename happens at the next start |
| pacman (`rex-linux-bin`) | `sudo pacman -Syu`, or `omarchy update` on Omarchy | pacman asks to replace `rex-linux-bin` with `rex-rune-bin`. The `[rex-linux]` entry in `pacman.conf` stays as it is |
| `.deb` | `sudo apt install ./rex-rune_*.deb` | apt removes `rex-linux` and installs `rex-rune` in one step. Rex Linux 0.7.x names the file `rex-linux_*.deb` in its update notice; there is no such file, so take `rex-rune_*.deb` |
| Nix | Update the flake input | The package and binary are `rex-rune`; the `rex-linux` output name still resolves |
| Source | `git pull`, then build and `mise run install` | `rex-rune` replaces the old launcher and copied binary |

- **One launcher.** Each route removes the old `dev.shuv.rex-linux` launcher and
  icons when it installs the new ones.
- **Settings and state move once**, at the first start: `~/.config/rex-linux`
  becomes `~/.config/rex-rune`, and `~/.local/state/rex-linux` becomes
  `~/.local/state/rex-rune` (under `XDG_CONFIG_HOME` and `XDG_STATE_HOME` when
  set). Nothing is merged: if the new folder already exists, the old one is left
  alone.
- **`rex-linux` still starts the client.** Packages install it as a link to
  `rex-rune`, and an upgraded `install.sh` install keeps one in `~/.local/bin`. A
  later release will remove the link, so change scripts, aliases and key bindings
  to `rex-rune`.
- `REX_LINUX_NO_UPDATE_CHECK` still works; the variable is now
  `REX_RUNE_NO_UPDATE_CHECK`.
- **Three names stay**, because installed clients and package managers look for
  them: the pacman repository `[rex-linux]` with its certificate
  `rex-linux-archive-key.asc`, the `rex-linux` product id inside signed release
  metadata, and the `rex-linux-*.tar.gz` release archives. Those archives exist
  only for the updater inside 0.7.x; install from `rex-rune-*.tar.gz`.
- **The app id is `dev.shuv.rex-rune`.** Window rules, dock favourites and pinned
  launchers keyed on `dev.shuv.rex-linux` need the new id.
- `install.sh` installs releases from the rename onwards. For an older one, use
  that release's own `install.sh` with `--version`, as it tells you. A copy of the
  0.7.x `install.sh` kept from before does not install a renamed release: it stops
  and names the current installer.
- **Going back to 0.7.x** is by hand. Uninstall first (`install.sh
  --uninstall`, `apt remove rex-rune` or `pacman -R rex-rune-bin`), so no second
  launcher is left, then install the old release with its own package or
  installer, pinned to that version:

  ```sh
  curl --proto '=https' --tlsv1.2 -fsSL https://github.com/shuv1337/rex-rune/releases/download/v0.7.5/install.sh | sh -s -- --version 0.7.5
  ```

  It starts with default settings unless you copy `~/.config/rex-rune` back to
  `~/.config/rex-linux`. On pacman the next `-Syu` offers the replacement again;
  add `IgnorePkg = rex-rune-bin` to stay.

The release workflow keeps a bridge for 0.7.x installs: the product id, bridge
tarballs, `rex-linux` command and `[rex-linux]` package repository. Each has a
dependency and a retirement rule in [Publishing a binary
release](releases.md#the-rex-linux-bridge). What the 0.7.x updater does after it
installs a renamed release is in [Updating from Rex Linux
0.7.x](self-updates.md#updating-from-rex-linux-07x).
