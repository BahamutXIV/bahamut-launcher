#!/usr/bin/env python3
"""Exercise the Linux archive packager, install.sh, and install-dependencies.sh."""

import hashlib
import json
import os
from pathlib import Path
import pwd
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parent
REPO = SCRIPTS.parent
PACKAGER = SCRIPTS / "package-linux-tarball.sh"
LINUX = REPO / "packaging" / "linux"
TOP = "bahamut-launcher"
LABEL = "bahamut-launcher-test-linux-x86_64"
PE_NAMES = ["bahamut-loader.exe", "bahamut.dll", "screenshot.dll", "discord-rpc.dll"]
ADDONS = ["chatlogs", "zonename", "packetlogger", "combatparser", "distance", "targethp", "fps", "pos", "wiki"]
LICENSES = [
    "MinHook-LICENSE.txt",
    "Dear-ImGui-LICENSE.txt",
    "Lua-COPYRIGHT.txt",
    "Inter-OFL.txt",
    "Cinzel-OFL.txt",
    "JetBrainsMono-OFL.txt",
    "Miniz-LICENSE.txt",
    "MinGW-w64-runtime-COPYING.txt",
]
ICON_SIZES = ["48x48", "128x128", "256x256"]
EXECUTABLES = {"bahamut-launcher", "install.sh", "install-dependencies.sh"}
INSTALLER_KEY = "X-Bahamut-Launcher-Installer=true"
MANIFEST = ".bahamut-launcher-install-manifest"
FILE_LIST = ".bahamut-launcher-install-files"
MARKER = ".bahamut-launcher-package"
MOVE_DOC = "https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/troubleshooting.md#moving-a-beside-the-launcher-tree"
PLACEHOLDER = "#!/bin/sh\nexit 0\n"
PACKAGE_TOOLS = ["sudo", "doas", "dpkg", "apt", "dnf", "pacman", "zypper", "emerge"]


def run(*command, cwd=None, env=None):
    return subprocess.run(
        [str(part) for part in command], cwd=cwd, env=env, capture_output=True, text=True, timeout=120,
    )


def expected_manifest():
    files = {
        "bahamut-launcher",
        ".bahamut-launcher-package",
        "bahamut-loader.exe",
        "bahamut.dll",
        "plugins/screenshot.dll",
        "plugins/discord-rpc.dll",
        "scripts/default.txt",
        "LICENSE.md",
        "README.md",
        "install.sh",
        "install-dependencies.sh",
        "Makefile",
        "share/applications/bahamut-launcher.desktop",
    }
    files |= {f"licenses/{name}" for name in LICENSES}
    for addon in ADDONS:
        files |= {f"addons/{addon}/addon.toml", f"addons/{addon}/{addon}.lua"}
    overlay = REPO / "plugins" / "dats" / "bahamut-dats-overlay"
    files |= {p.relative_to(REPO).as_posix() for p in overlay.rglob("*") if p.is_file()}
    files |= {f"share/icons/hicolor/{size}/apps/bahamut-launcher.png" for size in ICON_SIZES}
    return files


def tree_files(root):
    return {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file() or p.is_symlink()}


def snapshot(root):
    """Every entry under ROOT with its type and content, to prove nothing changed."""
    entries = {}
    for path in [root, *root.rglob("*")]:
        rel = path.relative_to(root).as_posix()
        if path.is_symlink():
            entries[rel] = ("link", os.readlink(path))
        elif path.is_dir():
            entries[rel] = ("dir", None)
        else:
            entries[rel] = ("file", path.read_bytes())
    return entries


def path_without(directory, *excluded):
    """A PATH directory linking every command on PATH except EXCLUDED."""
    directory.mkdir()
    for entry in os.environ.get("PATH", "").split(os.pathsep):
        if not entry or not os.path.isdir(entry):
            continue
        for name in sorted(os.listdir(entry)):
            source = os.path.join(entry, name)
            target = directory / name
            if name in excluded or target.exists() or target.is_symlink():
                continue
            if os.path.isfile(source) and os.access(source, os.X_OK):
                target.symlink_to(source)
    return str(directory)


def script_loaders():
    """The glibc_loaders list install-dependencies.sh probes."""
    text = (LINUX / "install-dependencies.sh").read_text(encoding="utf-8")
    return re.search(r"^glibc_loaders=\(([^)]*)\)", text, re.M).group(1).split()


def desktop_values(path):
    values = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if "=" in line and not line.startswith("#"):
            key, value = line.split("=", 1)
            values[key] = value
    return values


@unittest.skipUnless(sys.platform.startswith("linux"), "the Linux archive needs GNU tar and a Linux host")
class LinuxPackageTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        work = tempfile.TemporaryDirectory()
        cls.addClassCleanup(work.cleanup)
        cls.work = Path(work.name).resolve()
        client = cls.work / "client build"
        client.mkdir()
        for name in PE_NAMES:
            (client / name).write_bytes(b"synthetic native output")
        launcher = cls.work / "placeholder"
        launcher.write_text(PLACEHOLDER, encoding="utf-8")
        launcher.chmod(0o755)
        cls.dist = cls.work / "dist"
        env = dict(os.environ, SOURCE_DATE_EPOCH="1700000000")
        result = run(
            "bash", PACKAGER, "--launcher", launcher, "--client-build", client,
            "--label", LABEL, "--output", cls.dist, env=env,
        )
        if result.returncode != 0:
            raise RuntimeError("package-linux-tarball.sh failed:\n" + result.stderr)
        cls.packager_stdout = result.stdout
        cls.archive = cls.dist / f"{LABEL}.tar.gz"

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        extracted = self.root / "extracted"
        extracted.mkdir()
        with tarfile.open(self.archive) as archive:
            archive.extractall(extracted, filter="tar")
        self.payload = extracted / TOP
        self.env = dict(os.environ)
        for name in ["PREFIX", "PKGDIR", "DESTDIR", "BAHAMUT_WINE"]:
            self.env.pop(name, None)

    def install(self, prefix, *arguments, script=None, cwd=None):
        script = script or self.payload / "install.sh"
        return run(script, "--prefix", prefix, *arguments, cwd=cwd, env=self.env)

    def assert_ok(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def assert_no_launcher_files(self, prefix):
        leftovers = [p for p in prefix.rglob("*") if not p.is_dir() or p.is_symlink()]
        self.assertEqual(leftovers, [])

    def test_archive_layout(self):
        self.assertIn(f"PASS ({len(expected_manifest())} files)", self.packager_stdout)
        with tarfile.open(self.archive) as archive:
            members = archive.getmembers()
        names = {m.name.rstrip("/") for m in members}
        self.assertEqual({name.split("/", 1)[0] for name in names}, {TOP})
        files = {m.name[len(TOP) + 1:]: m for m in members if m.isfile()}
        self.assertEqual(set(files), expected_manifest())
        for member in members:
            self.assertTrue(member.isfile() or member.isdir(), member.name)
            self.assertEqual((member.uid, member.gid), (0, 0), member.name)
            self.assertEqual(member.mtime, 1700000000, member.name)
            if member.isdir():
                self.assertEqual(member.mode, 0o755, member.name)
        for rel, member in files.items():
            self.assertEqual(member.mode, 0o755 if rel in EXECUTABLES else 0o644, rel)
        directories = {m.name.rstrip("/") for m in members if m.isdir()}
        for directory in directories:
            prefix = directory + "/"
            self.assertTrue(any(name.startswith(prefix) for name in names), f"empty directory {directory}")
        sidecar = (self.dist / f"{LABEL}.tar.gz.sha256").read_text(encoding="ascii")
        digest = run("sha256sum", self.archive).stdout.split()[0]
        self.assertEqual(sidecar, f"{digest}  {LABEL}.tar.gz\n")
        marker = (self.payload / ".bahamut-launcher-package").read_bytes()
        self.assertEqual(marker, (LINUX / "package-marker.txt").read_bytes())
        self.assertEqual(
            (self.payload / "share/applications/bahamut-launcher.desktop").read_bytes(),
            (LINUX / "bahamut-launcher.desktop").read_bytes(),
        )

    def test_user_install_and_payload_uninstall(self):
        prefix = self.root / "prefix"
        self.assert_ok(self.install(prefix))
        pkgdir = prefix / "lib" / "bahamut-launcher"
        self.assertEqual(
            tree_files(pkgdir),
            {rel for rel in expected_manifest() if rel != "Makefile"} | {MANIFEST, FILE_LIST},
        )
        link = prefix / "bin" / "bahamut-launcher"
        self.assertTrue(link.is_symlink())
        self.assertEqual(os.readlink(link), str(pkgdir / "bahamut-launcher"))
        desktop = prefix / "share/applications/bahamut-launcher.desktop"
        values = desktop_values(desktop)
        self.assertEqual(values["Exec"], str(link))
        self.assertEqual(values["TryExec"], str(link))
        self.assertIn(INSTALLER_KEY, desktop.read_text(encoding="utf-8").splitlines())
        icons = [prefix / f"share/icons/hicolor/{size}/apps/bahamut-launcher.png" for size in ICON_SIZES]
        for size, icon in zip(ICON_SIZES, icons):
            self.assertEqual(icon.read_bytes(), (LINUX / f"icons/hicolor/{size}/apps/bahamut-launcher.png").read_bytes())
        manifest = (pkgdir / ".bahamut-launcher-install-manifest").read_text(encoding="utf-8").splitlines()
        self.assertEqual(manifest, [str(link), str(desktop), *map(str, icons), str(pkgdir)])
        for path in [prefix, *prefix.rglob("*")]:
            if path.is_symlink():
                continue
            mode = stat.S_IMODE(path.stat().st_mode)
            self.assertEqual(mode & 0o022, 0, f"{path} is group- or world-writable ({mode:o})")
        self.assertEqual(stat.S_IMODE((pkgdir / "bahamut-launcher").stat().st_mode), 0o755)
        self.assertEqual(stat.S_IMODE((pkgdir / ".bahamut-launcher-package").stat().st_mode), 0o644)
        if shutil.which("desktop-file-validate"):
            self.assert_ok(run("desktop-file-validate", desktop))

        result = self.assert_ok(self.install(prefix, "--uninstall", script=pkgdir / "install.sh"))
        self.assertIn("removed", result.stdout)
        self.assert_no_launcher_files(prefix)

    def test_recorded_file_list_verifies(self):
        prefix = self.root / "prefix"
        self.assert_ok(self.install(prefix, "--skip-checks"))
        pkgdir = prefix / "lib" / "bahamut-launcher"
        lines = (pkgdir / FILE_LIST).read_text(encoding="utf-8").splitlines()
        paths = []
        for line in lines:
            match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
            self.assertIsNotNone(match, line)
            digest, rel = match.groups()
            self.assertEqual(digest, hashlib.sha256((pkgdir / rel).read_bytes()).hexdigest(), rel)
            paths.append(rel)
        self.assertEqual(paths, sorted(paths, key=lambda rel: rel.encode()))
        self.assertEqual(set(paths), tree_files(pkgdir) - {MANIFEST, FILE_LIST})
        self.assertEqual(len(paths), len(set(paths)))
        self.assert_ok(run("sha256sum", "-c", "--quiet", FILE_LIST, cwd=pkgdir))

    def test_reinstall_replaces_managed_payload(self):
        prefix = self.root / "prefix"
        self.assert_ok(self.install(prefix))
        pkgdir = prefix / "lib" / "bahamut-launcher"
        before = snapshot(pkgdir)
        self.assert_ok(self.install(prefix, "--skip-checks"))
        self.assertEqual(snapshot(pkgdir), before)
        self.assertEqual(sorted(p.name for p in (prefix / "lib").iterdir()), ["bahamut-launcher"])

    def test_changed_payload_is_neither_replaced_nor_removed(self):
        def nested_addon(pkgdir):
            (pkgdir / "addons" / "myaddon").mkdir()
            (pkgdir / "addons" / "myaddon" / "addon.toml").write_bytes(b"id = 'myaddon'\n")
            return "added addons/myaddon/addon.toml"

        def edited_script(pkgdir):
            (pkgdir / "scripts" / "default.txt").write_bytes(b"/echo player macro\n")
            return "changed scripts/default.txt"

        def deleted_notice(pkgdir):
            (pkgdir / "licenses" / "Lua-COPYRIGHT.txt").unlink()
            return "missing licenses/Lua-COPYRIGHT.txt"

        for change in [nested_addon, edited_script, deleted_notice]:
            with self.subTest(change=change.__name__):
                prefix = self.root / change.__name__
                self.assert_ok(self.install(prefix, "--skip-checks"))
                pkgdir = prefix / "lib" / "bahamut-launcher"
                named = change(pkgdir)
                self.assertTrue((pkgdir / MARKER).is_file())
                before = snapshot(prefix)
                for arguments in [["--skip-checks"], ["--uninstall"]]:
                    result = self.install(prefix, *arguments)
                    self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                    self.assertIn(named, result.stderr)
                    self.assertIn("nothing was changed", result.stderr)
                    self.assertIn("player packages", result.stderr)
                    self.assertIn(MOVE_DOC, result.stderr)
                    self.assertEqual(snapshot(prefix), before)

    def test_force_replaces_and_removes_a_changed_payload(self):
        prefix = self.root / "prefix"
        self.assert_ok(self.install(prefix, "--skip-checks"))
        pkgdir = prefix / "lib" / "bahamut-launcher"
        extra = pkgdir / "addons" / "myaddon" / "addon.toml"
        extra.parent.mkdir()
        extra.write_bytes(b"id = 'myaddon'\n")
        (pkgdir / "scripts" / "default.txt").write_bytes(b"/echo player macro\n")
        refused = self.install(prefix, "--skip-checks")
        self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
        self.assertIn("--force", refused.stderr)
        self.assertTrue(extra.is_file())
        forced = self.install(prefix, "--skip-checks", "--force")
        self.assert_ok(forced)
        self.assertIn("added addons/myaddon/addon.toml", forced.stderr)
        self.assertFalse(extra.exists())
        self.assertNotEqual((pkgdir / "scripts" / "default.txt").read_bytes(), b"/echo player macro\n")
        extra.parent.mkdir()
        extra.write_bytes(b"id = 'myaddon'\n")
        self.assertEqual(self.install(prefix, "--uninstall").returncode, 1)
        self.assert_ok(self.install(prefix, "--uninstall", "--force"))
        self.assertFalse(pkgdir.exists())
        self.assertEqual(tree_files(prefix), set())

    def test_force_does_not_adopt_an_unmanaged_pkgdir(self):
        prefix = self.root / "prefix"
        pkgdir = prefix / "lib" / "bahamut-launcher"
        (pkgdir / "config").mkdir(parents=True)
        (pkgdir / "config" / "bahamut.ini").write_bytes(b"keep")
        result = self.install(prefix, "--skip-checks", "--force")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("not managed by install.sh", result.stderr)
        self.assertEqual((pkgdir / "config" / "bahamut.ini").read_bytes(), b"keep")

    def test_refusal_names_at_most_ten_paths(self):
        prefix = self.root / "prefix"
        self.assert_ok(self.install(prefix, "--skip-checks"))
        pkgdir = prefix / "lib" / "bahamut-launcher"
        for index in range(13):
            (pkgdir / "scripts" / f"extra-{index:02}.txt").write_bytes(b"x")
        result = self.install(prefix, "--uninstall")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("scripts/extra-09.txt and 3 more", result.stderr)
        self.assertNotIn("extra-10", result.stderr)
        self.assertTrue((pkgdir / "bahamut-launcher").is_file())

    def test_scoped_leftover_sweep(self):
        prefix = self.root / "prefix"
        self.assert_ok(self.install(prefix, "--skip-checks"))
        lib = prefix / "lib"
        other = lib / ".other-launcher.old.abc123"
        (other / "payload").mkdir(parents=True)
        (other / "payload" / "keep.txt").write_bytes(b"another pkgdir's aside")
        ours = [lib / ".bahamut-launcher.old.zzz999", lib / ".bahamut-launcher.new.yyy888"]
        for leftover in ours:
            leftover.mkdir()
            (leftover / "stale").write_bytes(b"stale")
        self.assert_ok(self.install(prefix, "--skip-checks"))
        for leftover in ours:
            self.assertFalse(leftover.exists(), leftover)
        self.assertEqual((other / "payload" / "keep.txt").read_bytes(), b"another pkgdir's aside")
        self.assertEqual(
            sorted(p.name for p in lib.iterdir()), [".other-launcher.old.abc123", "bahamut-launcher"],
        )

    def test_managed_pkgdir_holding_state_is_kept(self):
        for remove_marker in [False, True]:
            with self.subTest(remove_marker=remove_marker):
                prefix = self.root / f"prefix-{remove_marker}"
                self.assert_ok(self.install(prefix, "--skip-checks"))
                pkgdir = prefix / "lib" / "bahamut-launcher"
                if remove_marker:
                    (pkgdir / ".bahamut-launcher-package").unlink()
                (pkgdir / "config").mkdir()
                setting = pkgdir / "config" / "launcher.toml"
                setting.write_bytes(b"keep")
                before = tree_files(prefix)
                for arguments in [["--skip-checks"], ["--uninstall"]]:
                    result = self.install(prefix, *arguments)
                    self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                    self.assertIn("config", result.stderr)
                    self.assertIn("nothing was changed", result.stderr)
                    self.assertIn(MOVE_DOC, result.stderr)
                    self.assertEqual(tree_files(prefix), before)
                self.assertEqual(setting.read_bytes(), b"keep")
                self.assertEqual(sorted(p.name for p in (prefix / "lib").iterdir()), ["bahamut-launcher"])

    def test_installed_copy_uninstalls_its_own_install(self):
        first = self.root / "first"
        second = self.root / "second"
        self.assert_ok(self.install(first, "--skip-checks"))
        self.assert_ok(self.install(second, "--skip-checks"))
        own = first / "lib" / "bahamut-launcher" / "install.sh"
        result = self.install(second, "--uninstall", script=own)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(str(first / "lib" / "bahamut-launcher"), result.stderr)
        self.assertIn(str(second / "lib" / "bahamut-launcher"), result.stderr)
        self.assertTrue((second / "lib" / "bahamut-launcher" / "bahamut-launcher").is_file())
        result = self.assert_ok(run(own, "--uninstall", env=self.env))
        self.assertIn(f"removed the launcher from {first}", result.stdout)
        self.assert_no_launcher_files(first)
        self.assertTrue((second / "bin" / "bahamut-launcher").is_symlink())

    def test_make_prefix_with_trailing_slash(self):
        prefix = self.root / "slash"
        self.assert_ok(run("make", f"PREFIX={prefix}/", "install", cwd=self.payload, env=self.env))
        pkgdir = prefix / "lib" / "bahamut-launcher"
        manifest = (pkgdir / ".bahamut-launcher-install-manifest").read_text(encoding="utf-8")
        self.assertNotIn("//", manifest)
        self.assert_ok(run("make", f"PREFIX={prefix}/", "uninstall", cwd=self.payload, env=self.env))
        self.assert_no_launcher_files(prefix)

    def test_dot_components_are_refused(self):
        for prefix in [f"{self.root}/p/.", f"{self.root}/p/../q"]:
            with self.subTest(prefix=prefix):
                result = self.install(prefix)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn(". or .. component", result.stderr)
        self.assertFalse((self.root / "p").exists())

    def test_install_refuses_unmanaged_desktop_entry(self):
        prefix = self.root / "prefix"
        desktop = prefix / "share/applications/bahamut-launcher.desktop"
        desktop.parent.mkdir(parents=True)
        desktop.write_text("[Desktop Entry]\nType=Application\nName=Mine\nExec=/opt/my/launcher\n", encoding="utf-8")
        result = self.install(prefix)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("was not written by install.sh", result.stderr)
        self.assertIn("Name=Mine", desktop.read_text(encoding="utf-8"))
        self.assertFalse((prefix / "lib").exists())

    def test_install_refuses_foreign_icon_and_uninstall_keeps_changed_icon(self):
        prefix = self.root / "prefix"
        icon = prefix / "share/icons/hicolor/48x48/apps/bahamut-launcher.png"
        icon.parent.mkdir(parents=True)
        icon.write_bytes(b"someone else's icon")
        result = self.install(prefix)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("differs from the launcher icon", result.stderr)
        self.assertEqual(icon.read_bytes(), b"someone else's icon")
        icon.unlink()
        self.assert_ok(self.install(prefix, "--skip-checks"))
        icon.write_bytes(b"replaced by a distribution package")
        result = self.assert_ok(self.install(prefix, "--uninstall"))
        self.assertIn(f"kept {icon}", result.stdout)
        self.assertEqual(icon.read_bytes(), b"replaced by a distribution package")
        self.assertFalse((prefix / "share/icons/hicolor/128x128/apps/bahamut-launcher.png").exists())

    def test_pkgdir_overlapping_state_directory_is_refused(self):
        state = self.root / "state"
        self.env["BAHAMUT_LAUNCHER_HOME"] = str(state)
        prefix = self.root / "prefix"
        for pkgdir in [state, state / "payload", self.root]:
            with self.subTest(pkgdir=pkgdir):
                result = self.install(prefix, "--pkgdir", pkgdir)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("launcher state directory", result.stderr)
        self.assertFalse(state.exists())
        self.assertFalse(prefix.exists())

    def test_interrupted_swap_restores_previous_payload(self):
        older = self.root / "older" / TOP
        shutil.copytree(self.payload, older)
        (older / "scripts" / "default.txt").write_bytes(b"previous payload")
        prefix = self.root / "prefix"
        self.assert_ok(self.install(prefix, "--skip-checks", script=older / "install.sh"))
        pkgdir = prefix / "lib" / "bahamut-launcher"
        shims = self.root / "shims"
        shims.mkdir()
        real_mv = shutil.which("mv")
        shim = shims / "mv"
        shim.write_text(
            "#!/bin/sh\n"
            f'"{real_mv}" "$@"\n'
            "status=$?\n"
            'case "$3" in */.bahamut-launcher.old.*/payload) kill -TERM "$PPID" ;; esac\n'
            'exit "$status"\n',
            encoding="utf-8",
        )
        shim.chmod(0o755)
        self.env["PATH"] = f"{shims}{os.pathsep}{self.env.get('PATH', '')}"
        result = self.install(prefix, "--skip-checks")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((pkgdir / "scripts" / "default.txt").read_bytes(), b"previous payload")
        self.assert_ok(run("sha256sum", "-c", "--quiet", FILE_LIST, cwd=pkgdir))
        self.assertEqual(sorted(p.name for p in (prefix / "lib").iterdir()), ["bahamut-launcher"])

    def test_pkgdir_inside_source_creates_nothing(self):
        result = self.install(self.root / "prefix", "--pkgdir", self.payload / "sub" / "deep" / "pkg")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("inside the source directory", result.stderr)
        self.assertFalse((self.payload / "sub").exists())

    def test_relative_destdir_inside_source_is_named(self):
        result = run(self.payload / "install.sh", "--destdir", "./stage", "--prefix", "/usr", cwd=self.payload, env=self.env)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(f"install target {self.payload}/stage/usr/lib/bahamut-launcher", result.stderr)
        self.assertIn(f"destdir {self.payload}/stage", result.stderr)
        self.assertFalse((self.payload / "stage").exists())

    def test_relative_destdir_resolves_against_the_working_directory(self):
        work = self.root / "w" / "x"
        work.mkdir(parents=True)
        cases = [
            (["--destdir", "./stage"], {}, work / "stage"),
            (["--destdir", "../stage"], {}, self.root / "w" / "stage"),
            ([], {"DESTDIR": "sub/./../env-stage"}, work / "env-stage"),
        ]
        for arguments, extra, stage in cases:
            with self.subTest(arguments=arguments, env=extra):
                env = dict(self.env, **extra)
                script = self.payload / "install.sh"
                result = self.assert_ok(run(script, *arguments, "--prefix", "/usr", cwd=work, env=env))
                self.assertIn(f"destdir {stage}", result.stdout)
                link = stage / "usr/bin/bahamut-launcher"
                self.assertEqual(os.readlink(link), "/usr/lib/bahamut-launcher/bahamut-launcher")
                self.assertTrue((stage / "usr/lib/bahamut-launcher" / FILE_LIST).is_file())
                self.assert_ok(run(script, *arguments, "--prefix", "/usr", "--uninstall", cwd=work, env=env))
                self.assert_no_launcher_files(stage)
        result = run(self.payload / "install.sh", "--destdir", f"{self.root}/a/../b", "--prefix", "/usr", env=self.env)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(". or .. component", result.stderr)

    def test_icons_are_compared_without_cmp(self):
        self.env["PATH"] = path_without(self.root / "no-cmp", "cmp")
        self.assertIsNone(shutil.which("cmp", path=self.env["PATH"]))
        prefix = self.root / "prefix"
        icon = prefix / "share/icons/hicolor/48x48/apps/bahamut-launcher.png"
        icon.parent.mkdir(parents=True)
        icon.write_bytes(b"someone else's icon")
        result = self.install(prefix, "--skip-checks")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("differs from the launcher icon", result.stderr)
        shutil.copyfile(self.payload / "share/icons/hicolor/48x48/apps/bahamut-launcher.png", icon)
        self.assert_ok(self.install(prefix, "--skip-checks"))
        result = self.assert_ok(self.install(prefix, "--uninstall"))
        self.assertNotIn("kept", result.stdout)
        for size in ICON_SIZES:
            self.assertFalse((prefix / f"share/icons/hicolor/{size}/apps/bahamut-launcher.png").exists(), size)
        self.assert_no_launcher_files(prefix)

    @unittest.skipUnless(os.geteuid() == 0, "the invoking-user home guard applies only to root")
    def test_invoking_user_home_without_getent(self):
        self.env["PATH"] = path_without(self.root / "no-getent", "getent")
        self.assertIsNone(shutil.which("getent", path=self.env["PATH"]))
        home = os.environ.get("HOME", "")
        users = [
            entry for entry in pwd.getpwall()
            if entry.pw_dir.startswith("/") and entry.pw_dir != "/" and entry.pw_dir != home
            and not any(entry.pw_dir == top or entry.pw_dir.startswith(top + "/") for top in ["/usr", "/opt"])
        ]
        if not users:
            self.skipTest("no passwd entry with a home outside /, /usr, and /opt")
        user = users[0]
        for variables in [{"SUDO_USER": user.pw_name}, {"DOAS_USER": user.pw_name}, {"PKEXEC_UID": str(user.pw_uid)}]:
            with self.subTest(variables=variables):
                env = dict(self.env, **variables)
                prefix = f"{user.pw_dir}/.local"
                result = run(self.payload / "install.sh", "--prefix", prefix, "--skip-checks", env=env)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("does not write under a user's home directory", result.stderr)
        env = dict(self.env, SUDO_USER="bahamut-no-such-user")
        prefix = self.root / "prefix"
        result = run(self.payload / "install.sh", "--prefix", prefix, "--skip-checks", env=env)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("SUDO_USER=bahamut-no-such-user", result.stderr)
        self.assertIn("explicit --prefix under /usr, /opt, or /usr/local", result.stderr)
        self.assertFalse(prefix.exists())

    def test_prefix_outside_xdg_data_dirs_notes_the_menu(self):
        prefix = self.root / "custom"
        self.env.pop("XDG_DATA_HOME", None)
        self.env.pop("XDG_DATA_DIRS", None)
        result = self.assert_ok(self.install(prefix, "--skip-checks"))
        self.assertIn("XDG_DATA_DIRS", result.stdout)
        self.assertNotIn("application menu", result.stdout)
        self.env["XDG_DATA_DIRS"] = f"/usr/share:{prefix}/share/"
        result = self.assert_ok(self.install(prefix, "--skip-checks"))
        self.assertIn("application menu", result.stdout)
        self.assertNotIn("XDG_DATA_DIRS", result.stdout)

    def test_make_destdir_install(self):
        stage = self.root / "stage root"
        result = self.assert_ok(run("make", f"DESTDIR={stage}", "PREFIX=/usr", "install", cwd=self.payload, env=self.env))
        self.assertNotIn("dependency", result.stdout)
        desktop = stage / "usr/share/applications/bahamut-launcher.desktop"
        values = desktop_values(desktop)
        self.assertEqual(values["Exec"], "/usr/bin/bahamut-launcher")
        self.assertEqual(values["TryExec"], "/usr/bin/bahamut-launcher")
        link = stage / "usr/bin/bahamut-launcher"
        self.assertEqual(os.readlink(link), "/usr/lib/bahamut-launcher/bahamut-launcher")
        manifest = (stage / "usr/lib/bahamut-launcher/.bahamut-launcher-install-manifest").read_text(encoding="utf-8")
        self.assertNotIn(str(stage), manifest)
        self.assertEqual(manifest.splitlines()[-1], "/usr/lib/bahamut-launcher")
        for cache in ["icon-theme.cache", "mimeinfo.cache"]:
            self.assertEqual(list(stage.rglob(cache)), [], cache)
        self.assert_ok(run("make", f"DESTDIR={stage}", "PREFIX=/usr", "uninstall", cwd=self.payload, env=self.env))
        self.assert_no_launcher_files(stage)

    def test_uninstall_refuses_pkgdir_without_manifest(self):
        prefix = self.root / "prefix"
        pkgdir = prefix / "lib" / "bahamut-launcher"
        pkgdir.mkdir(parents=True)
        keep = pkgdir / "keep.txt"
        keep.write_bytes(b"keep")
        result = self.install(prefix, "--uninstall")
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("not managed by install.sh", result.stderr)
        self.assertEqual(keep.read_bytes(), b"keep")

    def test_install_refuses_unmanaged_pkgdir(self):
        prefix = self.root / "prefix"
        pkgdir = prefix / "lib" / "bahamut-launcher"
        pkgdir.mkdir(parents=True)
        keep = pkgdir / "keep.txt"
        keep.write_bytes(b"keep")
        result = self.install(prefix)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("not managed by install.sh", result.stderr)
        self.assertIn("launcher state may live there", result.stderr)
        self.assertIn("it holds keep.txt", result.stderr)
        self.assertIn(MOVE_DOC, result.stderr)
        self.assertIn("another --pkgdir", result.stderr)
        self.assertNotIn("remove it", result.stderr)
        self.assertEqual(sorted(p.name for p in pkgdir.iterdir()), ["keep.txt"])
        self.assertFalse((prefix / "bin").exists())
        self.assertEqual(sorted(p.name for p in (prefix / "lib").iterdir()), ["bahamut-launcher"])

    def test_install_refuses_regular_file_at_bin_entry(self):
        prefix = self.root / "prefix"
        (prefix / "bin").mkdir(parents=True)
        entry = prefix / "bin" / "bahamut-launcher"
        entry.write_bytes(b"someone else's launcher")
        result = self.install(prefix)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("not a symbolic link", result.stderr)
        self.assertEqual(entry.read_bytes(), b"someone else's launcher")
        self.assertFalse((prefix / "lib").exists())

    def test_payload_copy_refuses_to_install_over_itself(self):
        prefix = self.root / "prefix"
        self.assert_ok(self.install(prefix))
        pkgdir = prefix / "lib" / "bahamut-launcher"
        result = self.install(prefix, script=pkgdir / "install.sh")
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("source directory is the install target", result.stderr)
        self.assertTrue((pkgdir / "bahamut-launcher").is_file())

    def test_uninstall_keeps_foreign_desktop_entry(self):
        prefix = self.root / "prefix"
        self.assert_ok(self.install(prefix))
        desktop = prefix / "share/applications/bahamut-launcher.desktop"
        text = desktop.read_text(encoding="utf-8").replace(INSTALLER_KEY + "\n", "")
        desktop.write_text(text, encoding="utf-8")
        self.assert_ok(self.install(prefix, "--uninstall"))
        self.assertEqual(desktop.read_text(encoding="utf-8"), text)
        self.assertFalse((prefix / "lib" / "bahamut-launcher").exists())
        self.assertFalse((prefix / "bin" / "bahamut-launcher").is_symlink())

    def test_prefix_with_space_quotes_exec(self):
        prefix = self.root / "prefix with space"
        self.assert_ok(self.install(prefix))
        desktop = prefix / "share/applications/bahamut-launcher.desktop"
        values = desktop_values(desktop)
        binary = prefix / "bin" / "bahamut-launcher"
        self.assertEqual(values["Exec"], f'"{binary}"')
        self.assertEqual(values["TryExec"], str(binary))
        if shutil.which("desktop-file-validate"):
            self.assert_ok(run("desktop-file-validate", desktop))
        self.assert_ok(self.install(prefix, "--uninstall"))
        self.assert_no_launcher_files(prefix)

    def test_dependency_check_cannot_check_placeholder(self):
        result = run(self.payload / "install-dependencies.sh", "--check", env=self.env)
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn("not an ELF executable", result.stdout)

    def test_dependency_install_stops_when_libraries_cannot_be_checked(self):
        wine = self.root / "wine"
        (wine / "bin").mkdir(parents=True)
        (wine / "bin" / "wine").write_text(PLACEHOLDER, encoding="utf-8")
        (wine / "bin" / "wine").chmod(0o755)
        (wine / "lib" / "wine" / "i386-windows").mkdir(parents=True)
        (wine / "lib" / "wine" / "i386-windows" / "ntdll.dll").write_bytes(b"synthetic")
        env = dict(self.env, BAHAMUT_WINE=str(wine / "bin" / "wine"))
        result = run(self.payload / "install-dependencies.sh", "--install", "--yes", env=env)
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn("cannot check the libraries; nothing to install", result.stdout)
        self.assertNotIn("+ ", result.stdout)

    def test_dependency_script_rejects_unknown_argument(self):
        result = run(self.payload / "install-dependencies.sh", "--bogus", env=self.env)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)

    def test_window_class_names_agree(self):
        desktop = LINUX / "bahamut-launcher.desktop"
        self.assertEqual(desktop.stem, "bahamut-launcher")
        self.assertEqual(desktop_values(desktop)["StartupWMClass"], "bahamut-launcher")
        self.assertTrue((self.payload / "bahamut-launcher").is_file())

        def keys(node):
            if isinstance(node, dict):
                for key, value in node.items():
                    yield key
                    yield from keys(value)
            elif isinstance(node, list):
                for value in node:
                    yield from keys(value)

        config = json.loads((REPO / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))
        self.assertNotIn("enableGTKAppId", set(keys(config)))

    @unittest.skipUnless(shutil.which("desktop-file-validate"), "desktop-file-validate is not installed")
    def test_shipped_desktop_entry_validates(self):
        self.assert_ok(run("desktop-file-validate", self.payload / "share/applications/bahamut-launcher.desktop"))


@unittest.skipUnless(sys.platform.startswith("linux"), "the dependency check reads ELF files on a Linux host")
class DependencyClassificationTests(unittest.TestCase):
    """Feed install-dependencies.sh a stub ldd listing for a real ELF file."""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.elf = self.root / "bahamut-launcher"
        shutil.copyfile(Path(shutil.which("true")).resolve(), self.elf)
        self.elf.chmod(0o755)
        self.tools = self.root / "tools"
        self.tools.mkdir()
        wine = self.root / "wine"
        (wine / "bin").mkdir(parents=True)
        (wine / "bin" / "wine").write_text(PLACEHOLDER, encoding="utf-8")
        (wine / "bin" / "wine").chmod(0o755)
        (wine / "lib" / "wine" / "i386-windows").mkdir(parents=True)
        (wine / "lib" / "wine" / "i386-windows" / "ntdll.dll").write_bytes(b"synthetic")
        self.env = dict(
            os.environ,
            PATH=f"{self.tools}{os.pathsep}{os.environ.get('PATH', '')}",
            BAHAMUT_WINE=str(wine / "bin" / "wine"),
        )

    def check(self, listing, status=0, *arguments):
        ldd = self.tools / "ldd"
        ldd.write_text(f"#!/bin/sh\ncat <<'EOF'\n{listing}EOF\nexit {status}\n", encoding="utf-8")
        ldd.chmod(0o755)
        return run(
            "bash", LINUX / "install-dependencies.sh", *(arguments or ["--check"]), "--launcher", self.elf,
            env=self.env,
        )

    def fake_wine(self, root, binary, library_dir):
        (root / binary).parent.mkdir(parents=True, exist_ok=True)
        (root / binary).write_text(PLACEHOLDER, encoding="utf-8")
        (root / binary).chmod(0o755)
        (root / library_dir).mkdir(parents=True)
        (root / library_dir / "ntdll.dll").write_bytes(b"synthetic")
        return root / binary

    def test_missing_soname_exits_1(self):
        result = self.check("\tlibwebkit2gtk-4.1.so.0 => not found\n\tlibc.so.6 => /lib/libc.so.6\n")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("libwebkit2gtk-4.1.so.0", result.stdout)
        self.assertIn("32-bit support present", result.stdout)

    def test_glibc_version_error_exits_5(self):
        result = self.check(
            f"{self.elf}: /lib/libc.so.6: version `GLIBC_2.39' not found (required by {self.elf})\n"
            "\tlibc.so.6 => /lib/libc.so.6\n"
        )
        self.assertEqual(result.returncode, 5, result.stdout + result.stderr)
        self.assertIn("GLIBC_2.39", result.stdout)
        self.assertNotIn("to install:", result.stdout)

    def test_glibcxx_error_is_missing_not_too_old(self):
        line = "\t/usr/lib/libstdc++.so.6: version `GLIBCXX_3.4.32' not found (required by /usr/lib/libicuuc.so.74)"
        result = self.check(line + "\n\tlibc.so.6 => /lib/libc.so.6\n")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(line.strip(), result.stdout)
        self.assertNotIn("newer release of your distribution", result.stdout)

    def test_glibc_error_of_another_library_is_missing(self):
        result = self.check(
            "\t/lib/libc.so.6: version `GLIBC_2.40' not found (required by /usr/lib/libfoo.so.1)\n"
            "\tlibc.so.6 => /lib/libc.so.6\n"
        )
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("libfoo.so.1", result.stdout)

    @unittest.skipUnless(
        any(os.access(loader, os.X_OK) for loader in script_loaders()), "no glibc loader from the script's list on this host",
    )
    def test_ldd_not_a_dynamic_executable_falls_back(self):
        result = self.check("\tnot a dynamic executable\n", 1)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("libraries: all resolved", result.stdout)

    def test_no_listing_exits_3(self):
        self.elf.write_bytes(self.elf.read_bytes()[:64])
        result = self.check("\tnot a dynamic executable\n", 1)
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn("cannot check", result.stdout)

    def test_debian_multiarch_wine(self):
        wine = self.fake_wine(self.root / "deb", "bin/wine", "lib/i386-linux-gnu/wine/i386-windows")
        self.env["BAHAMUT_WINE"] = str(wine)
        result = self.check("\tlibc.so.6 => /lib/libc.so.6\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("32-bit support present", result.stdout)

    def test_gentoo_slotted_wine_through_symlinks(self):
        root = self.root / "gentoo"
        slot = self.fake_wine(root, "usr/lib/wine-vanilla-9.0/bin/wine", "usr/lib/wine-vanilla-9.0/wine/i386-windows")
        eselect = root / "etc/eselect/wine/bin/wine"
        eselect.parent.mkdir(parents=True)
        eselect.symlink_to(slot)
        entry = root / "usr/bin/wine"
        entry.parent.mkdir(parents=True)
        entry.symlink_to(os.path.relpath(eselect, entry.parent))
        self.env["BAHAMUT_WINE"] = str(entry)
        result = self.check("\tlibc.so.6 => /lib/libc.so.6\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f"{slot} (32-bit support present)", result.stdout)

    def test_wine_older_than_7_exits_1(self):
        root = self.root / "jammy"
        wine = root / "usr/bin/wine"
        wine.parent.mkdir(parents=True)
        wine.write_text("#!/bin/sh\necho 'wine-6.0.3 (Ubuntu 6.0.3~repack-1)'\n", encoding="utf-8")
        wine.chmod(0o755)
        (root / "usr/lib/i386-linux-gnu/wine").mkdir(parents=True)
        (root / "usr/lib/i386-linux-gnu/wine/ntdll.dll.so").write_bytes(b"synthetic")
        self.env["BAHAMUT_WINE"] = str(wine)
        result = self.check("\tlibc.so.6 => /lib/libc.so.6\n")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("Wine wine-6.0.3 (Ubuntu 6.0.3~repack-1) is older than 7", result.stdout)
        self.assertIn("WineHQ", result.stdout)
        self.assertNotIn("lacks 32-bit support", result.stdout)
        self.assertNotIn("to install:", result.stdout)

    def test_bahamut_wine_is_used_as_given(self):
        for value in ["wine", str(self.root)]:
            with self.subTest(value=value):
                self.env["BAHAMUT_WINE"] = value
                result = self.check("\tlibc.so.6 => /lib/libc.so.6\n")
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn(f"BAHAMUT_WINE is set but {value} is not a file", result.stdout)

    def test_architecture_mismatch_is_reported(self):
        data = bytearray(self.elf.read_bytes())
        data[18:20] = b"\xb7\x00" if os.uname().machine == "x86_64" else b"\x3e\x00"
        self.elf.write_bytes(bytes(data))
        result = self.check("\tlibc.so.6 => /lib/libc.so.6\n")
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn(f"this host is {os.uname().machine}", result.stdout)
        self.assertNotIn("cannot check", result.stdout)

    def test_architecture_mismatch_install_stops(self):
        for tool in PACKAGE_TOOLS:
            shim = self.tools / tool
            shim.write_text("#!/bin/sh\nexit 100\n", encoding="utf-8")
            shim.chmod(0o755)
        data = bytearray(self.elf.read_bytes())
        data[18:20] = b"\xb7\x00" if os.uname().machine == "x86_64" else b"\x3e\x00"
        self.elf.write_bytes(bytes(data))
        self.env["BAHAMUT_WINE"] = str(self.root / "no-such-wine")
        result = self.check("\tlibc.so.6 => /lib/libc.so.6\n", 0, "--install", "--yes")
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn("the launcher does not match this host; nothing to install", result.stdout)
        self.assertNotIn("cannot check", result.stdout)
        self.assertNotIn("to install:", result.stdout)
        self.assertNotIn("+ ", result.stdout)

    def test_failed_package_command_exits_1(self):
        for tool in PACKAGE_TOOLS:
            shim = self.tools / tool
            shim.write_text("#!/bin/sh\nexit 100\n", encoding="utf-8")
            shim.chmod(0o755)
        self.env["BAHAMUT_WINE"] = str(self.root / "no-such-wine")
        result = self.check("\tlibc.so.6 => /lib/libc.so.6\n", 0, "--install", "--yes")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        if "distribution: unknown" not in result.stdout:
            self.assertIn("the package command failed", result.stderr)

    def test_wine_without_32_bit_support_exits_1(self):
        shutil.rmtree(self.root / "wine" / "lib")
        result = self.check("\tlibc.so.6 => /lib/libc.so.6\n")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("Wine lacks 32-bit support", result.stdout)


@unittest.skipUnless(sys.platform.startswith("linux"), "the Linux archive needs GNU tar and a Linux host")
class PackagerInputTests(unittest.TestCase):
    """Run a copy of the packager in a synthetic repository with a stub stage script."""

    STAGE_STUB = r"""#!/usr/bin/env bash
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
repo="$(cd "$(dirname -- "$0")/.." && pwd -P)"
mkdir -p "$destination/scripts" "$destination/config" "$destination/logs/launcher" "$destination/plugins/dats"
cp "$launcher" "$destination/bahamut-launcher"
for rel in __FIXED__; do
    mkdir -p "$destination/$(dirname -- "$rel")"
    printf 'synthetic %s' "$rel" > "$destination/$rel"
done
cp -R "$repo/plugins/dats/bahamut-dats-overlay" "$destination/plugins/dats/"
"""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.repo = self.root / "source with spaces"
        scripts = self.repo / "scripts"
        scripts.mkdir(parents=True)
        shutil.copyfile(PACKAGER, scripts / PACKAGER.name)
        self.stage_stub = scripts / "stage-unix-release.sh"
        self.write_stage_stub()
        linux = self.repo / "packaging" / "linux"
        shutil.copytree(LINUX, linux)
        self.overlay = self.repo / "plugins" / "dats" / "bahamut-dats-overlay"
        (self.overlay / "data").mkdir(parents=True)
        (self.overlay / "overlay.toml").write_text("synthetic\n", encoding="utf-8")
        (self.overlay / "data" / "a b.txt").write_text("synthetic\n", encoding="utf-8")
        self.icon = linux / "icons/hicolor/128x128/apps/bahamut-launcher.png"
        self.launcher = self.root / "placeholder"
        self.launcher.write_text(PLACEHOLDER, encoding="utf-8")
        self.launcher.chmod(0o755)
        self.client = self.root / "client"
        self.client.mkdir()
        self.output = self.root / "dist"
        self.env = dict(os.environ, SOURCE_DATE_EPOCH="1700000000")

    def write_stage_stub(self, extra=""):
        fixed = [
            "bahamut-loader.exe", "bahamut.dll", "plugins/screenshot.dll", "plugins/discord-rpc.dll",
            "LICENSE.md", "README.md", "scripts/default.txt",
            *(f"licenses/{name}" for name in LICENSES),
            *(f"addons/{addon}/{file}" for addon in ADDONS for file in ["addon.toml", f"{addon}.lua"]),
        ]
        stub = self.STAGE_STUB.replace("__FIXED__", " ".join(fixed))
        self.stage_stub.write_text(stub + extra + "\n", encoding="utf-8")

    def package(self, label=LABEL, env=None):
        return run(
            "bash", self.repo / "scripts" / PACKAGER.name,
            "--launcher", self.launcher, "--client-build", self.client,
            "--label", label, "--output", self.output, env=env or self.env,
        )

    def assert_refused(self, result, message):
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(message, result.stderr)

    def test_stub_tree_packages(self):
        result = self.package()
        self.assertEqual(result.returncode, 0, result.stderr)
        with tarfile.open(self.output / f"{LABEL}.tar.gz") as archive:
            members = archive.getmembers()
        names = {m.name for m in members if m.isfile()}
        self.assertIn(f"{TOP}/scripts/default.txt", names)
        self.assertIn(f"{TOP}/plugins/dats/bahamut-dats-overlay/data/a b.txt", names)
        self.assertNotIn(f"{TOP}/config", {m.name.rstrip("/") for m in members})

    def test_extra_staged_file_is_rejected(self):
        self.write_stage_stub('printf x > "$destination/unexpected-extra.bin"')
        result = self.package()
        self.assert_refused(result, "unexpected:")
        self.assertIn("unexpected-extra.bin", result.stderr)
        self.assertFalse(self.output.exists() and any(self.output.iterdir()))

    def test_missing_staged_file_is_rejected(self):
        self.write_stage_stub('rm "$destination/bahamut.dll"')
        result = self.package()
        self.assert_refused(result, "missing:")
        self.assertIn("bahamut.dll", result.stderr)

    def test_non_ascii_overlay_name_under_c_locale(self):
        (self.overlay / "caf\u00e9.txt").write_text("synthetic\n", encoding="utf-8")
        env = {k: v for k, v in self.env.items() if not k.startswith("LC_") and k != "LANG"}
        env["LC_ALL"] = "C"
        result = self.package(env=env)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_missing_icon_fails_clearly(self):
        self.icon.unlink()
        result = self.package()
        self.assert_refused(result, "icon is missing")
        self.assertFalse(self.output.exists())

    def test_staged_link_is_rejected(self):
        self.write_stage_stub('ln -s default.txt "$destination/scripts/linked.txt"')
        self.assert_refused(self.package(), "symbolic link")
        self.assertEqual(list(self.output.glob("*.tar.gz")), [])

    def test_staged_collision_is_rejected(self):
        self.write_stage_stub('printf x > "$destination/install.sh"')
        self.assert_refused(self.package(), "collides with a packaging file")

    def test_existing_archive_is_not_overwritten(self):
        self.output.mkdir()
        existing = self.output / f"{LABEL}.tar.gz"
        existing.write_bytes(b"keep")
        self.assert_refused(self.package(), "refusing to overwrite")
        self.assertEqual(existing.read_bytes(), b"keep")

    def test_unsafe_label_is_rejected(self):
        for label in ["../escape", "with space", ".hidden", "a/b"]:
            with self.subTest(label=label):
                self.assert_refused(self.package(label=label), "the label must be a file name")

    def test_bad_source_date_epoch_is_rejected(self):
        env = dict(self.env, SOURCE_DATE_EPOCH="yesterday")
        self.assert_refused(self.package(env=env), "SOURCE_DATE_EPOCH")


if __name__ == "__main__":
    unittest.main()
