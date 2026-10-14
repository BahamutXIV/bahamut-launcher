#!/usr/bin/env python3
"""Exercise the Flatpak release archive packager with a synthetic bundle."""

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import unittest
import zipfile


SCRIPTS = Path(__file__).resolve().parent
REPO = SCRIPTS.parent
PACKAGER = SCRIPTS / "package-flatpak-archive.py"
SHIPPED_README = REPO / "packaging" / "flatpak" / "README.md"
MANIFEST = REPO / "packaging" / "flatpak" / "io.github.BahamutXIV.Launcher.Tester.yml"
BUILD_SCRIPT = SCRIPTS / "build-flatpak.sh"
RELEASE_WORKFLOW = REPO / ".github" / "workflows" / "release-binaries.yml"
FINISH_ARGS = [
    "--require-version=1.15.6",
    "--share=network",
    "--share=ipc",
    "--socket=fallback-x11",
    "--socket=wayland",
    "--socket=pulseaudio",
    "--device=dri",
    "--device=input",
    "--allow=multiarch",
    "--filesystem=~/Games:create",
]
TOP = "bahamut-launcher-flatpak"
LABEL = "bahamut-launcher-test-linux-flatpak"
BUNDLE = "bahamut-launcher-test-linux-flatpak.flatpak"
EPOCH = "1700000000"
README_TEXT = b"# Bahamut Launcher Flatpak\n\nSynthetic README for the packager test.\n"


def zip_stamp(epoch):
    """The zip header time for EPOCH: UTC with the 2-second DOS resolution."""
    parts = time.gmtime(int(epoch))
    return (parts.tm_year, parts.tm_mon, parts.tm_mday, parts.tm_hour, parts.tm_min, parts.tm_sec & ~1)


def run(*command, cwd=None, env=None):
    return subprocess.run(
        [str(part) for part in command], cwd=cwd, env=env, capture_output=True, text=True, timeout=120,
    )


class FlatpakArchiveTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.inputs = self.root / "flatpak out"
        self.inputs.mkdir()
        self.bundle = self.inputs / BUNDLE
        self.bundle.write_bytes(b"synthetic flatpak bundle\x00" * 4096)
        digest = hashlib.sha256(self.bundle.read_bytes()).hexdigest()
        self.checksum = self.inputs / f"{BUNDLE}.sha256"
        self.checksum.write_text(f"{digest}  {BUNDLE}\n", encoding="ascii")
        self.identity = self.inputs / f"{BUNDLE}.identity.json"
        self.identity.write_text(
            json.dumps(
                {
                    "schema": 1,
                    "app_id": "io.github.BahamutXIV.Launcher.Tester",
                    "branch": "s0",
                    "artifact": {"filename": BUNDLE, "format": "flatpak", "sha256": digest},
                    "source": {"launcher_version": "v9.9.9", "release_tag": "v9.9.9"},
                },
                indent=2,
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )
        self.readme = self.root / "README.md"
        self.readme.write_bytes(README_TEXT)
        self.env = dict(os.environ, SOURCE_DATE_EPOCH=EPOCH)
        self.env.pop("PYTHONPATH", None)

    def package(self, output, bundle=None, readme=None, label=LABEL, env=None):
        return run(
            sys.executable, PACKAGER,
            "--bundle", bundle or self.bundle,
            "--readme", readme or self.readme,
            f"--label={label}",
            "--output", output,
            env=self.env if env is None else env,
        )

    def assert_ok(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def assert_refused(self, result, text):
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("package-flatpak-archive.py: ", result.stderr)
        self.assertIn(text, result.stderr)

    def expected_members(self):
        return [f"{TOP}/{name}" for name in sorted(["README.md", BUNDLE, f"{BUNDLE}.sha256", f"{BUNDLE}.identity.json"])]

    def test_archive_layout_and_sidecar(self):
        output = self.root / "dist"
        result = self.assert_ok(self.package(output))
        archive = output / f"{LABEL}.zip"
        self.assertIn(f"PASS (4 files): {archive}", result.stdout)
        self.assertEqual(sorted(p.name for p in output.iterdir()), [f"{LABEL}.zip", f"{LABEL}.zip.sha256"])
        with zipfile.ZipFile(archive) as zipped:
            self.assertIsNone(zipped.testzip())
            infos = zipped.infolist()
            self.assertEqual([info.filename for info in infos], self.expected_members())
            self.assertEqual({info.filename.split("/", 1)[0] for info in infos}, {TOP})
            stamp = zip_stamp(EPOCH)
            for info in infos:
                self.assertFalse(info.is_dir(), info.filename)
                self.assertEqual(info.compress_type, zipfile.ZIP_DEFLATED, info.filename)
                self.assertEqual(info.external_attr >> 16, 0o100644, info.filename)
                self.assertEqual(info.date_time, stamp, info.filename)
            self.assertEqual(zipped.read(f"{TOP}/README.md"), README_TEXT)
            self.assertEqual(zipped.read(f"{TOP}/{BUNDLE}"), self.bundle.read_bytes())
            self.assertEqual(zipped.read(f"{TOP}/{BUNDLE}.sha256"), self.checksum.read_bytes())
            self.assertEqual(zipped.read(f"{TOP}/{BUNDLE}.identity.json"), self.identity.read_bytes())
        sidecar = (output / f"{LABEL}.zip.sha256").read_text(encoding="ascii")
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        self.assertEqual(sidecar, f"{digest}  {LABEL}.zip\n")

    def test_archive_bytes_are_deterministic(self):
        first = self.root / "first"
        second = self.root / "second"
        self.assert_ok(self.package(first))
        self.assert_ok(self.package(second))
        self.assertEqual((first / f"{LABEL}.zip").read_bytes(), (second / f"{LABEL}.zip").read_bytes())
        self.assertEqual(
            (first / f"{LABEL}.zip.sha256").read_bytes(), (second / f"{LABEL}.zip.sha256").read_bytes(),
        )

    def test_head_commit_time_is_the_fallback_mtime(self):
        head = run("git", "-C", REPO, "log", "-1", "--format=%ct")
        if head.returncode != 0 or not head.stdout.strip():
            self.skipTest("the repository HEAD commit time is not readable")
        env = dict(self.env)
        env.pop("SOURCE_DATE_EPOCH")
        output = self.root / "dist"
        self.assert_ok(self.package(output, env=env))
        stamp = zip_stamp(head.stdout.strip())
        with zipfile.ZipFile(output / f"{LABEL}.zip") as zipped:
            for info in zipped.infolist():
                self.assertEqual(info.date_time, stamp, info.filename)

    def test_bad_source_date_epoch_is_refused(self):
        env = dict(self.env, SOURCE_DATE_EPOCH="soon")
        self.assert_refused(self.package(self.root / "dist", env=env), "whole number of seconds")
        self.assertFalse((self.root / "dist" / f"{LABEL}.zip").exists())

    def test_tampered_checksum_sidecar_is_refused(self):
        self.checksum.write_text("0" * 64 + f"  {BUNDLE}\n", encoding="ascii")
        output = self.root / "dist"
        self.assert_refused(self.package(output), "checksum sidecar does not match the bundle")
        self.assertEqual(list(output.iterdir()), [])

    def test_single_space_checksum_sidecar_is_accepted(self):
        digest, _ = self.checksum.read_text(encoding="ascii").split()
        self.checksum.write_text(f"{digest} {BUNDLE}\n", encoding="ascii")
        self.assert_ok(self.package(self.root / "dist"))

    def test_checksum_sidecar_naming_another_file_is_refused(self):
        digest, _ = self.checksum.read_text(encoding="ascii").split()
        self.checksum.write_text(f"{digest}  other.flatpak\n", encoding="ascii")
        self.assert_refused(self.package(self.root / "dist"), "names other.flatpak")

    def test_tampered_identity_sidecar_is_refused(self):
        document = json.loads(self.identity.read_text(encoding="utf-8"))
        document["artifact"]["sha256"] = "f" * 64
        self.identity.write_text(json.dumps(document), encoding="utf-8")
        self.assert_refused(self.package(self.root / "dist"), "identity sidecar does not match the bundle")
        self.identity.write_text("{}", encoding="utf-8")
        self.assert_refused(self.package(self.root / "dist"), "artifact.sha256")

    def test_tampered_bundle_is_refused(self):
        self.bundle.write_bytes(self.bundle.read_bytes() + b"x")
        self.assert_refused(self.package(self.root / "dist"), "does not match the bundle")

    def test_missing_sidecar_is_refused(self):
        self.identity.unlink()
        self.assert_refused(self.package(self.root / "dist"), "identity sidecar is missing")
        self.identity.write_text("{}", encoding="utf-8")
        self.checksum.unlink()
        self.assert_refused(self.package(self.root / "dist"), "checksum sidecar is missing")

    def test_existing_output_is_refused(self):
        output = self.root / "dist"
        self.assert_ok(self.package(output))
        archive = output / f"{LABEL}.zip"
        before = archive.read_bytes()
        self.assert_refused(self.package(output), f"refusing to overwrite {archive}")
        self.assertEqual(archive.read_bytes(), before)
        archive.unlink()
        self.assert_refused(self.package(output), f"refusing to overwrite {output / f'{LABEL}.zip.sha256'}")
        self.assertFalse(archive.exists())

    def test_symlinked_inputs_are_refused(self):
        linked = self.root / "linked"
        linked.mkdir()
        for name in [BUNDLE, f"{BUNDLE}.sha256", f"{BUNDLE}.identity.json"]:
            (linked / name).symlink_to(self.inputs / name)
        self.assert_refused(self.package(self.root / "dist", bundle=linked / BUNDLE), "bundle is a symbolic link")
        (linked / BUNDLE).unlink()
        (linked / BUNDLE).write_bytes(self.bundle.read_bytes())
        self.assert_refused(self.package(self.root / "dist", bundle=linked / BUNDLE), "checksum sidecar is a symbolic link")
        readme_link = self.root / "README-link.md"
        readme_link.symlink_to(self.readme)
        self.assert_refused(self.package(self.root / "dist", readme=readme_link), "README is a symbolic link")
        output_link = self.root / "dist-link"
        output_link.symlink_to(self.root / "dist-target")
        self.assert_refused(self.package(output_link), "output is a symbolic link")
        self.assertFalse((self.root / "dist").exists())

    def test_label_and_bundle_name_are_validated(self):
        for label in [".hidden", "-x", "with space", "a/b", ""]:
            with self.subTest(label=label):
                self.assert_refused(self.package(self.root / "dist", label=label), "the label must be")
        other = self.inputs / "bundle.bin"
        other.write_bytes(self.bundle.read_bytes())
        self.assert_refused(self.package(self.root / "dist", bundle=other), "must end in .flatpak")
        self.assertFalse((self.root / "dist").exists())

    @unittest.skipUnless(SHIPPED_README.is_file(), "packaging/flatpak/README.md is not present")
    def test_shipped_readme_is_packaged_unchanged(self):
        output = self.root / "dist"
        self.assert_ok(self.package(output, readme=SHIPPED_README))
        with zipfile.ZipFile(output / f"{LABEL}.zip") as zipped:
            self.assertEqual(zipped.read(f"{TOP}/README.md"), SHIPPED_README.read_bytes())


class FlatpakPermissionTests(unittest.TestCase):
    """The manifest permissions and the Flatpak minimum version, read as text."""

    def manifest_finish_args(self, text=None):
        lines = (MANIFEST.read_text(encoding="utf-8") if text is None else text).splitlines()
        start = lines.index("finish-args:")
        args = []
        for line in lines[start + 1:]:
            if line.strip().startswith("#"):
                continue
            match = re.fullmatch(r"  - (--\S+)", line)
            if not match:
                break
            args.append(match.group(1))
        return args

    def test_finish_args_are_exact(self):
        args = self.manifest_finish_args()
        self.assertEqual(args, FINISH_ARGS)
        for forbidden in ["--device=all", "--filesystem=home", "--filesystem=host"]:
            self.assertNotIn(forbidden, args)

    def test_input_grant_removal_is_detected(self):
        text = MANIFEST.read_text(encoding="utf-8").replace("  - --device=input\n", "", 1)
        self.assertNotEqual(self.manifest_finish_args(text), FINISH_ARGS)

    def test_minimum_flatpak_version_matches_everywhere(self):
        required = [arg for arg in self.manifest_finish_args() if arg.startswith("--require-version=")]
        self.assertEqual(len(required), 1)
        version = required[0].split("=", 1)[1]
        script = re.search(r'^readonly min_flatpak_version="([^"]+)"$', BUILD_SCRIPT.read_text(encoding="utf-8"), re.M)
        self.assertIsNotNone(script, "build-flatpak.sh does not define min_flatpak_version")
        self.assertEqual(script.group(1), version)
        workflow = re.findall(r"^\s+minimum=(\S+)$", RELEASE_WORKFLOW.read_text(encoding="utf-8"), re.M)
        self.assertEqual(workflow, [version])

    def run_build_script(self, flatpak_version, *arguments):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name).resolve()
        stubs = root / "bin"
        stubs.mkdir()
        for name, body in {
            "flatpak": f'echo "Flatpak {flatpak_version}"',
            "flatpak-builder": "exit 0",
            "cargo": "exit 0",
        }.items():
            stub = stubs / name
            stub.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
            stub.chmod(0o755)
        output = root / "out"
        env = dict(os.environ, PATH=f"{stubs}{os.pathsep}{os.environ['PATH']}")
        result = run("bash", BUILD_SCRIPT, "--output", output, *arguments, env=env)
        return result, output

    def test_build_script_rejects_an_old_flatpak(self):
        result, output = self.run_build_script("1.14.6")
        self.assertEqual(result.returncode, 1)
        self.assertIn("Flatpak 1.15.6 or newer is required", result.stderr)
        self.assertIn("found 1.14.6", result.stderr)
        self.assertFalse(output.exists())

    def test_build_script_accepts_the_minimum_flatpak(self):
        for version in ["1.15.6", "1.16.6", "1.18.4"]:
            with self.subTest(version=version):
                result, _ = self.run_build_script(version, "--source-ref", "no-such-ref-for-this-test")
                self.assertEqual(result.returncode, 1)
                self.assertNotIn("or newer is required", result.stderr)
                self.assertIn("source ref is not a commit", result.stderr)


if __name__ == "__main__":
    unittest.main()
