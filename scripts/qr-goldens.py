#!/usr/bin/env python3
"""Regenerate committed byte-mode M matrices with independent qrencode 4.1.1.
Run from the repository root. ASCIIi uses spaces for dark modules, ## for light.
Fixtures contain input hex on the first line, then 1=dark, 0=light rows.
"""
import pathlib
import subprocess

root = pathlib.Path("tests/data/qr")
root.mkdir(parents=True, exist_ok=True)
inputs = {"one": b"a", "url": b"https://name-rex.tail123456.ts.net/", "utf8": "https://example.net/été/日本".encode()}
for version, capacity in enumerate([14, 26, 42, 62, 84, 106, 122, 152, 180, 213], 1):
    for extra in [0, 1] if version < 10 else [0]:
        n = capacity + extra
        inputs[f"v{version:02}-{'over' if extra else 'full'}"] = bytes(33 + i % 90 for i in range(n))
for name, data in inputs.items():
    result = subprocess.run(["/usr/bin/qrencode", "-l", "M", "-8", "-m", "0", "-t", "ASCIIi"], input=data, stdout=subprocess.PIPE, check=True).stdout.decode()
    rows = result.splitlines()
    assert len(rows) == len(rows[0]) // 2
    matrix = ["".join("1" if row[i:i+2] == "  " else "0" for i in range(0, len(row), 2)) for row in rows]
    (root / f"{name}.qr").write_text(data.hex() + "\n" + "\n".join(matrix) + "\n")
print(f"Generated {len(inputs)} matrices with qrencode")
