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


def my_server():
    """a beige server tower: a lit display, a floppy slot, a key lock, a power light, vents."""
    g = Grid(32)
    x, y, w, h = 6, 3, 19, 26
    g.solid(x, y, w, h)
    # the side of the case, for depth
    g.rect(x + w, y + 2, 2, h - 2, "d")
    g.rect(x + w, y + 2, 2, 1, "k")
    g.rect(x + w + 2, y + 2, 1, h - 1, "k")
    # the status display
    g.sunken(x + 3, y + 3, 13, 6, face="k")
    g.rect(x + 5, y + 5, 9, 1, "c")
    g.rect(x + 5, y + 7, 6, 1, "c")
    # the floppy slot
    g.solid(x + 3, y + 11, 13, 4, face="g")
    g.rect(x + 5, y + 13, 9, 1, "k")
    g.dot(x + 14, y + 12, "l")
    # the key lock, the power button and its light
    g.rect(x + 3, y + 17, 3, 3, "k")
    g.dot(x + 4, y + 18, "y")
    g.solid(x + 8, y + 17, 5, 3, face="g")
    g.rect(x + 14, y + 17, 3, 3, "k")
    g.rect(x + 15, y + 18, 1, 1, "l")
    # vents
    for i in range(4):
        g.rect(x + 3, y + 21 + i, 13, 1, "d" if i % 2 == 0 else "w")
    g.rect(x + 2, y + h, 4, 1, "k")
    g.rect(x + w - 6, y + h, 4, 1, "k")
    return g


def small_pc(g, x, y):
    """a small computer, 14 wide and 15 tall: monitor, stand and base."""
    g.solid(x, y, 14, 10)
    g.sunken(x + 2, y + 2, 10, 6, face="t")
    g.rect(x + 2, y + 2, 10, 1, "c")
    g.rect(x + 5, y + 10, 4, 2, "d")
    g.solid(x + 2, y + 12, 10, 3)
    g.dot(x + 9, y + 13, "l")


def networks():
    """two computers joined by a network cable, like the network neighbourhood."""
    g = Grid(32)
    g.line(9, 17, 9, 26, "k")
    g.line(9, 26, 16, 26, "k")
    g.line(10, 17, 10, 25, "d")
    g.line(10, 25, 16, 25, "d")
    small_pc(g, 1, 2)
    small_pc(g, 17, 15)
    g.dot(9, 26, "l")
    g.dot(16, 26, "l")
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


def s_my_server():
    g = Grid(16)
    g.solid(3, 1, 10, 14)
    g.sunken(5, 3, 6, 3)
    g.rect(6, 4, 4, 1, "c")
    g.rect(5, 8, 6, 1, "k")
    g.rect(6, 10, 2, 2, "k")
    g.dot(6, 10, "l")
    g.rect(9, 11, 3, 1, "d")
    g.rect(4, 15, 2, 1, "k")
    g.rect(10, 15, 2, 1, "k")
    return g


def s_networks():
    g = Grid(16)
    g.line(3, 8, 3, 13, "k")
    g.line(3, 13, 8, 13, "k")
    g.solid(0, 0, 8, 7)
    g.rect(2, 2, 4, 3, "t")
    g.rect(2, 2, 4, 1, "c")
    g.solid(8, 7, 8, 7)
    g.rect(10, 9, 4, 3, "t")
    g.rect(10, 9, 4, 1, "c")
    g.rect(9, 14, 6, 2, "d")
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


# ---------------------------------------------------------------- the logo
#
# a black top hat with a gold band on a short red box, drawn bold so it reads at 1x on the
# taskbar grey and on a white browser tab. the 16 pixel one is the start button, taskbar and
# title bar art; the 32 pixel one is used for the web favicon.


def s_logo():
    g = Grid(16)
    g.rect(4, 0, 8, 1, "k")
    g.rect(3, 1, 10, 9, "k")
    g.rect(4, 1, 1, 5, "d")
    g.rect(3, 6, 10, 3, "y")
    g.rect(3, 8, 10, 1, "o")
    g.rect(1, 10, 14, 1, "k")
    g.rect(0, 11, 16, 1, "k")
    g.rect(1, 12, 14, 4, "k")
    g.rect(2, 12, 12, 1, "r")
    g.rect(2, 13, 12, 2, "m")
    return g


def logo32():
    g = Grid(32)
    g.rect(8, 0, 16, 1, "k")
    g.rect(7, 1, 18, 21, "k")
    g.rect(9, 2, 1, 11, "d")
    g.rect(7, 14, 18, 6, "y")
    g.rect(7, 19, 18, 1, "o")
    g.rect(7, 14, 18, 1, "w")
    g.rect(3, 22, 26, 1, "k")
    g.rect(1, 23, 30, 2, "k")
    g.rect(2, 25, 28, 7, "k")
    g.rect(3, 25, 26, 1, "r")
    g.rect(3, 26, 26, 5, "m")
    g.rect(23, 27, 4, 3, "k")
    g.rect(24, 28, 2, 1, "l")
    return g


# ---------------------------------------------------------------- the update notice icon
#
# the tray icon beside the clock. several candidates are drawn below; the one in the tray is
# whichever UPDATE_ICON names, so switching is a one-line change. run with --update-sheet FILE
# to see them all side by side.

UPDATE_ICON = "globe"


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


# a coarse world map: 24 columns of longitude from -180 and 12 rows of latitude from the north pole
LAND = [
    "........................",
    "...XXX....XXXXXX.XX.....",
    "...XXXX..XXXXXXXXXXXX...",
    "....XXX...XXXXXXXXXXX...",
    ".....XX....XXXXXXX.XX...",
    ".....XXX...XXXX...XX....",
    "......XXX...XXX.........",
    "......XX....XXX....XX...",
    ".......X.....X.....XXX..",
    "........................",
    "........................",
    "..XXXXXXXXXXXXXXXXXXXX..",
]
GLOBE_FRAMES = 8
# how far the arrow has dipped on each frame: down for four frames and back
ARROW_BOB = [0, 0, 1, 1, 1, 1, 0, 0]


def globe_pixels(g, turn, cx, cy, r, grey=False):
    """a globe turned by `turn` radians, lit from the top left; grey for an idle state."""
    light = (-0.5, -0.5, 0.7)
    for y in range(16):
        for x in range(16):
            d = math.hypot(x - cx, y - cy)
            if d > r:
                continue
            if d > r - 1:
                g.dot(x, y, "k")
                continue
            nx, ny = (x - cx) / (r - 0.8), (y - cy) / (r - 0.8)
            nz = math.sqrt(max(0.0, 1 - nx * nx - ny * ny))
            lon = math.atan2(nx, nz) + turn
            lat = math.asin(max(-1.0, min(1.0, -ny)))
            col = int(((lon + math.pi) / (2 * math.pi)) * len(LAND[0])) % len(LAND[0])
            row = min(len(LAND) - 1, int((math.pi / 2 - lat) / math.pi * len(LAND)))
            land = LAND[row][col] == "X"
            shade = nx * light[0] + ny * light[1] + nz * light[2]
            if grey:
                g.dot(x, y, "w" if shade > 0.93 else ("g" if land else "d"))
            elif land:
                g.dot(x, y, "l" if shade > 0.93 else "G")
            else:
                g.dot(x, y, "c" if shade > 0.95 else ("n" if shade < 0.0 else "b"))


def globe_frame(i):
    """the globe turned `i` eighths of a turn, with the yellow arrow at its place in the bob."""
    g = Grid(16)
    globe_pixels(g, 2 * math.pi * i / GLOBE_FRAMES, 6.5, 6.5, 6)
    outlined(g, arrow_pixels(9, 8 + ARROW_BOB[i], 3), "y")
    return g


# badges for the corner of the globe, seven pixels square
BADGES = {
    "pause": ["..kkk..", ".kyyyk.", "kykykyk", "kykykyk", "kykykyk", ".kyyyk.", "..kkk.."],
    "x": ["..kkk..", ".krrrk.", "kwrrrwk", "krrwrrk", "kwrrrwk", ".krrrk.", "..kkk.."],
    "check": ["..kkk..", ".kGGGk.", "kGGGwwk", "kwGwGGk", "kGwGGGk", ".kGGGk.", "..kkk.."],
}


def with_badge(g, name):
    for j, row in enumerate(BADGES[name]):
        for i, c in enumerate(row):
            if c != ".":
                g.dot(9 + i, 9 + j, c)
    return g


def still_globe(grey=False):
    g = Grid(16)
    globe_pixels(g, 0.0, 6.5, 6.5, 6, grey=grey)
    return g


def u_paused():
    """a grey globe with a pause badge."""
    return with_badge(still_globe(grey=True), "pause")


def u_failed():
    """a grey globe with a red cross badge."""
    return with_badge(still_globe(grey=True), "x")


def u_done():
    """the globe in colour with a green check badge."""
    return with_badge(still_globe(), "check")


INSTALL_FILLS = [0, 1, 2, 3, 4, 4]


def install_frame(i):
    """a still globe over a segmented progress strip that fills block by block and wraps."""
    g = Grid(16)
    globe_pixels(g, 0.0, 7.5, 5.5, 5.7)
    g.rect(1, 12, 14, 4, "k")
    g.rect(2, 13, 12, 2, "w")
    for block in range(INSTALL_FILLS[i]):
        g.rect(2 + 3 * block, 13, 2, 2, "n")
    return g


def u_globe():
    """a small globe with a yellow down arrow, like the old windows update (the first frame)."""
    return globe_frame(0)


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


# the other states of the tray icon: a strip of frames, or a single sprite
INSTALL_FRAMES = len(INSTALL_FILLS)
UPDATE_SPRITES = {"UPDATE_PAUSED": u_paused, "UPDATE_FAILED": u_failed, "UPDATE_DONE": u_done}

# candidates that turn: how many frames each has and how to draw frame `i`
ANIMATED = {"globe": (GLOBE_FRAMES, globe_frame)}


def s_update():
    """the update notice in the tray: the candidate UPDATE_ICON names (the first frame)."""
    return UPDATE_CANDIDATES[UPDATE_ICON][1]()


def update_frames():
    """every frame of the tray icon: just one unless the chosen candidate animates."""
    if UPDATE_ICON in ANIMATED:
        count, draw = ANIMATED[UPDATE_ICON]
        return [draw(i) for i in range(count)]
    return [s_update()]


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


def frames_sheet(path):
    """every frame of the tray icon at 8x, then at 1x and 2x on the taskbar gray."""
    gray = RGB["g"]
    frames = update_frames()
    zoom, pad = 8, 10
    cell_w, cell_h = 16 * zoom + 40, 16 * zoom + 70
    cols = 4
    rows = (len(frames) + cols - 1) // cols
    w, h = cols * cell_w + pad, rows * cell_h + pad
    img = [[gray] * w for _ in range(h)]

    def block(x, y, art, z):
        for j, row in enumerate(art):
            for i, c in enumerate(row):
                if c in RGB:
                    for dy in range(z):
                        for dx in range(z):
                            img[y + j * z + dy][x + i * z + dx] = RGB[c]

    for n, frame in enumerate(frames):
        art = frame.rows()
        ox, oy = pad + (n % cols) * cell_w, pad + (n // cols) * cell_h
        block(ox, oy, art, zoom)
        block(ox, oy + 16 * zoom + 8, art, 1)
        block(ox + 30, oy + 16 * zoom + 8, art, 2)
    with open(path, "wb") as f:
        f.write(png(w, h, img))


def states_sheet(path):
    """every state of the tray icon: the spin and install frames, then the stills, at 8x."""
    gray = RGB["g"]
    items = update_frames() + [install_frame(i) for i in range(INSTALL_FRAMES)]
    items += [fn() for fn in UPDATE_SPRITES.values()]
    zoom, pad, cols = 6, 10, 8
    cell = 16 * zoom + 12
    rows = (len(items) + cols - 1) // cols
    w, h = cols * cell + pad, rows * (cell + 30) + pad
    img = [[gray] * w for _ in range(h)]
    for n, grid in enumerate(items):
        ox, oy = pad + (n % cols) * cell, pad + (n // cols) * (cell + 30)
        for j, row in enumerate(grid.rows()):
            for i, c in enumerate(row):
                if c in RGB:
                    for dy in range(zoom):
                        for dx in range(zoom):
                            img[oy + j * zoom + dy][ox + i * zoom + dx] = RGB[c]
        for j, row in enumerate(grid.rows()):
            for i, c in enumerate(row):
                if c in RGB:
                    img[oy + 16 * zoom + 6 + j][ox + i] = RGB[c]
    with open(path, "wb") as f:
        f.write(png(w, h, img))


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
    "MYSERVER": my_server, "NETWORKS": networks,
}
SMALL = {
    "GATEWAY": s_gateway, "DOCKER": s_docker, "NT4": s_tower, "WEB": s_globe,
    "COMMANDS": lambda: s_window(False), "CONSOLE": lambda: s_window(True), "DNS": s_dns,
    "SETTINGS": s_settings, "HOSTS": s_hosts, "FOLDER": s_folder, "LANLORD": s_lanlord, "LOGO": s_logo,
    "MYSERVER": s_my_server, "NETWORKS": s_networks,
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
    for name, fn in UPDATE_SPRITES.items():
        out.append(f"pub(crate) const SMALL_{name}: [&str; 16] = [\n")
        out += [f'    "{r}",\n' for r in fn().rows()]
        out.append("];\n\n")
    installing = [install_frame(i) for i in range(INSTALL_FRAMES)]
    out.append(f"pub(crate) const SMALL_UPDATE_INSTALL_FRAMES: [[&str; 16]; {len(installing)}] = [\n")
    for frame in installing:
        out.append("    [\n")
        out += [f'        "{r}",\n' for r in frame.rows()]
        out.append("    ],\n")
    out.append("];\n\n")
    frames = update_frames()
    out.append(f"pub(crate) const SMALL_UPDATE_FRAMES: [[&str; 16]; {len(frames)}] = [\n")
    for frame in frames:
        out.append("    [\n")
        out += [f'        "{r}",\n' for r in frame.rows()]
        out.append("    ],\n")
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


def png_rgba(width, height, pixels):
    raw = b"".join(b"\x00" + bytes(v for px in row for v in px) for row in pixels)

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def rgba(rows):
    return [[RGB[c] + (255,) if c in RGB else (0, 0, 0, 0) for c in row] for row in rows]


def favicon(path):
    """a 16 and 32 pixel .ico from the logo art, each entry a png."""
    images = [(16, s_logo().rows()), (32, logo32().rows())]
    blobs = [png_rgba(n, n, rgba(rows)) for n, rows in images]
    out = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    for (n, _), blob in zip(images, blobs):
        out += struct.pack("<BBBBHHII", n, n, 0, 0, 1, 32, len(blob), offset)
        offset += len(blob)
    with open(path, "wb") as f:
        f.write(out + b"".join(blobs))


def logo_strip(zoom):
    """the logo zoomed, then at 1x on the taskbar grey and on white."""
    arts = [s_logo().rows(), logo32().rows()]
    w = 640
    h = 16 * zoom * 2 + 80
    img = [[(0, 128, 128)] * w for _ in range(h)]

    def put(art, ox, oy, z, bg):
        n = len(art)
        for y in range(n * z):
            for x in range(n * z):
                img[oy + y][ox + x] = bg
        for y, row in enumerate(art):
            for x, c in enumerate(row):
                if c in RGB:
                    for dy in range(z):
                        for dx in range(z):
                            img[oy + y * z + dy][ox + x * z + dx] = RGB[c]

    x = 8
    for art in arts:
        z = zoom * 2 if len(art) == 16 else zoom
        put(art, x, 8, z, (192, 192, 192))
        x += len(art) * z + 8
    x = 8
    for bg in ((192, 192, 192), (255, 255, 255)):
        for art in arts:
            for gx in range(x, x + len(art) + 16):
                for gy in range(16 * zoom * 2 + 24, 16 * zoom * 2 + 24 + len(art) + 16):
                    img[gy][gx] = bg
            put(art, x + 8, 16 * zoom * 2 + 32, 1, bg)
            x += len(art) + 24
    return img


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
    img += logo_strip(zoom)
    img = [row + [teal] * (max(len(r) for r in img) - len(row)) for row in img]
    with open(path, "wb") as f:
        f.write(png(len(img[0]), len(img), img))


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--preview", help="write a zoomed contact sheet png here")
    ap.add_argument("--update-sheet", help="write a contact sheet of the update icon candidates here")
    ap.add_argument("--frames-sheet", help="write every frame of the chosen update icon here")
    ap.add_argument("--states-sheet", help="write every state of the tray icon here")
    ap.add_argument("--zoom", type=int, default=6)
    ap.add_argument("--no-write", action="store_true")
    a = ap.parse_args()
    if not a.no_write:
        root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
        emit(os.path.join(root, "crates", "ui", "src", "icon_data.rs"))
        favicon(os.path.join(root, "crates", "ui", "assets", "favicon.ico"))
    if a.preview:
        preview(a.preview, a.zoom)
    if a.update_sheet:
        update_sheet(a.update_sheet)
    if a.frames_sheet:
        frames_sheet(a.frames_sheet)
    if a.states_sheet:
        states_sheet(a.states_sheet)
