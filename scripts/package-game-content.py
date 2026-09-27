#!/usr/bin/env python3
"""Build deterministic base-game ZIPs and a delivery manifest from explicit inputs."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
import zipfile
from dataclasses import dataclass
from pathlib import Path


TARGET_VERSION = "2012.09.19.0001"
BOOT_VERSION = "2010.09.18.0000"
RECEIPT_NAME = ".bahamut-install.json"
CHUNK_BYTES = 1024 * 1024
REPARSE_ATTRIBUTE = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)


class PackageError(Exception):
    pass


@dataclass(frozen=True)
class FileIdentity:
    path: str
    length: int
    sha256: str
    source: Path

    def manifest_value(self) -> dict[str, object]:
        return {"path": self.path, "length": self.length, "sha256": self.sha256}


def validate_relative_path(value: str) -> str:
    if not value or not value.isascii() or "\\" in value or ":" in value:
        raise PackageError(f"Unsafe payload path: {value!r}")
    parts = value.split("/")
    for part in parts:
        stem = part.split(".", 1)[0].upper()
        if (
            not part
            or part in {".", ".."}
            or part.endswith((".", " "))
            or any(ord(char) < 32 or char in '<>"|?*' for char in part)
            or stem in {"CON", "PRN", "AUX", "NUL"}
            or (len(stem) == 4 and stem[:3] in {"COM", "LPT"} and stem[3] in "123456789")
        ):
            raise PackageError(f"Unsafe payload path: {value!r}")
    return value


def reject_sensitive_path(value: str) -> None:
    parts = [part.casefold() for part in value.split("/")]
    sensitive_names = {
        ".agents",
        ".codex",
        ".git",
        "backup",
        "backups",
        "cache",
        "caches",
        "config",
        "crash",
        "crashes",
        "log",
        "logs",
        "macro",
        "macros",
        "screenshot",
        "screenshots",
        "setting",
        "settings",
        "temp",
        "temporary",
        "tmp",
        "user",
        "users",
    }
    sensitive_stems = (
        "account",
        "auth",
        "credential",
        "password",
        "private-key",
        "secret",
        "session",
        "token",
    )
    if any(part in sensitive_names for part in parts) or any(
        stem in part for part in parts for stem in sensitive_stems
    ):
        raise PackageError(f"Sensitive or local-only payload path is forbidden: {value}")
    if Path(value).suffix.casefold() in {
        ".cfg",
        ".conf",
        ".dmp",
        ".ini",
        ".key",
        ".log",
        ".mdmp",
        ".p12",
        ".pfx",
        ".sqlite",
        ".toml",
    }:
        raise PackageError(f"Local settings, credentials, logs, and crash dumps are forbidden: {value}")


def is_reparse(path: Path, info: os.stat_result) -> bool:
    return stat.S_ISLNK(info.st_mode) or bool(getattr(info, "st_file_attributes", 0) & REPARSE_ATTRIBUTE)


def inspect_tree(root: Path) -> Path:
    root = absolute_clean_path(root)
    inspect_path_ancestors(root)
    try:
        root_info = root.lstat()
    except OSError as error:
        raise PackageError(f"Cannot inspect input directory {root}: {error}") from error
    if is_reparse(root, root_info) or not stat.S_ISDIR(root_info.st_mode):
        raise PackageError(f"Input root must be a regular directory: {root}")
    pending = [root]
    while pending:
        directory = pending.pop()
        try:
            entries = sorted(os.scandir(directory), key=lambda entry: entry.name.casefold())
        except OSError as error:
            raise PackageError(f"Cannot inspect input directory {directory}: {error}") from error
        for entry in entries:
            path = Path(entry.path)
            try:
                info = entry.stat(follow_symlinks=False)
            except OSError as error:
                raise PackageError(f"Cannot inspect input entry {path}: {error}") from error
            if is_reparse(path, info):
                raise PackageError(f"Reparse and symbolic links are forbidden: {path}")
            if stat.S_ISDIR(info.st_mode):
                pending.append(path)
            elif not stat.S_ISREG(info.st_mode):
                raise PackageError(f"Special files are forbidden: {path}")
    return root.resolve(strict=True)


def inspect_path_ancestors(path: Path) -> None:
    cursor = Path(path.anchor)
    for part in path.parts[1:]:
        cursor = cursor / part
        try:
            info = cursor.lstat()
        except FileNotFoundError:
            continue
        except OSError as error:
            raise PackageError(f"Cannot inspect path ancestor {cursor}: {error}") from error
        if is_reparse(cursor, info):
            raise PackageError(f"Reparse and symbolic links are forbidden: {cursor}")
        if cursor != path and not stat.S_ISDIR(info.st_mode):
            raise PackageError(f"Path ancestor is not a directory: {cursor}")


def absolute_clean_path(path: Path) -> Path:
    path = path.expanduser()
    if not path.is_absolute():
        raise PackageError(f"Expected an explicit absolute path: {path}")
    if any(part in {".", ".."} for part in path.parts):
        raise PackageError(f"Path aliases are forbidden: {path}")
    return path


def read_allowlist(path: Path) -> list[str]:
    try:
        raw = path.read_text(encoding="utf-8")
    except OSError as error:
        raise PackageError(f"Cannot read allowlist {path}: {error}") from error
    values: list[str] = []
    seen: set[str] = set()
    for line_number, line in enumerate(raw.splitlines(), 1):
        value = line.strip()
        if not value or value.startswith("#"):
            continue
        try:
            value = validate_relative_path(value)
            reject_sensitive_path(value)
        except PackageError as error:
            raise PackageError(f"{path}:{line_number}: {error}") from error
        key = value.casefold()
        if key in seen:
            raise PackageError(f"{path}:{line_number}: duplicate payload path: {value}")
        seen.add(key)
        values.append(value)
    if not values:
        raise PackageError(f"Allowlist is empty: {path}")
    return sorted(values, key=lambda value: (value.casefold(), value))


def digest_file(path: Path, expected_path: str) -> FileIdentity:
    try:
        before = path.lstat()
    except OSError as error:
        raise PackageError(f"Payload file is missing: {expected_path}") from error
    if is_reparse(path, before) or not stat.S_ISREG(before.st_mode):
        raise PackageError(f"Payload file must be a regular file: {expected_path}")
    hasher = hashlib.sha256()
    length = 0
    with path.open("rb") as source:
        while chunk := source.read(CHUNK_BYTES):
            length += len(chunk)
            hasher.update(chunk)
    after = path.lstat()
    if is_reparse(path, after) or not stat.S_ISREG(after.st_mode) or after.st_size != length:
        raise PackageError(f"Payload changed while it was being read: {expected_path}")
    return FileIdentity(expected_path, length, hasher.hexdigest(), path)


def inventory(root: Path, allowlist_path: Path) -> list[FileIdentity]:
    values: list[FileIdentity] = []
    for relative in read_allowlist(allowlist_path):
        candidate = root.joinpath(*relative.split("/"))
        values.append(digest_file(candidate, relative))
    return values


def same_tree(left: Path, right: Path) -> bool:
    left_value = os.path.normcase(str(left)).casefold().rstrip("\\/")
    right_value = os.path.normcase(str(right)).casefold().rstrip("\\/")
    return left_value == right_value


def hash_path(path: Path) -> tuple[int, str]:
    hasher = hashlib.sha256()
    length = 0
    with path.open("rb") as source:
        while chunk := source.read(CHUNK_BYTES):
            length += len(chunk)
            hasher.update(chunk)
    return length, hasher.hexdigest()


def paths_overlap(left: Path, right: Path) -> bool:
    left_value = os.path.normcase(str(left)).casefold().rstrip("\\/")
    right_value = os.path.normcase(str(right)).casefold().rstrip("\\/")
    try:
        return os.path.commonpath([left_value, right_value]) in {left_value, right_value}
    except ValueError:
        return False


def zip_partitions(files: list[FileIdentity], limit: int) -> list[list[FileIdentity]]:
    if limit <= 0:
        raise PackageError("The maximum uncompressed archive size must be positive.")
    partitions: list[list[FileIdentity]] = []
    current: list[FileIdentity] = []
    current_size = 0
    for file in files:
        if file.length > limit:
            raise PackageError(f"File exceeds the selected archive size limit: {file.path}")
        if current and current_size + file.length > limit:
            partitions.append(current)
            current = []
            current_size = 0
        current.append(file)
        current_size += file.length
    if current:
        partitions.append(current)
    return partitions


def write_zip(path: Path, files: list[FileIdentity]) -> None:
    with zipfile.ZipFile(
        path,
        mode="w",
        compression=zipfile.ZIP_DEFLATED,
        compresslevel=9,
        strict_timestamps=True,
    ) as archive:
        for file in files:
            info = zipfile.ZipInfo(file.path, date_time=(1980, 1, 1, 0, 0, 0))
            info.create_system = 3
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = (stat.S_IFREG | 0o644) << 16
            info.file_size = file.length
            hasher = hashlib.sha256()
            length = 0
            with file.source.open("rb") as source, archive.open(
                info,
                mode="w",
                force_zip64=file.length > zipfile.ZIP64_LIMIT,
            ) as target:
                while chunk := source.read(CHUNK_BYTES):
                    target.write(chunk)
                    length += len(chunk)
                    hasher.update(chunk)
            if length != file.length or hasher.hexdigest() != file.sha256:
                raise PackageError(f"Payload changed while writing the archive: {file.path}")


def build_manifest(
    output_dir: Path,
    base_files: list[FileIdentity],
    final_files: list[FileIdentity],
    baseline_version: str,
    transition: str,
    staging_bytes: int,
    max_archive_bytes: int,
) -> dict[str, object]:
    base_bytes = sum(file.length for file in base_files)
    final_bytes = sum(file.length for file in final_files)
    if staging_bytes < max(base_bytes, final_bytes):
        raise PackageError(
            "Peak staging bytes must cover both the base and final inventories."
        )
    if transition == "none" and [file.manifest_value() for file in base_files] != [
        file.manifest_value()
        for file in final_files
        if file.path.casefold() not in {"boot.ver", "game.ver"}
    ]:
        raise PackageError("A no-patch transition requires identical base and final payloads.")

    archives: list[dict[str, object]] = []
    for index, partition in enumerate(zip_partitions(base_files, max_archive_bytes), 1):
        archive_name = f"base-{index:04}.zip"
        archive_path = output_dir / archive_name
        write_zip(archive_path, partition)
        archive_length, archive_hash = hash_path(archive_path)
        archives.append(
            {
                "object": {
                    "object_key": f"game/{baseline_version}/{archive_hash}/{archive_name}",
                    "length": archive_length,
                    "sha256": archive_hash,
                },
                "files": [file.manifest_value() for file in partition],
            }
        )

    return {
        "schema_version": 1,
        "content_root": None,
        "base": {
            "baseline_version": baseline_version,
            "target_version": TARGET_VERSION,
            "transition": transition,
            "archives": archives,
            "final_files": [file.manifest_value() for file in final_files],
            "staging_bytes": staging_bytes,
        },
    }


def prepare_output(path: Path, roots: list[Path]) -> Path:
    path = absolute_clean_path(path)
    for root in roots:
        if paths_overlap(path, root):
            raise PackageError("Output directory must be outside both input trees.")
    inspect_path_ancestors(path)
    try:
        path.mkdir(parents=True, exist_ok=True)
    except OSError as error:
        raise PackageError(f"Cannot create output directory {path}: {error}") from error
    inspect_path_ancestors(path)
    resolved = path.resolve(strict=True)
    if any(path.iterdir()):
        raise PackageError(f"Output directory must be empty and must not be a link: {path}")
    for root in roots:
        if paths_overlap(resolved, root):
            raise PackageError("Output directory must be outside both input trees.")
    return resolved


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input-root", required=True, type=Path)
    parser.add_argument("--base-allowlist", required=True, type=Path)
    parser.add_argument("--final-root", required=True, type=Path)
    parser.add_argument("--final-allowlist", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--baseline-version", required=True)
    parser.add_argument("--transition", required=True, choices=("none", "full-chain"))
    parser.add_argument("--staging-bytes", required=True, type=int)
    parser.add_argument("--max-archive-uncompressed-bytes", required=True, type=int)
    return parser.parse_args(argv)


def run(args: argparse.Namespace) -> Path:
    if not re.fullmatch(r"[A-Za-z0-9._-]+", args.baseline_version) or args.baseline_version.endswith((".", " ")):
        raise PackageError("Baseline version must be a safe ASCII path segment.")
    if args.staging_bytes <= 0:
        raise PackageError("Peak staging bytes must be positive.")
    if args.transition == "none" and args.baseline_version != TARGET_VERSION:
        raise PackageError("A no-patch transition requires baseline and target versions to match.")

    base_root = inspect_tree(args.input_root)
    final_root = inspect_tree(args.final_root)
    if paths_overlap(base_root, final_root) and not (
        args.transition == "none" and same_tree(base_root, final_root)
    ):
        raise PackageError("Base and final inputs must be separate directory trees.")
    base_files = inventory(base_root, args.base_allowlist)
    final_files = inventory(final_root, args.final_allowlist)
    for files, label in ((base_files, "base"), (final_files, "final")):
        names = {file.path.casefold() for file in files}
        for required in ("ffxivboot.exe", "ffxivgame.exe"):
            if required not in names:
                raise PackageError(f"The {label} inventory must contain {required}.")
    if any(file.path.casefold() == RECEIPT_NAME.casefold() for file in final_files):
        raise PackageError("Final input cannot include the launcher install receipt.")
    if any(
        file.path.casefold() in {"boot.ver", "game.ver", RECEIPT_NAME.casefold()}
        for file in base_files
    ):
        raise PackageError("Base input cannot include managed version or receipt files.")
    final_by_name = {file.path.casefold(): file for file in final_files}
    for name, contents in (("boot.ver", BOOT_VERSION), ("game.ver", TARGET_VERSION)):
        file = final_by_name.get(name)
        if file is None or file.length != len(contents.encode("ascii")):
            raise PackageError(f"The final inventory must contain the expected {name}.")
        if file.source.read_bytes() != contents.encode("ascii"):
            raise PackageError(f"The final input has an unexpected {name} value.")

    output_dir = prepare_output(args.output_dir, [base_root, final_root])
    manifest = build_manifest(
        output_dir,
        base_files,
        final_files,
        args.baseline_version,
        args.transition,
        args.staging_bytes,
        args.max_archive_uncompressed_bytes,
    )
    manifest_path = output_dir / "game-delivery.json"
    manifest_path.write_text(
        json.dumps(manifest, ensure_ascii=True, indent=2) + "\n",
        encoding="ascii",
        newline="\n",
    )
    return manifest_path


def main(argv: list[str] | None = None) -> int:
    try:
        manifest_path = run(parse_args(sys.argv[1:] if argv is None else argv))
    except (PackageError, OSError, ValueError) as error:
        print(f"package-game-content: {error}", file=sys.stderr)
        return 2
    print(manifest_path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
