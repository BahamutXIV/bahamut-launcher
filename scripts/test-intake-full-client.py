#!/usr/bin/env python3
"""Test intake output safety without opening a retail archive."""

from contextlib import redirect_stderr, redirect_stdout
import io
import json
import os
from pathlib import Path
import runpy
import sys
import tempfile
import unittest
from unittest import mock


INTAKE = runpy.run_path(str(Path(__file__).with_name("intake-full-client.py")))


class OutputSafetyTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.archive = self.root / "client.zip"
        self.archive.write_bytes(b"input sentinel")
        self.output = self.root / "manifest.json"

    def assert_preflight_rejects(self, output):
        main = INTAKE["main"]
        inspect = mock.Mock(side_effect=AssertionError("must reject before reading the archive"))
        argv = [
            "intake-full-client.py", "--archive", str(self.archive),
            "--output", str(output), "--staging-bytes", "1",
        ]
        with mock.patch.dict(main.__globals__, {"inspect_archive": inspect}), \
                mock.patch.object(sys, "argv", argv), \
                redirect_stderr(io.StringIO()), redirect_stdout(io.StringIO()):
            self.assertEqual(main(), 1)
        inspect.assert_not_called()
        self.assertEqual(self.archive.read_bytes(), b"input sentinel")

    def test_same_path_is_rejected_before_archive_inspection(self):
        self.assert_preflight_rejects(self.archive)

    def test_hard_link_is_rejected_before_archive_inspection(self):
        os.link(self.archive, self.output)
        self.assert_preflight_rejects(self.output)
        self.assertTrue(os.path.samefile(self.archive, self.output))

    @unittest.skipIf(os.name == "nt", "symbolic-link creation can require Windows privileges")
    def test_symbolic_link_is_rejected_before_archive_inspection(self):
        self.output.symlink_to(self.archive)
        self.assert_preflight_rejects(self.output)
        self.assertTrue(self.output.is_symlink())

    @unittest.skipIf(os.name == "nt", "symbolic-link creation can require Windows privileges")
    def test_parent_alias_cannot_target_the_input(self):
        alias = self.root / "alias"
        alias.symlink_to(self.root, target_is_directory=True)
        self.assert_preflight_rejects(alias / self.archive.name)

    def test_success_preserves_manifest_encoding_and_input(self):
        self.output.write_bytes(b"previous output")
        manifest = {"schema_version": 3, "name": "example", "files": []}
        INTAKE["write_manifest"](self.archive, self.output, manifest)
        expected = (json.dumps(manifest, ensure_ascii=True, separators=(",", ":")) + "\n").encode("ascii")
        self.assertEqual(self.output.read_bytes(), expected)
        self.assertEqual(self.archive.read_bytes(), b"input sentinel")
        self.assertEqual(sorted(p.name for p in self.root.iterdir()), ["client.zip", "manifest.json"])

    def test_publication_failure_preserves_previous_output(self):
        self.output.write_bytes(b"previous output")
        writer = INTAKE["write_manifest"]
        with mock.patch.object(writer.__globals__["os"], "replace", side_effect=OSError("fixture failure")):
            with self.assertRaises(OSError):
                writer(self.archive, self.output, {"schema_version": 3})
        self.assertEqual(self.output.read_bytes(), b"previous output")
        self.assertEqual(self.archive.read_bytes(), b"input sentinel")
        self.assertEqual(sorted(p.name for p in self.root.iterdir()), ["client.zip", "manifest.json"])


if __name__ == "__main__":
    unittest.main()
