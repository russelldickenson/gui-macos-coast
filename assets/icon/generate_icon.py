#!/usr/bin/env python3
"""Generates assets/icon/AppIcon.png (1024x1024), the source for AppIcon.icns.

Extracts the squircle from the 1408x768 mockup `_aa_coast_icon.jpeg` at the
repository root: crops a square around it, keys out the beige backdrop into
transparency, converts the mockup's brown drop shadow into a dark one, and
scales the result to 1024x1024. Requires Pillow.
"""

from pathlib import Path
from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "_aa_coast_icon.jpeg"
OUT = Path(__file__).parent / "AppIcon.png"

SIZE = 1024
CROP = 600  # square crop side in source pixels
CX, CY = 703, 390  # squircle center in the source image
BAND = 24  # border band sampled to fit the beige background plane


def solve3(a, b):
    """Solve a 3x3 linear system by Gaussian elimination."""
    m = [list(a[i]) + [b[i]] for i in range(3)]
    for i in range(3):
        p = max(range(i, 3), key=lambda r: abs(m[r][i]))
        m[i], m[p] = m[p], m[i]
        d = m[i][i]
        m[i] = [v / d for v in m[i]]
        for r in range(3):
            if r != i:
                f = m[r][i]
                m[r] = [m[r][k] - f * m[i][k] for k in range(4)]
    return [m[i][3] for i in range(3)]


def fit_background(crop):
    """Fit a per-channel plane to the beige border around the icon."""
    px = crop.load()
    samples = [
        (x, y, px[x, y])
        for y in range(CROP)
        for x in range(CROP)
        if x < BAND or x >= CROP - BAND or y < BAND or y >= CROP - BAND
    ]
    coeffs = []
    for ch in range(3):
        a = [[0.0] * 3 for _ in range(3)]
        b = [0.0] * 3
        for x, y, c in samples:
            f = (1.0, float(x), float(y))
            for i in range(3):
                b[i] += f[i] * c[ch]
                for j in range(3):
                    a[i][j] += f[i] * f[j]
        coeffs.append(solve3(a, b))
    return coeffs


def clamp01(v):
    return 0.0 if v < 0 else (1.0 if v > 1 else v)


def main():
    src = Image.open(SRC).convert("RGB")
    crop = src.crop((CX - CROP // 2, CY - CROP // 2,
                     CX + CROP // 2, CY + CROP // 2)).convert("RGB")
    cp = crop.load()
    coeffs = fit_background(crop)

    out = Image.new("RGBA", (CROP, CROP))
    op = out.load()
    for y in range(CROP):
        by = [c[0] + c[2] * y for c in coeffs]
        for x in range(CROP):
            bg = [by[i] + coeffs[i][1] * x for i in range(3)]
            r, g, b = cp[x, y]
            dist = sum((cp[x, y][i] - bg[i]) ** 2 for i in range(3)) ** 0.5

            # The beige backdrop is close to the fitted plane; the squircle
            # and cyan glyph are far from it and become fully opaque.
            alpha = clamp01((dist - 12.0) / 40.0)

            # The mockup's drop shadow is warm brown (r > b), unlike the
            # cool squircle and glyph. Turn it into a soft dark shadow.
            if r - b > 12:
                alpha *= clamp01((dist - 40.0) / 120.0)
                r = g = b = 24

            alpha = alpha * alpha * (3 - 2 * alpha)  # smoothstep
            op[x, y] = (r, g, b, int(alpha * 255 + 0.5))

    out.resize((SIZE, SIZE), Image.LANCZOS).save(OUT)
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
