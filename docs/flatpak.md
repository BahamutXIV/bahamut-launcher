# Flatpak

[Back to the documentation index](README.md)

The Flatpak package runs Bahamut Launcher on SteamOS and Steam Deck using the
GNOME 50 runtime and the launcher's managed Wine engine. Use
[Installation](#installation) to install the release bundle or [Build](#build)
to produce one from source.

The manifest uses app ID `io.github.BahamutXIV.Launcher.Tester` and branch
`s0`. Its app payload is read from `/app/lib/bahamut-launcher`. The wrapper
keeps launcher state at `$XDG_DATA_HOME/launcher`, writes an
`XDG_DOCUMENTS_DIR` entry under its private `$XDG_DATA_HOME/xdg-config`, and
creates `$XDG_DATA_HOME/Documents` for the native launcher and Wine client to
share. The wrapper does not change `HOME`. It also suppresses Wine's optional
Mono and Gecko installers because the native S0 client and helper do not use
those components.

## Build

Use Linux with `flatpak`, `flatpak-builder`, `elfutils`, `desktop-file-utils`,
Python 3, Cargo, and remotes that provide GNOME Platform and SDK 50. The SDK
must include GTK 3, WebKitGTK 4.1, and CMake. The manifest supplies official
Rust 1.95.0 components and checks Rust and Cargo versions before compiling.

Add Flathub to the user installation and install the runtime:

```bash
flatpak remote-add --user --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
flatpak install --user flathub org.gnome.Platform//50
```

Also install `org.gnome.Sdk//50` when building with `flatpak-builder`. Put the
build directory on a native Linux filesystem, including under WSL.
Windows-backed WSL paths lack the filesystem behavior Flatpak builds need.

Build from a commit that contains the Flatpak packaging files:

```bash
git status --short
./scripts/build-flatpak.sh \
  --source-ref HEAD \
  --work-dir /absolute/path/to/empty-build-dir \
  --keep-work
```

The preparation step packages the selected Git commit. Uncommitted UI changes
are excluded. It vendors Cargo dependencies before the SDK build, then the
manifest runs Cargo offline. Official Rust 1.95.0 rustc, Cargo, and rust-std
component downloads are checked against SHA-256 digests.

Use an empty absolute `--work-dir` below the selected scratch root. With
`--keep-work`, you can inspect build files and CTest results afterwards.
Without it, the script removes its generated work directory after writing
the bundle and identity files. **The work directory is disposable; never
point it at an existing non-empty directory.**

The default output directory, `out/flatpak-s0/`, contains:

- `<label>.flatpak`, where the label defaults to
  `bahamut-launcher-tester-s0-<version>`
- A `<label>.flatpak.sha256` checksum sidecar
- A `<label>.flatpak.identity.json` file recording the source commit and tree,
  Cargo and Rust inputs, llvm-mingw digest, runtime and SDK commits, SDK
  toolchain receipt, and the bundle digest

`--label <name>` sets the bundle and sidecar names, and `--release-tag <tag>`
(default: the `BAHAMUT_RELEASE_TAG` environment variable) makes the packaged
launcher report that tag from `--version`.
`scripts/package-flatpak-archive.py` wraps the bundle, its sidecars, and the
[package README](../packaging/flatpak/README.md) into the release zip.
[Release process](releasing.md) describes the pipeline that publishes it.

The packaged source has no Git metadata. The launcher therefore reports
`source.launcher_version` from the identity file in `--version`, the log, and
its HTTP user agent. The bundle includes neither GNOME runtime nor SDK. It
names Flathub as its runtime source, so `flatpak install` offers to add the
remote and installs the runtime when it is missing.

To inspect source identity without building a bundle, prepare an empty scratch
directory:

```bash
python3 scripts/prepare-flatpak.py \
  --repo-root . \
  --source-ref HEAD \
  --work-dir /absolute/path/to/flatpak-s0-prep
```

The work directory is disposable. Do not point it at an existing non-empty
directory.

## Installation

Download `bahamut-launcher-vX.Y.Z-linux-flatpak.zip` from the
[releases page](https://github.com/BahamutXIV/bahamut-launcher/releases). It
unpacks into a single `bahamut-launcher-flatpak/` folder with four files:

- `README.md`, the [package README](../packaging/flatpak/README.md)
- `bahamut-launcher-vX.Y.Z-linux-flatpak.flatpak`, the bundle
- `bahamut-launcher-vX.Y.Z-linux-flatpak.flatpak.sha256`, its checksum
- `bahamut-launcher-vX.Y.Z-linux-flatpak.flatpak.identity.json`, its build
  identity

Install the bundle for the current user. Check its checksum and version before
opening the launcher:

```bash
unzip bahamut-launcher-vX.Y.Z-linux-flatpak.zip
cd bahamut-launcher-flatpak
sha256sum --check bahamut-launcher-vX.Y.Z-linux-flatpak.flatpak.sha256
flatpak install --user ./bahamut-launcher-vX.Y.Z-linux-flatpak.flatpak
flatpak run io.github.BahamutXIV.Launcher.Tester --version
flatpak run io.github.BahamutXIV.Launcher.Tester
```

The same install command with a newer bundle updates the package in place,
because every release uses the same app ID and branch. To remove the package,
run `flatpak uninstall --user io.github.BahamutXIV.Launcher.Tester`, and add
`--delete-data` to remove its state folder as well.

Launcher state is stored below
`~/.var/app/io.github.BahamutXIV.Launcher.Tester/data/launcher`. Install or
select the game client, choose a server profile, sign in, and choose Play.

A window that opens but stays blank is covered in
[Troubleshooting](troubleshooting.md). For Flatpak, pass the WebKitGTK
override as
`flatpak run --env=WEBKIT_DISABLE_DMABUF_RENDERER=1 io.github.BahamutXIV.Launcher.Tester`
when the blank-window workaround is needed.

The package declares these runtime permissions:

| Permission | Purpose |
| --- | --- |
| `--share=network` | launcher login, content, Wine, and DXVK downloads |
| `--share=ipc` | desktop toolkit and WebKit IPC |
| `--socket=fallback-x11` | X11 fallback display |
| `--socket=wayland` | Wayland display |
| `--socket=pulseaudio` | game audio |
| `--device=dri` | runtime graphics device access |
| `--allow=multiarch` | 32-bit PE execution (game and `bahamut-loader.exe`) under the WoW64 Wine engine; without it the Wine prefix is 64-bit only |
| `--filesystem=~/Games:create` | writable game destinations and sibling staging directories under `~/Games`; Flatpak creates `~/Games` when absent |

Host filesystem access is limited to `~/Games` and its subdirectories.
Flatpak applies this declared permission when starting the installed package;
the running launcher does not grant itself new filesystem permissions.

### Game directory selection

The default game destination is
`~/.var/app/io.github.BahamutXIV.Launcher.Tester/data/launcher/game`. Use
Home's Install action for this destination or Path to choose another folder.

Paths under `/run/user/<uid>/doc/` or `/run/flatpak/doc/` are
[document portal](https://flatpak.github.io/xdg-desktop-portal/docs/documents-and-fuse.html)
exports. Installation needs direct access to the destination and its parent:
it rejects symbolic-link ancestors and stages the game in a sibling directory
before publishing it. Exporting only the selected game folder through the
portal does not provide that sibling access. Restarting keeps the selected
path; it does not reset it to the default.

To install in a custom internal-storage directory under `~/Games`:

1. Create an empty `Bahamut` folder inside `~/Games` using the desktop file
   manager.
2. Use Home's Path action to select `~/Games/Bahamut`.
3. Check that the displayed destination is its ordinary absolute filesystem
   path before pressing Install. If it points into the document portal,
   include the path and launcher log in a support report.
4. Complete installation, choose a server profile, sign in, and choose Play.

An SD-card or external-drive destination needs its actual mounted parent
granted using a
[Flatpak override](https://docs.flatpak.org/en/latest/flatpak-command-reference.html#flatpak-override)
and the same displayed-path check. Close the launcher before changing its
permissions, then reopen it and select the directory again.

## Checking package permissions

When checking a filesystem-permission change, use a profile without
filesystem overrides so access comes from the package. Inspect the installed
permissions and both the global and application-specific override layers:

```bash
flatpak info --show-permissions io.github.BahamutXIV.Launcher.Tester
flatpak override --show
flatpak override --show io.github.BahamutXIV.Launcher.Tester
flatpak override --user --show
flatpak override --user --show io.github.BahamutXIV.Launcher.Tester
```

Confirm that the installed permissions include `~/Games` and that no override
grants or denies filesystem access. If `~/Games` was absent before starting
the package, confirm that Flatpak created it. Select an empty destination
under `~/Games`, complete installation, and confirm that the launcher retains
the installed client after exit and restart.
