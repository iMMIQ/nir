#!/usr/bin/env python3
"""Check the public Git file set and optional release archives without printing secrets."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile

PATTERNS = {
    "private_key": rb"-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----",
    "access_token": rb"(?:gh[pousr]_[A-Za-z0-9_]{30,}|github_pat_[A-Za-z0-9_]{50,}|AKIA[A-Z0-9]{16}|sk-[A-Za-z0-9_-]{30,}|xox[baprs]-[A-Za-z0-9-]{20,})",
    "url_credentials": rb"https?://[^\s/<>\"\x00]+:[^\s/<>\"\x00]+@",
}
PRIVATE_NAMES = {".env", ".npmrc", ".netrc", ".pypirc", "id_rsa", "id_ed25519", "credentials.json", "cookies.json"}
PRIVATE_SUFFIXES = {".pem", ".key", ".p12", ".pfx", ".sqlite", ".sqlite3"}
EXCLUDED_DIRS = {"target", "node_modules", "dist", "reports", ".git", ".nir", "test-results", "playwright-report", "__pycache__"}


def source_files():
    # Git's ignore rules define the public source set, including files not yet committed.
    result = subprocess.run(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], capture_output=True, check=True)
    return [Path(os.fsdecode(p)) for p in sorted(set(result.stdout.split(b"\0"))) if p]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archives", nargs="*", type=Path)
    args = parser.parse_args()
    findings = []
    count = 0
    byte_count = 0
    home = os.fsencode(str(Path.home())) + b"/"
    cwd = os.fsencode(str(Path.cwd())) + b"/"

    def check(name, data, *, source=False):
        nonlocal count, byte_count
        count += 1
        byte_count += len(data)
        p = Path(name)
        if p.name in PRIVATE_NAMES or p.suffix in PRIVATE_SUFFIXES or (p.name.startswith(".env.") and p.name != ".env.example"):
            findings.append({"file": name, "category": "private_filename"})
        if source and EXCLUDED_DIRS.intersection(p.parts):
            findings.append({"file": name, "category": "local_state_directory"})
        for category, pattern in PATTERNS.items():
            if re.search(pattern, data):
                findings.append({"file": name, "category": category})
        if home in data or cwd in data:
            findings.append({"file": name, "category": "developer_absolute_path"})

    for p in source_files():
        if p.is_symlink():
            findings.append({"file": str(p), "category": "symlink_requires_review"})
        else:
            check(str(p), p.read_bytes(), source=True)
    for archive in args.archives:
        with tarfile.open(archive, "r:gz") as tar:
            for member in tar:
                name = f"{archive.name}:{member.name}"
                if member.issym() or member.islnk() or Path(member.name).is_absolute() or ".." in Path(member.name).parts:
                    findings.append({"file": name, "category": "unsafe_archive_entry"})
                elif member.isfile():
                    check(name, tar.extractfile(member).read(), source="source" in archive.name)
                if member.uid or member.gid or member.uname or member.gname:
                    findings.append({"file": name, "category": "archive_owner_metadata"})
    print(json.dumps({"status": "PASS" if not findings else "FAIL", "files_scanned": count, "bytes_scanned": byte_count, "findings": findings}, indent=2))
    raise SystemExit(bool(findings))


if __name__ == "__main__":
    main()
