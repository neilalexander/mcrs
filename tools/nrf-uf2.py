#!/usr/bin/env python3
"""Wrap an application-only nRF52840 binary in UF2 at its linked application address."""
import argparse
import pathlib
import struct

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("input", type=pathlib.Path)
parser.add_argument("output", type=pathlib.Path)
parser.add_argument("--base", required=True, type=lambda value: int(value, 0))
args = parser.parse_args()
data = args.input.read_bytes()
base = args.base
if base not in (0x26000, 0x27000):
    parser.error("expected S140 application base 0x26000 or 0x27000")
if not data or len(data) > 0xED000 - base:
    parser.error("application is empty or overlaps the LittleFS region")
count = (len(data) + 255) // 256
with args.output.open("wb") as output:
    for index in range(count):
        payload = data[index * 256 : (index + 1) * 256].ljust(256, b"\xff")
        header = struct.pack(
            "<8I", 0x0A324655, 0x9E5D5157, 0x2000,
            base + index * 256, 256, index, count, 0xADA52840,
        )
        output.write(header + payload + bytes(220) + struct.pack("<I", 0x0AB16F30))
