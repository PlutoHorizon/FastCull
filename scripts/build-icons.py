"""Convert the approved RGBA artwork into native application icon formats.

Requires Pillow. Run from any directory; outputs stay in assets/.
"""
from pathlib import Path

from PIL import Image


def main():
    assets = Path(__file__).resolve().parents[1] / "assets"
    with Image.open(assets / "fastcull-master.png") as source:
        source.load()
        if source.width != source.height or source.mode != "RGBA":
            raise ValueError("Icon master must be a square RGBA PNG")
        image = source.resize((1024, 1024), Image.Resampling.LANCZOS)
    image.resize((256, 256), Image.Resampling.LANCZOS).save(assets / "fastcull.png")
    image.save(assets / "fastcull.ico", sizes=[(n, n) for n in (16, 20, 24, 32, 40, 48, 64, 128, 256)])
    image.save(assets / "fastcull.icns")
    print("Built assets/fastcull.png, fastcull.ico, fastcull.icns")


if __name__ == "__main__":
    main()
