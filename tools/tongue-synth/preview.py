"""Tiles frames of a tongue recording into one PNG, to compare synthetic
recordings with real ones by eye. Standard library only.

    python tools/tongue-synth/preview.py <recording dir> <out.png> [--frames 8]

Picks frames spread evenly through the recording and prints each one's pose
and labels (visibility, extension, horizontal, vertical, cheeks).
"""

import argparse
import json
import struct
import zlib
from pathlib import Path

WIDTH, HEIGHT = 800, 400


def write_png(path, width, height, rows):
    def chunk(kind, data):
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF))

    raw = b"".join(b"\x00" + row for row in rows)
    Path(path).write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 6))
        + chunk(b"IEND", b""))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("recording", type=Path)
    parser.add_argument("out", type=Path)
    parser.add_argument("--frames", type=int, default=8)
    args = parser.parse_args()

    samples = [json.loads(line) for line in
               (args.recording / "samples.jsonl").read_text().splitlines() if line.strip()]
    count = min(args.frames, len(samples))
    picks = [round(i * (len(samples) - 1) / max(1, count - 1)) for i in range(count)]
    rows = []
    with open(args.recording / "frames.gray8", "rb") as frames:
        for n, index in enumerate(picks):
            frames.seek(index * WIDTH * HEIGHT)
            strip = frames.read(WIDTH * HEIGHT)
            rows += [strip[r * WIDTH:(r + 1) * WIDTH] for r in range(HEIGHT)]
            t = samples[index]["targets"]
            cheeks = t[10:12] if len(t) >= 12 else []
            print(f"{n}: #{index} {samples[index]['pose']!r} "
                  f"vis={t[0]:.2f} ext={t[1]:.2f} h={t[2]:+.2f} v={t[3]:+.2f} cheeks={cheeks}")
    write_png(args.out, WIDTH, HEIGHT * count, rows)


if __name__ == "__main__":
    main()
