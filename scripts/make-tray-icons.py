#!/usr/bin/env python3
"""Draws the three tray template icons from the Sidevoice mark (stdlib only, no Pillow).

The mark (brand-resources/logo/mark): five bars on a 24x24 box, heights 6:12:21:12:6, width 3,
gap 1.5, fully rounded; the fourth bar at 45 % of the ink. Template images are black + alpha.

  idle.png   no call: the whole mark at 45 %
  live.png   in a call, microphone on: the mark as drawn
  muted.png  in a call, muted: the mark at 45 % crossed by a solid slash

Output: 44x44 (22 pt @2x), written to src-tauri/icons/tray/.
    python3 scripts/make-tray-icons.py
"""
import math
import os
import struct
import zlib

SIZE = 44
SS = 4  # supersampling per axis
BARS = [(1.5, 9, 6), (6, 6, 12), (10.5, 1.5, 21), (15, 6, 12), (19.5, 9, 6)]  # x, y, height (24-box)
OUT = os.path.join(os.path.dirname(__file__), "..", "src-tauri", "icons", "tray")


def in_bar(x, y, bx, by, h, w=3.0):
    r = w / 2
    if x < bx or x > bx + w:
        return False
    cx = bx + r
    if y < by + r:
        return (x - cx) ** 2 + (y - (by + r)) ** 2 <= r * r
    if y > by + h - r:
        return (x - cx) ** 2 + (y - (by + h - r)) ** 2 <= r * r
    return True


def slash_distance(x, y):
    # A line from top-left to bottom-right, in 24-box units.
    ax, ay, bx, by = 3.0, 3.0, 21.0, 21.0
    dx, dy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(x - (ax + t * dx), y - (ay + t * dy))


def coverage(px, py, variant):
    total = 0.0
    for sy in range(SS):
        for sx in range(SS):
            # pixel sample -> 24-box coordinates (the mark fills 20 of 22 pt, centred)
            x = ((px + (sx + 0.5) / SS) / SIZE) * 26.4 - 1.2
            y = ((py + (sy + 0.5) / SS) / SIZE) * 26.4 - 1.2
            a = 0.0
            for i, (bx, by, h) in enumerate(BARS):
                if in_bar(x, y, bx, by, h):
                    a = 0.45 if i == 3 else 1.0
                    break
            if variant == "idle" and a:
                a = 0.45
            if variant == "muted":
                d = slash_distance(x, y)
                if d <= 1.4:
                    a = 1.0
                elif d <= 2.9:
                    a = 0.0  # a clear gap around the slash so it reads at 16 px
                elif a:
                    a = 0.45
            total += a
    return total / (SS * SS)


def png(pixels):
    raw = b"".join(b"\x00" + bytes(row) for row in pixels)
    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def main():
    os.makedirs(OUT, exist_ok=True)
    for variant in ("idle", "live", "muted"):
        rows = []
        for py in range(SIZE):
            row = []
            for px in range(SIZE):
                row += [0, 0, 0, round(255 * coverage(px, py, variant))]
            rows.append(row)
        with open(os.path.join(OUT, f"{variant}.png"), "wb") as f:
            f.write(png(rows))


if __name__ == "__main__":
    main()
