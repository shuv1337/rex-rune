#!/usr/bin/env python3
"""Wrap the release binary in a macOS app bundle for a source build.

macOS only posts desktop notifications for a process with a main bundle identifier, a
code signature and LaunchServices registration; a bare `target/release/rex-rune` cannot
offer them. This makes `target/release/Rex Rune.app` from `target/release/rex-rune`:
Contents/MacOS/rex-rune, an Info.plist with the identifier the desktop entry uses
(dev.shuv.rex-rune), the Rex icon as an .icns made with iconutil, and an ad-hoc
signature (`codesign --force --sign -`). It then registers the bundle with
LaunchServices.

This is a local convenience for source builds, not a release: nothing is notarized,
signed with a Developer ID or published, and the ad-hoc signature changes with every
build. Start it with `open "target/release/Rex Rune.app" --args ...`.

Usage: scripts/macos-app.py [--quiet]
       scripts/macos-app.py --unregister   (unregister the bundle and remove it)
REX_BIN names the binary to wrap (default target/release/rex-rune).
"""
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get("REX_BIN", ROOT / "target/release/rex-rune"))
APP = ROOT / "target/release/Rex Rune.app"
ICONS = ROOT / "resources/icons"
ICONSET = [
    # (name in the iconset, pixels across, source pixels available). The largest source
    # is 256 px, so the iconset stops there; iconutil takes any subset of sizes.
    ("icon_16x16.png", 16, 32),
    ("icon_16x16@2x.png", 32, 32),
    ("icon_32x32.png", 32, 32),
    ("icon_32x32@2x.png", 64, 128),
    ("icon_128x128.png", 128, 128),
    ("icon_128x128@2x.png", 256, 256),
    ("icon_256x256.png", 256, 256),
]
# UNUserNotificationCenter needs macOS 10.14, and the banner presentation the client asks
# for while it is frontmost needs 11.0.
NOTIFICATIONS_MACOS = "11.0"
LSREGISTER = ("/System/Library/Frameworks/CoreServices.framework/Frameworks/"
              "LaunchServices.framework/Support/lsregister")


def run(*args, **kwargs):
    return subprocess.run(args, check=True, capture_output=True, text=True, **kwargs)


def version():
    with (ROOT / "Cargo.toml").open("rb") as file:
        return tomllib.load(file)["package"]["version"]


def release(text):
    return tuple(int(part) for part in text.split("."))


def minimum_macos():
    """The later of the binary's own minimum and what notifications need, so the bundle
    claims neither an older system than the binary loads on (Apple Silicon builds say
    11.0) nor one without the notification calls (an Intel build says 10.12)."""
    try:
        lines = run("otool", "-l", str(BINARY)).stdout.splitlines()
    except (OSError, subprocess.CalledProcessError):
        sys.exit(f"could not read the minimum macOS of {BINARY} with otool")
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("minos "):
            return max(stripped.split()[1], NOTIFICATIONS_MACOS, key=release)
    sys.exit(f"{BINARY} declares no minimum macOS (no minos in otool -l)")


def unregister():
    if APP.exists():
        run(LSREGISTER, "-u", str(APP))
        shutil.rmtree(APP)
        print(f"unregistered and removed {APP}")
    else:
        print(f"no bundle at {APP}")


def make_icon(destination):
    """An .icns from the PNGs the desktop entry uses, only ever scaled down."""
    iconset = Path(tempfile.mkdtemp(prefix="rex-rune-")) / "Rex Rune.iconset"
    iconset.mkdir()
    try:
        for name, pixels, source in ICONSET:
            png = ICONS / f"{source}x{source}/apps/dev.shuv.rex-rune.png"
            if not png.exists():
                sys.exit(f"missing icon source {png}")
            target = iconset / name
            assert pixels <= source, f"{name} would upscale a {source} px icon"
            if pixels == source:
                shutil.copy(png, target)
            else:
                run("sips", "-z", str(pixels), str(pixels), str(png), "--out", str(target))
        run("iconutil", "-c", "icns", str(iconset), "-o", str(destination))
    finally:
        shutil.rmtree(iconset.parent, ignore_errors=True)


def main():
    quiet = "--quiet" in sys.argv[1:]
    if sys.platform != "darwin":
        sys.exit("macos-app.py makes a macOS app bundle; run it on a Mac")
    if "--unregister" in sys.argv[1:]:
        unregister()
        return
    if not BINARY.is_file():
        sys.exit(f"no release binary at {BINARY}; run `cargo build --locked --release --bin rex-rune`")

    contents = APP / "Contents"
    if APP.exists():
        shutil.rmtree(APP)
    (contents / "MacOS").mkdir(parents=True)
    (contents / "Resources").mkdir()

    shutil.copy2(BINARY, contents / "MacOS/rex-rune")
    make_icon(contents / "Resources/rex-rune.icns")

    name = version()
    with (contents / "Info.plist").open("wb") as file:
        plistlib.dump({
            "CFBundleIdentifier": "dev.shuv.rex-rune",
            "CFBundleName": "Rex Rune",
            "CFBundleDisplayName": "Rex Rune",
            "CFBundleExecutable": "rex-rune",
            "CFBundleIconFile": "rex-rune.icns",
            "CFBundlePackageType": "APPL",
            "CFBundleInfoDictionaryVersion": "6.0",
            "CFBundleShortVersionString": name,
            "CFBundleVersion": name,
            "NSHighResolutionCapable": True,
            "LSMinimumSystemVersion": minimum_macos(),
        }, file)

    # Ad hoc only: enough for notifications on this Mac, no identity for a release.
    run("codesign", "--force", "--sign", "-", str(APP))
    # macOS refuses notification requests from a bundle LaunchServices does not know.
    run(LSREGISTER, "-f", str(APP))

    if not quiet:
        print(f"built {APP}")
        print("signed ad hoc, not notarized: this is a local source-build bundle, not a release")
        print(f"start it with: open \"{APP}\" --args --new work")


if __name__ == "__main__":
    main()
