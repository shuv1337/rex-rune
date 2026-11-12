# Update now for pacman installs: design note

Status: **proposal, nothing built.** Written 2026-11-12 (PST) against `20f64b4`.

## The ask

Settings → Updates offers **Update now** for install.sh installs under `~/.local`
([how that works](../self-updates.md)). For a pacman install it shows instructions
instead: `omarchy update` on Omarchy, `sudo pacman -Syu rex-linux-bin` elsewhere, with
**Copy command**. The owner wants pacman installs to update without pasting a sudo
command, and both install methods used daily (shuvdev runs the install.sh build,
shuvmbp the pacman package from `pkg.shuv.dev`).

This note compares three ways to get there and recommends one. It builds nothing.

## What exists today

- **The package.** `rex-linux-bin`, built from `packaging/arch/PKGBUILD.in`, installs
  `/usr/bin/rex-linux`, the desktop entry, icons, licenses and
  `/usr/share/rex-linux/channel` (`channel=pacman`). It has no install scriptlet.
- **The repository.** `https://pkg.shuv.dev/arch/$arch`, a signed pacman repository.
  The database names the newest version; the newest five releases' packages and their
  detached OpenPGP signatures stay in the directory, so an exact version can be
  installed by URL. A machine trusts it by importing and locally signing the Rex
  archive key and adding a `[rex-linux]` section with `SigLevel = Required
  DatabaseRequired` (the [operator walkthrough](../auto-update-operator-walkthrough.md)
  has the steps).
- **The signed manifest.** Every release publishes `latest.json` with a Minisign
  signature. The client embeds the two release public keys and already verifies the
  manifest to decide that an update exists. For each architecture the manifest carries
  a `pacman` entry beside `tarball` and `deb`: the package's file name, exact size and
  SHA-256. The client's `TargetArtifacts` type does not read that entry yet. Checked
  against the live 0.7.5 manifest:
  `rex-linux-bin-0.7.5-1-x86_64.pkg.tar.zst`, 22,135,553 bytes, with its hash.
- **The client's channel detection.** `InstallChannel::Pacman { omarchy, custom_hint }`
  in `src/update.rs`, from the channel file and `/etc/os-release`.
- **Two signing roots.** Minisign (a CI key and an offline backup key) signs the
  manifest, the checksums and install.sh. OpenPGP (an offline primary and a CI
  subkey) signs the packages and the repository database. The CI halves of both sit
  in the same protected `release` environment, behind one approval per release.

Checked on shuvdev (Omarchy, pacman 7.1, polkit 128), read-only:

- `pkexec` is installed; `polkitd` runs; `polkit-gnome` and `polkit-kde-agent` are
  installed. Which agent an Omarchy desktop session actually starts was **not**
  confirmed (no graphical session was inspected).
- `pacman.conf` leaves `RemoteFileSigLevel` unset, and pacman's manual says it then
  uses `SigLevel`, here `Required` with `TrustedOnly`. So `pacman -U <url>` fetches
  the package and its `.sig` and refuses an unsigned or untrusted package.
- `omarchy update` authorises with sudo once and runs a full `pacman -Syu`, then AUR
  packages through its own script. It updates Rex along with everything else.

## Why this needs root at all

install.sh installs live in files the user owns, so the client can replace them itself
after verifying the signed manifest. A pacman install lives in `/usr`, owned by root and
recorded in pacman's database. Changing it means running pacman as root. The whole
question is how the unprivileged client is allowed to cause that, and what else that
permission could be used for.

## Options

### (a) A polkit action and a small root helper in the package

The package adds two files:

- `/usr/lib/rex-linux/rex-linux-update`: a small program owned by root, **not** setuid.
- `/usr/share/polkit-1/actions/dev.shuv.rex-linux.update.policy`: a polkit action
  bound to that exact path (`org.freedesktop.policykit.exec.path`).

Update now runs `pkexec /usr/lib/rex-linux/rex-linux-update <version>`. polkit decides
whether to allow it. With the action's default (`auth_admin`), the desktop's polkit
agent shows its own password dialog, the same one the person sees for other system
changes; there is no command to paste and no terminal. On Arch an administrator is a
member of `wheel`, so the person types their own password. The default is
`auth_admin` and not `auth_admin_keep`: "keep" would let the same process run the
helper again for a few minutes without asking, and one update needs one run. The
helper then runs one fixed pacman command and exits. The client shows progress and, on success, the same
**Restart to finish** the install.sh path has.

What the helper does, in full:

1. Takes one argument, a version, and accepts only `digits.digits.digits`.
2. Works out the architecture itself (`uname -m`), never from the caller.
3. Reads the installed version from pacman and refuses unless the requested version
   is strictly newer (`vercmp`). No downgrade, no reinstall.
4. Refuses unless pacman's effective signature level for remote files is `Required`.
5. Asks pacman what installing
   `https://pkg.shuv.dev/arch/<arch>/rex-linux-bin-<version>-1-<arch>.pkg.tar.zst`
   would change, and refuses unless the only package is `rex-linux-bin`. This keeps a
   one-package update from pulling in dependencies from stale databases.
6. Runs `pacman -U --noconfirm` on that URL with absolute paths (`pkexec` already
   replaces the caller's environment with a minimal one). pacman downloads the
   package and its signature into its own root-owned cache and verifies it against
   the system keyring before touching anything.

It takes no file path, no URL, no option and no environment from the caller, and it
does not sync databases, so it cannot leave the system partly upgraded. The URL
assumes the package release number is 1, as every package so far has been; a rebuilt
package (`-2`) would need the helper to take the release number too, validated the
same way.

A variant makes it promptless: a polkit *rule* that answers yes for this one action
when the caller is in an active local session and in `wheel`. The package can ship
that rule as an example file that an administrator copies into
`/etc/polkit-1/rules.d/` once, or install it enabled. See "The owner's calls".

### (b) An AUR `rex-linux-bin` package, for yay

Publish the same `PKGBUILD` to the AUR. People who use yay would install and update
Rex with it, and on Omarchy `omarchy update` already updates AUR packages.

It does not meet the ask. yay calls sudo, in a terminal, so the client still cannot
update itself: the best it could do is open a terminal running yay. The signed
repository already gets the same result from `omarchy update` and `pacman -Syu`.
What the AUR adds is discovery and a first install with no key import, not an Update
now button.

It has costs of its own. A `PKGBUILD` on the AUR is unsigned, so its integrity is the
AUR account's; publishing needs that account and an SSH key in CI, a new surface
outside GitHub. The AUR officially targets x86_64. And the earlier roadmap review
answered this question with **no AUR** (`docs/plans/roadmap-0.5.2.md`, section 8).

### (c) Keep the instructions

Nothing new runs as root. On Omarchy, `omarchy update` is one command the owner runs
anyway and it updates Rex with the rest of the system. The cost is exactly the
complaint: a paste, a terminal, and on Omarchy a full system update when only Rex was
wanted.

## Security model for (a)

The client runs as the user and talks to servers, parses terminal output and loads
files, so assume it can be compromised. The question is what an attacker who controls
the client, as that user, gains from the new files.

**With the default action (password dialog).** Nothing without the person's password.
The attacker can make the dialog appear, at a moment that looks legitimate, and a
person who types their password authorises exactly one run of the helper. That run
can only install a newer, OpenPGP-signed `rex-linux-bin` from `pkg.shuv.dev`, which is
what the person would have installed anyway. The risk that remains is the general one
of teaching people to type a password into a dialog an application raised; the action's
message should name Rex and the version so the dialog says what it is for.

**With the promptless rule.** The attacker can run the helper whenever they like, and
the helper's checks are the whole boundary. What they can cause:

- An upgrade of `rex-linux-bin` to any newer version that the site serves and the key
  signed. Normally that is only the latest, which is the intended effect. A release
  that was withdrawn for a fault but is newer than the installed one could be chosen
  while the site still serves it; rebuilding the site with `repo.yml`'s `exclude` input removes it.
- Nothing older: step 3 refuses.
- Nothing unsigned or signed by another key: pacman refuses, and step 4 refuses to run
  at all on a machine whose configuration would not check.
- No other package and no arbitrary file: steps 1, 2 and 5, and the fixed URL.
- Nuisance: holding pacman's database lock for the length of a download, or filling
  the package cache with one package file. A lock on the helper and a refusal while
  one is running limit this.

What the helper deliberately does not defend against: someone who holds the OpenPGP
package key, or who controls `pkg.shuv.dev` **and** that key. They can already
publish a malicious package that `pacman -Syu` would install. The helper neither
widens nor narrows that.

**Things that would make it unsafe, and are therefore excluded:**

- The client downloading the package and passing a path. The file would sit in a
  directory the user can write, and could be swapped between the check and the
  install. The helper passes pacman a URL instead.
- Passing a URL, repository name, pacman option or configuration file through.
- Running `pacman -Sy rex-linux-bin`. Refreshing the databases and installing one
  package is a partial upgrade, which Arch does not support.
- A setuid helper, or a sudoers drop-in. polkit gives the same result with a narrower
  grant, a dialog the desktop owns, and a rule an administrator can read and remove.
- Fetching and parsing the manifest inside the helper. That would put an HTTP client
  and a JSON and signature parser in a root process to re-check what pacman's own
  signature verification already gates.

## What the Minisign-verified manifest gives

- **The client's side, unprivileged.** That an update exists, its version, and the
  package's exact name, size and hash for this architecture, all signed by keys built
  into the client. The client can show these before asking for authorisation, pass the
  verified version to the helper, and refuse to start if the manifest does not list a
  pacman package for this machine. Afterwards it can confirm the installed version is
  the one it asked for.
- **Not the privileged step.** The install is gated by pacman's OpenPGP check against
  the system keyring. The manifest's hash is not what stops a bad package.
- **How independent the two are.** The CI Minisign key and the CI OpenPGP subkey live
  in the same release environment, so an attacker who has one probably has the other;
  only the offline Minisign backup key is elsewhere. Checking the manifest's hash in
  the helper would therefore add little, at the cost above. The manifest's value here
  is in the client: it stops a compromised mirror or a stale database from deciding
  *which* version is offered, because the client only ever asks for the version its
  own verified manifest names.

## Omarchy fit

- Omarchy is Arch with pacman and yay, and its update command is a full `pacman -Syu`
  behind sudo. A one-package `pacman -U` of a third-party package whose dependencies
  are already satisfied does not interfere with that, and step 5 refuses when they
  are not.
- After Update now, `omarchy update` finds `rex-linux-bin` already current. Nothing
  conflicts: both go through pacman's database.
- A polkit agent must be running for the password dialog. Omarchy installs agent
  packages; whether the session starts one must be confirmed on shuvmbp before
  building. Without an agent, `pkexec` fails when there is no terminal. The client
  must detect that and fall back to today's instructions rather than appear to hang.
- The first release that carries the helper has to be installed the old way. After
  that, every pacman install has it.

## Recommendation

**Build (a) with the default password dialog, keep (c) as the fallback, and do not do
(b) for this.**

- (a) is the only option that removes the pasted command. With the default action its
  worst case under a compromised client is a dialog the person must still approve,
  for an install they would have made anyway.
- (c)'s text stays for every case (a) cannot serve: no polkit agent, a package from
  before the helper, a machine whose pacman would not verify the package, a failed
  authorisation, a refusal from the helper.
- (b) answers a different question (how people first find and install Rex on Arch) and
  reopens a decision that was made on purpose.

Outline of the build, if approved, as small PRs in this order:

1. The helper and the policy file in the package, with the helper tested as root in
   the existing Arch container (`scripts/test-arch-repo.py`): each refusal above, a
   real signed upgrade, a tampered package, an older version, an unknown version.
   `pkaction` confirms the action is registered. No client change yet.
2. The client: read the manifest's `pacman` entry; offer Update now on a pacman install
   only when the helper and `pkexec` exist; run it; show progress, failure reasons and
   Restart to finish; fall back to the instructions otherwise. Headless tests with a
   fake helper; nothing in the test suite runs `pkexec`.
3. Acceptance on shuvmbp (pacman) and shuvdev (install.sh), both by hand, since both
   need a real desktop session.

## The owner's calls

These are not engineering judgments:

1. **Promptless or not.** Whether the package should also make this work with no
   password dialog (a polkit rule answering yes for `wheel` in an active local
   session), and if so whether that rule is installed enabled or shipped as a file an
   administrator copies into place. It is a standing grant of a narrow root action to
   anything running as the user. The recommendation is the password dialog by default
   and the rule as an opt-in file.
2. **The AUR.** Publishing `rex-linux-bin` there puts a public name under an account
   of the owner's and adds a publishing key to CI. The recommendation is to leave the
   earlier "no AUR" decision in place; this feature does not need it.
3. **Who the helper trusts for the package.** The proposal uses the machine's own
   pacman keyring, which the administrator set up when adding the repository. The
   alternative, a second check in the helper against keys built into it, is argued
   against above but is a custody question if the owner sees it differently.

## Not verified

- Which polkit agent an Omarchy session runs, and what its dialog looks like for a
  custom action.
- That `pacman -U <url>` against `pkg.shuv.dev` fetches the `.sig` beside the package
  on a real machine (the site publishes one per package; not exercised here).
- The exact pacman invocation for step 5's dry run, and how `pkexec` reports a
  cancelled dialog versus a missing agent, so the client can tell them apart.
- Arch Linux ARM, and Arch without Omarchy.
