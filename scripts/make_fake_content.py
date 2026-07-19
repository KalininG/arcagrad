#!/usr/bin/env python3
"""Generate sample CBZ files so Arcagrad is testable without real content.

Pure stdlib (no Pillow): hand-writes solid-color PNGs at a manga-ish resolution
so libvips actually has something to downscale.

    python3 scripts/make_fake_content.py
"""
import os
import struct
import zlib
import zipfile


def png(width: int, height: int, rgb: tuple[int, int, int]) -> bytes:
    def chunk(typ: bytes, data: bytes) -> bytes:
        body = typ + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    sig = b"\x89PNG\r\n\x1a\n"
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)  # 8-bit RGB
    row = b"\x00" + bytes(rgb) * width  # filter byte + pixels
    raw = row * height
    idat = zlib.compress(raw, 6)
    return sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")


COLORS = [
    (220, 60, 60),
    (60, 160, 220),
    (80, 200, 120),
    (240, 200, 60),
    (160, 100, 220),
]

# How many sample archives to emit. Each must have DISTINCT content.
SAMPLE_COUNT = 5

here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
out_dir = os.path.join(here, "content")
os.makedirs(out_dir, exist_ok=True)

for n in range(1, SAMPLE_COUNT + 1):
    path = os.path.join(out_dir, f"sample-{n:02d}.cbz")
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for i, color in enumerate(COLORS, start=1):
            # Rotate the palette by n so each archive's pages differ.
            r, g, b = COLORS[(i - 1 + n) % len(COLORS)]
            shade = (r, g, (b + 17 * n) % 256)
            z.writestr(f"{i:03d}.png", png(1200, 1700, shade))
    print(f"wrote {path}")

print(f"done — {SAMPLE_COUNT} distinct sample archives")
