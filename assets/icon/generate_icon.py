#!/usr/bin/env python3
"""Generates assets/icon/AppIcon.png (1024x1024), the source for AppIcon.icns.

Draws a small terminal-window glyph (traffic lights + a ">_" prompt) on a
dark squircle, in Coast's built-in Campbell theme colors, using Menlo (the
app's default font) for the glyph.
"""

from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

SIZE = 1024
OUT = Path(__file__).parent / "AppIcon.png"

BG = (21, 23, 25, 255)  # outer squircle
SCREEN = (12, 12, 12, 255)  # Campbell background
CYAN = (58, 150, 221, 255)  # Campbell bright cyan
RED = (196, 15, 31, 255)
YELLOW = (193, 156, 0, 255)
GREEN = (19, 161, 14, 255)


def rounded_rect(draw, box, radius, fill):
    draw.rounded_rectangle(box, radius=radius, fill=fill)


def main():
    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    rounded_rect(draw, (0, 0, SIZE, SIZE), radius=224, fill=BG)

    inset = 96
    screen_box = (inset, inset, SIZE - inset, SIZE - inset)
    rounded_rect(draw, screen_box, radius=140, fill=SCREEN)

    dot_r = 20
    dot_y = inset + 76
    for i, color in enumerate((RED, YELLOW, GREEN)):
        cx = inset + 76 + i * 66
        draw.ellipse((cx - dot_r, dot_y - dot_r, cx + dot_r, dot_y + dot_r), fill=color)

    font = ImageFont.truetype("/System/Library/Fonts/Menlo.ttc", 340, index=1)  # bold face
    text = ">_"
    bbox = draw.textbbox((0, 0), text, font=font)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    tx = inset + (SIZE - 2 * inset - tw) / 2 - bbox[0]
    ty = inset + (SIZE - 2 * inset - th) / 2 - bbox[1] + 40
    draw.text((tx, ty), text, font=font, fill=CYAN)

    img.save(OUT)
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
