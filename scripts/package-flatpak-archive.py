#!/usr/bin/env python3
"""Package a Flatpak bundle and its sidecars as the <label>.zip release asset.

    scripts/package-flatpak-archive.py --bundle <path/to/X.flatpak>
        --readme packaging/flatpak/README.md --label <asset name> --output <dir>

Requires <bundle>.sha256 and <bundle>.identity.json beside the bundle and
verifies both against the bundle bytes. Writes <output>/<label>.zip with the
single top-level directory bahamut-launcher-flatpak/ holding README.md, the
bundle and its two sidecars, plus <output>/<label>.zip.sha256 in
"<hex>  <label>.zip" format. The entry mtime is SOURCE_DATE_EPOCH, else the
HEAD commit time of this repository.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import time
import zipfile
from pathlib import Path


NAME = "package-flatpak-archive.py"
REPO = Path(__file__).resolve().parent.parent
TOP = "bahamut-launcher-flatpak"
LABEL_PATTERN = re.compile(r"[A-Za-z0-9_][A-Za-z0-9._-]*")
SIDECAR_PATTERN = re.compile(r"([0-9a-f]{64}) {1,2}(\S.*)")
ZIP_UNIX_FILE = 0o100644 << 16


def fail(message: str) -> None:
    print(f"{NAME}: {message}", file=sys.stderr)
    raise SystemExit(1)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def regular_file(path: Path, role: str) -> Path:
    if path.is_symlink():
        fail(f"{role} is a symbolic link: {path}")
    if not path.is_file():
        fail(f"{role} is missing or not a regular file: {path}")
    return path


def source_epoch() -> int:
    value = os.environ.get("SOURCE_DATE_EPOCH")
    if not value:
        result = subprocess.run(
            ["git", "-C", str(REPO), "log", "-1", "--format=%ct"],
            capture_output=True,
            text=True,
            check=False,
        )
        value = result.stdout.strip()
        if result.returncode != 0 or not value:
            fail(f"set SOURCE_DATE_EPOCH: the HEAD commit time could not be read from {REPO}")
    if not value.isdigit():
        fail(f"SOURCE_DATE_EPOCH is not a whole number of seconds: {value}")
    return int(value)


def zip_date_time(epoch: int) -> tuple[int, int, int, int, int, int]:
    # Zip timestamps are local time with a 1980 floor; UTC keeps the archive host-independent.
    parts = time.gmtime(max(epoch, 315532800))
    return (parts.tm_year, parts.tm_mon, parts.tm_mday, parts.tm_hour, parts.tm_min, parts.tm_sec)


def verify_sidecars(bundle: Path, checksum: Path, identity: Path) -> str:
    digest = sha256_file(bundle)
    recorded = checksum.read_text(encoding="utf-8").strip()
    match = SIDECAR_PATTERN.fullmatch(recorded)
    if match is None:
        fail(f"checksum sidecar is not in '<hex>  <name>' format: {checksum}")
    if match.group(2) != bundle.name:
        fail(f"checksum sidecar names {match.group(2)}, not {bundle.name}: {checksum}")
    if match.group(1) != digest:
        fail(f"checksum sidecar does not match the bundle: {checksum}")
    try:
        document = json.loads(identity.read_text(encoding="utf-8"))
        artifact = document["artifact"]
        recorded_digest = artifact["sha256"]
        recorded_name = artifact["filename"]
    except (ValueError, KeyError, TypeError):
        fail(f"identity sidecar has no artifact.sha256 and artifact.filename: {identity}")
    if recorded_name != bundle.name:
        fail(f"identity sidecar names {recorded_name}, not {bundle.name}: {identity}")
    if recorded_digest != digest:
        fail(f"identity sidecar does not match the bundle: {identity}")
    return digest


def write_archive(archive: Path, members: list[tuple[str, Path]], epoch: int) -> None:
    date_time = zip_date_time(epoch)
    with zipfile.ZipFile(archive, "x") as zipped:
        for name, source in sorted(members):
            info = zipfile.ZipInfo(f"{TOP}/{name}", date_time=date_time)
            info.external_attr = ZIP_UNIX_FILE
            info.create_system = 3
            # writestr is the one writer that honors an explicit level for a caller-built ZipInfo.
            zipped.writestr(info, source.read_bytes(), compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)


def main() -> None:
    parser = argparse.ArgumentParser(prog=NAME, description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bundle", required=True, help="the .flatpak bundle with its sidecars beside it")
    parser.add_argument("--readme", required=True, help="the README.md copied into the archive")
    parser.add_argument("--label", required=True, help="the asset name without the .zip suffix")
    parser.add_argument("--output", required=True, help="the directory that receives the zip and its .sha256")
    args = parser.parse_args()

    if LABEL_PATTERN.fullmatch(args.label) is None:
        fail(f"the label must be a file name of letters, digits, '.', '_', and '-', not starting with '.' or '-': {args.label}")
    bundle = Path(args.bundle)
    if bundle.suffix != ".flatpak":
        fail(f"bundle must end in .flatpak: {bundle}")
    regular_file(bundle, "bundle")
    checksum = regular_file(bundle.with_name(bundle.name + ".sha256"), "checksum sidecar")
    identity = regular_file(bundle.with_name(bundle.name + ".identity.json"), "identity sidecar")
    readme = regular_file(Path(args.readme), "README")
    epoch = source_epoch()

    output = Path(args.output)
    if output.is_symlink():
        fail(f"output is a symbolic link: {output}")
    if output.exists() and not output.is_dir():
        fail(f"output exists and is not a directory: {output}")
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"{args.label}.zip"
    sidecar = output / f"{archive.name}.sha256"
    for existing in (archive, sidecar):
        if existing.exists() or existing.is_symlink():
            fail(f"refusing to overwrite {existing}")

    verify_sidecars(bundle, checksum, identity)
    members = [
        ("README.md", readme),
        (bundle.name, bundle),
        (checksum.name, checksum),
        (identity.name, identity),
    ]
    partial = output / f".{archive.name}.partial"
    if partial.exists() or partial.is_symlink():
        fail(f"refusing to overwrite {partial}")
    try:
        write_archive(partial, members, epoch)
        with zipfile.ZipFile(partial) as zipped:
            names = zipped.namelist()
        expected = sorted(f"{TOP}/{name}" for name, _ in members)
        if names != expected:
            fail(f"archive member list mismatch: {names}")
        partial.replace(archive)
    finally:
        if partial.exists():
            partial.unlink()
    sidecar.write_text(f"{sha256_file(archive)}  {archive.name}\n", encoding="ascii")
    print(f"{NAME}: PASS ({len(members)} files): {archive}")


if __name__ == "__main__":
    main()
