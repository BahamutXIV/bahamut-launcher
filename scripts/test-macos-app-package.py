#!/usr/bin/env python3
"""Exercise the macOS app packager with a synthetic repository and staged tree."""

import json
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
import zipfile


SOURCE = Path(__file__).with_name("package-macos-app.sh")
CHECKER_SOURCE = Path(__file__).with_name("check-macos-app-zip.sh")
REPO = SOURCE.resolve().parent.parent
APP = "Bahamut Launcher.app"
VERSION = "1.2.3"
# The macOS system Bash, which the packager must support.
BASH = "/bin/bash"

# The stub stage script writes these beside a copy of --launcher, in the shape
# stage-unix-release.sh stages.
STAGED_FILES = {
    "bahamut-loader.exe": "synthetic loader",
    "bahamut.dll": "synthetic client module",
    "plugins/screenshot.dll": "synthetic screenshot plugin",
    "plugins/discord-rpc.dll": "synthetic discord plugin",
    "addons/x/addon.toml": "synthetic addon manifest",
    "scripts/default.txt": "default startup",
    "licenses/a.txt": "synthetic notice",
    "LICENSE.md": "synthetic license",
    "README.md": "synthetic readme",
}
SKELETON_DIRS = ["config", "logs/launcher", "screenshots"]

STAGE_STUB_HEAD = r"""#!/usr/bin/env bash
set -euo pipefail
launcher=''
destination=''
while [ "$#" -gt 0 ]; do
    case "$1" in
        --launcher) launcher="$2"; shift 2 ;;
        --destination) destination="$2"; shift 2 ;;
        --client-build) shift 2 ;;
        *) exit 2 ;;
    esac
done
[ -n "$launcher" ] && [ -n "$destination" ]
"""


def run(*command, env=None):
    return subprocess.run(
        [str(part) for part in command], env=env, capture_output=True, text=True, timeout=120,
    )


def bundle_files(app):
    return {path.relative_to(app).as_posix() for path in app.rglob("*") if path.is_file()}


@unittest.skipUnless(sys.platform == "darwin", "the app bundle needs codesign, plutil, xattr, and lipo")
class AppBundleTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        build = tempfile.TemporaryDirectory()
        cls.addClassCleanup(build.cleanup)
        source = Path(build.name) / "placeholder.c"
        source.write_text("int main(void) { return 0; }\n", encoding="utf-8")
        cls.placeholder = Path(build.name) / "placeholder"
        # A source file, not stdin: clang compiles once per -arch and the second
        # compile would read empty input.
        result = run("cc", "-arch", "arm64", "-arch", "x86_64", source, "-o", cls.placeholder)
        if result.returncode != 0:
            raise RuntimeError("universal placeholder build failed:\n" + result.stderr)

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.repo = self.root / "source with spaces"
        self.scripts = self.repo / "scripts"
        self.scripts.mkdir(parents=True)
        self.script = self.scripts / SOURCE.name
        shutil.copyfile(SOURCE, self.script)
        self.checker = self.scripts / CHECKER_SOURCE.name
        shutil.copyfile(CHECKER_SOURCE, self.checker)
        self.write_stage_stub()
        packaging = self.repo / "packaging" / "macos"
        packaging.mkdir(parents=True)
        for name in ["Info.plist.in", "entitlements.plist"]:
            shutil.copyfile(REPO / "packaging" / "macos" / name, packaging / name)
        icons = self.repo / "src-tauri" / "icons"
        icons.mkdir(parents=True)
        (icons / "icon.icns").write_bytes(b"synthetic icon")
        self.tauri = json.loads((REPO / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))
        self.write_version(VERSION)
        self.client_build = self.root / "client build"
        self.client_build.mkdir()
        self.destination = self.root / "out" / "app output"
        self.app = self.destination / APP

    @staticmethod
    def executable(path, text):
        path.write_text(text, encoding="utf-8")
        path.chmod(0o755)

    def write_stage_stub(self, extra=""):
        parents = {str(Path(rel).parent) for rel in STAGED_FILES if "/" in rel}
        lines = [STAGE_STUB_HEAD]
        for directory in sorted(parents | set(SKELETON_DIRS)):
            lines.append(f'mkdir -p "$destination/{directory}"\n')
        lines.append('cp "$launcher" "$destination/bahamut-launcher"\n')
        lines.append('chmod 700 "$destination/bahamut-launcher"\n')
        for rel, text in STAGED_FILES.items():
            lines.append(f"printf '%s' '{text}' > \"$destination/{rel}\"\n")
        lines.append(extra + "\n")
        self.executable(self.scripts / "stage-unix-release.sh", "".join(lines))

    def write_version(self, version):
        config = {
            "productName": self.tauri["productName"],
            "version": version,
            "identifier": self.tauri["identifier"],
        }
        (self.repo / "src-tauri" / "tauri.conf.json").write_text(json.dumps(config), encoding="utf-8")

    def run_packager(self, *arguments, env=None):
        return run(
            BASH, self.script,
            "--launcher", self.placeholder,
            "--client-build", self.client_build,
            "--destination", self.destination,
            *arguments, env=env,
        )

    def package(self, *arguments):
        result = self.run_packager(*arguments)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result

    def run_checker(self, *arguments):
        return run(BASH, self.checker, *arguments)

    def zip_bundle(self, app, archive, *ditto_flags):
        result = run("ditto", "-c", "-k", *ditto_flags, "--keepParent", app, archive)
        self.assertEqual(result.returncode, 0, result.stderr)
        return archive

    def copy_app(self, destination_dir):
        destination_dir.mkdir(parents=True, exist_ok=True)
        copy = destination_dir / APP
        result = run("ditto", self.app, copy)
        self.assertEqual(result.returncode, 0, result.stderr)
        return copy

    def verify(self, app):
        return run("codesign", "--verify", "--strict", "--verbose=2", app)

    def plist_value(self, key):
        result = run("plutil", "-extract", key, "raw", "-o", "-", self.app / "Contents" / "Info.plist")
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def test_bundle_holds_exact_manifest(self):
        result = self.package()
        expected = {
            "Contents/Info.plist",
            "Contents/MacOS/bahamut-launcher",
            "Contents/Resources/icon.icns",
            "Contents/_CodeSignature/CodeResources",
        } | {f"Contents/Resources/{rel}" for rel in STAGED_FILES}
        self.assertEqual(bundle_files(self.app), expected)
        self.assertEqual(sorted(p.name for p in self.destination.iterdir()), [APP])
        empty = [p for p in self.app.rglob("*") if p.is_dir() and not any(p.iterdir())]
        self.assertEqual(empty, [])
        resources = self.app / "Contents" / "Resources"
        for directory in ["config", "logs", "screenshots"]:
            self.assertFalse((resources / directory).exists(), directory)
        self.assertTrue((resources / "scripts").is_dir())
        self.assertIn(f"PASS ({len(expected)} files, ad hoc signature)", result.stdout)

    def test_launcher_is_universal_executable(self):
        self.package()
        launcher = self.app / "Contents" / "MacOS" / "bahamut-launcher"
        self.assertEqual(stat.S_IMODE(launcher.stat().st_mode), 0o755)
        result = run("lipo", "-archs", launcher)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sorted(result.stdout.split()), ["arm64", "x86_64"])

    def test_info_plist_matches_tauri_config(self):
        self.package()
        self.assertEqual(self.plist_value("CFBundleExecutable"), "bahamut-launcher")
        self.assertEqual(self.plist_value("CFBundleIdentifier"), self.tauri["identifier"])
        self.assertEqual(self.plist_value("CFBundleName"), self.tauri["productName"])
        self.assertEqual(self.plist_value("CFBundleShortVersionString"), VERSION)
        self.assertEqual(self.plist_value("CFBundleVersion"), VERSION)

    def test_staged_link_or_special_file_is_rejected(self):
        cases = [
            ('ln -s ../LICENSE.md "$destination/licenses/linked.txt"', "symbolic link"),
            ('ln -s ../plugins "$destination/addons/linked"', "symbolic link"),
            ('mkfifo "$destination/licenses/pipe"', "not a regular file"),
        ]
        for extra, message in cases:
            with self.subTest(extra=extra):
                shutil.rmtree(self.destination, ignore_errors=True)
                self.write_stage_stub(extra)
                result = self.run_packager()
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn(message, result.stderr)
                self.assertFalse(self.app.exists())

    def test_staged_icon_collision_is_rejected(self):
        self.write_stage_stub("printf 'staged icon' > \"$destination/icon.icns\"")
        result = self.run_packager()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("collides with the bundle icon", result.stderr)
        self.assertFalse(self.app.exists())

    def test_finder_info_is_cleared_before_signing(self):
        finder_info = "00000000000000000004" + "0" * 44
        self.write_stage_stub(f'xattr -wx com.apple.FinderInfo {finder_info} "$destination/README.md"')
        self.package()
        readme = self.app / "Contents" / "Resources" / "README.md"
        self.assertNotIn("com.apple.FinderInfo", run("xattr", readme).stdout)
        result = self.verify(self.app)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_non_empty_destination_is_rejected(self):
        self.destination.mkdir(parents=True)
        keep = self.destination / "keep.txt"
        keep.write_bytes(b"keep")
        result = self.run_packager()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("destination is not empty", result.stderr)
        self.assertEqual(sorted(p.name for p in self.destination.iterdir()), ["keep.txt"])
        self.assertEqual(keep.read_bytes(), b"keep")

    def test_repeat_run_keeps_the_existing_bundle(self):
        self.package()
        result = self.run_packager()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("destination is not empty", result.stderr)
        self.assertEqual(self.verify(self.app).returncode, 0)

    def test_non_semver_version_is_rejected(self):
        for version in ["1.2", "1.2.3.4", "v1.2.3", "1.2.3-rc.1", "1.2.3\n", " 1.2.3", None]:
            with self.subTest(version=version):
                self.write_version(version)
                result = self.run_packager()
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn("is not MAJOR.MINOR.PATCH", result.stderr)
                self.assertFalse(self.destination.exists())

    def test_signature_verifies_strictly(self):
        for arguments in [[], ["--sign", "-"]]:
            with self.subTest(arguments=arguments):
                shutil.rmtree(self.destination, ignore_errors=True)
                self.package(*arguments)
                result = self.verify(self.app)
                self.assertEqual(result.returncode, 0, result.stderr)
                details = run("codesign", "-dv", self.app)
                self.assertIn("Signature=adhoc", details.stderr)
                self.assertIn(f"Identifier={self.tauri['identifier']}", details.stderr)

    def test_unknown_signing_identity_fails_closed(self):
        result = self.run_packager("--sign", "bahamut-launcher test identity that does not exist")
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(self.app.exists())

    def test_seal_survives_ditto_zip_round_trip(self):
        self.package()
        archive = self.root / "app.zip"
        result = run("ditto", "-c", "-k", "--norsrc", "--keepParent", self.app, archive)
        self.assertEqual(result.returncode, 0, result.stderr)
        with zipfile.ZipFile(archive) as bundle_zip:
            names = bundle_zip.namelist()
        self.assertTrue(names)
        for name in names:
            self.assertTrue(name.startswith(APP + "/"), name)
            parts = name.rstrip("/").split("/")
            self.assertFalse(any(p.startswith("._") or p == "__MACOSX" for p in parts), name)
        extracted = self.root / "extracted"
        extracted.mkdir()
        result = run("ditto", "-x", "-k", archive, extracted)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sorted(p.name for p in extracted.iterdir()), [APP])
        self.assertEqual(bundle_files(extracted / APP), bundle_files(self.app))
        result = self.verify(extracted / APP)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_writing_into_resources_breaks_seal(self):
        self.package()
        self.assertEqual(self.verify(self.app).returncode, 0)
        (self.app / "Contents" / "Resources" / "extra.txt").write_bytes(b"written after signing")
        result = self.verify(self.app)
        self.assertNotEqual(result.returncode, 0, result.stderr)

    def test_non_darwin_host_is_refused(self):
        tools = self.root / "tools"
        tools.mkdir()
        self.executable(tools / "uname", "#!/bin/sh\necho Linux\n")
        env = {"PATH": f"{tools}:/usr/bin:/bin"}
        result = self.run_packager(env=env)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("macOS only", result.stderr)
        self.assertFalse(self.destination.exists())

    def test_checker_accepts_clean_ditto_zip(self):
        self.package()
        archive = self.zip_bundle(self.app, self.root / "app.zip", "--norsrc")
        result = self.run_checker("--zip", archive)
        self.assertEqual(result.returncode, 0, result.stderr)
        last_line = result.stdout.rstrip("\n").splitlines()[-1]
        self.assertTrue(last_line.startswith(f"{CHECKER_SOURCE.name}: PASS ("), result.stdout)

    def test_checker_rejects_apple_double_metadata(self):
        self.package()
        readme = self.app / "Contents" / "Resources" / "README.md"
        run("xattr", "-wx", "com.apple.FinderInfo", "0" * 64, readme)
        archive = self.zip_bundle(self.app, self.root / "app.zip")
        with zipfile.ZipFile(archive) as bundle_zip:
            has_metadata = any(
                "__MACOSX" in name or "/._" in f"/{name}" for name in bundle_zip.namelist()
            )
        if not has_metadata:
            archive = self.root / "handmade.zip"
            with zipfile.ZipFile(archive, "w") as bundle_zip:
                bundle_zip.writestr(f"{APP}/Contents/Info.plist", "stand-in")
                bundle_zip.writestr(f"{APP}/Contents/._Info.plist", "\x00" * 4)
        result = self.run_checker("--zip", archive)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("metadata", result.stderr)

    def test_checker_rejects_entry_outside_bundle(self):
        self.package()
        archive = self.root / "app.zip"
        with zipfile.ZipFile(archive, "w") as bundle_zip:
            for path in self.app.rglob("*"):
                if path.is_file():
                    bundle_zip.write(path, f"{APP}/{path.relative_to(self.app).as_posix()}")
            bundle_zip.writestr("stray.txt", "outside the bundle")
        result = self.run_checker("--zip", archive)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("outside the app bundle", result.stderr)

    def test_checker_rejects_broken_seal(self):
        self.package()
        tampered = self.copy_app(self.root / "tampered")
        (tampered / "Contents" / "Resources" / "extra.txt").write_bytes(b"written after signing")
        archive = self.zip_bundle(tampered, self.root / "app.zip", "--norsrc")
        result = self.run_checker("--zip", archive)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("invalid", result.stderr)

    def test_checker_rejects_single_architecture_executable(self):
        self.package()
        tampered = self.copy_app(self.root / "tampered")
        single_arch = self.root / "single-arch"
        result = run("lipo", "-thin", "arm64", self.placeholder, "-output", single_arch)
        self.assertEqual(result.returncode, 0, result.stderr)
        launcher = tampered / "Contents" / "MacOS" / "bahamut-launcher"
        shutil.copyfile(single_arch, launcher)
        launcher.chmod(0o755)
        result = run("codesign", "--force", "--sign", "-", tampered)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.verify(tampered).returncode, 0, self.verify(tampered).stderr)
        archive = self.zip_bundle(tampered, self.root / "app.zip", "--norsrc")
        result = self.run_checker("--zip", archive)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("is missing the", result.stderr)
        self.assertIn("slice", result.stderr)

    def test_checker_zip_argument_errors(self):
        missing = self.root / "missing.zip"
        result = self.run_checker("--zip", missing)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("archive not found", result.stderr)
        result = self.run_checker()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("usage:", result.stderr)


if __name__ == "__main__":
    unittest.main()
