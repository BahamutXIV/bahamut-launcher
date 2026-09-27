#!/usr/bin/env python3
"""Exercise the Unix package publisher with synthetic build and staging inputs."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SOURCE = Path(__file__).with_name("build-unix-package.sh")


@unittest.skipIf(os.name == "nt", "Unix package publication requires Bash on a Unix host")
class PublishSafetyTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.repo = self.root / "source with spaces"
        self.scripts = self.repo / "scripts"
        self.scripts.mkdir(parents=True)
        self.script = self.scripts / SOURCE.name
        shutil.copyfile(SOURCE, self.script)
        self.out = self.repo / "out"
        self.destination = self.out / "dev"
        self.external = self.root / "external"
        self.external.mkdir()
        self.sentinel = self.external / "sentinel"
        self.sentinel.write_bytes(b"keep")
        for directory in ["client-mingw-debug", "client-mingw"]:
            build = self.out / directory
            build.mkdir(parents=True)
            for name in ["bahamut-loader.exe", "bahamut.dll", "screenshot.dll", "discord-rpc.dll"]:
                (build / name).write_bytes(b"synthetic native output")
        for profile in ["debug", "release"]:
            launcher = self.repo / "target" / profile / "bahamut-launcher-shell"
            launcher.parent.mkdir(parents=True)
            self.executable(launcher, "#!/bin/sh\nexit 0\n")
        tools = self.root / "tools"
        tools.mkdir()
        self.executable(tools / "cmake", "#!/bin/sh\nexit 0\n")
        self.executable(
            tools / "cargo",
            "#!/bin/sh\necho '{\"packages\":[{\"name\":\"bahamut-launcher-shell\",\"version\":\"1.0.0\"}]}'\n",
        )
        self.executable(self.scripts / "stage-unix-release.sh", r"""#!/usr/bin/env bash
set -euo pipefail
destination=''
while [[ $# -gt 0 ]]; do
    case "$1" in
        --destination) destination="$2"; shift 2 ;;
        --launcher | --client-build) shift 2 ;;
        *) exit 2 ;;
    esac
done
[[ -n "$destination" ]]
mkdir -p "$destination/licenses" "$destination/scripts"
printf 'replacement launcher' > "$destination/bahamut-launcher"
printf 'new notice' > "$destination/licenses/library.txt"
printf 'default startup' > "$destination/scripts/default.txt"
""")
        self.environment = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ.get("PATH", ""))
        self.environment.pop("LLVM_MINGW_ROOT", None)

    @staticmethod
    def executable(path, text):
        path.write_text(text, encoding="utf-8")
        path.chmod(0o755)

    def run_publisher(self, *arguments):
        return subprocess.run(
            ["bash", str(self.script), "--skip-build", *arguments],
            env=self.environment, capture_output=True, text=True, timeout=30,
        )

    def assert_link_rejected(self, *arguments):
        result = self.run_publisher(*arguments)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("symbolic link", result.stderr)
        self.assertEqual(self.sentinel.read_bytes(), b"keep")

    def test_regular_publish_preserves_custom_files_and_startup(self):
        (self.destination / "scripts").mkdir(parents=True)
        (self.destination / "scripts/default.txt").write_bytes(b"custom startup")
        (self.destination / "custom.txt").write_bytes(b"custom file")
        result = self.run_publisher()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.destination / "bahamut-launcher").read_bytes(), b"replacement launcher")
        self.assertEqual((self.destination / "scripts/default.txt").read_bytes(), b"custom startup")
        self.assertEqual((self.destination / "custom.txt").read_bytes(), b"custom file")
        self.assertFalse(list(self.out.glob("package-staging.*")))

    def test_linked_output_root_is_rejected(self):
        # This is a newly created fixture tree, not a user checkout.
        shutil.rmtree(self.out)
        self.out.symlink_to(self.external, target_is_directory=True)
        self.assert_link_rejected()
        self.assertEqual(sorted(p.name for p in self.external.iterdir()), ["sentinel"])

    def test_linked_package_root_is_rejected(self):
        self.destination.symlink_to(self.external, target_is_directory=True)
        self.assert_link_rejected()
        self.assertEqual(sorted(p.name for p in self.external.iterdir()), ["sentinel"])

    def test_linked_release_parent_is_rejected(self):
        (self.out / "release").symlink_to(self.external, target_is_directory=True)
        self.assert_link_rejected("--release")
        self.assertEqual(sorted(p.name for p in self.external.iterdir()), ["sentinel"])

    def test_linked_destination_directory_is_rejected(self):
        self.destination.mkdir()
        (self.destination / "licenses").symlink_to(self.external, target_is_directory=True)
        self.assert_link_rejected()
        self.assertFalse((self.external / "library.txt").exists())

    def test_linked_destination_file_is_rejected(self):
        self.destination.mkdir()
        (self.destination / "bahamut-launcher").symlink_to(self.sentinel)
        self.assert_link_rejected()

    def test_dangling_destination_file_is_rejected(self):
        self.destination.mkdir()
        missing = self.external / "missing"
        (self.destination / "bahamut-launcher").symlink_to(missing)
        self.assert_link_rejected()
        self.assertFalse(missing.exists())

    def test_preserved_startup_link_is_still_rejected(self):
        (self.destination / "scripts").mkdir(parents=True)
        (self.destination / "scripts/default.txt").symlink_to(self.sentinel)
        self.assert_link_rejected()


if __name__ == "__main__":
    unittest.main()
