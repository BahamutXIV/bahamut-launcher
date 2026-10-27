#!/usr/bin/env python3
"""Install or verify pinned official overlay files."""

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


NAME = "official-overlay-dats.py"
REPO = Path(__file__).resolve().parent.parent
PIN = REPO / "manifests" / "official-overlay-dats.json"
DEFAULT_PACKAGE = REPO / "plugins" / "dats" / "bahamut-dats-overlay"
PACKAGE_MANIFEST = "overlay.toml"
# Strip the source collection prefix to get the package path.
SOURCE_PATH = re.compile(
    r"bahamut-dats-overlay/(data/[0-9A-F]{2}/[0-9A-F]{2}/[0-9A-F]{2}/[0-9A-F]{2}\.DAT)"
)
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
    seen = set()
    for entry in files:
        source = entry.get("path")
        match = SOURCE_PATH.fullmatch(source) if isinstance(source, str) else None
        if match is None:
            fail(f"invalid pinned path: {source!r}")
        if source in seen:
            fail(f"duplicate pinned path: {source}")
        seen.add(source)
        size = entry.get("size")
        if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
            fail(f"invalid pinned size for {source}")
        digest = entry.get("sha256")
        if not isinstance(digest, str) or not SHA256.fullmatch(digest):
            fail(f"invalid pinned sha256 for {source}")
        entries.append((source, match.group(1), size, digest))
    return entries


def is_link(path: Path) -> bool:
    """Symbolic links and, on Windows, any other reparse point such as a junction."""
    attributes = getattr(os.lstat(path), "st_file_attributes", 0)
    return path.is_symlink() or bool(attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT)


def check_file(path: Path, size: int, digest: str) -> bytes:
    if not path.is_file() or is_link(path):
        fail(f"pinned DAT is missing, a link, or not a regular file: {path}")
    data = path.read_bytes()
    if len(data) != size:
        fail(f"size mismatch for {path}: expected {size}, found {len(data)}")
    if hashlib.sha256(data).hexdigest() != digest:
        fail(f"SHA-256 mismatch for {path}")
    return data


def package_files(package: Path) -> list[str]:
    """Every regular file below PACKAGE as a POSIX relative path; links are refused."""
    if not package.is_dir() or is_link(package):
        fail(f"official overlay package is missing or a link: {package}")
    files = []
    for root, directories, names in os.walk(package):
        for name in directories + names:
            entry = Path(root) / name
            if is_link(entry):
                fail(f"official overlay package contains a link: {entry}")
        for name in names:
            entry = Path(root) / name
            if not entry.is_file():
                fail(f"official overlay package entry is not a regular file: {entry}")
            files.append(entry.relative_to(package).as_posix())
    return sorted(files)


def verify(package: Path) -> int:
    entries = load_pin()
    files = package_files(package)
    if PACKAGE_MANIFEST not in files:
        fail(f"official overlay manifest is missing: {package / PACKAGE_MANIFEST}")
    payload = [path for path in files if path != PACKAGE_MANIFEST]
    if not payload:
        return 0
    expected = sorted(package_path for _, package_path, _, _ in entries)
    if payload != expected:
        missing = sorted(set(expected) - set(payload))
        unexpected = sorted(set(payload) - set(expected))
        fail(
            "official overlay payload differs from the pin; "
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
            fail(f"official overlay package has a linked ancestor: {current}")


def install(source: Path, package: Path) -> int:
    entries = load_pin()
    check_repo_ancestors(package)
    if package_files(package) != [PACKAGE_MANIFEST]:
        fail(f"official overlay package must hold only {PACKAGE_MANIFEST} before install: {package}")
    # Assemble beside the package so a failed install leaves it unchanged.
    scratch = Path(tempfile.mkdtemp(prefix=".official-dats-", dir=package.parent))
    try:
        for source_path, package_path, size, digest in entries:
            data = check_file(source / source_path, size, digest)
            target = scratch / package_path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        (scratch / "data").rename(package / "data")
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    return verify(package)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog=NAME, description=__doc__.splitlines()[0])
    parser.add_argument("command", choices=["install", "verify"])
    parser.add_argument("--source", help="assets repository checkout at the pinned commit (install)")
    parser.add_argument("--package", default=str(DEFAULT_PACKAGE), help="official overlay package directory")
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
        print(f"{NAME}: pinned official overlay files verified")
    else:
        print(f"{NAME}: official overlay manifest verified")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
