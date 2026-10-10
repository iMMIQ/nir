"""Verify a distributed native Linux game with isolated save data."""
import os
from pathlib import Path
import sys
from verify_native import verify_native

assert sys.platform.startswith("linux"), "Run native acceptance on Linux"
root = Path(sys.argv[1]).resolve()
assert os.access(root / "Game", os.X_OK), "Game must keep its executable bit"
verify_native(root, "linux", "Game", "vulkan")
