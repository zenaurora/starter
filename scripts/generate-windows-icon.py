"""Wrap the shared 64px PNG as a Windows icon (Vista and newer)."""
import struct
from pathlib import Path

icons = Path(__file__).resolve().parent.parent / "resources" / "icons"
png = (icons / "starter-64.png").read_bytes()
header = struct.pack("<HHH", 0, 1, 1)
entry = struct.pack("<BBBBHHII", 64, 64, 0, 0, 1, 32, len(png), 22)
(icons / "Starter.ico").write_bytes(header + entry + png)
