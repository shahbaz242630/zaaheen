"""Rebuild zaaheen.com's icon set from the app icon.

Source: public/icon-512.png, byte-identical to crates/vault-tauri/icons/icon.png.
If the app icon changes, copy it over icon-512.png and re-run this script:

    python scripts/make-favicons.py      (from site/, needs Pillow)

Outputs, all in public/:
  favicon.ico        16/32/48, cropped tight to the disc (a tab icon has no room
                     for the app icon's padding)
  favicon-192.png    tight, for Google Search results: Google does not accept SVG
                     favicons and wants a multiple of 48px, ideally above 48
  apple-touch-icon   180, opaque paper ground (iOS paints transparency black)
  icon-192.png       app-icon framing, for manifest.webmanifest
favicon.svg is hand-written vector geometry (the same as components/Logo.astro).
"""
import os
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
PUBLIC = os.path.normpath(os.path.join(HERE, "..", "public"))
PAPER = (0xFA, 0xF8, 0xF3, 255)

source = Image.open(os.path.join(PUBLIC, "icon-512.png")).convert("RGBA")
bbox = source.getchannel("A").getbbox()
disc = source.crop(bbox)
side = max(disc.size)
tight = Image.new("RGBA", (side, side), (0, 0, 0, 0))
tight.paste(disc, ((side - disc.width) // 2, (side - disc.height) // 2))


def out(name):
    return os.path.join(PUBLIC, name)


frames = {s: tight.resize((s, s), Image.LANCZOS) for s in (16, 32, 48)}
frames[48].save(out("favicon.ico"), format="ICO", sizes=[(16, 16), (32, 32), (48, 48)],
                append_images=[frames[16], frames[32]])

# Prove the ICO holds exactly the frames we meant to write.
check = Image.open(out("favicon.ico"))
assert sorted(check.info["sizes"]) == [(16, 16), (32, 32), (48, 48)], check.info["sizes"]
for s, frame in frames.items():
    check.size = (s, s)
    assert check.convert("RGBA").tobytes() == frame.tobytes(), f"ico frame {s} differs"

tight.resize((192, 192), Image.LANCZOS).save(out("favicon-192.png"), optimize=True)

ground = Image.new("RGBA", source.size, PAPER)
ground.alpha_composite(source)
ground.convert("RGB").resize((180, 180), Image.LANCZOS).save(out("apple-touch-icon.png"), optimize=True)

source.resize((192, 192), Image.LANCZOS).save(out("icon-192.png"), optimize=True)

for name in ("favicon.ico", "favicon-192.png", "apple-touch-icon.png", "icon-192.png"):
    print(f"{name:22} {os.path.getsize(out(name)):>6} bytes  {Image.open(out(name)).size}")
