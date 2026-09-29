# Release process

[Back to the documentation index](README.md)

Merging a pull request into `main` releases the launcher. The
[Release workflow](../.github/workflows/release.yml) runs on every push to
`main`. It reads the highest existing `vMAJOR.MINOR.PATCH` tag (no tags means
a `v0.0.0` baseline, so an unlabeled first merge releases `v0.0.1`), bumps the patch
component by default, rewrites `[workspace.package].version` in `Cargo.toml`
and `version` in `src-tauri/tauri.conf.json` to match, syncs `Cargo.lock`,
commits the result to `main` as `chore: release vX.Y.Z` authored by
`github-actions[bot]`, and pushes that commit and an annotated tag atomically
using the `RELEASE_PAT` secret. It then creates the GitHub Release titled
`Bahamut Launcher vX.Y.Z` with generated notes. The tag push starts the
[Release Binaries workflow](../.github/workflows/release-binaries.yml), which
builds and publishes the platform archives.

Windows uses MSVC for the x86 client module. Linux and macOS use the
llvm-mingw release pinned in the workflow.

Player releases use the default Windows helper/runtime path. Do not include the
build only `diagnostic-no-injection` feature. It selects a separate direct launch
transaction for controlled troubleshooting, and its test client does not prove
retail no-injection compatibility.

## Branching

Pull requests target `develop`. Every push and pull request against `develop`
or `main` runs the [Checks workflow](../.github/workflows/ci.yml). A release
is a pull request from `develop` into `main`. Nothing merged into `develop`
produces a tag or a release.

Only the release bump commit edits the version fields in `Cargo.toml`,
`Cargo.lock`, and `src-tauri/tauri.conf.json`, so `develop` may carry an older
version than `main` between releases without a merge conflict. Merge `main`
back into `develop` after a release to keep it current.

## Bump levels

| Level | How it is selected |
|---|---|
| Patch | Default, when the merged pull request carries neither label below. |
| Minor | Add the `release:minor` label to the pull request before merging. |
| Major | Add the `release:major` label to the pull request before merging. Wins when both labels are present. |

Merges that land on `main` while a release run is still working are released
together by the next run, which reads only its own pull request's label. The
displaced run fails when its own label was not patch, so the missed bump is
visible.

Actions -> Release -> Run workflow, started from `main`, is the other way to
choose the level. It releases the current `main` tip at the chosen level, even
when that tip already carries a tag, so it promotes an already released commit
to a new version with its package versions rewritten. It needs no new commit
on `develop`.

Pushing a tag directly is the last alternative to a labeled merge:

```text
git tag -a v1.0.0 -m v1.0.0
git push origin v1.0.0
```

The [Release Binaries workflow](../.github/workflows/release-binaries.yml)
builds and publishes that version, creating the GitHub Release when it does
not already exist. The next merge to `main` continues from the highest
existing tag, so a manual `v1.0.0` is followed by `v1.0.1`, and that release's
bump commit re-syncs the package versions. A manually tagged commit keeps the
package versions of the commit it points at, so its Windows executable reports
the earlier version in its file properties until the next bump commit. To
reach a new major or minor version with matching package versions, label the
`develop` -> `main` pull request instead: while the highest stable tag is
below `v1.0.0`, or no tag exists, a `release:major` label on that merge
produces `v1.0.0`. The tag is the source of truth: a hand-edited package
version above the highest tag is rewritten back down by the next release.

## Version identity

The runtime version the launcher reports is `BAHAMUT_RELEASE_TAG` on a tag
build, such as `v1.0.0`. An ordinary branch build reports the latest reachable
tag, a hyphen, and the short commit hash, such as `v1.0.0-222f317`, with a
`-dirty` suffix when tracked files have uncommitted changes; see
[`build.rs`](../build.rs). The release bump commit keeps the Cargo and Tauri
package versions in lockstep, but that package version is metadata, not the
runtime identity. The [signed release metadata](release-metadata.md) version
is a separate value, ordered independently per product and target.

`--version` (or `-V`) prints that runtime version and `--help` (or `-h`)
prints the usage, on every platform, before the launcher opens a window,
writes logs, or creates state. The Windows release build uses the windows
subsystem and prints only to redirected stdout, for example
`bahamut-launcher.exe --version > version.txt`.

The macOS app's `CFBundleShortVersionString` and `CFBundleVersion` come from
`src-tauri/tauri.conf.json`, which this bump commit rewrites only on a push to
`main`. Any manually pushed tag or a branch dispatch build therefore shows the
version of the last bump commit in Finder, while the launcher's own version
display still comes from the values above.

## One-time setup

1. `RELEASE_PAT` repository secret: a fine-grained personal access token
   owned by a repository admin, with repository access limited to this
   repository and permission Contents: Read and write. A classic token with
   the `repo` scope works when the organization does not allow fine-grained
   tokens. When the secret is missing, the run fails before changing anything;
   add the secret and re-run the failed run.
2. The `release:minor` and `release:major` labels must exist on the
   repository.
3. Branch protection on `main` requires the `Repository checks`,
   `Checks (Linux)`, `Checks (Windows)`, and `Checks (macOS)` status checks
   and a pull request. Keep "Do not allow bypassing the above settings" off,
   so the admin-owned token can push the bump commit. Turning it on rejects
   that push and fails the run before tagging.
4. `DISCORD_RELEASE_WEBHOOK_URL` secret is optional. When set, a tag push
   announces the release on Discord. When absent, the announcement step logs
   a warning and the release still succeeds.

The `RELEASE_PAT` token can push to `main` past the required checks. Give it
a short expiry and rotate it.

## Loop prevention

The bump commit is pushed with `RELEASE_PAT`, so it re-triggers the Release
workflow. The job skips commits authored by `github-actions[bot]` to break
the loop. The bump commit is not marked `[skip ci]`, because that would also
suppress the tag push run that builds the platform binaries.

## Tag releases

Tags match:

```text
vMAJOR.MINOR.PATCH
vMAJOR.MINOR.PATCH-PRERELEASE
```

The prerelease suffix may contain letters, digits, dots, and hyphens.
Hyphenated tags are marked as prereleases.

The Release Binaries workflow resolves the tag to a build ref, runs the full
checks, builds all three platforms, verifies each archive and its SHA-256
sidecar, verifies that the remote tag resolves to the commit every platform
actually built (including when the tag is annotated), and attaches the assets
to the tag's GitHub Release. Attaching replaces same-named assets, so reruns
are idempotent.

The published asset set contains exactly these six files:

```text
bahamut-launcher-vX.Y.Z-windows-x86_64.zip
bahamut-launcher-vX.Y.Z-windows-x86_64.zip.sha256
bahamut-launcher-vX.Y.Z-linux-x86_64.tar.gz
bahamut-launcher-vX.Y.Z-linux-x86_64.tar.gz.sha256
bahamut-launcher-vX.Y.Z-macos-universal.zip
bahamut-launcher-vX.Y.Z-macos-universal.zip.sha256
```

GitHub Release publication does not authorize an in-app stable update. The
owner separately signs [release metadata](release-metadata.md)
locally. CI has no private signing key. A stable updater offer requires that
signature, the expected product and target, and valid release state.
Prerelease tags retain the GitHub publication behavior above, but cannot
become stable updater offers.

## Manual artifact builds

Use Actions -> Release Binaries -> Run workflow to run the workflow outside a
tag push.

Leave the `tag` input empty to build the currently selected branch. The
archives are labeled `dev-<commit>`, using the first seven characters of the
built commit's SHA. The workflow uploads `dist-Windows`, `dist-Linux`, and
`dist-macOS` artifacts and skips the publish job, so no GitHub Release is
created or updated.

Enter an existing `vMAJOR.MINOR.PATCH[-PRERELEASE]` tag to rebuild and
re-attach that release's assets. Re-attaching replaces same-named files, the
same as a tag push rerun, but does not send a Discord announcement.
Rebuilding an existing tag requires a tag whose tree contains
`scripts/package-macos-app.sh`; older tags fail in the macOS build leg.

## Archive contents

| Archive | Contents and runtime limit |
|---|---|
| Windows x86_64 ZIP | Win32 loader, native runtime, Screenshot and DiscordRPC plugins and addons maintained in this repository, the empty official DAT overlay, and the update helper |
| Linux x86_64 tar.gz | One top-level `bahamut-launcher/` folder holding the launcher executable and the Windows ZIP's payload without the update helper, with the Win32 loader, native runtime, and plugins built by llvm-mingw and the MinGW-w64 runtime notice added. It adds the `.bahamut-launcher-package` marker, `install.sh`, `install-dependencies.sh`, a `Makefile`, the desktop entry under `share/applications/`, and the hicolor icons under `share/icons/`. Built on `ubuntu-22.04`, so it needs glibc 2.35 or newer. WebKitGTK 4.1 and GTK 3 are required; the first game launch downloads the Wine engine described in [Linux Wine engine](configuration.md#linux-wine-engine). |
| macOS universal ZIP | One item, `Bahamut Launcher.app`, a universal app (Apple Silicon and Intel). The launcher sits at `Contents/MacOS/bahamut-launcher`; the Windows ZIP's payload minus the update helper sits under `Contents/Resources` with the MinGW-w64 runtime notice added, alongside `icon.icns`. `Info.plist` sits at `Contents/Info.plist`. Managed Sikarugir Wine is downloaded on first game launch. Apple Silicon needs Rosetta 2 to run the Wine engine. |

Every archive carries `README.md`, `LICENSE.md`, and the MinHook, Dear ImGui,
Lua, Miniz, and bundled font notices under `licenses/` (under
`Contents/Resources/licenses/` inside the macOS app). Linux and macOS
archives add the MinGW-w64 runtime notice. The archive README is sourced from
`docs/getting-started.md`. The workflow publishes a SHA-256 sidecar alongside
each archive. [Platform support](extensions.md#platform-support)
defines which platforms load the client module and records its status.
The Windows launcher downloads pinned prerequisite installers when their
runtimes are missing. It is distributed as a portable archive without a
platform installer.

When the repository secrets `MACOS_CERT_P12_BASE64`, `MACOS_CERT_PWD`,
`ASC_API_KEY_P8_BASE64`, `ASC_KEY_ID`, and `ASC_ISSUER_ID` are set, the macOS
app is Developer ID signed with the hardened runtime and the entitlements in
[`packaging/macos/entitlements.plist`](../packaging/macos/entitlements.plist),
notarized, and stapled. Signing follows the secrets, not the tag, so a manual
Release Binaries run with an empty tag also produces a signed dev build.
Without both `MACOS_CERT_P12_BASE64` and `ASC_API_KEY_P8_BASE64` the app is ad
hoc signed instead, and the publish job appends a note to the release body. On
macOS 15 and later, the user allows an ad hoc signed app once under
System Settings > Privacy & Security > Open Anyway after the first blocked
launch; Control-click Open no longer bypasses Gatekeeper.

### Linux gates

The [Checks workflow](../.github/workflows/ci.yml) gates the Linux package on
every push and pull request against `develop` or `main`. The
`Repository checks` job runs `shellcheck` on the Linux install scripts,
`package-linux-tarball.sh`, and `stage-unix-release.sh`, runs
`desktop-file-validate` on
`packaging/linux/bahamut-launcher.desktop`, and runs
[`test-linux-package.py`](../scripts/test-linux-package.py). The
`Checks (Linux)` job packages the archive from its placeholder executable,
runs `install.sh` into a temporary prefix, runs
`make DESTDIR=... PREFIX=/usr install`, and uninstalls with the installed
`install.sh --uninstall`.

The Linux leg of the Release Binaries workflow builds on `ubuntu-22.04`,
packages the real launcher, extracts the archive, and runs
`bahamut-launcher/bahamut-launcher --version`. When the build carries
`BAHAMUT_RELEASE_TAG`, the printed version must equal it, or the leg fails.
This proves the packaged binary loads on the build host; it does not prove
game launch.

## Portable updates

The Windows package includes the update helper in its signed managed
inventory. The helper runs from staging on the same volume outside the
portable directory, applies managed files after normal launcher exit, keeps
recoverable prior state, and restarts the updated launcher. Startup confirms a
complete signed inventory or restores the previous release after a failed
update. A newer release is installed from its archive.

The portable archive does not contain production update endpoints, trust keys,
or a signed bootstrap release. The owner must provision the explicit
`config/launcher-updates.json`, trusted public key, and signed bootstrap files
for the exact starting package outside the release archive. Production provisioning
is controlled by the operator.
Private signing material is not stored in this repository or CI.

The official DAT overlay is part of the signed launcher inventory. Launcher
updates replace its managed files and preserve custom overlay packages.

Launcher updates and the signed managed inventory cover the Windows package
only. The Linux package's files, in the extracted folder or an installed copy,
and the files inside the macOS app bundle change only when a newer archive is
extracted or installed. The launcher never writes into either and keeps its
writable state in `~/.bahamut-launcher` instead. See
[Getting started](getting-started.md#portable-archives) for where each
platform keeps that state. Every Linux archive extracts to the same
`bahamut-launcher/` folder, so an extracted copy is updated by deleting or
renaming the old folder before extracting, or by rerunning `./install.sh`
from the new archive; see
[Linux first launch](getting-started.md#linux-first-launch).

The Windows package is staged through the archive manifest check, which also
rejects unexpected package files. Linux and macOS packages are staged by
[`stage-unix-release.sh`](../scripts/stage-unix-release.sh), which applies the
Windows exact file and directory manifest without `bahamut-update-helper.exe`,
with the launcher named `bahamut-launcher`, plus
`licenses/MinGW-w64-runtime-COPYING.txt`, and rejects symbolic links. For
macOS, [`package-macos-app.sh`](../scripts/package-macos-app.sh) wraps that
staged tree into `Bahamut Launcher.app`: the launcher becomes
`Contents/MacOS/bahamut-launcher`, every other staged file keeps its relative
path under `Contents/Resources`, and the empty writable skeleton directories
are dropped, since the app keeps its writable state outside the bundle. For
Linux, [`package-linux-tarball.sh`](../scripts/package-linux-tarball.sh)
stages the same tree into a private directory, drops the empty skeleton
directories, adds the package marker, install scripts, desktop entry, and
icons from [`packaging/linux/`](../packaging/linux/), asserts the exact file
manifest and file modes, and writes the archive and its SHA-256 sidecar. The
Windows launcher downloads both Microsoft prerequisite installers from pinned
R2 URLs and checks exact length and SHA-256 before running them. The
synthetic fixture can inspect package contents without a full build. It does
not prove installer execution or game launch compatibility.

Game archives have a separate
[game content delivery reference](content-delivery.md). A release with
installation enabled needs pinned base metadata and live HTTPS range, full
download, and fresh installation checks for its production host.
