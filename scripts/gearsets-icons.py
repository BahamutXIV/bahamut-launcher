#!/usr/bin/env python3
"""Install or verify the pinned Gear Sets class icons."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import sys
import tempfile
from pathlib import Path


NAME = "gearsets-icons.py"
REPO = Path(__file__).resolve().parent.parent
PIN = REPO / "manifests" / "gearsets-icons.json"
DEFAULT_PACKAGE = REPO / "addons" / "gearsets" / "icons"
# Source path in the assets repository; the package path is <key>.png. Install
# reads the file name from the directory given as the source.
SOURCE_PATH = re.compile(r"icons-1\.23b/0/icon[0-9]{5}\.png")
KEY = re.compile(r"[a-z]{3}")
SHA256 = re.compile(r"[0-9a-f]{64}")


def fail(message: str) -> None:
    raise SystemExit(f"{NAME}: {message}")


def load_pin() -> list[tuple[str, str, int, str]]:
    """(source path, package path, size, sha256) for every pinned file."""
    pin = json.loads(PIN.read_text(encoding="utf-8"))
    files = pin.get("files")
    if not isinstance(files, list) or not files:
        fail(f"pin lists no files: {PIN}")
    entries = []
    sources = set()
    keys = set()
    for entry in files:
        source = entry.get("path")
        if not isinstance(source, str) or SOURCE_PATH.fullmatch(source) is None:
            fail(f"invalid pinned path: {source!r}")
        key = entry.get("key")
        if not isinstance(key, str) or KEY.fullmatch(key) is None:
            fail(f"invalid pinned key for {source}")
        if source in sources or key in keys:
            fail(f"duplicate pinned entry: {source}")
        sources.add(source)
        keys.add(key)
        size = entry.get("size")
        if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
            fail(f"invalid pinned size for {source}")
        digest = entry.get("sha256")
        if not isinstance(digest, str) or not SHA256.fullmatch(digest):
            fail(f"invalid pinned sha256 for {source}")
        entries.append((source, f"{key}.png", size, digest))
    return entries


def is_link(path: Path) -> bool:
    """Symbolic links and, on Windows, any other reparse point such as a junction."""
    attributes = getattr(os.lstat(path), "st_file_attributes", 0)
    return path.is_symlink() or bool(attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT)


def check_file(path: Path, size: int, digest: str) -> bytes:
    if not path.is_file() or is_link(path):
        fail(f"pinned icon is missing, a link, or not a regular file: {path}")
    data = path.read_bytes()
    if len(data) != size:
        fail(f"size mismatch for {path}: expected {size}, found {len(data)}")
    if hashlib.sha256(data).hexdigest() != digest:
        fail(f"SHA-256 mismatch for {path}")
    return data


def package_files(package: Path) -> list[str]:
    """Every file directly in PACKAGE; a missing package holds none, links are refused."""
    if not os.path.lexists(package):
        return []
    if not package.is_dir() or is_link(package):
        fail(f"Gear Sets icon package is not a plain directory: {package}")
    files = []
    for entry in package.iterdir():
        if is_link(entry):
            fail(f"Gear Sets icon package contains a link: {entry}")
        if not entry.is_file():
            fail(f"Gear Sets icon package entry is not a regular file: {entry}")
        files.append(entry.name)
    return sorted(files)


def verify(package: Path) -> int:
    """Number of verified icons; 0 when the optional package is absent or empty."""
    entries = load_pin()
    files = package_files(package)
    if not files:
        return 0
    expected = sorted(package_path for _, package_path, _, _ in entries)
    if files != expected:
        missing = sorted(set(expected) - set(files))
        unexpected = sorted(set(files) - set(expected))
        fail(
            "Gear Sets icon payload differs from the pin; "
            f"missing: {', '.join(missing) or 'none'}; "
            f"unexpected: {', '.join(unexpected) or 'none'}"
        )
    for _, package_path, size, digest in entries:
        check_file(package / package_path, size, digest)
    return len(expected)


def check_repo_ancestors(package: Path) -> None:
    """Refuse links between the repository root and PACKAGE; install writes there."""
    try:
        parts = package.relative_to(REPO).parts
    except ValueError:
        return
    current = REPO
    for part in parts[:-1]:
        current = current / part
        if is_link(current):
            fail(f"Gear Sets icon package has a linked ancestor: {current}")


def install(source: Path, package: Path) -> int:
    entries = load_pin()
    check_repo_ancestors(package)
    if package_files(package):
        # A repeated local install is a no-op when the package already matches the pin.
        return verify(package)
    if not package.parent.is_dir():
        fail(f"Gear Sets icon package parent is missing: {package.parent}")
    # Assemble beside the package so a failed install leaves it unchanged.
    scratch = Path(tempfile.mkdtemp(prefix=".gearsets-icons-", dir=package.parent))
    try:
        for source_path, package_path, size, digest in entries:
            (scratch / package_path).write_bytes(check_file(source / source_path.rsplit("/", 1)[1], size, digest))
        if package.is_dir():
            package.rmdir()
        scratch.rename(package)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    return verify(package)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog=NAME, description=__doc__.splitlines()[0])
    parser.add_argument("command", choices=["install", "verify"])
    parser.add_argument("--source", help="directory holding the pinned icon PNGs, such as icons-1.23b/0 in an assets checkout at the pinned commit (install)")
    parser.add_argument("--package", default=str(DEFAULT_PACKAGE), help="Gear Sets icon directory")
    args = parser.parse_args(argv)
    # Not resolved: package_files must see a link or junction to refuse it.
    package = Path(os.path.abspath(args.package))
    if args.command == "install":
        if not args.source:
            parser.error("install requires --source")
        count = install(Path(os.path.abspath(args.source)), package)
    else:
        count = verify(package)
    if count:
        print(f"{NAME}: pinned Gear Sets icons verified")
    else:
        print(f"{NAME}: no Gear Sets icons installed")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
