# Development

[Back to the documentation index](README.md)

Run these commands from the repository root to build and check the launcher.
The workspace has a pure-Rust core, a Tauri 2 shell, and an x86 Win32 client
module. Commands match the hosted workflows unless a section specifies a
platform-only check.

## Prerequisites

- Rust 1.95.0, selected by [`rust-toolchain.toml`](../rust-toolchain.toml),
  with `rustfmt` and `clippy`.
- Node.js 22 for browser tests, Markdown link validation, and the C++ formatter
  wrapper.
- Python with `clang-format==22.1.8` for native C++ formatting.
- [actionlint 1.7.12](https://github.com/rhysd/actionlint/releases/tag/v1.7.12)
  for workflow checks.
- For native client work on Windows: Visual Studio 2022 with the Win32 C++
  toolchain.
- For cross-compiling the client module on Linux or macOS: CMake 3.25 or
  later and the [llvm-mingw](https://github.com/mstorsjo/llvm-mingw/releases)
  release pinned in
  [`release-binaries.yml`](../.github/workflows/release-binaries.yml).
  Running its tests also requires Wine.
- For Linux Tauri work: the packages in [Tauri and WebView](#tauri-and-webview).
- For Linux packaging and package checks with
  [`package-linux-tarball.sh`](../scripts/package-linux-tarball.sh) and
  [`test-linux-package.py`](../scripts/test-linux-package.py): a Linux host
  with GNU tar, gzip, Python 3, and `make`. Install `shellcheck` and
  `desktop-file-validate` for the matching repository checks.
- For macOS packaging and ZIP checks with
  [`package-macos-app.sh`](../scripts/package-macos-app.sh) and
  [`check-macos-app-zip.sh`](../scripts/check-macos-app-zip.sh): macOS with
  Xcode's `codesign`, `plutil`, `xattr`, `ditto`, and `lipo`.

Use the tracked formatter configuration for each language you change. Follow
the [comments and prose policy](ai_agents/comments-and-prose.md) for comments
and public documentation. Install the pinned native formatter with:

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
audits changed files and untracked project files in `client/{api,src,tests}`.
Add `--base <ref>` to check committed changes since a merge base; reserve
`--all` for an intentional whole-tree audit. Format your files with
`clang-format -i path/to/file.cpp` and leave vendor sources untouched.
`actionlint` checks workflow syntax. Hosted checks also run the platform
checks below.

On Linux, `cargo test --lib pinned_engine -- --ignored` downloads the pinned
[Wine engine](configuration.md#linux-wine-engine) and verifies that it checks
and unpacks successfully. This runs on any Linux CPU architecture without
starting Wine.

## Rust and NASM

The checkout's `rust-toolchain.toml` pins Rust 1.95.0 with `rustfmt` and
`clippy`. Run `rustup show` if Cargo selects an unexpected toolchain.

On Windows with MSVC, `.cargo/config.toml` sets
`AWS_LC_SYS_PREBUILT_NASM=1` so `aws-lc-sys` uses prebuilt objects. It also
statically links the x64 launcher's Rust runtime, allowing the shell to report
and repair a missing WebView2 Runtime before opening the UI. If Cargo reports
a missing NASM executable, check that `.cargo/config.toml` is present and
rerun `cargo build --workspace`.

The Windows package's MSVC-built Win32 loader and native modules require the
x86 Visual C++ Runtime. Linux and macOS llvm-mingw builds statically link
their runtime and import only Windows system DLLs and UCRT API-set DLLs,
which Wine provides.

## Tauri and WebView

The root crate is Tauri-free; `cargo test --workspace` needs no WebView. The
shell is the Tauri 2 package. For shell-only development on Linux or macOS:

```powershell
cargo run -p bahamut-launcher-shell
```

The frontend uses vanilla HTML, CSS, and JavaScript with no separate build
step. On Windows, the shell uses system WebView2. If that runtime is missing,
the launcher downloads and runs its pinned bootstrapper at startup. It does
the same for a missing x86 Visual C++ redistributable before game launch.
Release preparation verifies each pinned R2 installer without bundling it.

For Linux builds, install CI's development packages:

```bash
sudo apt-get update
sudo apt-get install --no-install-recommends -y \
  libwebkit2gtk-4.1-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev
```

A built launcher needs only WebKitGTK 4.1 and GTK 3 at runtime, plus their
GLib, libsoup, GStreamer, cairo, pango, Wayland, and X11 dependencies. Its
dynamic dependencies include neither libayatana-appindicator nor librsvg.
Use [`install-dependencies.sh`](../packaging/linux/install-dependencies.sh)
with `--check --launcher <path>` to check a built launcher.

Preview the install strip in nine isolated Chrome or Edge tabs:

```powershell
node scripts/preview-install-strip.mjs
```

The preview opens the real frontend with mocked launcher commands. It cannot
change configuration, client files, or the download cache. Use Ctrl+Tab to
move through stage labels. Closing the preview browser stops its temporary
local server.

## Native client and browser checks

Build the x86 client module with the Visual Studio generator on Windows or
llvm-mingw on Linux and macOS. Both use CTest. Cross-built tests run under
Wine only when `-DCMAKE_CROSSCOMPILING_EMULATOR` points to
`client/tools/run-under-wine.sh`; see
[Tests under Wine](../client/README.md#tests-under-wine).

[client/README.md](../client/README.md) has the full commands and coverage
limits. Wine results apply only to the selected build, prefix, and bundled
stub client. They do not establish retail behavior or support for other Wine
builds.

Run browser checks with Node.js:

```powershell
node --test src-tauri/ui/browser-test.mjs
```

The game content publisher needs Python 3.10 or later and only the standard
library. Its synthetic tests need neither game files nor storage credentials:

```powershell
python scripts/test-package-game-content.py
python scripts/test-intake-full-client.py
```

On Linux or macOS, check package publication with synthetic build and staging
inputs:

```bash
python3 scripts/test-unix-package.py
```

This tests destination links and file preservation. It does not compile code
or launch the game.

Full-client intake is optional because it reads the entire retail ZIP. Run
`python scripts/intake-full-client.py --help` for archive, output, and staging
inputs. The live reqwest test takes its local cache directory from
`BAHAMUT_LIVE_CONTENT_CACHE`. See the
[game content delivery reference](content-delivery.md) for the separate,
optional production download check.

Platform CI adds these package checks:

- Windows checks synthetic release ZIPs and package updates. The fixture
  validates contents, not game launch.
- Linux packages a placeholder executable, installs with `install.sh` and
  `make DESTDIR=... PREFIX=/usr install`, then uninstalls it. The repository
  job runs `shellcheck` on Linux packaging scripts,
  `desktop-file-validate` on the desktop entry, and
  `python scripts/test-linux-package.py`.
- macOS cross-compiles the loader, `bahamut.dll`, and both plugins with
  llvm-mingw. It stages an app bundle around a placeholder universal
  executable and runs `python3 scripts/test-macos-app-package.py`, but does
  not run client tests.

See [Release process](releasing.md) for workflow artifact requirements.

## Staged Windows package

Close the launcher and game. From the repository root, with the Windows native
toolchain available, choose one command:

```powershell
.\scripts\build-windows-package.ps1
.\scripts\build-windows-package.ps1 -Configuration Release
```

Debug is the default. It builds the launcher and native payload, then stages
`out/dev/bahamut-launcher.exe`. Release stages the same package at
`out/release/<version>/bahamut-launcher.exe`.

Both produce a folder rather than a ZIP and preserve local settings, custom
packages, and existing `scripts/default.txt` commands. Validate packaged
launches with the staged executable. The unpackaged Cargo shell does not
provide the same package.

Run the synthetic archive check without a build:

```powershell
.\scripts\windows_release_archive_manifest_and_cleanliness.ps1 -AllowDirtyWorktree
```

`-AllowDirtyWorktree` permits local source edits during the check. Add
`-KeepArtifacts` to retain the generated ZIP and staging files for inspection.
This fixture checks package contents, not game launch.

## Staged Linux or macOS tree

Close the launcher and game. From the repository root, with llvm-mingw
installed, choose one command:

```bash
./scripts/build-unix-package.sh
./scripts/build-unix-package.sh --release
```

The default Debug build cross-compiles the client module into
`out/client-mingw-debug`, builds the shell, and publishes
`out/dev/bahamut-launcher`. Release uses `out/client-mingw` and publishes
`out/release/<version>/bahamut-launcher`. Publication preserves an existing
`scripts/default.txt` and never deletes files.

Add `--test` to run client tests under Wine first. The script uses the
launcher's managed macOS or Linux engine if installed, otherwise `wine` on
`PATH`, with a prefix at `out/wine-test-prefix`. Use `--skip-build` to
republish existing outputs. The script's header lists the toolchain lookup
order and remaining options.

`scripts/build-and-run-unix.sh` accepts the same options, then starts the
published launcher from its package directory.

To stage the release layout manually:

```bash
cargo build --release --locked -p bahamut-launcher-shell
./scripts/stage-unix-release.sh \
  --launcher target/release/bahamut-launcher-shell \
  --client-build out/client-mingw \
  --destination <empty dir>
```

Run `bahamut-launcher` from the staged folder. On Linux and macOS, this tree
includes the loader, `bahamut.dll`, plugins, and addons maintained in the
repository, so Play uses the extension launch. The bare Cargo shell includes
none of them. Without a package marker, the staged tree uses the
[portable layout](configuration.md#portable-launcher-tree).

## Linux archive

Use [`package-linux-tarball.sh`](../scripts/package-linux-tarball.sh) to create
the release tar.gz from a launcher binary and client build. It stages through
`stage-unix-release.sh`, adds the package marker, install scripts, desktop
entry, and icons, checks the exact file manifest and modes, then writes the
tar.gz with GNU tar. Its `README.md` comes from
[`packaging/linux/README.md`](../packaging/linux/README.md), replacing the
stage script's copy of `docs/getting-started.md`:

```bash
cargo build --release --locked -p bahamut-launcher-shell
./scripts/package-linux-tarball.sh \
  --launcher target/release/bahamut-launcher-shell \
  --client-build out/client-mingw \
  --label bahamut-launcher-dev-linux-x86_64 \
  --output out/linux-dist
```

Output is `<output>/<label>.tar.gz` with a `.sha256` sidecar. File timestamps
come from `SOURCE_DATE_EPOCH`, or the last commit if unset. The script defines
the exact archive manifest.

[`test-linux-package.py`](../scripts/test-linux-package.py) packages a
placeholder executable and synthetic client artifacts, installs into a
temporary prefix and `DESTDIR` staging root, then uninstalls:

```bash
python3 scripts/test-linux-package.py
```

Both scripts run directly on a Linux host without a container. The test needs
no Cargo build and skips other platforms. It validates package contents and
install scripts, not the launcher binary or game launch.

[`install.sh`](../packaging/linux/install.sh) accepts `--prefix`, `--pkgdir`,
and `--destdir`, plus environment variables `PREFIX` and `DESTDIR`. Only
`--pkgdir` selects the payload directory; an exported `PKGDIR`, such as
Portage's, cannot redirect it.

The package's [`Makefile`](../packaging/linux/Makefile) runs `./install.sh`
from its own directory. Use `make -C <extracted dir> install`. For example,
`make -C bahamut-launcher DESTDIR=<root> PREFIX=/usr install` stages a package
root; a command-line `PKGDIR=<dir>` selects the payload directory.

## Staged macOS app

Build and package a universal launcher using the release workflow's steps in
[`release-binaries.yml`](../.github/workflows/release-binaries.yml):

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

Without `--sign`, packaging signs the app ad hoc, as a release build does
without signing secrets. For local development,
`./scripts/build-unix-package.sh` still publishes the bare portable tree
without an app bundle.

## Additional launcher logs

Enable debug detail in a staged Windows package from the repository root:

```powershell
$env:RUST_LOG = "bahamut_launcher=debug,wry=warn"
.\out\dev\bahamut-launcher.exe
```

Use the complete staged package. The unpackaged Cargo shell is not equivalent.
Before sharing logs, review them for private paths and account information.

## Exact-binary checks

Manually dispatch [`Retail Checks`](../.github/workflows/retail-checks.yml)
to verify the manifest-pinned `ffxivgame.exe` identity, original bytes at
supported patch sites, and bytes produced by applying the patches. This
validates the exact binary but does not establish live client behavior.

[`manifests/retail-inputs.json`](../manifests/retail-inputs.json) declares the
input identity. The binary itself is not in the repository.

## Generated icon assets

After replacing `src-tauri/icons/icon-source.png`, regenerate the application
icons:

```powershell
cargo run --release --manifest-path tools/icon-gen/Cargo.toml
```

This creates the PNG, six-size Windows ICO, macOS ICNS with transparent
icon-grid padding, and Linux hicolor icons at
`packaging/linux/icons/hicolor/{48x48,128x128,256x256}/apps/bahamut-launcher.png`.
The macOS bundle uses `src-tauri/icons/icon.icns` at
`Contents/Resources/icon.icns`. The Linux tar.gz ships hicolor icons under
`share/icons/hicolor/`.

## What the checks cover

Tests and implementation describe the launcher's current behavior. They do
not prove retail client or server behavior, or compatibility across every
Wine and operating system combination. Keep compatibility, protocol, launch
patch, and platform claims limited to the versions and environments named
by their sources.
