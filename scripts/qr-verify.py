#!/usr/bin/env python3
"""Independent zbar decoding of our encoder and optionally real dialog captures.
First: cargo build --locked --no-default-features --example qr_dump
Then: python3 scripts/qr-verify.py [--capture path.png ...] [--address URL]
No server, display, HOME or clipboard access. Requires magick and zbarimg.
"""
import argparse
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
p = argparse.ArgumentParser(description=__doc__)
p.add_argument("--binary", type=Path, default=ROOT / "target/debug/examples/qr_dump")
p.add_argument("--out", type=Path, default=Path.home() / ".cache/agent-ws/rex-m33-pair/decodes")
p.add_argument("--capture", type=Path, action="append", default=[])
p.add_argument("--address", default="https://build.tail1234.ts.net/")
a = p.parse_args()
a.out.mkdir(parents=True, exist_ok=True)


def decode(path, expected):
    result = subprocess.run(["zbarimg", "--quiet", "--raw", "--set", "*.binary=1", str(path)],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True)
    assert result.stdout == expected + b"\n", (path, result.stdout, expected)
    print(f"PASS {path}: {result.stdout!r}")


fixtures = sorted((ROOT / "tests/data/qr").glob("*.qr"))
for fixture in fixtures:
    data = bytes.fromhex(fixture.read_text().splitlines()[0])
    input_path = a.out / (fixture.stem + ".bin")
    input_path.write_bytes(data)
    pbm = a.out / (fixture.stem + ".pbm")
    with pbm.open("wb") as stream:
        subprocess.run([str(a.binary), str(input_path)], stdout=stream, check=True)
    png = pbm.with_suffix(".png")
    subprocess.run(["magick", str(pbm), "-filter", "point", "-resize", "800%", str(png)], check=True)
    decode(png, data)
for capture in a.capture:
    decode(capture, a.address.encode())
print(f"Decoded exactly: {len(fixtures)} encoder symbols, {len(a.capture)} full-window captures")
