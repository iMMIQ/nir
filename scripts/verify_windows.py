"""Verify a distributed native Windows game with isolated save data."""
import os
from pathlib import Path
import sys
from verify_native import verify_native

assert os.name == "nt", "Run native acceptance on Windows"
verify_native(Path(sys.argv[1]).resolve(), "windows", "Game.exe", "dx12")
