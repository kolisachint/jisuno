#!/usr/bin/env python3
"""Placeholder macOS app icon for Jisuno.

Draws a 1024x1024 master at 4x supersampling (4096, then LANCZOS downscale):
dark squircle body with a vertical gradient, a 7-bar waveform (dictate
accent, center bar in the command accent), and a faint top inner edge.
Writes out/icon.iconset/ PNGs and converts to packaging/AppIcon.icns
via `iconutil`. All paths are relative to this script's directory.

Tokens from docs/design/app-ui.md section 1 (dark surface, accents):
  surface #1c1c1e, dictate #3a85f7, command #e39b17.
"""

from pathlib import Path
import subprocess

from PIL import Image, ImageChops, ImageDraw

SIZE = 1024
SS = 4
BIG = SIZE * SS

SURFACE = (28, 28, 30)  # #1c1c1e
TOP = (46, 46, 50)  # surface, slightly lighter
BOTTOM = (15, 15, 18)  # surface, darker
DICTATE = (58, 133, 247)  # #3a85f7
COMMAND = (227, 155, 23)  # #e39b17

INSET = 100  # px at 1024
BODY = 824  # px at 1024
RADIUS = 185  # px at 1024
BAR_W = 56  # px at 1024
BAR_GAP = 34  # px at 1024
BAR_H = [180, 320, 460, 560, 460, 320, 180]

ICONSET = [
    (16, "icon_16x16.png"),
    (32, "icon_16x16@2x.png"),
    (32, "icon_32x32.png"),
    (64, "icon_32x32@2x.png"),
    (128, "icon_128x128.png"),
    (256, "icon_128x128@2x.png"),
    (256, "icon_256x256.png"),
    (512, "icon_256x256@2x.png"),
    (512, "icon_512x512.png"),
    (1024, "icon_512x512@2x.png"),
]


def vertical_gradient(w, h, y0, y1, top, bottom):
    """RGB image with a top->bottom gradient mapped over rows y0..y1."""
    strip = Image.new("RGB", (1, h))
    px = strip.load()
    span = max(1, y1 - y0)
    for y in range(h):
        t = min(1.0, max(0.0, (y - y0) / span))
        px[0, y] = tuple(int(a + (b - a) * t) for a, b in zip(top, bottom))
    return strip.resize((w, h))


def top_fade(w, h, full_below, zero_at):
    """L mask: 255 above full_below, fading to 0 at zero_at."""
    strip = Image.new("L", (1, h))
    px = strip.load()
    for y in range(h):
        if y <= full_below:
            px[0, y] = 255
        elif y >= zero_at:
            px[0, y] = 0
        else:
            px[0, y] = int(255 * (zero_at - y) / (zero_at - full_below))
    return strip.resize((w, h))


def main():
    here = Path(__file__).resolve().parent
    out = here / "out" / "icon.iconset"
    out.mkdir(parents=True, exist_ok=True)

    inset, body, radius = INSET * SS, BODY * SS, RADIUS * SS
    x0, y0 = inset, inset
    x1, y1 = inset + body, inset + body

    mask = Image.new("L", (BIG, BIG), 0)
    ImageDraw.Draw(mask).rounded_rectangle([x0, y0, x1, y1], radius=radius, fill=255)

    grad = vertical_gradient(BIG, BIG, y0, y1, TOP, BOTTOM)
    canvas = Image.new("RGBA", (BIG, BIG), (0, 0, 0, 0))
    canvas.paste(grad, (0, 0), mask)

    # Centered 7-bar waveform.
    bw, gap = BAR_W * SS, BAR_GAP * SS
    heights = [h * SS for h in BAR_H]
    total = len(heights) * bw + (len(heights) - 1) * gap
    x = (BIG - total) // 2
    cy = BIG // 2
    d = ImageDraw.Draw(canvas)
    for i, bh in enumerate(heights):
        x1b = x + bw
        yb0, yb1 = cy - bh // 2, cy + bh // 2
        color = COMMAND if i == len(heights) // 2 else DICTATE
        d.rounded_rectangle([x, yb0, x1b, yb1], radius=bw // 2, fill=color + (255,))
        x = x1b + gap

    # Faint 1 px (at 1024) lighter top inner edge, faded out below the top.
    edge = 4 * SS  # inset just inside the body border
    hl = Image.new("RGBA", (BIG, BIG), (0, 0, 0, 0))
    ImageDraw.Draw(hl).rounded_rectangle(
        [x0 + edge, y0 + edge, x1 - edge, y1 - edge],
        radius=radius - edge,
        outline=(255, 255, 255, 42),
        width=1 * SS,
    )
    fade = top_fade(BIG, BIG, y0 + 120 * SS, y0 + 380 * SS)
    alpha = ImageChops.multiply(hl.split()[3], fade)
    hl.putalpha(alpha)
    canvas = Image.alpha_composite(canvas, hl)

    master = canvas.resize((SIZE, SIZE), Image.LANCZOS)
    for px, name in ICONSET:
        master.resize((px, px), Image.LANCZOS).save(out / name)

    icns = (here / ".." / "AppIcon.icns").resolve()
    subprocess.run(["iconutil", "-c", "icns", str(out), "-o", str(icns)], check=True)
    print(f"wrote {icns}")


if __name__ == "__main__":
    main()
