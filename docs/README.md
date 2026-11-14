# Docs

What is in this folder, one line per document or subfolder.

## Installing and updating

- [nix.md](nix.md): the Linux Nix flake, its package, development shell and checks for `x86_64-linux` and `aarch64-linux`.
- [self-updates.md](self-updates.md): how Settings → Updates installs an update, for install.sh/tarball installs and, separately, pacman installs.

## Publishing releases

- [releases.md](releases.md): how `.github/workflows/release.yml` builds, checks and signs a binary release, and the release sequence to follow.
- [auto-update-operator-walkthrough.md](auto-update-operator-walkthrough.md): the order of work for whoever runs Rex Rune releases and update channels, with `releases.md` as the reference for signing.
- [install-script-review.md](install-script-review.md): the independent supply-chain review of the installer from 2026-11-10 (PST) and the findings applied from it.

## Server compatibility

- [server-preview14.md](server-preview14.md): compatibility with Rex server preview.14, verified on 2026-11-09 (PST) for 0.4.1, which advertises API revisions 10–12.
- [server-api13.md](server-api13.md): compatibility with Rex API 13, verified on 2026-11-10 (PST), from which the client advertises API revisions 10–13.

## Verification records

- [0.4.2-verification.md](0.4.2-verification.md): how the 0.4.2 changes, listener discovery (#13) and opt-in SSH setup (#11), were verified.
- [0.5.2-verification.md](0.5.2-verification.md): verification of the `v0.5.2` release candidate on 2026-11-10 (PST), with the changes and reviews behind it.
- [readme-captures.md](readme-captures.md): where the README images came from (native X11 captures with real fonts of 0.7.4) and what each one shows.

## Plans and design

- [plans/](plans/): plans and design notes for particular pieces of work (the roadmap from 0.5.2, Remote control, Update now for pacman installs, the cross-platform study and the M2.7 toolbar visibility slice), each stating its own status.
- [design/](design/): the proposed, unimplemented design for consent and the host boundary in the cross-server relay (`client_call_requested`, issue #20).

## Parity analysis

- [parity-2026-11/](parity-2026-11/): the November 2026 comparison of Rex Linux 0.6.0 with Rex Beta 0.1.0 for macOS, starting from its gap report, with the implementation records that followed it.

## Process

- [fork-harvest-2026-11.md](fork-harvest-2026-11.md): the November 2026 survey of every fork and branch of the repository and the verdict on each change found there.

Images: screenshots live in `docs/images/`, `docs/screenshots/` and `docs/parity-2026-11/images/`, and `screenshot.png`, `ssh-setup.png` and `ssh-setup-error.png` sit in this folder; `docs/images/readme-captures.json` records the README captures' source, sizes and hashes.
