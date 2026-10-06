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
ADDONS = ["chatlogs", "zonename", "packetlogger", "combatparser", "distance", "targethp", "fps", "pos", "wiki", "targetlines"]
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
        self.assertEqual((self.payload / "README.md").read_bytes(), (LINUX / "README.md").read_bytes())
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
        result = self.check("\tlibwebkit2gtk-4.1.so.0 => not found\n", 0, "--install", "--yes")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        if "to install: sudo" in result.stdout:
            self.assertIn("the package command failed", result.stderr)

    def test_wine_without_32_bit_support_exits_1(self):
        shutil.rmtree(self.root / "wine" / "lib")
        result = self.check("\tlibc.so.6 => /lib/libc.so.6\n")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("Wine lacks 32-bit support", result.stdout)


    # Managed-engine, tool, Vulkan, and distribution cases. BAHAMUT_DEPENDENCY_ROOT
    # is a test-only prefix for the paths those probes read.
    LIBS_OK = "\tlibc.so.6 => /lib/libc.so.6\n"
    LIBS_MISSING = "\tlibwebkit2gtk-4.1.so.0 => not found\n"

    def simulate_host(self, arch="x86_64", exclude=(), missing_tools=()):
        """Make the launcher and `uname -m` report ARCH, with a PATH that has no Wine unless a test adds one."""
        data = bytearray(self.elf.read_bytes())
        data[18:20] = b"\x3e\x00" if arch == "x86_64" else b"\xb7\x00"
        self.elf.write_bytes(bytes(data))
        uname = self.tools / "uname"
        uname.write_text(
            f'#!/bin/sh\nif [ "$1" = "-m" ]; then echo {arch}; exit 0; fi\nexec {shutil.which("uname")} "$@"\n',
            encoding="utf-8",
        )
        uname.chmod(0o755)
        filtered = path_without(self.root / "filtered", "wine", "tar", "xz", *exclude)
        for tool in ("tar", "xz"):
            if tool not in missing_tools:
                (self.tools / tool).write_text(PLACEHOLDER, encoding="utf-8")
                (self.tools / tool).chmod(0o755)
        self.env["PATH"] = f"{self.tools}{os.pathsep}{filtered}"
        self.env.pop("BAHAMUT_WINE", None)

    ENGINE_LIBS = [
        "libX11.so.6", "libXext.so.6", "libXcomposite.so.1", "libXcursor.so.1", "libXfixes.so.3",
        "libXi.so.6", "libXinerama.so.1", "libXrandr.so.2", "libXrender.so.1", "libXxf86vm.so.1",
        "libGL.so.1", "libfreetype.so.6", "libfontconfig.so.1",
    ]

    def dependency_root(self, os_release=None, vulkan=False, ostree=False, libs=()):
        root = self.root / "hostroot"
        (root / "etc").mkdir(parents=True, exist_ok=True)
        if os_release is not None:
            (root / "etc" / "os-release").write_text(os_release, encoding="utf-8")
        if vulkan:
            (root / "usr/lib64").mkdir(parents=True, exist_ok=True)
            (root / "usr/lib64/libvulkan.so.1").write_bytes(b"synthetic")
            (root / "usr/share/vulkan/icd.d").mkdir(parents=True, exist_ok=True)
            (root / "usr/share/vulkan/icd.d/radeon_icd.json").write_text("{}", encoding="utf-8")
        if libs:
            (root / "usr/lib64").mkdir(parents=True, exist_ok=True)
        for soname in libs:
            (root / "usr/lib64" / soname).write_bytes(b"synthetic")
        if ostree:
            (root / "run").mkdir(parents=True, exist_ok=True)
            (root / "run/ostree-booted").write_text("", encoding="utf-8")
        self.env["BAHAMUT_DEPENDENCY_ROOT"] = str(root)

    def stub_system_wine(self, version="wine-9.0"):
        wine = self.tools / "wine"
        wine.write_text(f"#!/bin/sh\necho '{version}'\n", encoding="utf-8")
        wine.chmod(0o755)
        return wine

    def test_x86_64_without_system_wine_exits_0(self):
        self.simulate_host()
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("wine: the launcher downloads its own Wine on the first Play\n", result.stdout)
        self.assertNotIn("is the fallback", result.stdout)
        self.assertNotIn("wine: missing", result.stdout)
        self.assertIn("unpack tools: tar and xz found", result.stdout)

    def test_system_wine_is_reported_as_the_fallback(self):
        self.simulate_host()
        wine = self.stub_system_wine()
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f"wine: system Wine {wine} (wine-9.0) is the fallback\n", result.stdout)

    def test_unusable_system_wine_does_not_change_the_exit_code(self):
        # No i386-windows tree next to it, and a version older than 7.
        self.simulate_host()
        self.stub_system_wine("wine-6.0.3")
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("lacks 32-bit support", result.stdout)
        self.assertNotIn("older than 7", result.stdout)

    def test_bahamut_wine_keeps_the_32_bit_check_on_x86_64(self):
        self.simulate_host()
        wine = self.root / "wine"
        shutil.rmtree(wine / "lib")
        self.env["BAHAMUT_WINE"] = str(wine / "bin" / "wine")
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("wine: Wine lacks 32-bit support", result.stdout)
        self.assertNotIn("downloads its own Wine", result.stdout)
        self.assertNotIn("to install:", result.stdout)

    def test_bahamut_wine_keeps_the_version_check_on_x86_64(self):
        self.simulate_host()
        root = self.root / "jammy"
        wine = root / "usr/bin/wine"
        wine.parent.mkdir(parents=True)
        wine.write_text("#!/bin/sh\necho 'wine-6.0.3'\n", encoding="utf-8")
        wine.chmod(0o755)
        (root / "usr/lib/i386-linux-gnu/wine").mkdir(parents=True)
        (root / "usr/lib/i386-linux-gnu/wine/ntdll.dll.so").write_bytes(b"synthetic")
        self.env["BAHAMUT_WINE"] = str(wine)
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("is older than 7", result.stdout)

    def test_bahamut_wine_set_and_empty_is_used_as_given(self):
        self.simulate_host()
        self.env["BAHAMUT_WINE"] = ""
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("BAHAMUT_WINE is set but  is not a file", result.stdout)

    def test_other_architecture_probes_system_wine(self):
        self.simulate_host("aarch64")
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("wine: missing: wine was not found on PATH", result.stdout)
        self.assertNotIn("downloads its own Wine", result.stdout)
        self.assertNotIn("unpack tools", result.stdout)
        self.assertNotIn("to install:", result.stdout)

    def test_missing_xz_is_exit_1_and_installable(self):
        self.simulate_host(missing_tools=("xz",))
        self.dependency_root('ID=ubuntu\nID_LIKE=debian\n')
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("unpack tools: missing xz", result.stdout)
        self.assertNotIn("missing tar", result.stdout)
        self.assertIn("to install: sudo apt update && sudo apt install libwebkit2gtk-4.1-0 xz-utils\n", result.stdout)

    def test_missing_tar_and_xz_are_named_in_the_package_command(self):
        self.simulate_host(missing_tools=("tar", "xz"))
        self.dependency_root('ID=gentoo\n')
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("unpack tools: missing tar xz", result.stdout)
        self.assertIn(
            "to install: sudo emerge --ask --noreplace net-libs/webkit-gtk:4.1 x11-libs/gtk+:3 "
            "app-arch/tar app-arch/xz-utils\n",
            result.stdout,
        )

    def test_unpack_tools_are_not_required_with_bahamut_wine(self):
        self.simulate_host(missing_tools=("tar", "xz"))
        self.env["BAHAMUT_WINE"] = str(self.root / "wine" / "bin" / "wine")
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("unpack tools", result.stdout)

    def test_package_commands_do_not_install_wine(self):
        cases = {
            "gentoo": "sudo emerge --ask --noreplace net-libs/webkit-gtk:4.1 x11-libs/gtk+:3",
            "debian": "sudo apt update && sudo apt install libwebkit2gtk-4.1-0",
            "fedora": "sudo dnf install webkit2gtk4.1",
            "arch": "sudo pacman -S webkit2gtk-4.1",
            "opensuse-leap": "sudo zypper install libwebkit2gtk-4_1-0",
        }
        self.simulate_host()
        for distro, command in cases.items():
            with self.subTest(distro=distro):
                self.dependency_root(f"ID={distro}\n")
                result = self.check(self.LIBS_MISSING)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn(f"to install: {command}\n", result.stdout)
                self.assertNotIn("wine32", result.stdout.split("to install:")[1])
                self.assertNotIn("i386", result.stdout)
                self.assertNotRegex(result.stdout.split("to install:")[1], r"\bwine\b|virtual/wine")

    def test_vulkan_found_with_loader_and_driver(self):
        self.simulate_host()
        self.dependency_root("ID=ubuntu\n", vulkan=True)
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("vulkan: loader and driver found\n", result.stdout)
        self.assertNotIn("vulkan: not found", result.stdout)

    def test_vulkan_missing_is_informational(self):
        self.simulate_host()
        self.dependency_root("ID=ubuntu\nID_LIKE=debian\n")
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(
            "vulkan: not found; the game uses the slower OpenGL renderer until a Vulkan driver is installed\n",
            result.stdout,
        )
        self.assertIn("libvulkan1 mesa-vulkan-drivers", result.stdout)

    def test_vulkan_loader_without_driver_is_reported_missing(self):
        self.simulate_host()
        self.dependency_root("ID=arch\n", vulkan=True)
        (self.root / "hostroot/usr/share/vulkan/icd.d/radeon_icd.json").unlink()
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("vulkan: not found", result.stdout)

    def test_rhel_family_is_reported_unsupported(self):
        self.simulate_host()
        self.dependency_root('ID="rocky"\nID_LIKE="rhel centos fedora"\nVERSION_ID="9.5"\n')
        result = self.check(self.LIBS_MISSING)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("distribution: rocky (RHEL family)\n", result.stdout)
        self.assertIn(
            "this release does not package WebKitGTK 4.1; the launcher is not supported on it\n",
            result.stdout,
        )
        self.assertNotIn("to install:", result.stdout)
        self.assertNotIn("sudo dnf", result.stdout)

    def test_rhel_family_install_runs_no_package_manager(self):
        for tool in PACKAGE_TOOLS:
            shim = self.tools / tool
            shim.write_text("#!/bin/sh\necho ran >&2\nexit 100\n", encoding="utf-8")
            shim.chmod(0o755)
        self.simulate_host()
        self.dependency_root('ID="almalinux"\nID_LIKE="rhel centos fedora"\n')
        result = self.check(self.LIBS_MISSING, 0, "--install", "--yes")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertNotIn("ran", result.stderr)
        self.assertNotIn("+ ", result.stdout)

    def test_read_only_root_keeps_the_family_and_prints_no_command(self):
        self.simulate_host()
        self.dependency_root('ID=bazzite\nID_LIKE="fedora"\n')
        result = self.check(self.LIBS_MISSING)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("distribution: fedora\n", result.stdout)
        self.assertIn(
            "this system has a read-only root; install the missing packages with the system's own "
            "tooling (for example rpm-ostree or a distrobox container)\n",
            result.stdout,
        )
        self.assertNotIn("to install:", result.stdout)

    def test_ostree_booted_marks_a_read_only_root(self):
        self.simulate_host()
        self.dependency_root("ID=fedora\n", ostree=True)
        result = self.check(self.LIBS_MISSING)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("distribution: fedora\n", result.stdout)
        self.assertIn("this system has a read-only root", result.stdout)
        self.assertNotIn("to install:", result.stdout)

    def test_steamos_is_a_read_only_root(self):
        self.simulate_host()
        self.dependency_root("ID=steamos\nID_LIKE=arch\n")
        result = self.check(self.LIBS_MISSING)
        self.assertIn("distribution: arch\n", result.stdout)
        self.assertIn("this system has a read-only root", result.stdout)
        self.assertNotIn("to install:", result.stdout)

    def test_nobara_resolves_to_fedora_with_its_dnf_command(self):
        self.simulate_host()
        self.dependency_root('ID=nobara\nID_LIKE="rhel centos fedora"\nVERSION_ID=42\n')
        result = self.check(self.LIBS_MISSING)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("distribution: fedora\n", result.stdout)
        self.assertIn("to install: sudo dnf install webkit2gtk4.1\n", result.stdout)
        self.assertNotIn("RHEL family", result.stdout)

    def test_oracle_linux_is_the_rhel_family(self):
        self.simulate_host()
        self.dependency_root('ID="ol"\nID_LIKE="fedora"\nVERSION_ID="9.4"\n')
        result = self.check(self.LIBS_MISSING)
        self.assertIn("distribution: ol (RHEL family)\n", result.stdout)
        self.assertIn("this release does not package WebKitGTK 4.1", result.stdout)
        self.assertNotIn("to install:", result.stdout)

    def test_rhel_without_a_version_is_unsupported(self):
        self.simulate_host()
        self.dependency_root('ID="centos"\n')
        result = self.check(self.LIBS_MISSING)
        self.assertIn("this release does not package WebKitGTK 4.1; the launcher is not supported on it\n", result.stdout)

    def test_rhel_10_points_at_epel(self):
        self.simulate_host(missing_tools=("xz",))
        self.dependency_root('ID="almalinux"\nID_LIKE="rhel centos fedora"\nVERSION_ID="10.2"\n')
        result = self.check(self.LIBS_MISSING)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("distribution: almalinux (RHEL family)\n", result.stdout)
        self.assertIn(
            "WebKitGTK 4.1 is in EPEL on this release: enable EPEL (AlmaLinux, Rocky Linux, and CentOS Stream: "
            "sudo dnf install epel-release), then run: sudo dnf install webkit2gtk4.1 xz\n",
            result.stdout,
        )
        self.assertNotIn("to install:", result.stdout)
        self.assertNotIn("not supported", result.stdout)

    def test_rhel_10_install_runs_no_package_manager(self):
        for tool in PACKAGE_TOOLS:
            shim = self.tools / tool
            shim.write_text("#!/bin/sh\necho ran >&2\nexit 100\n", encoding="utf-8")
            shim.chmod(0o755)
        self.simulate_host()
        self.dependency_root('ID="rocky"\nVERSION_ID="10.0"\n')
        result = self.check(self.LIBS_MISSING, 0, "--install", "--yes")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertNotIn("ran", result.stderr)
        self.assertNotIn("+ ", result.stdout)

    def test_engine_libraries_all_found(self):
        self.simulate_host()
        self.dependency_root("ID=ubuntu\n", vulkan=True, libs=self.ENGINE_LIBS + ["libpulse.so.0"])
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("engine libraries: all found\n", result.stdout)
        self.assertIn("audio: libpulse.so.0 found\n", result.stdout)

    def test_alsa_is_the_audio_fallback(self):
        self.simulate_host()
        self.dependency_root("ID=ubuntu\n", libs=self.ENGINE_LIBS + ["libasound.so.2"])
        result = self.check(self.LIBS_OK)
        self.assertIn("audio: libasound.so.2 found\n", result.stdout)

    def test_one_missing_engine_library_is_named_and_not_fatal(self):
        self.simulate_host()
        libs = [name for name in self.ENGINE_LIBS if name != "libXxf86vm.so.1"]
        self.dependency_root("ID=ubuntu\n", vulkan=True, libs=libs + ["libpulse.so.0"])
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("engine libraries: missing libXxf86vm.so.1\n", result.stdout)
        self.assertNotIn("libXext.so.6", result.stdout)
        self.assertIn("apt-file search", result.stdout)

    def test_missing_audio_is_reported_and_not_fatal(self):
        self.simulate_host()
        self.dependency_root("ID=ubuntu\n", vulkan=True, libs=self.ENGINE_LIBS)
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(
            "audio: not found; the game has no sound until libpulse.so.0 or libasound.so.2 is installed\n",
            result.stdout,
        )

    def test_engine_and_audio_lines_are_absent_with_bahamut_wine(self):
        self.simulate_host()
        self.dependency_root("ID=ubuntu\n")
        self.env["BAHAMUT_WINE"] = str(self.root / "wine" / "bin" / "wine")
        result = self.check(self.LIBS_OK)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("engine libraries", result.stdout)
        self.assertNotIn("audio:", result.stdout)

    def test_engine_and_audio_lines_are_absent_off_x86_64(self):
        self.simulate_host("aarch64")
        self.dependency_root("ID=ubuntu\n")
        self.env["BAHAMUT_WINE"] = str(self.root / "wine" / "bin" / "wine")
        result = self.check(self.LIBS_OK)
        self.assertNotIn("engine libraries", result.stdout)
        self.assertNotIn("audio:", result.stdout)

    def test_usage_describes_the_managed_wine(self):
        result = run("bash", LINUX / "install-dependencies.sh", "--help")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("downloads its own Wine on the first Play", result.stdout)


@unittest.skipUnless(sys.platform.startswith("linux"), "the Linux archive needs GNU tar and a Linux host")
class LinuxReadmeTests(unittest.TestCase):
    """Guard packaging/linux/README.md, the archive's README.md, against drift."""

    README = LINUX / "README.md"
    SCRIPT_NAME = re.compile(r"install-dependencies\.sh|install\.sh")
    OPTION = re.compile(r"(?<![\w-])--[a-z][a-z-]*")

    def usage(self, script):
        result = run(LINUX / script, "--help")
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout

    def test_options_appear_in_the_usage_of_the_script_they_are_attributed_to(self):
        text = self.README.read_text(encoding="ascii")
        usages = {name: set(self.OPTION.findall(self.usage(name))) for name in ["install.sh", "install-dependencies.sh"]}
        attributed = {name: set() for name in usages}
        current = None
        for match in re.finditer(f"{self.SCRIPT_NAME.pattern}|{self.OPTION.pattern}", text):
            token = match.group(0)
            if token.startswith("--"):
                self.assertIsNotNone(current, f"{token} precedes any script name")
                attributed[current].add(token)
            else:
                current = token
        for name, options in attributed.items():
            self.assertNotEqual(options, set(), name)
            self.assertEqual(options - usages[name], set(), name)

    def test_relative_link_targets_are_limited_to_files_beside_the_readme(self):
        text = self.README.read_text(encoding="ascii")
        targets = {t.split("#", 1)[0] for t in re.findall(r"\]\(([^)\s]+)\)", text) if not t.startswith("https://")}
        self.assertLessEqual(targets, {"install.sh", "install-dependencies.sh", "Makefile"})

    def test_readme_is_plain_ascii(self):
        self.README.read_bytes().decode("ascii")


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

    def test_staged_readme_is_replaced_by_the_linux_readme(self):
        self.assertEqual(self.package().returncode, 0)
        with tarfile.open(self.output / f"{LABEL}.tar.gz") as archive:
            packaged = archive.extractfile(f"{TOP}/README.md").read()
        self.assertEqual(packaged, (LINUX / "README.md").read_bytes())
        self.assertNotEqual(packaged, b"synthetic README.md")

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
