"""Generates img/app.ico and img/app-icon.svg (green dot on a dark square).

Usage: python tools/make_icon.py   (requires Pillow)
"""
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
IMG = ROOT / "img"
BG = (0x1C, 0x1C, 0x1C, 255)
RING = (0x16, 0xA3, 0x4A, 255)
DOT = (0x22, 0xC5, 0x5E, 255)
BASE = 1024


def draw_base() -> Image.Image:
    img = Image.new("RGBA", (BASE, BASE), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle((32, 32, BASE - 32, BASE - 32), radius=200, fill=BG)
    c = BASE // 2
    d.ellipse((c - 330, c - 330, c + 330, c + 330), fill=RING)
    d.ellipse((c - 270, c - 270, c + 270, c + 270), fill=DOT)
    return img


def main() -> None:
    IMG.mkdir(exist_ok=True)
    base = draw_base()
    sizes = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
    base.save(IMG / "app.ico", format="ICO", sizes=sizes)
    svg = (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">\n'
        '  <rect x="32" y="32" width="960" height="960" rx="200" fill="#1C1C1C"/>\n'
        '  <circle cx="512" cy="512" r="330" fill="#16A34A"/>\n'
        '  <circle cx="512" cy="512" r="270" fill="#22C55E"/>\n'
        "</svg>\n"
    )
    (IMG / "app-icon.svg").write_text(svg, encoding="utf-8")
    print("Wrote img/app.ico and img/app-icon.svg")


if __name__ == "__main__":
    main()
