"""Draw the Edge Touch app icon (blue rounded square, white ring and dot)
and write assets/AppIcon.icns. Run: python3 scripts/make_icon.py"""
import os, subprocess, tempfile
from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def icon(size):
    s = 4  # draw big, then shrink for smooth edges
    n = size * s
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    # macOS icon grid: the shape fills ~80% of the canvas.
    pad = round(n * 0.1)
    box = (pad, pad, n - pad, n - pad)
    # Vertical blue gradient inside a rounded square.
    grad = Image.new("RGBA", (n, n))
    top, bottom = (88, 170, 255), (20, 98, 230)
    gd = ImageDraw.Draw(grad)
    for y in range(n):
        t = y / (n - 1)
        gd.line([(0, y), (n, y)], fill=tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)) + (255,))
    mask = Image.new("L", (n, n), 0)
    ImageDraw.Draw(mask).rounded_rectangle(box, radius=round((n - 2 * pad) * 0.225), fill=255)
    img.paste(grad, (0, 0), mask)
    d = ImageDraw.Draw(img)
    c, w = n / 2, n - 2 * pad
    r, stroke, dot = w * 0.30, w * 0.065, w * 0.115
    d.ellipse((c - r, c - r, c + r, c + r), outline=(255, 255, 255, 255), width=round(stroke))
    d.ellipse((c - dot, c - dot, c + dot, c + dot), fill=(255, 255, 255, 255))
    return img.resize((size, size), Image.LANCZOS)


with tempfile.TemporaryDirectory() as tmp:
    iconset = os.path.join(tmp, "AppIcon.iconset")
    os.makedirs(iconset)
    for base in (16, 32, 128, 256, 512):
        icon(base).save(os.path.join(iconset, f"icon_{base}x{base}.png"))
        icon(base * 2).save(os.path.join(iconset, f"icon_{base}x{base}@2x.png"))
    out = os.path.join(ROOT, "assets", "AppIcon.icns")
    subprocess.run(["iconutil", "-c", "icns", iconset, "-o", out], check=True)
    icon(512).save(os.path.join(ROOT, "assets", "icon-512.png"))
    print("wrote", out)
