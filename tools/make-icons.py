#!/usr/bin/env python3
"""draws the lanlord desktop icons as shaded windows 95 / nt4 style pixel art.

writes crates/ui/src/icon_data.rs (32x32 desktop icons and hand-tuned 16x16 small ones, one
palette letter per pixel) and, with --preview FILE, a zoomed png contact sheet to look at.
only the standard library is used. run from the repository root: python3 tools/make-icons.py
"""
import argparse
import math
import os
import struct
import zlib

# palette letters, the sixteen windows colours (see icons.rs)
RGB = {
    "k": (0, 0, 0), "w": (255, 255, 255), "g": (192, 192, 192), "d": (128, 128, 128),
    "n": (0, 0, 128), "b": (0, 0, 255), "c": (0, 255, 255), "t": (0, 128, 128),
    "G": (0, 128, 0), "l": (0, 255, 0), "y": (255, 255, 0), "o": (128, 128, 0),
    "r": (255, 0, 0), "m": (128, 0, 0), "p": (128, 0, 128), "f": (255, 0, 255),
}


class Grid:
    def __init__(self, size):
        self.n = size
        self.p = [["."] * size for _ in range(size)]

    def dot(self, x, y, c):
        if 0 <= x < self.n and 0 <= y < self.n:
            self.p[y][x] = c

    def rect(self, x, y, w, h, c):
        for j in range(y, y + h):
            for i in range(x, x + w):
                self.dot(i, j, c)

    def frame(self, x, y, w, h, c):
        self.rect(x, y, w, 1, c)
        self.rect(x, y + h - 1, w, 1, c)
        self.rect(x, y, 1, h, c)
        self.rect(x + w - 1, y, 1, h, c)

    def line(self, x0, y0, x1, y1, c):
        dx, dy = abs(x1 - x0), -abs(y1 - y0)
        sx, sy = (1 if x0 < x1 else -1), (1 if y0 < y1 else -1)
        err = dx + dy
        while True:
            self.dot(x0, y0, c)
            if x0 == x1 and y0 == y1:
                break
            e2 = 2 * err
            if e2 >= dy:
                err += dy
                x0 += sx
            if e2 <= dx:
                err += dx
                y0 += sy

    def solid(self, x, y, w, h, face="g", hi="w", lo="d", edge="k"):
        """a raised box: black edge, light top and left, dark bottom and right."""
        self.rect(x, y, w, h, edge)
        self.rect(x + 1, y + 1, w - 2, h - 2, face)
        self.rect(x + 1, y + 1, w - 2, 1, hi)
        self.rect(x + 1, y + 1, 1, h - 2, hi)
        self.rect(x + 1, y + h - 2, w - 2, 1, lo)
        self.rect(x + w - 2, y + 1, 1, h - 2, lo)

    def sunken(self, x, y, w, h, face="k"):
        """a recessed panel: dark top and left, light bottom and right."""
        self.rect(x, y, w, h, face)
        self.rect(x - 1, y - 1, w + 2, 1, "d")
        self.rect(x - 1, y - 1, 1, h + 2, "d")
        self.rect(x - 1, y + h, w + 2, 1, "w")
        self.rect(x + w, y - 1, 1, h + 2, "w")

    def rows(self):
        return ["".join(r) for r in self.p]


# ---------------------------------------------------------------- 32 x 32 desktop icons


def unit(g, x, y, w, h):
    """one rack mounted unit: ears, a drive slot, vents and two status lights."""
    g.solid(x, y, w, h)
    g.rect(x + 3, y + 3, 2, 2, "k")
    g.dot(x + 3, y + 3, "l")
    g.dot(x + 4, y + 3, "l")
    g.rect(x + 3, y + 6, 2, 2, "k")
    g.dot(x + 3, y + 6, "y")
    g.dot(x + 4, y + 6, "y")
    for i in range(6):
        g.rect(x + 8 + i * 2, y + 3, 1, h - 6, "d")
        g.rect(x + 9 + i * 2, y + 3, 1, h - 6, "w")
    g.sunken(x + w - 11, y + 4, 8, 3)
    g.rect(x + w - 11, y + 4, 8, 1, "d")
    g.dot(x + w - 2 - 1, y + h - 4, "k")


def gateway():
    g = Grid(32)
    unit(g, 2, 4, 28, 11)
    unit(g, 2, 16, 28, 11)
    g.rect(4, 27, 4, 2, "k")
    g.rect(24, 27, 4, 2, "k")
    g.rect(5, 27, 2, 1, "d")
    g.rect(25, 27, 2, 1, "d")
    return g


def docker():
    g = Grid(32)
    fx, fy, fw, fh = 2, 11, 21, 18
    # top, then side, then the corrugated front
    for y in range(6, 11):
        x0, x1 = fx + (11 - y), fx + fw + (10 - y) + 1
        g.rect(x0, y, x1 - x0, 1, "g")
        g.dot(x0, y, "k")
        g.dot(x1 - 1, y, "k")
    g.rect(fx + 5, 5, fw, 1, "k")
    g.rect(fx + 6, 6, fw - 2, 1, "w")
    for x in range(fx + fw, fx + fw + 6):
        s = x - (fx + fw)
        g.rect(x, fy - 1 - s, 1, fh, "n")
        g.dot(x, fy - 1 - s, "k")
        g.dot(x, fy - 1 - s + fh - 1, "k")
    g.rect(fx + fw + 5, fy - 6, 1, fh, "k")
    g.rect(fx, fy, fw, fh, "t")
    for i in range(fw):
        col = "n" if i % 3 == 2 else "t"
        g.rect(fx + i, fy + 1, 1, fh - 2, col)
    g.frame(fx, fy, fw, fh, "k")
    g.rect(fx + 1, fy + 1, fw - 2, 1, "c")
    g.rect(fx + 1, fy + 1, 1, fh - 2, "c")
    g.rect(fx + 1, fy + fh - 2, fw - 2, 1, "n")
    # door rods and lock
    for x in (fx + 7, fx + 13):
        g.rect(x, fy + 2, 1, fh - 4, "g")
        g.rect(x + 1, fy + 2, 1, fh - 4, "k")
    g.rect(fx + 5, fy + 8, 11, 2, "d")
    g.rect(fx + 5, fy + 8, 11, 1, "w")
    return g


def tower():
    g = Grid(32)
    x, y, w, h = 8, 2, 16, 28
    g.solid(x, y, w, h)
    # cd drive
    g.solid(x + 2, y + 3, 12, 5, face="g")
    g.sunken(x + 4, y + 5, 8, 1)
    g.dot(x + 11, y + 6, "l")
    # two drive bays
    g.sunken(x + 2, y + 10, 12, 2)
    g.rect(x + 2, y + 13, 12, 1, "d")
    g.sunken(x + 4, y + 15, 8, 1)
    # vents
    for i in range(5):
        g.rect(x + 3, y + 18 + i * 1, 10, 1, "d" if i % 2 == 0 else "g")
    g.rect(x + 3, y + 23, 10, 1, "d")
    # power button and lights
    g.solid(x + 3, y + 24, 4, 3, face="g")
    g.rect(x + 9, y + 25, 2, 1, "l")
    g.dot(x + 12, y + 25, "y")
    g.rect(x + 2, y + h, 3, 1, "k")
    g.rect(x + w - 5, y + h, 3, 1, "k")
    return g


def globe():
    g = Grid(32)
    cx, cy, r = 15.5, 14.5, 13.0
    land = [(10.5, 9.5, 5.5, 3.5), (19.0, 17.5, 4.0, 5.0), (9.0, 19.5, 3.0, 3.0), (21.5, 7.5, 3.0, 2.0), (14.5, 14.0, 2.0, 1.5)]
    for y in range(32):
        for x in range(32):
            d = ((x - cx) ** 2 + (y - cy) ** 2) ** 0.5
            if d <= r:
                col = "b"
                lx, ly = x - cx, y - cy
                shade = (lx + ly) / r
                if shade > 0.55:
                    col = "n"
                elif shade < -0.7:
                    col = "c"
                for ex, ey, rx, ry in land:
                    if ((x - ex) / rx) ** 2 + ((y - ey) / ry) ** 2 <= 1:
                        col = "G" if shade > -0.2 else "l"
                        if shade > 0.55:
                            col = "G"
                g.dot(x, y, col)
    for y in range(32):
        for x in range(32):
            if g.p[y][x] != ".":
                d = ((x - cx) ** 2 + (y - cy) ** 2) ** 0.5
                if d > r - 1.1:
                    g.dot(x, y, "k")
    g.dot(8, 6, "w")
    g.dot(9, 6, "w")
    g.dot(7, 7, "w")
    # a stand
    g.rect(11, 28, 9, 1, "k")
    g.rect(10, 29, 11, 2, "g")
    g.rect(10, 29, 11, 1, "w")
    g.rect(10, 31 - 1, 11, 1, "d")
    g.frame(9, 29, 13, 3, "k")
    g.rect(14, 27, 3, 2, "d")
    return g


def window(g, x, y, w, h, title="n"):
    g.rect(x, y, w, h, "k")
    g.rect(x + 1, y + 1, w - 2, h - 2, "g")
    g.rect(x + 1, y + 1, w - 2, 1, "w")
    g.rect(x + 1, y + 1, 1, h - 2, "w")
    g.rect(x + 2, y + 2, w - 4, 4, title)
    g.rect(x + w - 7, y + 3, 2, 2, "g")
    g.rect(x + w - 4, y + 3, 2, 2, "g")
    g.rect(x + w - 4, y + h - 2, 3, 1, "d")


def remote():
    g = Grid(32)
    window(g, 2, 3, 28, 26)
    g.sunken(4, 9, 24, 18)
    # prompt: a greater-than sign and a cursor
    for i in range(4):
        g.dot(6 + i, 12 + i, "g")
        g.dot(6 + i, 20 - i, "g")
    g.rect(12, 20, 5, 1, "w")
    g.rect(6, 22, 3, 1, "d")
    g.rect(10, 22, 6, 1, "d")
    # a small arrow for the remote part
    g.rect(18, 24, 7, 1, "c")
    g.line(25, 24, 22, 22, "c")
    g.line(25, 24, 22, 26, "c")
    return g


def console():
    g = Grid(32)
    window(g, 2, 3, 28, 26)
    g.sunken(4, 9, 24, 18)
    for i, (n, c) in enumerate([(8, "g"), (13, "d"), (5, "g"), (11, "d"), (9, "g"), (14, "d")]):
        g.rect(6, 11 + i * 2, n, 1, c)
    g.rect(6, 23, 2, 2, "l")
    g.rect(9, 24, 4, 1, "w")
    return g


def dns():
    g = Grid(32)
    # a hub at the top
    g.solid(9, 2, 14, 8)
    g.rect(11, 5, 2, 2, "k")
    g.dot(11, 5, "l")
    g.rect(15, 5, 2, 2, "k")
    g.dot(15, 5, "l")
    g.rect(19, 5, 2, 2, "k")
    g.dot(19, 5, "y")
    # wires
    g.rect(15, 10, 2, 4, "k")
    g.rect(5, 13, 22, 1, "k")
    for x in (5, 15, 26):
        g.rect(x, 13, 1, 6, "k")
    # three small computers
    for x in (1, 11, 22):
        g.solid(x, 18, 9, 7)
        g.rect(x + 2, 20, 5, 3, "t")
        g.rect(x + 2, 20, 5, 1, "c")
        g.rect(x + 3, 25, 3, 1, "d")
        g.rect(x + 1, 26, 7, 2, "g")
        g.frame(x + 1, 26, 7, 2, "k")
    return g


def settings():
    g = Grid(32)
    # two tabs and a page
    g.rect(3, 4, 9, 4, "k")
    g.rect(4, 5, 7, 3, "g")
    g.rect(4, 5, 7, 1, "w")
    g.rect(12, 5, 8, 3, "k")
    g.rect(13, 6, 6, 2, "d")
    g.solid(3, 7, 26, 21)
    # checkboxes with labels
    for i, c in enumerate(["k", "k", " "]):
        y = 11 + i * 5
        g.rect(7, y, 5, 5, "k")
        g.rect(8, y + 1, 3, 3, "w")
        if c == "k":
            g.dot(8, y + 2, "k")
            g.dot(9, y + 3, "k")
            g.dot(10, y + 1, "k")
        g.rect(14, y + 2, 10 - i * 2, 1, "k" if i < 2 else "d")
    g.solid(8, 23, 8, 4)
    g.solid(18, 23, 8, 4)
    return g


def hosts():
    g = Grid(32)
    # the desk case
    g.solid(2, 19, 22, 9)
    g.sunken(5, 22, 9, 1)
    g.dot(20, 22, "l")
    g.rect(16, 24, 4, 1, "d")
    # the monitor on top
    g.solid(5, 2, 16, 14)
    g.sunken(7, 4, 12, 9, face="t")
    g.rect(7, 4, 12, 1, "c")
    g.rect(7, 4, 1, 9, "c")
    g.rect(10, 16, 6, 3, "d")
    g.rect(10, 16, 6, 1, "k")
    # a small network card in front, board and bracket
    g.rect(17, 22, 12, 7, "k")
    g.rect(18, 23, 10, 5, "G")
    g.rect(18, 23, 10, 1, "l")
    g.rect(20, 25, 3, 2, "k")
    for i in range(4):
        g.dot(24 + i, 27, "y")
    g.rect(28, 21, 3, 9, "k")
    g.rect(29, 22, 1, 7, "g")
    g.rect(29, 24, 1, 3, "k")
    return g


def folder():
    g = Grid(32)
    g.rect(3, 6, 12, 5, "k")
    g.rect(4, 7, 10, 4, "y")
    g.rect(4, 7, 10, 1, "w")
    g.rect(3, 10, 26, 18, "k")
    g.rect(4, 11, 24, 16, "y")
    g.rect(4, 11, 24, 1, "w")
    g.rect(4, 11, 1, 16, "w")
    g.rect(4, 26, 24, 1, "o")
    g.rect(27, 11, 1, 16, "o")
    return g


# ---------------------------------------------------------------- 16 x 16 small icons


def s_gateway():
    g = Grid(16)
    for y in (2, 8):
        g.solid(1, y, 14, 6)
        g.dot(3, y + 2, "l")
        g.rect(6, y + 2, 1, 2, "d")
        g.rect(8, y + 2, 1, 2, "d")
        g.rect(10, y + 2, 3, 1, "k")
    g.rect(2, 14, 3, 1, "k")
    g.rect(11, 14, 3, 1, "k")
    return g


def s_docker():
    g = Grid(16)
    g.rect(1, 5, 11, 9, "k")
    g.rect(2, 6, 9, 7, "t")
    for x in (3, 6, 9):
        g.rect(x, 6, 1, 7, "n")
    g.rect(2, 6, 9, 1, "c")
    for y in range(2, 5):
        g.rect(1 + (5 - y), y, 11, 1, "g")
        g.dot(1 + (5 - y), y, "k")
        g.dot(11 + (5 - y), y, "k")
    g.rect(5, 1, 11 - 0, 1, "k")
    for x in range(12, 16):
        g.rect(x, 5 - (x - 11), 1, 9, "n")
        g.dot(x, 5 - (x - 11), "k")
    g.rect(15, 1, 1, 9, "k")
    g.rect(1, 13, 11, 1, "k")
    return g


def s_tower():
    g = Grid(16)
    g.solid(4, 1, 8, 14)
    g.sunken(6, 3, 4, 2)
    g.rect(6, 7, 4, 1, "k")
    g.rect(6, 9, 4, 1, "d")
    g.rect(6, 12, 2, 1, "l")
    g.rect(5, 15, 2, 1, "k")
    g.rect(9, 15, 2, 1, "k")
    return g


def s_globe():
    g = Grid(16)
    cx, cy, r = 7.5, 7.0, 6.5
    for y in range(16):
        for x in range(16):
            d = ((x - cx) ** 2 + (y - cy) ** 2) ** 0.5
            if d <= r:
                col = "b" if (x + y) / 2 < 9 else "n"
                if ((x - 5) / 2.6) ** 2 + ((y - 5) / 2.2) ** 2 <= 1 or ((x - 10) / 1.8) ** 2 + ((y - 9) / 2.6) ** 2 <= 1:
                    col = "G"
                g.dot(x, y, col)
    for y in range(16):
        for x in range(16):
            if g.p[y][x] != ".":
                d = ((x - cx) ** 2 + (y - cy) ** 2) ** 0.5
                if d > r - 0.9:
                    g.dot(x, y, "k")
    g.dot(4, 3, "w")
    g.rect(5, 14, 6, 1, "d")
    g.rect(4, 15, 8, 1, "k")
    return g


def s_window(lines):
    g = Grid(16)
    g.rect(1, 2, 14, 12, "k")
    g.rect(2, 3, 12, 10, "g")
    g.rect(2, 3, 12, 1, "w")
    g.rect(3, 4, 10, 2, "n")
    g.rect(3, 7, 10, 5, "k")
    if lines:
        g.rect(4, 8, 4, 1, "g")
        g.rect(4, 10, 6, 1, "d")
        g.dot(11, 10, "l")
    else:
        g.dot(4, 8, "g")
        g.dot(5, 9, "g")
        g.dot(4, 10, "g")
        g.rect(7, 10, 3, 1, "w")
    return g


def s_dns():
    g = Grid(16)
    g.solid(5, 1, 6, 4)
    g.dot(6, 3, "l")
    g.dot(9, 3, "y")
    g.rect(7, 5, 2, 2, "k")
    g.rect(2, 7, 12, 1, "k")
    for x in (2, 7, 13):
        g.rect(x, 7, 1, 3, "k")
    for x in (0, 5, 11):
        g.rect(x, 10, 5, 4, "k")
        g.rect(x + 1, 11, 3, 2, "t")
        g.rect(x, 14, 5, 1, "d")
    return g


def s_settings():
    g = Grid(16)
    g.rect(2, 1, 5, 2, "k")
    g.rect(3, 2, 3, 1, "g")
    g.solid(1, 3, 14, 12)
    for i in range(3):
        y = 6 + i * 3
        g.rect(3, y, 3, 3, "k")
        g.rect(4, y + 1, 1, 1, "w")
        g.rect(7, y + 1, 5 - i, 1, "k")
    return g


def s_hosts():
    g = Grid(16)
    g.solid(1, 10, 13, 5)
    g.dot(11, 12, "l")
    g.solid(3, 1, 9, 8)
    g.rect(4, 2, 7, 5, "t")
    g.rect(4, 2, 7, 1, "c")
    g.rect(12, 11, 4, 4, "k")
    g.rect(13, 12, 2, 2, "G")
    return g


def s_folder():
    g = Grid(16)
    g.rect(1, 3, 6, 3, "k")
    g.rect(2, 4, 4, 2, "y")
    g.rect(1, 5, 14, 10, "k")
    g.rect(2, 6, 12, 8, "y")
    g.rect(2, 6, 12, 1, "w")
    g.rect(2, 6, 1, 8, "w")
    g.rect(2, 13, 12, 1, "o")
    g.rect(13, 6, 1, 8, "o")
    return g


def s_lanlord():
    """the menu button tile: a navy square with a white L."""
    g = Grid(16)
    g.rect(1, 1, 14, 14, "k")
    g.rect(2, 2, 12, 12, "n")
    g.rect(2, 2, 12, 1, "b")
    g.rect(2, 2, 1, 12, "b")
    g.rect(5, 4, 2, 8, "w")
    g.rect(5, 10, 6, 2, "w")
    return g


# ---------------------------------------------------------------- the update notice icon
#
# the tray icon beside the clock. several candidates are drawn below; the one in the tray is
# whichever UPDATE_ICON names, so switching is a one-line change. run with --update-sheet FILE
# to see them all side by side.

UPDATE_ICON = "round"


def disc(g, cx, cy, r, face, hi=None, lo=None, edge="k"):
    """a round shape with a 1px outline, light from the top left."""
    for y in range(g.n):
        for x in range(g.n):
            d = math.hypot(x - cx, y - cy)
            if d > r:
                continue
            if d > r - 1.0:
                g.dot(x, y, edge)
                continue
            c = face
            t = (x - cx) + (y - cy)
            if hi and t < -r * 0.75 and d > r * 0.45:
                c = hi
            if lo and t > r * 0.75 and d > r * 0.45:
                c = lo
            g.dot(x, y, c)


def outlined(g, pixels, fill, edge="k"):
    """fill the set of (x, y) pixels and draw a 1px outline around it."""
    for (x, y) in pixels:
        for dx in (-1, 0, 1):
            for dy in (-1, 0, 1):
                if (x + dx, y + dy) not in pixels:
                    g.dot(x + dx, y + dy, edge)
    for (x, y) in pixels:
        g.dot(x, y, fill)


def arrow_pixels(x0, y0, shaft_h, down=True):
    """a six wide arrow: a two wide shaft and a three row head."""
    shaft = {(x0 + 2 + i, y0 + j) for i in range(2) for j in range(shaft_h)}
    head = set()
    for k, (lo, hi) in enumerate([(0, 5), (1, 4), (2, 3)]):
        for x in range(x0 + lo, x0 + hi + 1):
            head.add((x, y0 + shaft_h + k))
    pixels = shaft | head
    if not down:
        height = shaft_h + 3
        pixels = {(x, y0 + (y0 + height - 1 - y)) for (x, y) in pixels}
    return pixels


def u_balloon():
    """a yellow circle with a black exclamation mark, like a balloon warning."""
    g = Grid(16)
    disc(g, 7.5, 7.5, 7, "y", hi="w", lo="o")
    g.rect(7, 4, 2, 5, "k")
    g.rect(7, 10, 2, 2, "k")
    return g


def u_info():
    """a blue circle with a white i."""
    g = Grid(16)
    disc(g, 7.5, 7.5, 7, "b", hi="c", lo="n")
    g.rect(7, 4, 2, 2, "w")
    g.rect(6, 7, 3, 1, "w")
    g.rect(7, 8, 2, 3, "w")
    g.rect(6, 11, 4, 1, "w")
    return g


def u_globe():
    """a small globe with a yellow down arrow, like the old windows update."""
    g = Grid(16)
    disc(g, 6.5, 6.5, 6, "b", hi="c", lo="n")
    for (x, y) in [(4, 3), (5, 3), (3, 4), (4, 4), (5, 4), (4, 5), (4, 6), (7, 8), (8, 8), (6, 9),
                   (7, 9), (8, 9), (7, 10), (9, 5), (10, 5), (9, 6), (10, 6), (10, 7)]:
        g.dot(x, y, "G")
    outlined(g, arrow_pixels(9, 8, 3), "y")
    return g


def u_monitor():
    """a small monitor with a white down arrow on its screen."""
    g = Grid(16)
    g.solid(1, 1, 14, 11)
    g.rect(3, 3, 10, 7, "n")
    g.rect(3, 3, 10, 1, "d")
    g.rect(3, 3, 1, 7, "d")
    for (x, y) in arrow_pixels(5, 4, 1):
        g.dot(x, y, "w")
    g.rect(6, 12, 4, 1, "d")
    g.solid(4, 13, 8, 2)
    return g


def u_cog():
    """a grey cog with eight teeth and a yellow spark."""
    g = Grid(16)
    cx, cy = 6.5, 8.5
    mask = {(x, y) for y in range(16) for x in range(16) if math.hypot(x - cx, y - cy) <= 4.1}
    teeth = [(6, 3, 2, 2), (6, 13, 2, 2), (1, 8, 2, 2), (11, 8, 2, 2),
             (2, 4, 2, 2), (10, 4, 2, 2), (2, 12, 2, 2), (10, 12, 2, 2)]
    for (x, y, w, h) in teeth:
        mask |= {(x + i, y + j) for i in range(w) for j in range(h)}
    outlined(g, mask, "g")
    for (x, y) in mask:
        t = (x - cx) + (y - cy)
        if t < -3.0:
            g.dot(x, y, "w")
        elif t > 3.0:
            g.dot(x, y, "d")
    disc(g, cx, cy, 1.9, "t")
    spark = {(13, 0), (13, 1), (13, 2), (13, 3), (13, 4), (11, 2), (12, 2), (14, 2), (15, 2)}
    outlined(g, spark, "y")
    g.dot(13, 2, "w")
    return g


def u_mail():
    """an envelope with a red notification dot."""
    g = Grid(16)
    g.rect(1, 4, 13, 10, "k")
    g.rect(2, 5, 11, 8, "w")
    g.rect(2, 12, 11, 1, "d")
    g.rect(12, 5, 1, 8, "d")
    for i in range(5):
        g.dot(2 + i, 5 + i, "d")
        g.dot(12 - i, 5 + i, "d")
    g.dot(7, 10, "d")
    disc(g, 12.5, 3.5, 3.2, "r", lo="m")
    g.dot(11, 2, "w")
    return g


def u_package():
    """a blue square with a white up arrow above a box."""
    g = Grid(16)
    g.rect(1, 1, 14, 14, "k")
    g.rect(2, 2, 12, 12, "b")
    g.rect(2, 2, 12, 1, "c")
    g.rect(2, 2, 1, 12, "c")
    g.rect(2, 13, 12, 1, "n")
    g.rect(13, 2, 1, 12, "n")
    for (x, y) in arrow_pixels(5, 3, 3, down=False):
        g.dot(x, y, "w")
    g.rect(4, 10, 1, 2, "w")
    g.rect(11, 10, 1, 2, "w")
    g.rect(4, 12, 8, 1, "w")
    return g


def u_window():
    """a window in four of our own colours: blue, teal, yellow and white panes."""
    g = Grid(16)
    g.solid(1, 1, 14, 14)
    g.rect(3, 3, 10, 2, "n")
    for (x, y, c) in [(3, 6, "b"), (9, 6, "t"), (3, 10, "y"), (9, 10, "w")]:
        g.rect(x, y, 4, 3, c)
        g.rect(x, y + 2, 4, 1, {"b": "n", "t": "G", "y": "o", "w": "d"}[c])
    return g


def u_round():
    """a round windows blue button with a white down arrow, lit from the top left."""
    rows = [
        "................",
        "....kkkkkkkk....",
        "..kkbbbbbbbbkk..",
        ".kbbcbbbbbbbbnk.",
        ".kbcbbbwwbbbbnk.",
        ".kbcbbbwwbbbbnk.",
        ".kbbbbbwwbbbbnk.",
        ".kbbbwwwwwwbbnk.",
        ".kbbbbwwwwbbbnk.",
        ".kbbbbbwwbbbbnk.",
        ".knbbbbbbbbbnnk.",
        "..knnbbbbbbnnk..",
        "..kknnnnnnnnkk..",
        "....kkkkkkkk....",
        "................",
        "................",
    ]
    g = Grid(16)
    for y, row in enumerate(rows):
        for x, c in enumerate(row):
            g.dot(x, y, c)
    return g


UPDATE_CANDIDATES = {
    "balloon": ("BALLOON", u_balloon),
    "info": ("INFO", u_info),
    "globe": ("GLOBE", u_globe),
    "monitor": ("MONITOR", u_monitor),
    "cog": ("COG", u_cog),
    "mail": ("MAIL", u_mail),
    "package": ("PACKAGE", u_package),
    "window": ("WINDOW", u_window),
    "round": ("ROUND", u_round),
}


def s_update():
    """the update notice in the tray: the candidate UPDATE_ICON names."""
    return UPDATE_CANDIDATES[UPDATE_ICON][1]()


# a 3x5 font, just enough for the labels on the contact sheet
FONT = {
    "A": [".X.", "X.X", "XXX", "X.X", "X.X"], "B": ["XX.", "X.X", "XX.", "X.X", "XX."],
    "C": [".XX", "X..", "X..", "X..", ".XX"], "D": ["XX.", "X.X", "X.X", "X.X", "XX."],
    "E": ["XXX", "X..", "XX.", "X..", "XXX"], "F": ["XXX", "X..", "XX.", "X..", "X.."],
    "G": [".XX", "X..", "X.X", "X.X", ".XX"], "I": ["XXX", ".X.", ".X.", ".X.", "XXX"],
    "K": ["X.X", "X.X", "XX.", "X.X", "X.X"], "L": ["X..", "X..", "X..", "X..", "XXX"],
    "M": ["X.X", "XXX", "XXX", "X.X", "X.X"], "N": ["XX.", "X.X", "X.X", "X.X", "X.X"],
    "O": [".X.", "X.X", "X.X", "X.X", ".X."], "P": ["XX.", "X.X", "XX.", "X..", "X.."],
    "R": ["XX.", "X.X", "XX.", "X.X", "X.X"], "T": ["XXX", ".X.", ".X.", ".X.", ".X."],
    "U": ["X.X", "X.X", "X.X", "X.X", "XXX"], "W": ["X.X", "X.X", "XXX", "XXX", "X.X"],
    "1": [".X.", "XX.", ".X.", ".X.", "XXX"], "2": ["XX.", "..X", ".X.", "X..", "XXX"],
    "3": ["XX.", "..X", ".X.", "..X", "XX."], "4": ["X.X", "X.X", "XXX", "..X", "..X"],
    "5": ["XXX", "X..", "XX.", "..X", "XX."], "6": [".XX", "X..", "XX.", "X.X", ".X."],
    "7": ["XXX", "..X", ".X.", ".X.", ".X."], "8": [".X.", "X.X", ".X.", "X.X", ".X."],
    "9": [".X.", "X.X", ".XX", "..X", "XX."], "X": ["X.X", "X.X", ".X.", "X.X", "X.X"],
    " ": ["...", "...", "...", "...", "..."],
}


def update_sheet(path):
    """eight candidates for the tray icon: each at 8x, then a 1x and a 2x copy on the taskbar gray."""
    gray = RGB["g"]
    zoom, pad, cell_w, cell_h = 8, 14, 214, 190
    names = [k for k in UPDATE_CANDIDATES if k != "round"]
    cols, rows = 4, 2
    w, h = cols * cell_w + pad, rows * cell_h + pad
    img = [[gray] * w for _ in range(h)]

    def put(x, y, c):
        if 0 <= x < w and 0 <= y < h:
            img[y][x] = c

    def block(x, y, art, z):
        for j, row in enumerate(art):
            for i, c in enumerate(row):
                if c in RGB:
                    for dy in range(z):
                        for dx in range(z):
                            put(x + i * z + dx, y + j * z + dy, RGB[c])

    def text(x, y, label, z=3):
        for ch in label:
            for j, row in enumerate(FONT.get(ch, FONT[" "])):
                for i, bit in enumerate(row):
                    if bit == "X":
                        for dy in range(z):
                            for dx in range(z):
                                put(x + i * z + dx, y + j * z + dy, (0, 0, 0))
            x += 4 * z

    for n, key in enumerate(names):
        label, fn = UPDATE_CANDIDATES[key]
        art = fn().rows()
        ox = pad + (n % cols) * cell_w
        oy = pad + (n // cols) * cell_h
        # the 8x zoom, on a sunken frame
        for i in range(16 * zoom + 4):
            put(ox - 2 + i, oy - 2, RGB["d"])
            put(ox - 2, oy - 2 + i, RGB["d"])
            put(ox - 2 + i, oy + 16 * zoom + 1, RGB["w"])
            put(ox + 16 * zoom + 1, oy - 2 + i, RGB["w"])
        block(ox, oy, art, zoom)
        # a 1x and a 2x copy beside it, where they would sit in the tray
        block(ox + 16 * zoom + 14, oy + 6, art, 1)
        block(ox + 16 * zoom + 14, oy + 40, art, 2)
        text(ox + 16 * zoom + 12, oy + 28, "1X", 2)
        text(ox + 16 * zoom + 12, oy + 76, "2X", 2)
        text(ox, oy + 16 * zoom + 12, f"{n + 1} {label}", 3)
    with open(path, "wb") as f:
        f.write(png(w, h, img))


BIG = {
    "GATEWAY": gateway, "DOCKER": docker, "NT4": tower, "WEB": globe, "COMMANDS": remote,
    "CONSOLE": console, "DNS": dns, "SETTINGS": settings, "HOSTS": hosts, "FOLDER": folder,
}
SMALL = {
    "GATEWAY": s_gateway, "DOCKER": s_docker, "NT4": s_tower, "WEB": s_globe,
    "COMMANDS": lambda: s_window(False), "CONSOLE": lambda: s_window(True), "DNS": s_dns,
    "SETTINGS": s_settings, "HOSTS": s_hosts, "FOLDER": s_folder, "LANLORD": s_lanlord,
    "UPDATE": s_update,
}


def emit(path):
    out = ["// generated by tools/make-icons.py; edit the script, not this file.\n",
           "// one palette letter per pixel (see icons.rs); `.` is transparent.\n\n"]
    for name, fn in BIG.items():
        rows = fn().rows()
        out.append(f"pub(crate) const {name}: [&str; 32] = [\n")
        out += [f'    "{r}",\n' for r in rows]
        out.append("];\n\n")
    for name, fn in SMALL.items():
        rows = fn().rows()
        out.append(f"pub(crate) const SMALL_{name}: [&str; 16] = [\n")
        out += [f'    "{r}",\n' for r in rows]
        out.append("];\n\n")
    with open(path, "w") as f:
        f.write("".join(out).rstrip() + "\n")


def png(width, height, pixels):
    raw = b"".join(b"\x00" + bytes(v for px in row for v in px) for row in pixels)

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def preview(path, zoom):
    teal = (0, 128, 128)
    cells = [(n, fn().rows()) for n, fn in BIG.items()] + [(n, fn().rows()) for n, fn in SMALL.items()]
    per_row, pad = 6, 8
    cw = 32 * zoom + pad * 2
    rows_n = (len(cells) + per_row - 1) // per_row
    w, h = per_row * cw, rows_n * cw
    img = [[teal] * w for _ in range(h)]
    for i, (_, art) in enumerate(cells):
        ox, oy = (i % per_row) * cw + pad, (i // per_row) * cw + pad
        z = zoom if len(art) == 32 else zoom * 2
        for y, row in enumerate(art):
            for x, c in enumerate(row):
                if c in RGB:
                    for dy in range(z):
                        for dx in range(z):
                            img[oy + y * z + dy][ox + x * z + dx] = RGB[c]
    with open(path, "wb") as f:
        f.write(png(w, h, img))


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--preview", help="write a zoomed contact sheet png here")
    ap.add_argument("--update-sheet", help="write a contact sheet of the update icon candidates here")
    ap.add_argument("--zoom", type=int, default=6)
    ap.add_argument("--no-write", action="store_true")
    a = ap.parse_args()
    if not a.no_write:
        emit(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "crates", "ui", "src", "icon_data.rs"))
    if a.preview:
        preview(a.preview, a.zoom)
    if a.update_sheet:
        update_sheet(a.update_sheet)
