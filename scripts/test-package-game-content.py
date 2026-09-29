#!/usr/bin/env python3
"""Synthetic checks for deterministic game-content packaging."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock

sys.dont_write_bytecode = True


SCRIPT = Path(__file__).with_name("package-game-content.py")
TARGET = "2012.09.19.0001"
BOOT = "2010.09.18.0000"
SPEC = importlib.util.spec_from_file_location("package_game_content", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
PACKAGER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PACKAGER
SPEC.loader.exec_module(PACKAGER)


class PackageGameContentTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="bahamut-package-game-")
        self.root = Path(self.temporary.name)
        self.base = self.root / "base"
        self.final = self.root / "final"
        self.base.mkdir()
        self.final.mkdir()
        self._populate(self.base, b"base game")
        self._populate(self.final, b"base game")
        (self.final / "boot.ver").write_text(BOOT, encoding="ascii")
        (self.final / "game.ver").write_text(TARGET, encoding="ascii")
        self.base_allow = self.root / "base-allowlist.txt"
        self.base_allow.write_text(
            "ffxivboot.exe\nffxivgame.exe\nsqpack/data.bin\n", encoding="ascii"
        )
        self.final_allow = self.root / "final-allowlist.txt"
        self.final_allow.write_text(
            "boot.ver\nffxivboot.exe\nffxivgame.exe\ngame.ver\nsqpack/data.bin\n",
            encoding="ascii",
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    @staticmethod
    def _populate(root: Path, data: bytes) -> None:
        (root / "ffxivboot.exe").write_bytes(b"boot executable")
        (root / "ffxivgame.exe").write_bytes(b"game executable")
        (root / "sqpack").mkdir()
        (root / "sqpack" / "data.bin").write_bytes(data)

    def _run(
        self,
        output: Path,
        *,
        base: Path | None = None,
        final: Path | None = None,
        base_allow: Path | None = None,
        final_allow: Path | None = None,
        staging_bytes: int = 1024,
        max_archive_bytes: int = 1024,
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--input-root",
                str(base or self.base),
                "--base-allowlist",
                str(base_allow or self.base_allow),
                "--final-root",
                str(final or self.final),
                "--final-allowlist",
                str(final_allow or self.final_allow),
                "--output-dir",
                str(output),
                "--staging-bytes",
                str(staging_bytes),
                "--max-archive-uncompressed-bytes",
                str(max_archive_bytes),
            ],
            check=False,
            capture_output=True,
            text=True,
        )

    def test_two_runs_emit_byte_identical_archives_and_manifest(self) -> None:
        first = self.root / "package-a"
        second = self.root / "package-b"
        for output in (first, second):
            result = self._run(output)
            self.assertEqual(result.returncode, 0, result.stderr)
        first_names = sorted(path.name for path in first.iterdir())
        second_names = sorted(path.name for path in second.iterdir())
        self.assertEqual(first_names, second_names)
        for name in first_names:
            self.assertEqual((first / name).read_bytes(), (second / name).read_bytes())

    def test_manifest_hashes_and_archive_inventory_match_output(self) -> None:
        output = self.root / "package"
        result = self._run(output)
        self.assertEqual(result.returncode, 0, result.stderr)
        manifest = json.loads((output / "game-delivery.json").read_text(encoding="ascii"))
        package = manifest["base"]
        self.assertEqual(package["target_version"], TARGET)
        self.assertEqual(manifest["schema_version"], 3)
        self.assertNotIn("baseline_version", package)
        self.assertNotIn("transition", package)
        self.assertEqual(package["staging_bytes"], 1024)
        self.assertEqual(len(package["archives"]), 1)
        archive_spec = package["archives"][0]
        archive_path = output / "base-0001.zip"
        self.assertEqual(archive_path.stat().st_size, archive_spec["object"]["length"])
        self.assertEqual(
            hashlib.sha256(archive_path.read_bytes()).hexdigest(),
            archive_spec["object"]["sha256"],
        )
        with zipfile.ZipFile(archive_path) as archive:
            self.assertEqual(sorted(archive.namelist()), ["ffxivboot.exe", "ffxivgame.exe", "sqpack/data.bin"])

    def test_explicit_inputs_and_package_choices_are_required(self) -> None:
        result = subprocess.run(
            [sys.executable, str(SCRIPT)], check=False, capture_output=True, text=True
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("--input-root", result.stderr)

    def test_allowlist_rejects_settings_and_local_secrets(self) -> None:
        bad_allow = self.root / "bad-allowlist.txt"
        bad_allow.write_text("ffxivboot.exe\nsettings/account.ini\n", encoding="ascii")
        result = self._run(self.root / "package", base_allow=bad_allow)
        self.assertEqual(result.returncode, 2)
        self.assertIn("Sensitive or local-only", result.stderr)

    def test_reparse_entries_are_rejected_even_when_unlisted(self) -> None:
        marker = self.base / "unused-reparse"
        marker.write_bytes(b"not allowlisted")
        original = PACKAGER.is_reparse

        def report_reparse(path: Path, info: os.stat_result) -> bool:
            return path == marker or original(path, info)

        with mock.patch.object(PACKAGER, "is_reparse", side_effect=report_reparse):
            with self.assertRaisesRegex(PACKAGER.PackageError, "Reparse and symbolic links"):
                PACKAGER.inspect_tree(self.base)

    def test_peak_size_must_cover_input_inventories(self) -> None:
        result = self._run(self.root / "package", staging_bytes=1)
        self.assertEqual(result.returncode, 2)
        self.assertIn("Peak staging bytes", result.stderr)

    def test_archive_partition_limit_is_enforced(self) -> None:
        result = self._run(self.root / "package", max_archive_bytes=4)
        self.assertEqual(result.returncode, 2)
        self.assertIn("exceeds the selected archive size limit", result.stderr)

    def test_object_key_layout_is_hash_then_archive_name(self) -> None:
        output = self.root / "package"
        result = self._run(output)
        self.assertEqual(result.returncode, 0, result.stderr)
        manifest = json.loads((output / "game-delivery.json").read_text(encoding="ascii"))
        for index, archive in enumerate(manifest["base"]["archives"], 1):
            item = archive["object"]
            self.assertEqual(item["object_key"], f"game/{item['sha256']}/base-{index:04}.zip")

    def test_existing_output_is_preserved(self) -> None:
        output = self.root / "existing-output"
        output.mkdir()
        sentinel = output / "keep.txt"
        sentinel.write_text("owner data", encoding="ascii")
        result = self._run(output)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(sentinel.read_text(encoding="ascii"), "owner data")

    def test_final_payload_must_match_base_payload(self) -> None:
        changed = self.root / "changed-final"
        changed.mkdir()
        self._populate(changed, b"changed game")
        (changed / "boot.ver").write_text(BOOT, encoding="ascii")
        (changed / "game.ver").write_text(TARGET, encoding="ascii")
        result = self._run(self.root / "package", final=changed)
        self.assertEqual(result.returncode, 2)
        self.assertIn("Base and final payloads must be identical", result.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
