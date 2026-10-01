# Flatpak S0 tester

The S0 tester packages the native Linux launcher in the GNOME 50 Flatpak
runtime. It is a reproducibility and runtime-boundary test artifact. It does
not establish SteamOS or retail-client support.

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
`--version`, the launcher log and its HTTP user agent. The bundle does not include the GNOME runtime or SDK; install those
from the configured Flatpak remote before installing the bundle.

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

## Install and smoke test

Install the produced bundle for the current user, then query the packaged
launcher before opening its window:

```bash
cd out/flatpak-s0
sha256sum --check bahamut-launcher-tester-s0-<version>.flatpak.sha256
flatpak install --user ./bahamut-launcher-tester-s0-<version>.flatpak
flatpak run io.github.BahamutXIV.Launcher.Tester --version
flatpak run io.github.BahamutXIV.Launcher.Tester
```

Use a fresh Flatpak data directory for the first run. Confirm that the WebView
opens, launcher state is written below
`~/.var/app/io.github.BahamutXIV.Launcher.Tester/data/launcher`, and the
Documents directory is present. A second run should retain the launcher state.
Record failures with the bundle digest and the identity file beside the test
notes.

If the window opens but stays blank, WebKitGTK's DMA-BUF renderer has failed
on the host graphics driver. Rerun with
`flatpak run --env=WEBKIT_DISABLE_DMABUF_RENDERER=1 io.github.BahamutXIV.Launcher.Tester`
and record that the override was needed.

The tester grants only these runtime permissions:

| Permission | Purpose |
| --- | --- |
| `--share=network` | launcher login, content, Wine, and DXVK downloads |
| `--share=ipc` | desktop toolkit and WebKit IPC |
| `--socket=fallback-x11` | X11 fallback display |
| `--socket=wayland` | Wayland display |
| `--socket=pulseaudio` | game audio |
| `--device=dri` | runtime graphics device access |

No host filesystem, broad home directory, i386, or GL32 permission is part of
this tester manifest. If a test requires one, record the concrete failure
before proposing a manifest change.

## Deck evidence template

Leave each value blank or mark it `unverified` until it is observed on the
named device. This template records evidence; it does not make an acceptance
claim.

```text
Tester branch: s0
Bundle filename:
Bundle SHA-256:
Source commit:
Source tree:
Deck model:                    [unverified]
SteamOS build:                 [unverified]
Flatpak Platform branch/commit:[unverified]
Flatpak SDK branch/commit:     [unverified]
Graphics GL branches/commits:  25.08; 25.08-extra; 1.4 [unverified]
Rust/Cargo toolchain receipt:
Wine engine version/hash:      Wine 11.18 [unverified]
DXVK version/hash:             DXVK 3.0 [unverified]
Client identity/build:         [unverified]
WebView opened:                [unverified]
Real client launch:            [unverified]
x86 helper transaction:        [unverified]
Real extensions/addons:        [unverified]
Actual renderer:               [unverified]
Audio and input:               [unverified]
State retained after restart:  [unverified]
Notes and logs:
```

For a test run, include the exact launch command, whether Desktop or Gaming
Mode was used, the selected game path, the renderer evidence, and the relevant
launcher and Wine logs. Separate a successful package smoke test from any
claim about a real client, helper injection, extensions, or SteamOS hardware.
