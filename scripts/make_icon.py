#!/usr/bin/env python3
"""Render the SVG master on Linux/macOS; no Apple tools or Rust build needed."""

import argparse
import io
from pathlib import Path
import re
import sys
import tomllib

try:
    import cairosvg
    from PIL import Image
except ImportError as error:
    sys.exit(f"{error}. Install scripts/icon-requirements.txt (and system Cairo).")

ROOT = Path(__file__).resolve().parents[1]
MASTER = ROOT / "design/icons/app.lyra.player.svg"
SIZES = (16, 24, 32, 48, 64, 96, 128, 256, 512, 1024)
ICNS = ROOT / "app/Resources/AppIcon.icns"


def validate_image(image, size):
    assert image.mode == "RGBA", f"{size}: missing RGBA channels"
    assert image.size == (size, size), f"{size}: incorrect dimensions"
    alpha = image.getchannel("A")
    assert alpha.getextrema() == (0, 255), f"{size}: missing transparency/opaque body"
    for point in ((0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)):
        assert alpha.getpixel(point) == 0, f"{size}: corner is not transparent"
    assert alpha.getpixel((size // 2, size // 2)) == 255


def generate():
    svg = MASTER.read_bytes()
    tokens = tomllib.loads((ROOT / "design/tokens.toml").read_text())
    palette = set(tokens["lavender"]["dark"].values())
    colors = set(re.findall(r"#[0-9A-Fa-f]{6}\b", svg.decode()))
    if not colors <= palette:
        sys.exit(f"SVG colors outside lavender.dark tokens: {sorted(colors - palette)}")

    # One supersampled render, then premultiplied-alpha Lanczos reduction in Pillow.
    # SVG contains no fonts, external resources, filters, or platform-specific art.
    rendered = cairosvg.svg2png(bytestring=svg, output_width=2048, output_height=2048)
    with Image.open(io.BytesIO(rendered)) as source:
        source = source.convert("RGBA")
        images = {
            size: source.resize((size, size), Image.Resampling.LANCZOS)
            for size in SIZES
        }
    # A vertical stroked path has a zero-width object bounding box; an SVG
    # gradient without user-space coordinates can silently hide this string.
    red, green, blue, alpha = images[1024].getpixel((512, 512))
    assert green > red and green > blue and alpha == 255, "Missing mint center string"
    outputs = {}
    for size, image in images.items():
        validate_image(image, size)
        buffer = io.BytesIO()
        image.save(buffer, format="PNG", optimize=False, compress_level=9)
        outputs[MASTER.parent / f"{size}x{size}/app.lyra.player.png"] = buffer.getvalue()

    buffer = io.BytesIO()
    images[1024].save(
        buffer, format="ICNS", append_images=[images[s] for s in (32, 64, 128, 256, 512)]
    )
    data = buffer.getvalue()
    # Decode every representation, including Retina entries, before publishing.
    with Image.open(io.BytesIO(data)) as icon:
        expected = {(16, 16, 2), (32, 32, 2), (128, 128, 1), (128, 128, 2),
                    (256, 256, 1), (256, 256, 2), (512, 512, 1), (512, 512, 2)}
        assert set(icon.info["sizes"]) == expected, "Unexpected ICNS representations"
        for representation in icon.info["sizes"]:
            size = representation[0] * representation[2]
            validate_image(icon.icns.getimage(representation).convert("RGBA"), size)
    outputs[ICNS] = data
    return outputs


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="validate and compare without writing")
    args = parser.parse_args()
    outputs = generate()
    stale = []
    for path, data in outputs.items():
        if args.check:
            if not path.exists() or path.read_bytes() != data:
                stale.append(str(path.relative_to(ROOT)))
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
    if stale:
        sys.exit("Stale icon assets (run make icons):\n" + "\n".join(stale))
    print(f"{'Verified' if args.check else 'Generated'} {len(outputs)} icon assets; "
          "RGBA, transparent corners, all ICNS representations valid.")


if __name__ == "__main__":
    main()
