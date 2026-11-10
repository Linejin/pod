"""Measure wide glyph placement in a terminal capture (output of cjk-compare.ps1).

    python -I tools/visual/cjk-measure.py target/cjk/pod.png [--box x0,y0,x1,y1]

Finds the text lines of the `editor-terminal-cjk` sample inside --box (physical px,
default: whole image), takes the cell pitch and origin from the ASCII ruler line
(first line, 40 digits) and prints, per sample line, every glyph's ink box relative
to its cell slot: width as a fraction of the slot and left/right margins in px.
Sample lines and column layout are those of CJK_OUTPUT in fixture_scenes.rs.
"""
import io
import sys
import unicodedata

import numpy as np
from PIL import Image

LINES = [
    "0123456789012345678901234567890123456789",
    "ひらがな あいうえお かきくけこ さしすせそ",
    "カタカナ アイウエオ カキクケコ サシスセソ",
    "半角 ｶﾀｶﾅ ｱｲｳｴｵ ｶｷｸｹｺ ﾊﾟﾋﾟ",
    "漢字 日本語 東京 中文 简体字 繁體字",
    "한글 가나다라마바사 문서 확인",
    "emoji \U0001F600\U0001F44D\U0001F389 end",
]


def width(c):
    if 0x1F300 <= ord(c) <= 0x1FAFF:
        return 2
    return 2 if unicodedata.east_asian_width(c) in "WF" else 1


def cells(line):
    col = 0
    out = []
    for c in line:
        w = width(c)
        if c == "ﾟ":  # half-width semi-voiced mark: its own cell
            w = 1
        out.append((c, col, w))
        col += w
    return out


def main():
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")
    path = sys.argv[1]
    img = np.asarray(Image.open(path).convert("RGB")).astype(int)
    if "--box" in sys.argv:
        x0, y0, x1, y1 = map(int, sys.argv[sys.argv.index("--box") + 1].split(","))
        img = img[y0:y1, x0:x1]
    else:
        x0 = y0 = 0
    # Background = most common color; ink = clearly different from it.
    flat = img.reshape(-1, 3)
    vals, counts = np.unique(flat, axis=0, return_counts=True)
    bg = vals[counts.argmax()]
    ink = np.abs(img - bg).max(axis=2) > 60
    rows = ink.any(axis=1)
    # Text lines = runs of ink rows (gap of >=2 blank rows separates lines).
    lines = []
    y = 0
    while y < len(rows):
        if rows[y]:
            s = y
            gap = 0
            while y < len(rows) and gap < 2:
                gap = 0 if rows[y] else gap + 1
                y += 1
            lines.append((s, y - gap))
        else:
            y += 1
    # The ruler: first line whose ink has 40 separate column runs.
    def runs(mask):
        out, x = [], 0
        while x < len(mask):
            if mask[x]:
                s = x
                while x < len(mask) and mask[x]:
                    x += 1
                out.append((s, x))
            else:
                x += 1
        return out

    # The ruler is the first line (40 digits): pitch from its outer digits
    # (neighbouring digits may touch, so runs are not counted).
    ruler = 0
    r = runs(ink[lines[0][0]:lines[0][1]].any(axis=0))
    if len(r) < 20:
        sys.exit("ruler line (40 digits) not found; pass --box around the terminal text")
    first, last = (r[0][0] + r[0][1]) / 2, (r[-1][0] + r[-1][1]) / 2
    centers = np.array([first, last])
    pitch = (last - first) / 39
    origin = centers[0] - pitch / 2
    print(f"{path}: cell pitch {pitch:.3f} px, origin x {origin + x0:.1f}, line height ~{lines[ruler + 1][0] - lines[ruler][0]} px")
    for li, text in enumerate(LINES[1:], start=1):
        if ruler + li >= len(lines):
            break
        a, b = lines[ruler + li]
        band = ink[a - 4:b + 4]
        print(f"\n{text}")
        for c, col, w in cells(text):
            if c == " ":
                continue
            sx0 = origin + col * pitch
            sx1 = sx0 + w * pitch
            lo, hi = int(round(sx0)), int(round(sx1))
            cols = band[:, lo:hi].any(axis=0)
            ys = band[:, lo:hi].any(axis=1)
            if not cols.any():
                print(f"  {c} col {col:2}: no ink")
                continue
            xs = np.nonzero(cols)[0]
            yy = np.nonzero(ys)[0]
            iw = xs[-1] - xs[0] + 1
            print(
                f"  {c} col {col:2} w{w}: ink {iw:3d}px = {iw / (sx1 - sx0):.2f} slot, "
                f"left {xs[0]:2d} right {hi - lo - 1 - xs[-1]:2d}, height {yy[-1] - yy[0] + 1}px"
            )


if __name__ == "__main__":
    main()
