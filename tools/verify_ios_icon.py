"""Check branding provenance and iOS icon PNG sizes before invoking actool."""
import hashlib
import json
import struct
from decimal import Decimal
from pathlib import Path

root = Path(__file__).resolve().parent.parent
icons = root / "ios/Assets.xcassets/AppIcon.appiconset"
source = json.loads((root / "ios/app-icon-source.json").read_text(encoding="utf-8"))
assert hashlib.sha256((root / source["source"]).read_bytes()).hexdigest() == source["sha256"], (
    "Launcher artwork changed: run tools/make-ios-icon.ps1 to refresh the iOS icons"
)
contents = json.loads((icons / "Contents.json").read_text(encoding="utf-8"))
slots = set()
for entry in contents["images"]:
    name = entry["filename"]
    assert Path(name).name == name
    data = (icons / name).read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    assert data[12:16] == b"IHDR"
    w, h, bits, color = struct.unpack_from(">IIBB", data, 16)
    size = int(Decimal(entry["size"].split("x")[0]) * Decimal(entry["scale"][:-1]))
    assert (w, h) == (size, size), name
    assert bits == 8 and color == 2, f"Expected opaque RGB icon: {name}"
    slots.add((entry["idiom"], entry["size"], entry["scale"]))
assert len(slots) == 18
for needed in [("iphone", "60x60", "2x"), ("iphone", "60x60", "3x"),
               ("ipad", "76x76", "2x"), ("ipad", "83.5x83.5", "2x"),
               ("ios-marketing", "1024x1024", "1x")]:
    assert needed in slots, needed
print("Launcher artwork matches; 18 iPhone/iPad icon slots have valid RGB PNG sizes.")
