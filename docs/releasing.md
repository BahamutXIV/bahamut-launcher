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

Pushing a tag directly is the manual alternative to a labeled merge:

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

The runtime version the launcher reports comes from `BAHAMUT_RELEASE_TAG` on a
tag build and from `git describe` on an ordinary branch build; see
[`build.rs`](../build.rs). The release bump commit keeps the Cargo and Tauri
package versions in lockstep, but that package version is metadata, not the
runtime identity. The [signed release metadata](release-metadata.md) version
is a separate value, ordered independently per product and target.

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
bahamut-launcher-vX.Y.Z-macos-universal.tar.gz
bahamut-launcher-vX.Y.Z-macos-universal.tar.gz.sha256
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

## Archive contents

| Archive | Contents and runtime limit |
|---|---|
| Windows x86_64 ZIP | Win32 loader, native runtime, Screenshot and DiscordRPC plugins and addons maintained in this repository, the empty official DAT overlay, and the update helper |
| Linux x86_64 tar.gz | Portable launcher executable plus the Windows ZIP's payload without the update helper, with the Win32 loader, native runtime, and plugins built by llvm-mingw and the MinGW-w64 runtime notice added. System Wine and Linux Tauri libraries are required. |
| macOS universal tar.gz | Portable universal executable (Apple Silicon and Intel in one file, signed ad hoc) plus the same payload as the Linux archive, without an app bundle. Managed Sikarugir Wine is downloaded on first game launch. Apple Silicon needs Rosetta 2 to run the Wine engine. |

Every archive carries `README.md`, `LICENSE.md`, and the MinHook, Dear ImGui,
Lua, Miniz, and bundled font notices under `licenses/`. Linux and macOS
archives add the MinGW-w64 runtime notice. The archive README is sourced from
`docs/getting-started.md`. The workflow publishes a SHA-256 sidecar alongside
each archive. [Platform support](extensions.md#platform-support)
defines which platforms load the client module and records its status.
The Windows launcher downloads pinned prerequisite installers when their
runtimes are missing. It is distributed as a portable archive without a
platform installer. Releases do not include Developer ID code signing or
macOS notarization.

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
