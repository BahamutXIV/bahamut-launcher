# Release process

[Back to the documentation index](README.md)

To release the launcher, merge a pull request from `develop` into `main`.
The [Release workflow](../.github/workflows/release.yml) runs on every push
to `main` and:

1. Reads the highest existing `vMAJOR.MINOR.PATCH` tag and bumps the patch
   version unless a label requests another level. With no tags, it starts
   from `v0.0.0`, so the first unlabeled merge releases `v0.0.1`.
2. Updates `[workspace.package].version` in `Cargo.toml` and `version` in
   `src-tauri/tauri.conf.json`, then syncs `Cargo.lock`.
3. Creates `chore: release vX.Y.Z` on `main`, authored by
   `github-actions[bot]`, and atomically pushes the commit and annotated tag
   using `RELEASE_PAT`.
4. Creates a GitHub Release titled `Bahamut Launcher vX.Y.Z` with generated
   notes.

The tag push starts [Release Binaries](../.github/workflows/release-binaries.yml),
which builds and publishes the platform downloads. Windows uses MSVC for the
x86 client module. Linux and macOS use the workflow's pinned llvm-mingw release.

Use the default Windows helper/runtime path for player releases. Never include
the build-only `diagnostic-no-injection` feature: it selects a separate direct
launch transaction for controlled troubleshooting, and its test client does
not establish retail no-injection compatibility.

## Branching

Target `develop` for pull requests, then open a `develop` to `main` pull request
when ready to release. Pushes and pull requests against either branch run
[Checks](../.github/workflows/ci.yml). Merging into `develop` creates no tag or
release.

Only the release bump commit edits version fields in `Cargo.toml`,
`Cargo.lock`, and `src-tauri/tauri.conf.json`. This lets `develop` retain an
older version between releases without a merge conflict. Merge `main` back
into `develop` after each release.

## Bump levels

| Level | Selection |
|---|---|
| Patch | Default when the merged pull request has neither label below. |
| Minor | Add `release:minor` before merging. |
| Major | Add `release:major` before merging. Takes priority if both labels are present. |

If more merges reach `main` while a release is running, the next run releases
them together and reads only its own pull request's label. A displaced run
fails if its label requested more than a patch bump, making the missed bump
visible.

You can also choose a level through Actions -> Release -> Run workflow on
`main`. This releases the current tip at that level, even if it already has a
tag, and rewrites the package versions for the new version. No new commit on
`develop` is needed.

A direct tag push is another option:

```text
git tag -a v1.0.0 -m v1.0.0
git push origin v1.0.0
```

[Release Binaries](../.github/workflows/release-binaries.yml) builds and
publishes the tagged version, creating the GitHub Release if needed. The next
merge to `main` continues from the highest tag: a manual `v1.0.0` is followed
by `v1.0.1`, whose bump commit resyncs the package versions.

A manual tag leaves its commit's package versions unchanged. The Windows
executable therefore reports the earlier package version in file properties
until the next bump commit. To release a new major or minor version with
matching package versions, label the `develop` -> `main` pull request. For
example, `release:major` produces `v1.0.0` when the highest stable tag is
below `v1.0.0` or no tag exists.

The highest tag determines the next version. The release workflow overwrites
a hand-edited package version even if doing so lowers it.

## Version identity

Tag builds report `BAHAMUT_RELEASE_TAG`, such as `v1.0.0`, as their runtime
version. Branch builds report the latest reachable tag and short commit hash,
such as `v1.0.0-222f317`, with `-dirty` when tracked files have uncommitted
changes. See [`build.rs`](../build.rs).

The bump commit keeps Cargo and Tauri package versions synchronized. Those
versions are package metadata; the runtime identity follows the rules above.
The [signed release metadata](release-metadata.md) version is separate and
ordered independently per product and target.

On every platform, `--version` or `-V` prints the runtime version, and
`--help` or `-h` prints usage before any window opens, logs are written, or
state is created. The Windows release build uses the windows subsystem, so
it prints only to redirected stdout, for example:
`bahamut-launcher.exe --version > version.txt`.

The macOS app takes `CFBundleShortVersionString` and `CFBundleVersion` from
`src-tauri/tauri.conf.json`, which the bump commit rewrites only on a push to
`main`. Finder therefore shows the last bump commit's version for a manually
pushed tag or branch dispatch build. The launcher's own version display still
uses its runtime identity.

## One-time setup

1. Add the repository secret `RELEASE_PAT`. Use a fine-grained personal access
   token owned by a repository admin, limited to this repository, with
   Contents: Read and write. If the organization disallows fine-grained tokens,
   a classic token with `repo` scope works. A missing secret fails the run
   before any changes; add it and rerun the failed run.
2. Create the `release:minor` and `release:major` repository labels.
3. Protect `main` with a pull request requirement and these required checks:
   `Repository checks`, `Checks (Linux)`, `Checks (Windows)`, and
   `Checks (macOS)`. Leave "Do not allow bypassing the above settings" off
   so the admin-owned token can push the bump commit. Enabling it rejects
   that push and fails the run before tagging.
4. Optionally add `DISCORD_RELEASE_WEBHOOK_URL`. A tag push then announces
   the release on Discord. Without it, the announcement step warns but the
   release still succeeds.

`RELEASE_PAT` can bypass required checks to push to `main`. Use a short expiry
and rotate the token.

## Loop prevention

Pushing the bump commit with `RELEASE_PAT` triggers Release again. The job
skips commits authored by `github-actions[bot]` to stop the loop. Do not add
`[skip ci]` to the bump commit: it would also suppress the tag run that builds
the platform binaries.

## Tag releases

Accepted tags follow these forms:

```text
vMAJOR.MINOR.PATCH
vMAJOR.MINOR.PATCH-PRERELEASE
```

Prerelease suffixes can contain letters, digits, dots, and hyphens. Hyphenated
tags are marked as prereleases.

Release Binaries resolves the tag to a build ref, runs the full checks, and
builds all three platforms. It verifies each ZIP or tar.gz and SHA-256 sidecar,
then checks that the remote tag resolves to the commit every platform built,
including for annotated tags. It attaches the assets to the tag's GitHub
Release, replacing same-named files on reruns.

Each release publishes exactly six assets:

```text
bahamut-launcher-vX.Y.Z-windows-x86_64.zip
bahamut-launcher-vX.Y.Z-windows-x86_64.zip.sha256
bahamut-launcher-vX.Y.Z-linux-x86_64.tar.gz
bahamut-launcher-vX.Y.Z-linux-x86_64.tar.gz.sha256
bahamut-launcher-vX.Y.Z-macos-universal.zip
bahamut-launcher-vX.Y.Z-macos-universal.zip.sha256
```

Publishing to GitHub does not authorize an in-app stable update. The owner
must separately sign [release metadata](release-metadata.md) locally; CI has
no private signing key. A stable offer needs that signature, the expected
product and target, and valid release state. Prerelease tags use the same
GitHub publication process but cannot become stable offers.

## Manual artifact builds

Open Actions -> Release Binaries -> Run workflow to build without a tag push.

- Leave `tag` empty to build the selected branch. Downloads use the label
  `dev-<commit>`, with the first seven characters of the built commit's SHA.
  The workflow uploads `dist-Windows`, `dist-Linux`, and `dist-macOS`
  artifacts and skips publication. It creates or updates no GitHub Release.
- Enter an existing `vMAJOR.MINOR.PATCH[-PRERELEASE]` tag to rebuild and
  replace its same-named release assets. This does not send a Discord
  announcement.

The tagged tree must contain `scripts/package-macos-app.sh`. Rebuilding older
tags fails in the macOS build leg.

## Archive contents

| Download | Contents and requirements |
|---|---|
| Windows x86_64 ZIP | Win32 loader, native runtime, Screenshot and DiscordRPC plugins and addons maintained in this repository, the empty official DAT overlay, and update helper. |
| Linux x86_64 tar.gz | One top-level `bahamut-launcher/` folder containing the executable and the Windows payload without the update helper. The Win32 loader, runtime, and plugins use llvm-mingw, with the MinGW-w64 runtime notice added. Also includes `.bahamut-launcher-package`, `install.sh`, `install-dependencies.sh`, a `Makefile`, the desktop entry under `share/applications/`, and hicolor icons under `share/icons/`. Built on `ubuntu-22.04`; requires glibc 2.35 or newer, WebKitGTK 4.1, and GTK 3. First game launch downloads the [Linux Wine engine](configuration.md#linux-wine-engine). |
| macOS universal ZIP | One `Bahamut Launcher.app` for Apple Silicon and Intel. The launcher is at `Contents/MacOS/bahamut-launcher`. `Contents/Resources` holds the Windows payload without the update helper, plus the MinGW-w64 runtime notice and `icon.icns`. `Info.plist` is at `Contents/Info.plist`. First game launch downloads managed Sikarugir Wine; Apple Silicon requires Rosetta 2 to run it. |

Every download includes `README.md`, `LICENSE.md`, and notices for MinHook,
Dear ImGui, Lua, Miniz, and bundled fonts under `licenses/`. In the macOS
app, those notices are under `Contents/Resources/licenses/`. Linux and macOS
also include the MinGW-w64 runtime notice.

The Linux README comes from `packaging/linux/README.md`. Windows and macOS
use `docs/getting-started.md`. Each ZIP or tar.gz has a published SHA-256
sidecar. See [Platform support](extensions.md#platform-support) for client
module availability and status.

Windows is distributed as a portable ZIP without a platform installer. When
prerequisite runtimes are missing, the launcher downloads pinned installers.

Configure these repository secrets for a Developer ID signed, notarized, and
stapled macOS app: `MACOS_CERT_P12_BASE64`, `MACOS_CERT_PWD`,
`ASC_API_KEY_P8_BASE64`, `ASC_KEY_ID`, and `ASC_ISSUER_ID`. Signing uses the
hardened runtime and
[`packaging/macos/entitlements.plist`](../packaging/macos/entitlements.plist).
The secrets determine signing, regardless of the tag, so a manual run with an
empty tag also produces a signed development build.

Without both `MACOS_CERT_P12_BASE64` and `ASC_API_KEY_P8_BASE64`, the app is
ad hoc signed and publication appends a note to the release body. On macOS 15
and later, users must allow that app once through System Settings > Privacy &
Security > Open Anyway after a blocked launch. Control-click Open no longer
bypasses Gatekeeper.

### Linux gates

On every push and pull request against `develop` or `main`, the
[Checks workflow](../.github/workflows/ci.yml) validates the Linux package:

- `Repository checks` runs `shellcheck` on the Linux install scripts,
  `package-linux-tarball.sh`, and `stage-unix-release.sh`;
  `desktop-file-validate` on `packaging/linux/bahamut-launcher.desktop`;
  and [`test-linux-package.py`](../scripts/test-linux-package.py).
- `Checks (Linux)` packages a placeholder executable, installs it into a
  temporary prefix with `install.sh`, stages it with
  `make DESTDIR=... PREFIX=/usr install`, and removes it with the installed
  `install.sh --uninstall`.

Release Binaries builds the real Linux launcher on `ubuntu-22.04`, packages
and extracts it, then runs `bahamut-launcher/bahamut-launcher --version`.
If `BAHAMUT_RELEASE_TAG` is set, the printed version must match or the build
leg fails. This checks that the packaged binary loads on the build host;
it does not test game launch.

## Portable updates

The Windows signed managed inventory includes the update helper. After a normal
launcher exit, the helper runs from staging outside the portable directory on
the same volume. It applies managed files, retains recoverable prior state,
and restarts the launcher. Startup confirms the complete signed inventory or
restores the previous release after a failed update. New releases are installed
from their ZIPs.

Production update endpoints, trust keys, and a signed bootstrap release are
absent from the portable ZIP. The owner must provision
`config/launcher-updates.json`, the trusted public key, and signed bootstrap
files for the exact starting package separately. The operator controls this
provisioning. Private signing material belongs in neither this repository nor
CI.

The official DAT overlay is part of the signed launcher inventory. Updates
replace its managed files and preserve custom overlay packages.

Launcher updates and signed managed replacement currently cover Windows only.
Linux package files, whether extracted or installed, and macOS bundle files
change only when a newer release is extracted or installed. The launcher never
writes into either package and keeps writable state in `~/.bahamut-launcher`.
See [Getting started](getting-started.md#portable-archives) for state locations
on each platform.

Every Linux tar.gz extracts into `bahamut-launcher/`. Before extracting an
update, delete or rename the old folder, or rerun `./install.sh` from the new
release. See [Linux first launch](getting-started.md#linux-first-launch).

The Windows package passes through the archive manifest check, which rejects
unexpected files. Linux and macOS use
[`stage-unix-release.sh`](../scripts/stage-unix-release.sh). It applies the
Windows exact file and directory manifest with these changes: omit
`bahamut-update-helper.exe`, name the launcher `bahamut-launcher`, and add
`licenses/MinGW-w64-runtime-COPYING.txt`. It rejects symbolic links.

For macOS, [`package-macos-app.sh`](../scripts/package-macos-app.sh) wraps the
staged tree in `Bahamut Launcher.app`. The executable becomes
`Contents/MacOS/bahamut-launcher`; other files retain their relative paths
under `Contents/Resources`. Empty writable skeleton directories are dropped
because writable state lives outside the bundle.

For Linux, [`package-linux-tarball.sh`](../scripts/package-linux-tarball.sh)
stages the same tree in a private directory, drops empty skeleton directories,
and adds the package marker, install scripts, desktop entry, and icons from
[`packaging/linux/`](../packaging/linux/). It checks the exact file manifest
and modes, then writes the tar.gz and SHA-256 sidecar.

The Windows launcher downloads both Microsoft prerequisite installers from
pinned R2 URLs and checks their exact length and SHA-256 before running them.
The synthetic fixture checks package contents without a full build. It does
not establish installer execution or game launch compatibility.

For game downloads, see the separate
[game content delivery reference](content-delivery.md). A release that enables
installation needs pinned base metadata and live HTTPS range, full-download,
and fresh-installation checks against its production host.
