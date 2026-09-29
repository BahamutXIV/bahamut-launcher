# Development

[Back to the documentation index](README.md)

The workspace contains a pure-Rust core, a Tauri 2 shell, and an x86 Win32
client module. Run the commands below from the repository root. They match the
hosted workflows unless a section names a platform-specific check.

## Prerequisites

- Rust 1.95.0, selected by [`rust-toolchain.toml`](../rust-toolchain.toml),
  with `rustfmt` and `clippy` available.
- Node.js 22 for browser tests, Markdown-reference validation, and the C++
  formatter wrapper.
- Python with `clang-format==22.1.8` for the project's native C++ formatting.
- [actionlint 1.7.12](https://github.com/rhysd/actionlint/releases/tag/v1.7.12)
  for workflow checks.
- Native client work on Windows requires Visual Studio 2022 with the Win32
  C++ toolchain.
- Cross-compiling the client module on macOS or Linux requires CMake 3.25 or
  later and the [llvm-mingw](https://github.com/mstorsjo/llvm-mingw/releases)
  release pinned in
  [`release-binaries.yml`](../.github/workflows/release-binaries.yml). Running
  its tests requires Wine.
- Linux Tauri work requires the packages listed in
  [Tauri and WebView](#tauri-and-webview).
- Packaging and checking the Linux archive
  ([`package-linux-tarball.sh`](../scripts/package-linux-tarball.sh),
  [`test-linux-package.py`](../scripts/test-linux-package.py)) requires a
  Linux host with GNU tar, gzip, Python 3, and `make`. `shellcheck` and
  `desktop-file-validate` run the matching repository checks.
- Packaging and checking the macOS app bundle
  ([`package-macos-app.sh`](../scripts/package-macos-app.sh),
  [`check-macos-app-zip.sh`](../scripts/check-macos-app-zip.sh)) requires
  macOS itself, with Xcode's `codesign`, `plutil`, `xattr`, `ditto`, and
  `lipo`.

Use the tracked formatter configuration for each changed language. Keep
comments and public documentation within the [comments and prose policy](ai_agents/comments-and-prose.md).
Install the pinned native formatter with:

```powershell
python -m pip install clang-format==22.1.8
```

## Workspace checks

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
node scripts/check-markdown-links.mjs
node scripts/check-cpp-format.mjs
actionlint
python tools/format_lua.py install
python tools/format_lua.py check
```

The Markdown check validates tracked files and local targets. The C++ check
audits changed files and untracked project files under `client/{api,src,tests}`.
Use `--base <ref>` for committed changes since a merge base and `--all` only
for an intentional whole-tree audit. Format an owned file with
`clang-format -i path/to/file.cpp` and preserve vendor sources. `actionlint`
checks workflow syntax. The hosted workflow runs these checks and adds the
platform checks below.

On Linux, `cargo test --lib pinned_engine -- --ignored` downloads the pinned
[Wine engine](configuration.md#linux-wine-engine) and checks that it verifies
and unpacks. It runs on any Linux CPU architecture and does not start Wine.

## Rust and NASM

The checkout pins Rust 1.95.0 and includes `rustfmt` and `clippy` in
`rust-toolchain.toml`. Run `rustup show` if Cargo is using an unexpected
toolchain. On Windows with MSVC, `.cargo/config.toml` sets
`AWS_LC_SYS_PREBUILT_NASM=1`, so `aws-lc-sys` uses its prebuilt objects, and
statically links the x64 launcher shell's Rust runtime so it can report and
repair a missing WebView2 Runtime before its UI starts. If Cargo
reports a missing NASM executable, confirm that file is present and rerun
`cargo build --workspace`.

The MSVC-built Win32 loader and native modules in the Windows package require
the x86 Visual C++ Runtime. The llvm-mingw builds in the Linux and macOS
archives link their runtime statically and import only Windows system DLLs and
the UCRT API-set DLLs, which Wine supplies.

## Tauri and WebView

The root crate is Tauri-free, so `cargo test --workspace` does not require a
WebView. The shell is the Tauri 2 package. For shell-only development on Linux
or macOS, run:

```powershell
cargo run -p bahamut-launcher-shell
```

The frontend is vanilla HTML, CSS, and JavaScript with no separate frontend
build step. The shell uses system WebView2 on Windows. If WebView2 is missing,
the launcher downloads and runs its pinned bootstrapper at startup. Before game
launch, it does the same for the x86 Visual C++ redistributable when missing.
Release preparation verifies each pinned R2 installer without bundling it.
On Linux, install the same packages used by CI:

```bash
sudo apt-get update
sudo apt-get install --no-install-recommends -y \
  libwebkit2gtk-4.1-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev
```

That list is for building. A built launcher needs only WebKitGTK 4.1 and
GTK 3 at runtime, with their GLib, libsoup, GStreamer, cairo, pango, Wayland,
and X11 dependencies; its dynamic closure includes neither
libayatana-appindicator nor librsvg.
[`install-dependencies.sh`](../packaging/linux/install-dependencies.sh)
checks a built launcher with `--check --launcher <path>`.

The install-strip preview opens the real frontend in nine isolated Chrome or
Edge tabs. It uses mocked launcher commands and cannot change configuration,
client files, or the download cache:

```powershell
node scripts/preview-install-strip.mjs
```

Use Ctrl+Tab to move through the stage labels and close the preview browser to
stop its temporary local server.

## Native client and browser checks

The x86 client module builds with the Visual Studio generator on Windows and
cross-compiles with llvm-mingw on macOS and Linux. Both use CTest. Cross-built
tests run under Wine only when configured with
`-DCMAKE_CROSSCOMPILING_EMULATOR` pointing at `client/tools/run-under-wine.sh`,
as shown in [Tests under Wine](../client/README.md#tests-under-wine). The full
commands and test coverage limits are in [client/README.md](../client/README.md).
Wine results apply only to the selected Wine build, prefix, and bundled stub
client. They do not establish retail client behavior or support for another
Wine build.

The browser checks run with Node.js:

```powershell
node --test src-tauri/ui/browser-test.mjs
```

The publisher for game content uses Python 3.10 or later and only its standard
library. Its synthetic tests need no game files or storage credentials:

```powershell
python scripts/test-package-game-content.py
python scripts/test-intake-full-client.py
```

On Linux or macOS, test package publication with synthetic build and staging
inputs. This checks destination links and file preservation, not compilation
or game launch:

```bash
python3 scripts/test-unix-package.py
```

Full-client intake is optional because it reads the complete retail ZIP. Use
`python scripts/intake-full-client.py --help` for its archive, output, and
staging inputs. A live reqwest test uses `BAHAMUT_LIVE_CONTENT_CACHE` to name a
local cache directory.

See the [game content delivery reference](content-delivery.md) for the
separate optional production download check.

The Windows workflow also checks synthetic release archives and package updates.
The fixture checks package contents, not game launch.
The Linux CI job packages the Linux archive from a placeholder executable,
installs it with `install.sh` and with `make DESTDIR=... PREFIX=/usr install`,
and uninstalls it again. The repository job runs `shellcheck` on the Linux
packaging scripts, `desktop-file-validate` on the desktop entry, and
`python scripts/test-linux-package.py`.
The macOS CI job cross-compiles the loader, `bahamut.dll`, and both plugins
with llvm-mingw, stages the app bundle from a placeholder universal
executable, and runs `python3 scripts/test-macos-app-package.py`, but does
not run the client tests. See [Release process](releasing.md) for the
workflow's artifact requirements.

## Staged Windows package

Close the launcher and game, then choose one command from the repository root
with the Windows native toolchain:

```powershell
.\scripts\build-windows-package.ps1
.\scripts\build-windows-package.ps1 -Configuration Release
```

The default Debug build compiles the launcher and native payload, then stages
`out/dev/bahamut-launcher.exe`. Release stages the same package at
`out/release/<version>/bahamut-launcher.exe`. These commands produce a folder,
not a ZIP, and retain local settings, custom packages, and existing
`scripts/default.txt` commands. Use the staged executable for packaged launch
validation. The unpackaged Cargo shell is not equivalent.

The synthetic archive check can run without a build:

```powershell
.\scripts\windows_release_archive_manifest_and_cleanliness.ps1 -AllowDirtyWorktree
```

`-AllowDirtyWorktree` permits local source edits during the check.
`-KeepArtifacts` retains its generated archive and staging files for inspection.
The fixture checks package contents, not game launch.

## Staged Linux or macOS tree

Close the launcher and game, then choose one command from the repository root
with llvm-mingw installed:

```bash
./scripts/build-unix-package.sh
./scripts/build-unix-package.sh --release
```

The default Debug build cross-compiles the client module into
`out/client-mingw-debug`, builds the shell, and publishes `out/dev/bahamut-launcher`.
Release uses `out/client-mingw` and publishes
`out/release/<version>/bahamut-launcher`. Publishing keeps an existing
`scripts/default.txt` and never deletes files. `--test` first runs the client
module tests under Wine (the launcher's managed macOS or Linux engine when
installed, otherwise `wine` on `PATH`, with the prefix under
`out/wine-test-prefix`).
`--skip-build` republishes existing build outputs. The script's header lists
the toolchain lookup order and the remaining options.
`scripts/build-and-run-unix.sh` takes the same options, then starts the
published launcher from its package directory.

To stage the same release layout manually:

```bash
cargo build --release --locked -p bahamut-launcher-shell
./scripts/stage-unix-release.sh \
  --launcher target/release/bahamut-launcher-shell \
  --client-build out/client-mingw \
  --destination <empty dir>
```

Run `bahamut-launcher` from the staged folder. On macOS and Linux the tree
carries the loader, `bahamut.dll`, and the plugins and addons maintained in this
repository, so Play takes the extension launch. The bare Cargo shell has none
of them. The staged tree has no package marker, so it keeps the
[portable layout](configuration.md#portable-launcher-tree).

## Linux archive

[`package-linux-tarball.sh`](../scripts/package-linux-tarball.sh) builds the
release archive from a launcher binary and a client build. It stages through
`stage-unix-release.sh`, adds the package marker, install scripts, desktop
entry, and icons, asserts the exact file manifest and modes, and writes the
archive with GNU tar:

```bash
cargo build --release --locked -p bahamut-launcher-shell
./scripts/package-linux-tarball.sh \
  --launcher target/release/bahamut-launcher-shell \
  --client-build out/client-mingw \
  --label bahamut-launcher-dev-linux-x86_64 \
  --output out/linux-dist
```

The script writes `<output>/<label>.tar.gz` and its `.sha256` sidecar. File
times come from `SOURCE_DATE_EPOCH`, or from the last commit when it is unset.
The script is the canonical record of the archive manifest.

[`test-linux-package.py`](../scripts/test-linux-package.py) builds an archive
from a placeholder executable and synthetic client artifacts, installs it
into a temporary prefix and a `DESTDIR` staging root, and uninstalls it:

```bash
python3 scripts/test-linux-package.py
```

Both run directly on a Linux host without a container, and the test needs no
Cargo build. The test is skipped on other platforms. It checks package
contents and the install scripts, not the launcher binary or game launch.

[`install.sh`](../packaging/linux/install.sh) takes `--prefix`, `--pkgdir`,
and `--destdir`, and also reads `PREFIX` and `DESTDIR` from the environment.
The payload directory comes only from `--pkgdir`, so an exported `PKGDIR`,
such as the one Portage sets, does not redirect it. The archive's
[`Makefile`](../packaging/linux/Makefile) runs `./install.sh` from its own
directory, so run it as `make -C <extracted dir> install`; for example
`make -C bahamut-launcher DESTDIR=<root> PREFIX=/usr install` stages a
package root, and a command-line `PKGDIR=<dir>` sets the payload directory.

## Staged macOS app

Build the universal launcher binary the way
[`release-binaries.yml`](../.github/workflows/release-binaries.yml) does,
then package and zip it the way the release does:

```bash
cargo build --release --locked --target aarch64-apple-darwin -p bahamut-launcher-shell
cargo build --release --locked --target x86_64-apple-darwin -p bahamut-launcher-shell
mkdir -p out/universal
lipo -create \
  target/aarch64-apple-darwin/release/bahamut-launcher-shell \
  target/x86_64-apple-darwin/release/bahamut-launcher-shell \
  -output out/universal/bahamut-launcher-shell
./scripts/package-macos-app.sh --launcher out/universal/bahamut-launcher-shell \
  --client-build out/client-mingw --destination out/macos-app
ditto -c -k --norsrc --keepParent "out/macos-app/Bahamut Launcher.app" out/macos-app.zip
./scripts/check-macos-app-zip.sh --zip out/macos-app.zip
```

Omitting `--sign` signs the app ad hoc, the same as a release build without
the signing secrets. `./scripts/build-unix-package.sh` still publishes the
bare portable tree, without an app bundle, for the dev loop.

## Additional launcher logs

To enable debug detail in a staged Windows package, run from the repository root:

```powershell
$env:RUST_LOG = "bahamut_launcher=debug,wry=warn"
.\out\dev\bahamut-launcher.exe
```

Use the complete staged package, not the unpackaged Cargo shell. Review logs
for private paths and account information before sharing them.

## Exact-binary checks

The manually dispatched [`Retail Checks`](../.github/workflows/retail-checks.yml)
workflow verifies the manifest-pinned `ffxivgame.exe` identity, the expected
original bytes at supported patch sites, and the bytes produced by applying the
patches. This is exact-binary validation, not proof of live client behavior. The
input identity is declared in
[`manifests/retail-inputs.json`](../manifests/retail-inputs.json). The binary
itself is not part of this repository.

## Generated icon assets

The application icon is generated from `src-tauri/icons/icon-source.png`. After
replacing the source artwork, run:

```powershell
cargo run --release --manifest-path tools/icon-gen/Cargo.toml
```

This regenerates PNG, the six-size Windows ICO, the macOS ICNS with its
transparent icon-grid padding, and the Linux hicolor icons at
`packaging/linux/icons/hicolor/{48x48,128x128,256x256}/apps/bahamut-launcher.png`.
The macOS app bundle consumes `src-tauri/icons/icon.icns` as
`Contents/Resources/icon.icns`. The Linux archive ships the hicolor icons
under `share/icons/hicolor/`.

## What the checks cover

Tests and implementation in this tree describe the launcher's current behavior.
They do not prove retail client behavior, server behavior, or compatibility on
every Wine and operating system combination. Limit compatibility, protocol,
launch patch, and platform claims to the versions and environments named by their
source.
