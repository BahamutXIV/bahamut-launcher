# Release process

[Back to the documentation index](README.md)

The [Release Binaries workflow](../.github/workflows/release-binaries.yml)
builds Windows, Linux, and macOS x86_64 archives after the full checks pass.
Windows uses MSVC for the x86 client module. Linux and macOS use the
llvm-mingw release pinned in the workflow.
Cargo and Tauri package metadata must match the release being prepared. Release
identity is taken from the selected tag when a tag starts the build and from
`git describe` for ordinary local builds.

Player releases use the default Windows helper/runtime path. Do not include the
build only `diagnostic-no-injection` feature. It selects a separate direct launch
transaction for controlled troubleshooting, and its test client does not prove
retail no-injection compatibility.

## Tag releases

Push a tag matching:

```text
vMAJOR.MINOR.PATCH
vMAJOR.MINOR.PATCH-PRERELEASE
```

The prerelease suffix may contain letters, digits, dots, and hyphens. The
workflow runs the full checks, builds all three platforms, verifies each
archive and SHA-256 sidecar, and publishes one GitHub Release. Hyphenated tags
are marked as prereleases. The tag must resolve to the exact commit being
built, including when the tag is annotated.

The published asset set contains exactly these six files, where `REVISION` is
the checked-out commit SHA:

```text
bahamut-launcher-REVISION-windows-x86_64.zip
bahamut-launcher-REVISION-windows-x86_64.zip.sha256
bahamut-launcher-REVISION-linux-x86_64.tar.gz
bahamut-launcher-REVISION-linux-x86_64.tar.gz.sha256
bahamut-launcher-REVISION-macos-x86_64.tar.gz
bahamut-launcher-REVISION-macos-x86_64.tar.gz.sha256
```

GitHub Release publication does not authorize an in-app stable update. The
owner separately signs [release metadata](release-metadata.md)
locally. CI has no private signing key. A stable updater offer requires that
signature, the expected product and target, and valid release state. Prerelease
tags retain the GitHub publication behavior above, but
cannot become stable updater offers.

## Manual artifact builds

Use Actions -> Release Binaries -> Run workflow to build artifacts without
publishing a GitHub Release. The workflow uploads `dist-Windows`, `dist-Linux`,
and `dist-macOS`. It builds all three archives but skips the tag-only publish
job and aggregate six-file checksum validation.

## Archive contents

| Archive | Contents and runtime limit |
|---|---|
| Windows x86_64 ZIP | Win32 loader, native runtime, Screenshot and DiscordRPC plugins and addons maintained in this repository, the empty official DAT overlay, and the update helper |
| Linux x86_64 tar.gz | Portable launcher executable plus the Windows ZIP's payload without the update helper, with the Win32 loader, native runtime, and plugins built by llvm-mingw and the MinGW-w64 runtime notice added. System Wine and Linux Tauri libraries are required. |
| macOS x86_64 tar.gz | Portable Intel executable plus the same payload as the Linux archive, without an app bundle. Managed Sikarugir Wine is downloaded on first game launch. |

Every archive carries `README.md`, `LICENSE.md`, and the MinHook, Dear ImGui,
Lua, Miniz, and bundled font notices under `licenses/`. Linux and macOS
archives add the MinGW-w64 runtime notice. The archive README is sourced from
`docs/getting-started.md`. The workflow publishes a SHA-256 sidecar alongside
each archive. [Platform support](extensions.md#platform-support)
defines which platforms load the client module and records its status.
The Windows launcher downloads pinned prerequisite installers when their
runtimes are missing. It is distributed as a portable archive without a
platform installer. Releases do
not include code signing or macOS notarization.

## Portable updates

Windows checks signed stable launcher metadata quietly at startup. Settings >
Misc > Install Location offers **Check for Updates** for a manual check. When a
new release is available, the same button becomes **Update Launcher**. That
action downloads the verified package and applies it after gameplay, install,
patch, repair, restore, launch, and backup operations are idle. The separate
helper process is included in the signed managed inventory and runs from staging
on the same volume outside the portable directory. It applies managed files after
normal launcher exit, keeps recoverable prior state, and restarts the updated
launcher. Startup confirms a complete signed inventory or restores the previous
release after a failed update. Offline checks or downloads leave the installed
launcher available.

The portable archive does not contain production update endpoints, trust keys,
or a signed bootstrap release. The owner must provision the explicit
`config/launcher-updates.json`, trusted public key, and signed bootstrap files
for the exact starting package outside the release archive. Production provisioning
is controlled by the operator.
Private signing material is not stored in this repository or CI.

The official DAT overlay is part of the signed launcher inventory. Launcher
updates replace its managed files and preserve custom overlay packages.

Launcher updates and the signed managed inventory cover the Windows package
only. Files in a Linux or macOS archive change only when a newer archive is
extracted.

The Windows package is staged through the archive manifest check, which also
rejects unexpected package files. Linux and macOS packages are staged by
[`stage-unix-release.sh`](../scripts/stage-unix-release.sh), which applies the
Windows exact file and directory manifest without `bahamut-update-helper.exe`,
with the launcher named `bahamut-launcher`, plus
`licenses/MinGW-w64-runtime-COPYING.txt`, and rejects symbolic links. The
Windows launcher downloads both Microsoft prerequisite installers from pinned
R2 URLs and checks exact length and SHA-256 before running them. The
synthetic fixture can inspect package contents without a full build. It does
not prove installer execution or game launch compatibility.

Game archives and patch objects have a separate
[game content delivery reference](content-delivery.md). A release with
installation enabled needs pinned base metadata and live HTTPS range, full
download, and fresh installation checks for its production host.
