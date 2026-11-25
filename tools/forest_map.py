"""Paints assets/forest.png: the office as a forest camp, 1254x1254 like
assets/office.png, every room, door, seat and desk where the walk graph in
assets/office.json expects it. Trails follow the graph's corridors.

    python3 tools/forest_map.py [OUT.png]      # needs Pillow and numpy

Coordinates are office.png pixels, as in assets/office.json; the picture is drawn
at S times the size and scaled down, for smooth edges.
"""

import math
import random
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

SIZE = 1254
S = 2
W = SIZE * S
rng = random.Random(7)
nrng = np.random.default_rng(7)


def s(v):
    return int(round(v * S))


def box(x0, y0, x1, y1):
    return [s(x0), s(y0), s(x1), s(y1)]


def rgb(h):
    h = h.lstrip("#")
    return tuple(int(h[i : i + 2], 16) for i in (0, 2, 4))


def shade(c, f):
    return tuple(max(0, min(255, int(v * f))) for v in c[:3])


# The rooms, as office.png has them: (x0, y0, x1, y1).
ROOMS = {
    "lounge": (40, 35, 378, 330),
    "lobby": (392, 22, 845, 440),
    "meeting": (858, 35, 1215, 495),
    "wash": (40, 345, 268, 530),
    "copy": (40, 545, 268, 700),
    "kitchen": (40, 715, 285, 1162),
    "open": (285, 440, 872, 1162),
    "manager": (890, 515, 1215, 872),
    "spring": (920, 890, 1215, 1162),
}

# The walk graph, copied from src/actors.rs: nodes and corridors.
N = {}


def n(name, x, y):
    N[name] = (x, y)


n("exit", 630, 118)
for k, v in dict(lob_n=(630, 190), lob_nw=(455, 190), lob_ne=(805, 190), lob_sw=(455, 420), lob_s=(630, 420), lob_se=(805, 420)).items():
    n(k, *v)
for k, v in dict(tl_out=(425, 330), tl_in=(318, 305), tl_s=(205, 315), tl_gap_s=(257, 292), tl_gap_n=(257, 210), tl_sw=(150, 300), tl_sofa=(203, 198), tl_arm_l=(119, 266), tl_arm_r=(290, 250)).items():
    n(k, *v)
for k, v in dict(wc_out=(310, 500), wc_in=(228, 500), wc_n=(228, 425), wc_w=(110, 425)).items():
    n(k, *v)
for k, v in dict(mt_out=(805, 390), mt_in=(915, 390), mt_w=(915, 268), mt_nw=(915, 125), mt_n1=(992, 125), mt_n2=(1093, 125), mt_sw=(915, 475), mt_s=(1023, 478), mt_se=(1182, 475), mt_e2=(1182, 368), mt_e1=(1182, 268)).items():
    n(k, *v)
CHAIRS = [(992, 172), (1093, 172), (955, 268), (1132, 268), (955, 368), (1132, 368), (1023, 458)]
for i, c in enumerate(CHAIRS):
    n(f"ch{i}", *c)
n("op_nw", 345, 470)
n("op_ne", 835, 470)
XS, YS = [340, 505, 650, 822], [565, 742, 905, 1100]
for c, x in enumerate(XS):
    for r, y in enumerate(YS):
        n(f"g{c}{r}", x, y)
COLS, ROWS = [430, 575, 720], [642, 806, 977]
DESKS = []
for r, y in enumerate(ROWS):
    for c, x in enumerate(COLS):
        n(f"de{c}{r}", x, YS[r])
        n(f"d{c}{r}", x, y)
        DESKS.append((x, y))
for k, v in dict(mgr_out=(845, 812), mgr_in=(945, 800), mgr_w=(962, 735), mgr_nw=(962, 628), lead=(1048, 640), mgr_sofa=(990, 815)).items():
    n(k, *v)
for k, v in dict(br_out=(850, 1120), br_in=(975, 1112), br_w=(975, 1000), br_n=(1060, 1000), br_s=(1060, 1150), br_tr=(1105, 1042), br_bl=(1015, 1132), br_br=(1108, 1134)).items():
    n(k, *v)
for k, v in dict(kit_in=(265, 745), kit_e=(265, 850), kit_se=(268, 978), kit_mid=(180, 985), kit_nw=(70, 985), kit_sw=(70, 1150), kit_s=(200, 1155), stool_l=(120, 945), stool_r=(217, 945), din_tl=(155, 1037), din_tr=(245, 1037), din_bl=(155, 1137), din_br=(245, 1137)).items():
    n(k, *v)

E = [
    ("exit", "lob_n"), ("lob_n", "lob_nw"), ("lob_n", "lob_ne"), ("lob_nw", "lob_sw"), ("lob_ne", "mt_out"), ("mt_out", "lob_se"), ("lob_sw", "lob_s"), ("lob_s", "lob_se"),
    ("lob_nw", "tl_out"), ("lob_sw", "tl_out"), ("tl_out", "tl_in"), ("tl_in", "tl_gap_s"), ("tl_gap_s", "tl_gap_n"), ("tl_gap_n", "tl_sofa"),
    ("tl_in", "tl_s"), ("tl_s", "tl_sw"), ("tl_sw", "tl_arm_l"), ("tl_in", "tl_arm_r"),
    ("op_nw", "wc_out"), ("wc_out", "wc_in"), ("wc_in", "wc_n"), ("wc_n", "wc_w"),
    ("mt_out", "mt_in"), ("mt_in", "mt_w"), ("mt_w", "mt_nw"), ("mt_nw", "mt_n1"), ("mt_n1", "mt_n2"), ("mt_in", "mt_sw"), ("mt_sw", "mt_s"), ("mt_s", "mt_se"), ("mt_se", "mt_e2"), ("mt_e2", "mt_e1"),
    ("lob_sw", "op_nw"), ("lob_se", "op_ne"), ("op_nw", "g00"), ("op_ne", "g30"),
    ("g31", "mgr_out"), ("g32", "mgr_out"), ("mgr_out", "mgr_in"), ("mgr_in", "mgr_w"), ("mgr_w", "mgr_nw"), ("mgr_nw", "lead"), ("mgr_in", "mgr_sofa"),
    ("g33", "br_out"), ("br_out", "br_in"), ("br_in", "br_w"), ("br_w", "br_n"), ("br_n", "br_tr"), ("br_in", "br_bl"), ("br_in", "br_s"), ("br_s", "br_br"),
    ("g01", "kit_in"), ("kit_in", "kit_e"), ("kit_e", "kit_se"), ("kit_se", "kit_mid"), ("kit_mid", "kit_nw"), ("kit_nw", "kit_sw"), ("kit_sw", "kit_s"),
    ("kit_mid", "stool_l"), ("kit_mid", "stool_r"), ("kit_mid", "din_tl"), ("kit_mid", "din_tr"), ("kit_s", "din_bl"), ("kit_s", "din_br"),
]
E += list(zip(["mt_n1", "mt_n2", "mt_w", "mt_e1", "mt_in", "mt_e2", "mt_s"], [f"ch{i}" for i in range(7)]))
for c in range(4):
    for r in range(4):
        if c < 3:
            E.append((f"g{c}{r}", f"g{c+1}{r}"))
        if r < 3:
            E.append((f"g{c}{r}", f"g{c}{r+1}"))
for r in range(3):
    for c in range(3):
        E += [(f"de{c}{r}", f"g{c}{r}"), (f"de{c}{r}", f"g{c+1}{r}"), (f"de{c}{r}", f"d{c}{r}")]


# Textures.


def smooth_noise(w, h, cell, octaves=3):
    out = np.zeros((h, w), np.float32)
    amp, total = 1.0, 0.0
    for o in range(octaves):
        c = max(2, cell >> o)
        small = nrng.random((h // c + 2, w // c + 2)).astype(np.float32)
        img = Image.fromarray((small * 255).astype(np.uint8)).resize((w + c * 2, h + c * 2), Image.BICUBIC)
        out += amp * (np.asarray(img, np.float32)[:h, :w] / 255.0)
        total += amp
        amp *= 0.5
    return out / total


def texture(w, h, dark, light, cell=40, grain=0.08):
    t = smooth_noise(w, h, cell)
    t = np.clip(t + nrng.normal(0, grain, (h, w)).astype(np.float32), 0, 1)[..., None]
    a, b = np.array(rgb(dark), np.float32), np.array(rgb(light), np.float32)
    return Image.fromarray((a + (b - a) * t).astype(np.uint8), "RGB")


def paste_texture(img, rect, dark, light, cell=40, grain=0.08):
    x0, y0, x1, y1 = box(*rect)
    img.paste(texture(x1 - x0, y1 - y0, dark, light, cell, grain), (x0, y0))


def planks(img, rect, base, vertical=False, width=34):
    x0, y0, x1, y1 = rect
    d = ImageDraw.Draw(img)
    span = (x1 - x0) if vertical else (y1 - y0)
    pos = 0
    while pos < span:
        c = shade(rgb(base), 0.88 + 0.22 * rng.random())
        r = (x0 + pos, y0, min(x1, x0 + pos + width), y1) if vertical else (x0, y0 + pos, x1, min(y1, y0 + pos + width))
        d.rectangle(box(*r), fill=c)
        for _ in range(int(span / 6)):  # grain
            if vertical:
                gx, gy = rng.uniform(r[0] + 3, r[2] - 3), rng.uniform(y0, y1 - 40)
                d.line(box(gx, gy, gx + rng.uniform(-1, 1), gy + rng.uniform(15, 50)), fill=shade(c, 0.85), width=S)
            else:
                gy, gx = rng.uniform(r[1] + 3, r[3] - 3), rng.uniform(x0, x1 - 40)
                d.line(box(gx, gy, gx + rng.uniform(15, 60), gy + rng.uniform(-1, 1)), fill=shade(c, 0.85), width=S)
        if vertical:  # seam and butt joints
            d.line(box(r[0], y0, r[0], y1), fill=shade(c, 0.55), width=s(2))
            jy = y0 + rng.uniform(40, 200)
            while jy < y1:
                d.line(box(r[0], jy, r[2], jy), fill=shade(c, 0.6), width=s(2))
                jy += rng.uniform(120, 260)
        else:
            d.line(box(x0, r[1], x1, r[1]), fill=shade(c, 0.55), width=s(2))
            jx = x0 + rng.uniform(40, 200)
            while jx < x1:
                d.line(box(jx, r[1], jx, r[3]), fill=shade(c, 0.6), width=s(2))
                jx += rng.uniform(120, 260)
        pos += width


def flagstones(img, rect, base, mortar, size=46):
    x0, y0, x1, y1 = rect
    d = ImageDraw.Draw(img)
    d.rectangle(box(*rect), fill=rgb(mortar))
    y, row = y0, 0
    while y < y1:
        h = size * rng.uniform(0.75, 1.1)
        x = x0 - (size / 2 if row % 2 else 0)
        while x < x1:
            w = size * rng.uniform(0.8, 1.4)
            c = shade(rgb(base), rng.uniform(0.86, 1.1))
            r = (max(x0, x + 3), max(y0, y + 3), min(x1, x + w - 3), min(y1, y + h - 3))
            if r[2] - r[0] > 6 and r[3] - r[1] > 6:
                d.rounded_rectangle(box(*r), radius=s(6), fill=c)
                d.line(box(r[0] + 3, r[1] + 2, r[2] - 4, r[1] + 2), fill=shade(c, 1.12), width=s(2))
                d.line(box(r[0] + 3, r[3] - 2, r[2] - 4, r[3] - 2), fill=shade(c, 0.8), width=s(2))
                if rng.random() < 0.3:  # moss in a corner
                    mx, my = rng.uniform(r[0], r[2]), rng.uniform(r[1], r[3])
                    d.ellipse(box(mx - 5, my - 3, mx + 5, my + 3), fill=rgb("#6f8f45"))
            x += w
        y += h
        row += 1


# Things.


def soft_shadow(img, cx, cy, rx, ry, alpha=90):
    layer = Image.new("L", img.size, 0)
    ImageDraw.Draw(layer).ellipse(box(cx - rx, cy - ry, cx + rx, cy + ry), fill=alpha)
    layer = layer.filter(ImageFilter.GaussianBlur(s(4)))
    img.paste(Image.new("RGB", img.size, (10, 25, 12)), (0, 0), layer)


def glow(img, cx, cy, r, color=(255, 190, 90), alpha=110):
    layer = Image.new("L", img.size, 0)
    ImageDraw.Draw(layer).ellipse(box(cx - r, cy - r, cx + r, cy + r), fill=alpha)
    layer = layer.filter(ImageFilter.GaussianBlur(s(r / 2.5)))
    lit = Image.blend(img, Image.new("RGB", img.size, color), 0.55)
    img.paste(lit, (0, 0), layer)


def blob(d, cx, cy, r, color, rough=0.18, pts=18):
    poly = []
    for i in range(pts):
        a = 2 * math.pi * i / pts
        rr = r * (1 + rng.uniform(-rough, rough))
        poly.append((s(cx + rr * math.cos(a)), s(cy + rr * math.sin(a) * 0.95)))
    d.polygon(poly, fill=color)


def tree(img, cx, cy, r, hue="#2f6b2f", shadow=True):
    """A canopy seen from above, centred at (cx, cy)."""
    if shadow:
        soft_shadow(img, cx + r * 0.25, cy + r * 0.35, r * 1.0, r * 0.8, 110)
    d = ImageDraw.Draw(img)
    base = rgb(hue)
    blob(d, cx, cy, r, shade(base, 0.62), 0.12)
    for _ in range(6):
        a, k = rng.uniform(0, 2 * math.pi), rng.uniform(0.2, 0.5)
        blob(d, cx + math.cos(a) * r * k, cy + math.sin(a) * r * k, r * rng.uniform(0.45, 0.6), shade(base, 0.85), 0.2, 12)
    for _ in range(5):
        a, k = rng.uniform(math.pi, 1.6 * math.pi), rng.uniform(0.15, 0.45)
        blob(d, cx + math.cos(a) * r * k, cy + math.sin(a) * r * k, r * rng.uniform(0.25, 0.38), shade(base, 1.12), 0.25, 10)
    for _ in range(int(r / 2)):
        a, k = rng.uniform(0, 2 * math.pi), rng.uniform(0, 0.8)
        x, y = cx + math.cos(a) * r * k, cy + math.sin(a) * r * k
        d.ellipse(box(x - 1.5, y - 1.5, x + 1.5, y + 1.5), fill=shade(base, 1.35 if y < cy else 0.7))


def pine(img, cx, cy, r):
    soft_shadow(img, cx + r * 0.3, cy + r * 0.4, r * 0.9, r * 0.7, 110)
    d = ImageDraw.Draw(img)
    for k, f in [(1.0, 0.55), (0.78, 0.72), (0.55, 0.9), (0.3, 1.1)]:
        pts = []
        for i in range(16):
            a = 2 * math.pi * i / 16
            rr = r * k * (1.0 if i % 2 == 0 else 0.72)
            pts.append((s(cx + rr * math.cos(a)), s(cy + rr * math.sin(a))))
        d.polygon(pts, fill=shade(rgb("#24563a"), f))


def bush(img, cx, cy, r, hue="#3b7a34", berries=None):
    soft_shadow(img, cx + 3, cy + r * 0.45, r, r * 0.6, 90)
    d = ImageDraw.Draw(img)
    base = rgb(hue)
    for _ in range(4):
        blob(d, cx + rng.uniform(-r * 0.3, r * 0.3), cy + rng.uniform(-r * 0.2, r * 0.2), r * rng.uniform(0.6, 0.8), shade(base, 0.75), 0.2, 12)
    for _ in range(3):
        blob(d, cx + rng.uniform(-r * 0.35, r * 0.1), cy + rng.uniform(-r * 0.4, 0), r * rng.uniform(0.35, 0.5), shade(base, 1.1), 0.25, 10)
    if berries:
        for _ in range(6):
            x, y = cx + rng.uniform(-r * 0.6, r * 0.6), cy + rng.uniform(-r * 0.5, r * 0.4)
            d.ellipse(box(x - 2.5, y - 2.5, x + 2.5, y + 2.5), fill=rgb(berries))


def hedge(img, x0, y0, x1, y1):
    """A hedgerow filling a wall's rectangle."""
    horizontal = (x1 - x0) >= (y1 - y0)
    length = (x1 - x0) if horizontal else (y1 - y0)
    thick = (y1 - y0) if horizontal else (x1 - x0)
    soft_shadow(img, (x0 + x1) / 2 + 4, (y0 + y1) / 2 + 6, (x1 - x0) / 2 + 6, (y1 - y0) / 2 + 6, 120)
    d = ImageDraw.Draw(img)
    d.rounded_rectangle(box(x0, y0, x1, y1), radius=s(thick / 2.2), fill=rgb("#1f4a24"))
    step = max(8, thick * 0.55)
    t = 0.0
    while t <= length:
        cx = x0 + t if horizontal else (x0 + x1) / 2 + rng.uniform(-2, 2)
        cy = (y0 + y1) / 2 + rng.uniform(-2, 2) if horizontal else y0 + t
        cx, cy = min(max(cx, x0 + thick / 2.4), x1 - thick / 2.4), min(max(cy, y0 + thick / 2.4), y1 - thick / 2.4)
        blob(d, cx, cy, thick * 0.52, rgb("#2c6230"), 0.18, 12)
        blob(d, cx - thick * 0.12, cy - thick * 0.14, thick * 0.3, rgb("#3f8340"), 0.25, 10)
        if rng.random() < 0.35:
            d.ellipse(box(cx - thick * 0.25 - 1.5, cy - thick * 0.2 - 1.5, cx - thick * 0.25 + 1.5, cy - thick * 0.2 + 1.5), fill=rgb("#79b55a"))
        if rng.random() < 0.12:
            fx, fy = cx + rng.uniform(-4, 4), cy + rng.uniform(-4, 4)
            d.ellipse(box(fx - 2.5, fy - 2.5, fx + 2.5, fy + 2.5), fill=rgb(rng.choice(["#f2e6a0", "#f5b8c8", "#ffffff"])))
        t += step


def stump(img, cx, cy, r, tall=0.45):
    soft_shadow(img, cx + 3, cy + r * 0.5, r * 1.05, r * 0.55, 100)
    d = ImageDraw.Draw(img)
    h = r * tall
    d.rounded_rectangle(box(cx - r, cy - r * 0.55, cx + r, cy + h), radius=s(r * 0.4), fill=rgb("#5b3a1e"))
    for i in range(5):
        x = cx - r + (i + 0.5) * 2 * r / 5
        d.line(box(x, cy, x + rng.uniform(-1, 1), cy + h - 2), fill=rgb("#432a14"), width=s(1.5))
    d.ellipse(box(cx - r, cy - r * 0.55 - r * 0.5, cx + r, cy - r * 0.55 + r * 0.5), fill=rgb("#c99b62"))
    for k in (0.75, 0.5, 0.25):
        d.ellipse(box(cx - r * k, cy - r * 0.55 - r * 0.5 * k, cx + r * k, cy - r * 0.55 + r * 0.5 * k), outline=rgb("#a87a46"), width=s(1.2))
    d.ellipse(box(cx - 1.5, cy - r * 0.55 - 1, cx + 1.5, cy - r * 0.55 + 1), fill=rgb("#8a5f31"))


def log(img, x0, y0, x1, y1, moss=True):
    """A log lying in the rectangle, along its longer side."""
    horizontal = (x1 - x0) >= (y1 - y0)
    soft_shadow(img, (x0 + x1) / 2 + 4, y1 - 2, (x1 - x0) / 2 + 4, (y1 - y0) / 2.5 + 3, 110)
    d = ImageDraw.Draw(img)
    r = ((y1 - y0) if horizontal else (x1 - x0)) / 2
    d.rounded_rectangle(box(x0, y0, x1, y1), radius=s(r), fill=rgb("#6b4523"))
    if horizontal:
        d.rounded_rectangle(box(x0 + 2, y0 + 2, x1 - 2, y0 + r * 0.8), radius=s(r * 0.6), fill=rgb("#8a5c30"))
        for _ in range(int((x1 - x0) / 9)):
            x = rng.uniform(x0 + r, x1 - r)
            d.line(box(x, y0 + 3, x + rng.uniform(4, 14), y0 + 3 + rng.uniform(4, 2 * r - 6)), fill=rgb("#4d3018"), width=s(1.3))
        d.ellipse(box(x1 - r * 1.1, y0, x1, y1), fill=rgb("#c99b62"))
        d.ellipse(box(x1 - r * 0.85, y0 + r * 0.25, x1 - r * 0.25, y1 - r * 0.25), outline=rgb("#a87a46"), width=s(1.2))
        if moss:
            for _ in range(int((x1 - x0) / 14)):
                blob(d, rng.uniform(x0 + r, x1 - 2 * r), y0 + rng.uniform(2, r * 0.7), rng.uniform(3, 6), rgb("#6f9a3e"), 0.3, 8)
    else:
        d.rounded_rectangle(box(x0 + 2, y0 + 2, x0 + r * 0.8, y1 - 2), radius=s(r * 0.6), fill=rgb("#8a5c30"))
        for _ in range(int((y1 - y0) / 9)):
            y = rng.uniform(y0 + r, y1 - r)
            d.line(box(x0 + 3, y, x0 + 3 + rng.uniform(4, 2 * r - 6), y + rng.uniform(4, 14)), fill=rgb("#4d3018"), width=s(1.3))
        d.ellipse(box(x0, y1 - r * 1.1, x1, y1), fill=rgb("#c99b62"))
        d.ellipse(box(x0 + r * 0.25, y1 - r * 0.85, x1 - r * 0.25, y1 - r * 0.25), outline=rgb("#a87a46"), width=s(1.2))
        if moss:
            for _ in range(int((y1 - y0) / 14)):
                blob(d, x0 + rng.uniform(2, r * 0.7), rng.uniform(y0 + r, y1 - 2 * r), rng.uniform(3, 6), rgb("#6f9a3e"), 0.3, 8)


def rock(img, cx, cy, r, hue="#8c8f86"):
    soft_shadow(img, cx + 3, cy + r * 0.5, r * 1.05, r * 0.6, 100)
    d = ImageDraw.Draw(img)
    blob(d, cx, cy, r, shade(rgb(hue), 0.75), 0.15, 9)
    blob(d, cx - r * 0.15, cy - r * 0.2, r * 0.7, rgb(hue), 0.15, 9)
    blob(d, cx - r * 0.3, cy - r * 0.35, r * 0.3, shade(rgb(hue), 1.18), 0.2, 7)
    if rng.random() < 0.5:
        blob(d, cx + r * 0.3, cy - r * 0.4, r * 0.3, rgb("#6f9a3e"), 0.3, 8)


def flowers(img, cx, cy, r, count=10, colors=("#f4d35e", "#f78fb3", "#ffffff", "#c4a7e7")):
    d = ImageDraw.Draw(img)
    for _ in range(count):
        a, k = rng.uniform(0, 2 * math.pi), rng.uniform(0, 1) ** 0.5
        x, y = cx + math.cos(a) * r * k, cy + math.sin(a) * r * k * 0.7
        c = rgb(rng.choice(colors))
        for dx, dy in ((-2, 0), (2, 0), (0, -2), (0, 2)):
            d.ellipse(box(x + dx - 1.2, y + dy - 1.2, x + dx + 1.2, y + dy + 1.2), fill=c)
        d.ellipse(box(x - 1, y - 1, x + 1, y + 1), fill=rgb("#e09b2d"))


def mushrooms(img, cx, cy, count=3):
    d = ImageDraw.Draw(img)
    for _ in range(count):
        x, y = cx + rng.uniform(-10, 10), cy + rng.uniform(-6, 6)
        r = rng.uniform(3.5, 6)
        d.rectangle(box(x - r * 0.3, y, x + r * 0.3, y + r * 0.9), fill=rgb("#efe6cf"))
        d.pieslice(box(x - r, y - r * 0.8, x + r, y + r * 0.8), 180, 360, fill=rgb("#c8402f"))
        d.ellipse(box(x - r * 0.45, y - r * 0.55, x - r * 0.15, y - r * 0.3), fill=(255, 255, 255))
        d.ellipse(box(x + r * 0.15, y - r * 0.45, x + r * 0.4, y - r * 0.22), fill=(255, 255, 255))


def tufts(img, rect, count, hue="#3f7a2c"):
    d = ImageDraw.Draw(img)
    x0, y0, x1, y1 = rect
    for _ in range(count):
        x, y = rng.uniform(x0, x1), rng.uniform(y0, y1)
        for k in range(3):
            d.line(box(x + k * 2 - 2, y, x + k * 2 - 2 + rng.uniform(-3, 3), y - rng.uniform(4, 8)), fill=shade(rgb(hue), rng.uniform(0.85, 1.25)), width=s(1.2))


def lantern(img, cx, cy):
    glow(img, cx, cy - 18, 34)
    soft_shadow(img, cx + 3, cy + 2, 6, 3, 100)
    d = ImageDraw.Draw(img)
    d.rectangle(box(cx - 2, cy - 22, cx + 2, cy), fill=rgb("#3d2a17"))
    d.rounded_rectangle(box(cx - 6, cy - 34, cx + 6, cy - 20), radius=s(2), fill=rgb("#2b2b2b"))
    d.rectangle(box(cx - 4, cy - 32, cx + 4, cy - 22), fill=rgb("#ffd479"))
    d.polygon([(s(cx - 8), s(cy - 34)), (s(cx + 8), s(cy - 34)), (s(cx), s(cy - 41))], fill=rgb("#2b2b2b"))


def campfire(img, cx, cy):
    glow(img, cx, cy, 70, (255, 160, 70), 120)
    for i in range(10):
        a = 2 * math.pi * i / 10
        rock(img, cx + math.cos(a) * 24, cy + math.sin(a) * 17, 6.5, "#7d7f78")
    d = ImageDraw.Draw(img)
    for a in (20, 80, 140):
        r = math.radians(a)
        d.line(box(cx - math.cos(r) * 15, cy - math.sin(r) * 9, cx + math.cos(r) * 15, cy + math.sin(r) * 9), fill=rgb("#5b3a1e"), width=s(5))
    for col, k in [("#e2531f", 1.0), ("#f39a2b", 0.72), ("#ffd86b", 0.42)]:
        pts = [(cx - 11 * k, cy + 3), (cx - 6 * k, cy - 14 * k), (cx - 2 * k, cy - 7 * k), (cx + 2 * k, cy - 20 * k), (cx + 6 * k, cy - 8 * k), (cx + 11 * k, cy + 3)]
        d.polygon([(s(x), s(y)) for x, y in pts], fill=rgb(col))


def table_slab(img, x0, y0, x1, y1):
    """A plank table seen from above."""
    soft_shadow(img, (x0 + x1) / 2 + 4, y1 + 2, (x1 - x0) / 2 + 2, 7, 120)
    d = ImageDraw.Draw(img)
    d.rounded_rectangle(box(x0, y0 + 6, x1, y1 + 6), radius=s(6), fill=rgb("#4a2f17"))
    d.rounded_rectangle(box(x0, y0, x1, y1), radius=s(6), fill=rgb("#9a6634"))
    k = y0 + 4
    while k < y1 - 12:
        d.line(box(x0 + 3, k + 9, x1 - 3, k + 9), fill=rgb("#7a4f27"), width=s(1.5))
        k += 11
    d.line(box(x0 + 4, y0 + 2, x1 - 4, y0 + 2), fill=rgb("#b98149"), width=s(2))


def book(img, x, y, col="#7a2f2f"):
    d = ImageDraw.Draw(img)
    d.rectangle(box(x - 9, y - 6, x + 9, y + 6), fill=rgb(col))
    d.rectangle(box(x - 7, y - 5, x + 7, y + 5), fill=rgb("#efe3c4"))
    d.line(box(x, y - 5, x, y + 5), fill=rgb("#8f8064"), width=s(1))


def water(img, rect, ellipse=True, hot=False):
    x0, y0, x1, y1 = rect
    mask = Image.new("L", img.size, 0)
    md = ImageDraw.Draw(mask)
    if ellipse:
        pts = []
        cx, cy, rx, ry = (x0 + x1) / 2, (y0 + y1) / 2, (x1 - x0) / 2, (y1 - y0) / 2
        for i in range(40):
            a = 2 * math.pi * i / 40
            k = 1 + 0.06 * math.sin(3 * a + 1) + 0.04 * math.sin(5 * a)
            pts.append((s(cx + rx * k * math.cos(a)), s(cy + ry * k * math.sin(a))))
        md.polygon(pts, fill=255)
    else:
        md.rectangle(box(*rect), fill=255)
    edge = mask.filter(ImageFilter.MaxFilter(s(4) * 2 + 1))
    img.paste(Image.new("RGB", img.size, rgb("#5f5a45")), (0, 0), edge)
    dark, light = ("#4f8fa3", "#9fd3d6") if hot else ("#245d7a", "#4f94b8")
    img.paste(texture(W, W, dark, light, 30, 0.03), (0, 0), mask)
    d = ImageDraw.Draw(img)
    for _ in range(int((x1 - x0) * (y1 - y0) / 900)):
        x, y = rng.uniform(x0 + 8, x1 - 8), rng.uniform(y0 + 6, y1 - 6)
        if mask.getpixel((s(x - 8), s(y))) and mask.getpixel((s(x + 8), s(y))):
            d.arc(box(x - 8, y - 2, x + 8, y + 2), 200, 340, fill=rgb("#eafaf8" if hot else "#cfe9ef"), width=s(1))


def lily(img, x, y, r=7):
    d = ImageDraw.Draw(img)
    d.pieslice(box(x - r, y - r * 0.7, x + r, y + r * 0.7), 30, 330, fill=rgb("#4f8a3a"))
    if rng.random() < 0.5:
        d.ellipse(box(x - 2.5, y - 2.5, x + 2.5, y + 2.5), fill=rgb("#f7c6d9"))


def reeds(img, x, y):
    d = ImageDraw.Draw(img)
    for k in range(5):
        dx = k * 3 - 6
        top = y - rng.uniform(14, 24)
        d.line(box(x + dx, y, x + dx + rng.uniform(-3, 3), top), fill=rgb("#5d7d32"), width=s(1.4))
        if k % 2 == 0:
            d.ellipse(box(x + dx - 1.6, top - 1, x + dx + 1.6, top + 6), fill=rgb("#6b4523"))


def trail(img, pairs, width=20):
    mask = Image.new("L", img.size, 0)
    md = ImageDraw.Draw(mask)
    for a, b in pairs:
        md.line(box(*a, *b), fill=255, width=s(width))
        for p in (a, b):
            md.ellipse(box(p[0] - width / 2, p[1] - width / 2, p[0] + width / 2, p[1] + width / 2), fill=255)
    rough = smooth_noise(W, W, 10, 2) * 1.6 * 255 + 60
    mask = Image.fromarray(np.minimum(np.asarray(mask.filter(ImageFilter.GaussianBlur(s(2.5))), np.float32), rough).astype(np.uint8))
    img.paste(texture(W, W, "#8a6a42", "#b48f5c", 12, 0.12), (0, 0), mask)
    d = ImageDraw.Draw(img)
    for a, b in pairs:  # pebbles along it
        for _ in range(int(math.dist(a, b) / 25)):
            t = rng.random()
            x, y = a[0] + (b[0] - a[0]) * t + rng.uniform(-7, 7), a[1] + (b[1] - a[1]) * t + rng.uniform(-7, 7)
            d.ellipse(box(x - 1.8, y - 1.3, x + 1.8, y + 1.3), fill=shade(rgb("#9b8a70"), rng.uniform(0.8, 1.15)))


def room_of(p):
    for k, (x0, y0, x1, y1) in ROOMS.items():
        if x0 <= p[0] <= x1 and y0 <= p[1] <= y1:
            return k
    return None


def main(out):
    img = Image.new("RGB", (W, W))
    img.paste(texture(W, W, "#16301c", "#2a4d2b", 60, 0.06))  # deep forest floor

    # Each room's ground.
    paste_texture(img, (22, 18, 1232, 1162), "#3f6f2e", "#6f9f45", 50)
    flagstones(img, ROOMS["lobby"], "#a59c86", "#5f6b4a")
    paste_texture(img, ROOMS["lounge"], "#4c5a2c", "#7d7a3c", 30)  # leaf litter
    d = ImageDraw.Draw(img)
    for _ in range(500):
        x, y = rng.uniform(42, 376), rng.uniform(37, 328)
        d.ellipse(box(x - 2.5, y - 1.5, x + 2.5, y + 1.5), fill=rgb(rng.choice(["#a65f2a", "#c9872f", "#8a4f24", "#b3a03a"])))
    paste_texture(img, ROOMS["wash"], "#3e6e30", "#629846", 30)
    paste_texture(img, ROOMS["copy"], "#1d3d22", "#2f5a2c", 30)
    planks(img, ROOMS["kitchen"], "#a7743f", vertical=True)
    paste_texture(img, ROOMS["open"], "#4b7f33", "#79ad4c", 45)
    paste_texture(img, ROOMS["meeting"], "#41693a", "#5f8a49", 35)
    planks(img, ROOMS["manager"], "#8f5f33")
    paste_texture(img, ROOMS["spring"], "#b9ad8e", "#d8ccaa", 14, 0.12)  # raked gravel
    d = ImageDraw.Draw(img)
    x0, y0, x1, y1 = ROOMS["spring"]
    for y in range(y0 + 8, y1, 9):
        d.line(box(x0 + 4, y, x1 - 4, y + 1), fill=rgb("#a99d7d"), width=s(1))

    # Trails along the corridors where the ground is earth or grass; stepping
    # stones across the gravel garden. The decks and the plaza need none.
    soft = {"lounge", "wash", "open", "meeting"}
    pairs = [(N[a], N[b]) for a, b in E if room_of(N[a]) in soft or room_of(N[b]) in soft]
    trail(img, pairs + [((630, 0), N["exit"])])
    d = ImageDraw.Draw(img)
    for a, b in E:
        if room_of(N[a]) == "spring" and room_of(N[b]) == "spring":
            k = int(math.dist(N[a], N[b]) / 26)
            for i in range(1, k + 1):
                t = i / (k + 1)
                x, y = N[a][0] + (N[b][0] - N[a][0]) * t, N[a][1] + (N[b][1] - N[a][1]) * t
                d.ellipse(box(x - 9, y - 6, x + 9, y + 6), fill=rgb("#8d8a80"), outline=rgb("#6c695f"), width=s(1))

    tufts(img, ROOMS["open"], 900)
    tufts(img, ROOMS["meeting"], 300, "#2f5f2a")
    tufts(img, ROOMS["wash"], 200)

    # Washroom: a pond south of the path, a spring where the sinks were.
    water(img, (50, 448, 200, 522))
    for p in [(80, 470), (130, 495), (165, 468), (100, 505)]:
        lily(img, *p)
    for p in [(56, 452), (196, 515), (205, 470)]:
        reeds(img, *p)
    water(img, (70, 362, 180, 402))
    for x in range(64, 190, 14):
        rock(img, x, 358 + rng.uniform(-2, 2), 7)
    rock(img, 200, 380, 10)
    rock(img, 245, 470, 8)
    flowers(img, 248, 382, 14, 10)

    # Copy room: no door, so a thicket.
    for _ in range(14):
        tree(img, rng.uniform(60, 250), rng.uniform(560, 690), rng.uniform(26, 40), rng.choice(["#2c5e2f", "#2f6b2f", "#356f2c"]))
    rock(img, 100, 640, 16)

    # Lounge: a campfire clearing; a log bench for the sofa, stumps for the armchairs.
    campfire(img, 205, 248)
    log(img, 140, 168, 268, 200)
    stump(img, 119, 266, 15)
    stump(img, 290, 250, 15)
    tree(img, 72, 88, 34)
    tree(img, 335, 78, 30, "#356f2c")
    pine(img, 68, 300, 22)
    mushrooms(img, 95, 145)
    mushrooms(img, 320, 150, 2)
    flowers(img, 175, 90, 26, 14)
    lantern(img, 352, 250)

    # Lobby: a gate at the entrance, a log reception counter.
    d = ImageDraw.Draw(img)
    for x in (560, 700):
        soft_shadow(img, x + 4, 128, 10, 5, 110)
        d = ImageDraw.Draw(img)
        d.rectangle(box(x - 8, 50, x + 8, 128), fill=rgb("#7a3b22"))
        d.rectangle(box(x - 8, 50, x - 3, 128), fill=rgb("#94502d"))
    d.rectangle(box(536, 40, 724, 56), fill=rgb("#5b2a17"))
    d.rectangle(box(546, 70, 714, 80), fill=rgb("#7a3b22"))
    d.rectangle(box(612, 56, 648, 70), fill=rgb("#d9c08a"))
    d.line(box(618, 63, 642, 63), fill=rgb("#5b2a17"), width=s(2))
    lantern(img, 540, 150)
    lantern(img, 720, 150)
    log(img, 505, 236, 535, 330)
    log(img, 725, 236, 755, 330)
    log(img, 505, 300, 755, 344)
    stump(img, 590, 262, 16)
    stump(img, 670, 262, 16)
    book(img, 560, 318, "#2f5f7a")
    bush(img, 470, 95, 26, berries="#d23c4b")
    bush(img, 790, 95, 26)
    flowers(img, 480, 280, 20, 12)
    flowers(img, 780, 280, 20, 12)

    # Meeting: a long plank table on ring stools; a notice board at the north hedge.
    table_slab(img, 978, 190, 1112, 425)
    for p in [(1045, 232), (1045, 302), (1045, 372)]:
        book(img, *p, rng.choice(["#7a2f2f", "#2f5f7a", "#5a7a2f"]))
    for p in CHAIRS:
        stump(img, p[0], p[1] + 6, 13)
    soft_shadow(img, 1043, 104, 60, 6, 110)
    d = ImageDraw.Draw(img)
    d.rectangle(box(985, 52, 1100, 96), fill=rgb("#6b4523"))
    d.rectangle(box(990, 57, 1095, 91), fill=rgb("#b8915c"))
    for i, c in enumerate(["#f3ead2", "#f7d8a8", "#e9f0d8", "#f3ead2"]):
        d.rectangle(box(996 + i * 25, 61 + (i % 2) * 5, 1014 + i * 25, 82 + (i % 2) * 5), fill=rgb(c))
    pine(img, 1185, 80, 26)
    tree(img, 880, 75, 20, "#356f2c")
    bush(img, 1192, 430, 16, berries="#3c4bd2")

    # Open plan: a flower bed for the planter shelf, a beehive for the copier, stump desks.
    soft_shadow(img, 560, 535, 160, 10, 110)
    d = ImageDraw.Draw(img)
    d.rounded_rectangle(box(402, 488, 712, 536), radius=s(8), fill=rgb("#6b4523"))
    d.rounded_rectangle(box(408, 492, 706, 530), radius=s(6), fill=rgb("#4a3420"))
    for x in range(420, 700, 26):
        bush(img, x, 508, 14, "#3f8340")
        flowers(img, x, 505, 12, 5)
    soft_shadow(img, 765, 530, 30, 7, 110)
    d = ImageDraw.Draw(img)
    for i, w in enumerate([46, 40, 34, 26]):
        y = 518 - i * 12
        d.rounded_rectangle(box(765 - w / 2, y - 12, 765 + w / 2, y), radius=s(5), fill=shade(rgb("#e0a83a"), 1 - i * 0.05))
        d.line(box(765 - w / 2 + 3, y - 1, 765 + w / 2 - 3, y - 1), fill=rgb("#a8741f"), width=s(1.5))
    d.ellipse(box(758, 509, 772, 517), fill=rgb("#3b2410"))
    for _ in range(5):
        bx, by = 765 + rng.uniform(-30, 30), 470 + rng.uniform(-14, 14)
        d.ellipse(box(bx - 2, by - 1.5, bx + 2, by + 1.5), fill=rgb("#f2c230"))
    for x, y in DESKS:
        stump(img, x, y - 4, 14, 0.3)
        table_slab(img, x - 48, y + 14, x + 48, y + 64)
        book(img, x - 22, y + 36, rng.choice(["#7a2f2f", "#2f5f7a", "#5a7a2f"]))
        lantern(img, x + 26, y + 46)
    for p in [(470, 718), (680, 885), (610, 1050)]:
        mushrooms(img, *p, 2)
    flowers(img, 310, 1140, 18, 10)
    flowers(img, 852, 640, 14, 8)
    rock(img, 308, 640, 12)
    rock(img, 852, 1010, 10)

    # Kitchen: a cooking corner, a log counter with stools, a picnic table.
    soft_shadow(img, 150, 815, 105, 8, 120)
    d = ImageDraw.Draw(img)
    d.rectangle(box(48, 725, 255, 815), fill=rgb("#6e7068"))
    for y in range(728, 812, 14):
        for x in range(50 + (y // 14 % 2) * 10, 252, 22):
            d.rounded_rectangle(box(x, y, x + 19, y + 11), radius=s(3), fill=shade(rgb("#8e9087"), rng.uniform(0.85, 1.1)))
    glow(img, 95, 772, 30, (255, 140, 60), 120)
    d = ImageDraw.Draw(img)
    d.ellipse(box(72, 752, 118, 792), fill=rgb("#2d2d2d"))
    d.ellipse(box(78, 756, 112, 784), fill=rgb("#4a3a2a"))
    d.ellipse(box(84, 762, 106, 778), fill=rgb("#d08a3a"))
    for x, c in [(150, "#c8402f"), (180, "#e0a83a"), (210, "#5a7a2f")]:
        d.rounded_rectangle(box(x - 12, 760, x + 12, 790), radius=s(3), fill=rgb("#8a5c30"))
        for _ in range(6):
            fx, fy = x + rng.uniform(-8, 8), 768 + rng.uniform(-4, 8)
            d.ellipse(box(fx - 3, fy - 3, fx + 3, fy + 3), fill=rgb(c))
    bush(img, 225, 845, 14, berries="#7a2fd2")
    log(img, 85, 878, 252, 916, moss=False)
    for x in (120, 168, 217):
        stump(img, x, 945, 12, 0.3)
    table_slab(img, 140, 1055, 262, 1118)
    for p in [(155, 1037), (245, 1037), (155, 1137), (245, 1137)]:
        stump(img, p[0], p[1] + 4, 12, 0.3)
    book(img, 190, 1080, "#c8402f")
    d = ImageDraw.Draw(img)
    d.ellipse(box(222, 1072, 238, 1088), fill=rgb("#f3ead2"))
    d.ellipse(box(226, 1076, 234, 1084), fill=rgb("#d08a3a"))
    lantern(img, 110, 1120)

    # Manager: a ranger's deck; desk, a log bench, a stump table, a sign.
    table_slab(img, 985, 655, 1115, 718)
    stump(img, 1048, 636, 15, 0.3)
    d = ImageDraw.Draw(img)
    d.rectangle(box(1005, 668, 1045, 700), fill=rgb("#e8dcb5"))
    d.line(box(1010, 690, 1022, 676), fill=rgb("#6b8a3a"), width=s(2))
    d.line(box(1022, 676, 1038, 684), fill=rgb("#3a6b8a"), width=s(2))
    lantern(img, 1090, 690)
    log(img, 972, 765, 1006, 852)
    stump(img, 1065, 792, 22, 0.35)
    book(img, 1065, 774, "#2f5f7a")
    bush(img, 935, 560, 22, "#3f8340")
    bush(img, 1188, 562, 18, berries="#d23c4b")
    pine(img, 1180, 836, 22)
    d = ImageDraw.Draw(img)
    d.rectangle(box(1010, 530, 1090, 560), fill=rgb("#6b4523"))
    d.rectangle(box(1014, 534, 1086, 556), fill=rgb("#d9c08a"))
    for i in range(4):
        d.line(box(1020 + i * 17, 540, 1030 + i * 17, 550), fill=rgb("#4f8a3a"), width=s(2))

    # Spring: a hot spring and a tea table, cushion stones at the seats.
    water(img, (1110, 920, 1205, 1000), hot=True)
    d = ImageDraw.Draw(img)
    for _ in range(6):
        x, y = rng.uniform(1125, 1190), rng.uniform(930, 985)
        d.arc(box(x - 6, y - 10, x + 6, y), 200, 340, fill=(255, 255, 255), width=s(1))
    for a in range(0, 360, 30):
        r = math.radians(a)
        rock(img, 1157 + math.cos(r) * 52, 960 + math.sin(r) * 44, 8)
    stump(img, 1060, 1080, 24, 0.35)
    d = ImageDraw.Draw(img)
    for x in (1050, 1070):
        d.ellipse(box(x - 5, 1053, x + 5, 1061), fill=rgb("#f3ead2"))
        d.ellipse(box(x - 3, 1055, x + 3, 1059), fill=rgb("#7a9a3a"))
    for p in [(1105, 1042), (1015, 1132), (1108, 1134)]:
        rock(img, p[0], p[1] + 6, 13, "#9a9688")
    tree(img, 958, 932, 24, "#4f7a2f")
    bush(img, 1192, 1140, 16, berries="#f78fb3")
    lantern(img, 940, 1060)

    # Hedgerows where the walls were, the doors left open.
    walls = [
        (22, 18, 40, 1162), (1215, 18, 1232, 1162), (22, 18, 392, 35), (845, 18, 1232, 35),
        (378, 35, 392, 282), (40, 330, 378, 345), (40, 530, 268, 545), (40, 700, 268, 715),
        (258, 345, 272, 470), (258, 545, 272, 715), (276, 772, 290, 1162),
        (845, 35, 858, 345), (845, 430, 858, 495), (858, 495, 1215, 508), (872, 508, 890, 768),
        (872, 856, 890, 1080), (890, 872, 1215, 890), (890, 508, 1215, 520), (900, 890, 920, 1078),
        (22, 1155, 1232, 1170), (392, 18, 556, 35), (704, 18, 845, 35),
    ]
    for w in walls:
        hedge(img, *w)
    for p in [(366, 352), (866, 352), (866, 860), (905, 1088)]:
        lantern(img, *p)

    # A stream where the windows were; forest round everything.
    water(img, (40, 1180, 1215, 1234), ellipse=False)
    for x in range(60, 1210, 60):
        reeds(img, x + rng.uniform(-10, 10), 1188)
        rock(img, x + 25, 1230, 6)
    for x in range(0, SIZE + 40, 46):
        if abs(x - 630) > 40:
            tree(img, x + rng.uniform(-8, 8), rng.uniform(-6, 8), rng.uniform(22, 30), rng.choice(["#24502a", "#2a5a2c", "#1f4a26"]))
        tree(img, x + rng.uniform(-8, 8), SIZE - rng.uniform(-4, 8), rng.uniform(16, 24), rng.choice(["#24502a", "#2a5a2c"]))
    for y in range(20, SIZE, 46):
        tree(img, rng.uniform(-6, 10), y, rng.uniform(16, 22), rng.choice(["#24502a", "#2a5a2c", "#1f4a26"]))
        tree(img, SIZE - rng.uniform(-6, 10), y, rng.uniform(16, 22), rng.choice(["#24502a", "#2a5a2c"]))

    img.resize((SIZE, SIZE), Image.LANCZOS).save(out, optimize=True)
    print("wrote", out)


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "assets/forest.png")
