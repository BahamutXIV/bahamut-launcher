#!/usr/bin/env python3
"""Validate the pinned 1.23b ZIP without repackaging its bytes."""

import argparse
import collections
import hashlib
import json
import os
import tempfile
from pathlib import Path
import runpy
import stat
import sys
import zipfile


SOURCE_ROOT = "FINAL FANTASY XIV/"
HOST = "https://pub-f164b0f74f9d45769188b8a2924542d0.r2.dev"
OBJECT_KEY = "xiv1point0.zip"
ARCHIVE_LENGTH = 7764445250
ARCHIVE_SHA256 = "c48ab1ad84137c410c2f26ac6a9ce7c757d9f9014013fd6565caf865fd9eb17a"
VERSION_FILES = {"boot.ver": b"2010.09.18.0000", "game.ver": b"2012.09.19.0001"}
EXCLUDED_FILES = {
    "data/.DS_Store", "client/.DS_Store",
    "data/03/.DS_Store", "data/89/.DS_Store", "data/72/.DS_Store",
    "data/1C/.DS_Store", "client/chara/.DS_Store",
    "data/03/5B/.DS_Store", "client/chara/pc/.DS_Store",
    *(f"client/chara/pc/c{index:03}/.DS_Store" for index in range(1, 10)),
}
PUBLISHER_NAME_EXCEPTIONS = {
    "client/sqwt/system/bootup/AccountSelectionPhase.form",
    "client/sqwt/system/bootup/AccountSelectionPhase.sd.tpl",
    "client/sqwt/system/bootup/AccountSelectionPhase.tpl",
    "client/sqwt/system/bootup/AccountSelectionPhase.sd.form",
}


def fail(message):
    raise ValueError(message)


def digest_stream(source):
    digest = hashlib.sha256()
    length = 0
    while chunk := source.read(1024 * 1024):
        digest.update(chunk)
        length += len(chunk)
    return length, digest.hexdigest()


def file_identity(archive, entry, relative):
    with archive.open(entry) as source:
        length, digest = digest_stream(source)
    if length != entry.file_size:
        fail(f"ZIP entry length changed: {entry.filename}")
    return {"path": relative, "length": length, "sha256": digest}


def parent_names(path):
    parts = path.split("/")
    return {"/".join(parts[:end]) for end in range(1, len(parts))}


def metadata_counterpart(name):
    if not name.startswith("__MACOSX/"):
        fail(f"Unexpected metadata path: {name}")
    relative = name[len("__MACOSX/"):]
    parent, _, leaf = relative.rpartition("/")
    if not leaf.startswith("._") or len(leaf) <= 2:
        fail(f"Unexpected metadata entry: {name}")
    counterpart = f"{parent}/{leaf[2:]}" if parent else leaf[2:]
    if counterpart != SOURCE_ROOT[:-1] and not counterpart.startswith(SOURCE_ROOT):
        fail(f"Metadata outside selected root: {name}")
    return counterpart


def inspect_archive(path):
    publisher = runpy.run_path(str(Path(__file__).with_name("package-game-content.py")))
    PackageError = publisher["PackageError"]
    validate_relative_path = publisher["validate_relative_path"]
    reject_sensitive_path = publisher["reject_sensitive_path"]

    with path.open("rb") as source:
        length, digest = digest_stream(source)
    if length != ARCHIVE_LENGTH or digest != ARCHIVE_SHA256:
        fail("Archive bytes differ from the expected length or SHA-256.")

    selected = []
    excluded = []
    directory_names = set()
    all_names = set()
    metadata = []
    metadata_dirs = set()
    with zipfile.ZipFile(path, allowZip64=True) as archive:
        for entry in archive.infolist():
            raw = entry.filename
            directory = entry.is_dir()
            name = raw[:-1] if directory else raw
            validate_relative_path(name)
            folded = name.casefold()
            if folded in all_names:
                fail(f"Duplicate archive source path: {name}")
            all_names.add(folded)
            mode = entry.external_attr >> 16
            kind = stat.S_IFMT(mode)
            if mode & 0o7000 or kind not in ((0, stat.S_IFDIR) if directory else (0, stat.S_IFREG)):
                fail(f"Link or special ZIP entry: {name}")
            if directory:
                if name.startswith(SOURCE_ROOT) or name == SOURCE_ROOT[:-1]:
                    directory_names.add(name)
                elif name == "__MACOSX" or name.startswith("__MACOSX/"):
                    metadata_dirs.add(name)
                else:
                    fail(f"Unexpected source directory: {name}")
                continue
            if name.startswith("__MACOSX/"):
                metadata.append(name)
                continue
            if not name.startswith(SOURCE_ROOT):
                fail(f"Unexpected source file: {name}")
            relative = name[len(SOURCE_ROOT):]
            validate_relative_path(relative)
            try:
                reject_sensitive_path(relative)
            except PackageError:
                if relative not in PUBLISHER_NAME_EXCEPTIONS:
                    fail(f"Publisher excludes unexpected source file: {relative}")
            identity = file_identity(archive, entry, relative)
            if relative in EXCLUDED_FILES:
                excluded.append(identity)
            else:
                selected.append(identity)

        for name in metadata:
            if metadata_counterpart(name).casefold() not in all_names:
                fail(f"Apple metadata has no source counterpart: {name}")
        metadata_parents = set()
        for name in metadata:
            metadata_parents.update(parent_names(name))
        if not metadata_dirs.issubset(metadata_parents):
            fail("Unexpected Apple metadata directory.")
        if SOURCE_ROOT[:-1] not in directory_names:
            fail("Wrapped game root directory is missing.")
        payload_names = {item["path"].casefold() for item in selected + excluded}
        if len(payload_names) != len(selected) + len(excluded):
            fail("Source paths alias a destination.")
        if {item["path"] for item in excluded} != EXCLUDED_FILES:
            fail("The source exclusions differ from the archive.")
        by_name = {item["path"]: item for item in selected}
        for name, contents in VERSION_FILES.items():
            entry = archive.getinfo(SOURCE_ROOT + name)
            with archive.open(entry) as source:
                if source.read() != contents:
                    fail(f"Unexpected source {name} value.")
            if by_name[name]["sha256"] != hashlib.sha256(contents).hexdigest():
                fail(f"Unexpected source {name} identity.")
        occupied = set()
        for item in selected + excluded:
            occupied.update(parent_names(item["path"]))
        payload_directories = {name[len(SOURCE_ROOT):] for name in directory_names if name != SOURCE_ROOT[:-1]}
        empty_directories = sorted(payload_directories - occupied)
        for directory in payload_directories:
            if directory.casefold() in payload_names:
                fail(f"Source directory aliases a file: {directory}")

    selected.sort(key=lambda item: item["path"])
    excluded.sort(key=lambda item: item["path"])
    final_files = list(selected)
    base_files = [item for item in selected if item["path"] not in VERSION_FILES]
    return base_files, final_files, excluded, empty_directories, len(metadata)


def validate_distinct_output(archive, output):
    if archive.resolve() == output.resolve():
        fail("Archive input and manifest output must be different files.")
    if output.is_symlink():
        fail("Manifest output must not be a symbolic link.")
    if output.exists() and os.path.samefile(archive, output):
        fail("Archive input and manifest output refer to the same file.")


def write_manifest(archive, output, manifest):
    validate_distinct_output(archive, output)
    payload = (json.dumps(manifest, ensure_ascii=True, separators=(",", ":")) + "\n").encode("ascii")
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="wb", dir=output.parent, prefix=f".{output.name}.", suffix=".tmp", delete=False
        ) as stream:
            temporary = Path(stream.name)
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        # Archive inspection can take time; recheck before replacing the output.
        validate_distinct_output(archive, output)
        os.replace(temporary, output)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--staging-bytes", type=int, required=True)
    args = parser.parse_args()
    try:
        validate_distinct_output(args.archive, args.output)
        base_files, final_files, excluded, empty_dirs, metadata_count = inspect_archive(args.archive)
        final_bytes = sum(item["length"] for item in final_files)
        if args.staging_bytes < final_bytes:
            fail("Staging bytes are smaller than the final inventory.")
        manifest = {
            "schema_version": 3,
            "content_root": HOST,
            "base": {
                "target_version": "2012.09.19.0001",
                "archives": [{
                    "object": {"object_key": OBJECT_KEY, "length": ARCHIVE_LENGTH, "sha256": ARCHIVE_SHA256},
                    "files": base_files,
                    "layout": "final-fantasy-xiv-wrapper",
                    "excluded_files": excluded,
                    "empty_directories": empty_dirs,
                    "apple_metadata_files": metadata_count,
                }],
                "final_files": final_files,
                "staging_bytes": args.staging_bytes,
            },
        }
        write_manifest(args.archive, args.output, manifest)
        print(json.dumps({"archive_sha256": ARCHIVE_SHA256, "base_files": len(base_files),
                          "final_files": len(final_files), "excluded_files": len(excluded),
                          "empty_directories": len(empty_dirs), "apple_metadata_files": metadata_count,
                          "final_bytes": final_bytes, "manifest_bytes": args.output.stat().st_size}))
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        print(f"intake-full-client: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
