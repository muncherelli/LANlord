#!/usr/bin/env python3
"""draws the lanlord icon and logo as pixel art and writes png, ico and icns files to assets/.

only the standard library is used. run from the repository root: python3 tools/make-assets.py
"""
import os
import shutil
import struct
import subprocess
import tempfile
import zlib

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets")

# palette shared with the desktop theme
TEAL = (0, 128, 128)
TEAL_DARK = (0, 70, 70)
WHITE = (255, 255, 255)
GRAY = (192, 192, 192)
SHADOW = (128, 128, 128)
BLACK = (0, 0, 0)
NAVY = (0, 0, 128)
GOLD = (255, 204, 0)
GOLD_DARK = (170, 110, 0)
RED = (220, 30, 30)
BLUE = (40, 90, 255)
GREEN = (60, 255, 90)
CLEAR = None


class Canvas:
    def __init__(self, w, h):
        self.w, self.h = w, h
        self.px = [[CLEAR] * w for _ in range(h)]

    def rect(self, x, y, w, h, color):
        for j in range(y, y + h):
            for i in range(x, x + w):
                if 0 <= i < self.w and 0 <= j < self.h:
                    self.px[j][i] = color

    def dot(self, x, y, color):
        self.rect(x, y, 1, 1, color)

    def bevel(self, x, y, w, h, raised=True):
        light, dark = (WHITE, SHADOW) if raised else (SHADOW, WHITE)
        self.rect(x, y, w, 1, light)
        self.rect(x, y, 1, h, light)
        self.rect(x, y + h - 1, w, 1, dark)
        self.rect(x + w - 1, y, 1, h, dark)

    def scaled(self, factor):
        out = Canvas(self.w * factor, self.h * factor)
        for y in range(out.h):
            for x in range(out.w):
                out.px[y][x] = self.px[y // factor][x // factor]
        return out

    def resized(self, size):
        """nearest neighbour to any square size, so 16 and 48 px stay crisp."""
        out = Canvas(size, size)
        for y in range(size):
            for x in range(size):
                out.px[y][x] = self.px[y * self.h // size][x * self.w // size]
        return out

    def blit(self, other, x, y):
        for j in range(other.h):
            for i in range(other.w):
                c = other.px[j][i]
                if c is not CLEAR:
                    self.dot(x + i, y + j, c)

    def png(self):
        raw = b""
        for row in self.px:
            raw += b"\x00" + b"".join(bytes(c + (255,)) if c else b"\x00\x00\x00\x00" for c in row)

        def chunk(tag, data):
            body = tag + data
            return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

        return (
            b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", self.w, self.h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b"")
        )


def monitor_with_crown(tile):
    """a 32x32 pixel-art icon: a crown on a crt monitor; with a teal tile behind it when asked."""
    c = Canvas(32, 32)
    if tile:
        c.rect(0, 0, 32, 32, TEAL)
        c.bevel(0, 0, 32, 32, raised=True)
        c.rect(1, 30, 30, 1, TEAL_DARK)
    # monitor body
    c.rect(6, 13, 20, 14, GRAY)
    c.bevel(6, 13, 20, 14, raised=True)
    c.rect(8, 15, 16, 10, BLACK)
    c.bevel(8, 15, 16, 10, raised=False)
    # a green prompt on the screen
    c.rect(10, 17, 1, 5, GREEN)
    c.rect(10, 21, 4, 1, GREEN)
    c.rect(16, 21, 3, 1, GREEN)
    # stand
    c.rect(13, 27, 6, 1, SHADOW)
    c.rect(10, 28, 12, 2, GRAY)
    c.rect(10, 28, 12, 1, WHITE)
    c.rect(10, 29, 12, 1, SHADOW)
    # crown: band, three points, gems
    c.rect(9, 9, 14, 4, GOLD)
    c.rect(9, 12, 14, 1, GOLD_DARK)
    for x in (9, 15, 21):
        c.rect(x, 5, 2, 4, GOLD)
        c.rect(x, 4, 2, 1, GOLD_DARK)
    c.rect(11, 7, 4, 2, GOLD)
    c.rect(17, 7, 4, 2, GOLD)
    c.dot(15, 10, RED)
    c.dot(16, 10, RED)
    c.dot(11, 10, BLUE)
    c.dot(20, 10, BLUE)
    c.dot(10, 4, WHITE)
    c.dot(16, 3, WHITE)
    c.dot(22, 4, WHITE)
    return c


# a 5x7 pixel font for the wordmark
FONT = {
    "L": ["X....", "X....", "X....", "X....", "X....", "X....", "XXXXX"],
    "A": [".XXX.", "X...X", "X...X", "XXXXX", "X...X", "X...X", "X...X"],
    "N": ["X...X", "XX..X", "X.X.X", "X..XX", "X...X", "X...X", "X...X"],
    "l": ["XX...", "XX...", "XX...", "XX...", "XX...", "XX...", "XX..."],
    "o": [".....", ".....", ".XXX.", "X...X", "X...X", "X...X", ".XXX."],
    "r": [".....", ".....", "X.XX.", "XX..X", "X....", "X....", "X...."],
    "d": ["....X", "....X", ".XXXX", "X...X", "X...X", "X...X", ".XXXX"],
}


def wordmark():
    """LANlord in pixel letters: LAN in navy, lord in gold, with a shadow."""
    text = "LANlord"
    gap = 1
    w = sum(5 for _ in text) + gap * (len(text) - 1) + 1
    c = Canvas(w, 9)
    x = 0
    for ch in text:
        glyph = FONT[ch]
        navy = ch in "LAN"
        for j, row in enumerate(glyph):
            for i, bit in enumerate(row):
                if bit == "X":
                    c.dot(x + i + 1, j + 2, BLACK)
                    c.dot(x + i, j + 1, NAVY if navy else GOLD)
        x += 5 + gap
    return c


def logo():
    """the icon beside the wordmark on a transparent background."""
    icon = monitor_with_crown(tile=False)
    mark = wordmark()
    c = Canvas(32 + 3 + mark.w, 32)
    c.blit(icon, 0, 0)
    c.blit(mark, 35, 12)
    return c


def ico(pngs):
    """an ico file with png-compressed images (windows vista and later)."""
    head = struct.pack("<HHH", 0, 1, len(pngs))
    entries, data, offset = b"", b"", 6 + 16 * len(pngs)
    for size, png in pngs:
        d = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", d, d, 0, 0, 1, 32, len(png), offset)
        data += png
        offset += len(png)
    return head + entries + data


def write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data)


def main():
    art = monitor_with_crown(tile=True)
    flat = monitor_with_crown(tile=False)
    write(os.path.join(OUT, "icon-32.png"), art.png())
    write(os.path.join(OUT, "icon-512.png"), art.scaled(16).png())
    write(os.path.join(OUT, "icon-transparent-512.png"), flat.scaled(16).png())
    mark = logo()
    write(os.path.join(OUT, "logo.png"), mark.scaled(8).png())
    pngs = [(n, art.resized(n).png()) for n in (16, 32, 48, 256)]
    write(os.path.join(OUT, "icon.ico"), ico(pngs))
    # a macos icon set needs iconutil, which only exists on macos
    if shutil.which("iconutil"):
        with tempfile.TemporaryDirectory() as tmp:
            iconset = os.path.join(tmp, "icon.iconset")
            os.makedirs(iconset)
            for name, size in [
                ("icon_16x16", 16), ("icon_16x16@2x", 32), ("icon_32x32", 32), ("icon_32x32@2x", 64),
                ("icon_128x128", 128), ("icon_128x128@2x", 256), ("icon_256x256", 256),
                ("icon_256x256@2x", 512), ("icon_512x512", 512), ("icon_512x512@2x", 1024),
            ]:
                write(os.path.join(iconset, name + ".png"), art.resized(size).png())
            subprocess.run(["iconutil", "-c", "icns", iconset, "-o", os.path.join(OUT, "icon.icns")], check=True)
    print("wrote", sorted(os.listdir(OUT)))


if __name__ == "__main__":
    main()
