# Flatpak

[Back to the documentation index](README.md)

The Flatpak package runs Bahamut Launcher on SteamOS and Steam Deck using the
GNOME 50 runtime and the launcher's managed Wine engine. Use
[Installation](#installation) to install a bundle or [Build](#build) to
produce one from source.

The manifest uses app ID `io.github.BahamutXIV.Launcher.Tester` and branch
`s0`. Its app payload is read from `/app/lib/bahamut-launcher`. The wrapper
keeps launcher state at `$XDG_DATA_HOME/launcher`, writes an
`XDG_DOCUMENTS_DIR` entry under its private `$XDG_DATA_HOME/xdg-config`, and
creates `$XDG_DATA_HOME/Documents` for the native launcher and Wine client to
share. The wrapper does not change `HOME`. It also suppresses Wine's optional
Mono and Gecko installers because the native S0 client and helper do not use
those components.

## Build

Run the build on Linux with `flatpak`, `flatpak-builder`, `elfutils`,
`desktop-file-utils`, Python 3, Cargo, and
the Flatpak remotes that provide GNOME Platform and SDK 50. The SDK must
provide GTK 3, WebKitGTK 4.1, and CMake. The manifest supplies official Rust
1.95.0 components and checks the Rust and Cargo versions before compiling.

Configure Flathub for the user installation and install the runtime before
building or installing the bundle:

```bash
flatpak remote-add --user --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
flatpak install --user flathub org.gnome.Platform//50
```

Install `org.gnome.Sdk//50` as well when using `flatpak-builder`. Use a native
Linux filesystem for the build directory, including when building under WSL.
Windows-backed WSL paths do not provide the filesystem behavior required by
Flatpak build processing.

The preparation step archives the selected Git commit and requires the
Flatpak packaging files in that commit. Uncommitted UI files therefore do not
enter the source context. It vendors Cargo dependencies before the SDK build
and uses Cargo offline in the manifest. The manifest downloads the official
Rust 1.95.0 rustc, Cargo, and rust-std component archives with SHA-256
checksums.

```bash
git status --short
./scripts/build-flatpak.sh \
  --source-ref HEAD \
  --work-dir /absolute/path/to/empty-build-dir \
  --keep-work
```

The default output is `out/flatpak-s0/`. It contains a versioned
`bahamut-launcher-tester-s0-<version>.flatpak`, its `.sha256` sidecar, and a
`.identity.json` file containing the source commit and tree, Cargo and Rust
inputs, llvm-mingw digest, runtime and SDK commits, and the SDK toolchain
receipt. The archived source carries no Git metadata, so the packaged
launcher reports the `source.launcher_version` value from that file for
`--version`, the launcher log and its HTTP user agent. The bundle does not
include the GNOME runtime or SDK; install those from the configured Flatpak
remote before installing the bundle.

Use an empty absolute `--work-dir` below the selected scratch root when build
files or CTest results must be inspected after the build. `--keep-work` keeps
that directory. Without it, the script removes its generated work directory
after the bundle and identity files are written.

To inspect the source identity without making a bundle, run the preparation
script with an empty scratch directory:

```bash
python3 scripts/prepare-flatpak.py \
  --repo-root . \
  --source-ref HEAD \
  --work-dir /absolute/path/to/flatpak-s0-prep
```

The work directory is disposable. Do not point it at an existing non-empty
directory.

## Installation

Install the produced bundle for the current user, then query the packaged
launcher before opening its window:

```bash
cd out/flatpak-s0
sha256sum --check bahamut-launcher-tester-s0-<version>.flatpak.sha256
flatpak install --user ./bahamut-launcher-tester-s0-<version>.flatpak
flatpak run io.github.BahamutXIV.Launcher.Tester --version
flatpak run io.github.BahamutXIV.Launcher.Tester
```

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

A selected path under `/run/user/<uid>/doc/` or `/run/flatpak/doc/` refers to
the [document portal](https://flatpak.github.io/xdg-desktop-portal/docs/documents-and-fuse.html).
This installer requires a directly accessible destination and parent: it
rejects symbolic-link ancestors and stages the game in a sibling directory
before publishing it. A portal export of the selected game folder does not
provide the required sibling access. Restarting retains the selected
destination; it does not restore the default path.

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
