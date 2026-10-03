# Signed release metadata

[Back to the documentation index](README.md)

Sign release metadata with Ed25519 before offering a stable update. The
verifier accepts it only against an explicitly trusted key. Metadata cannot
add keys or authorize its own key.

The signature covers the original file bytes. Each artifact has a declared
length and SHA-256 identity. Inventory entries are signed inline, or, for game
data, pinned by the delivery manifest's hash. A digest downloaded beside
untrusted content does not establish who published it.

Stable versions use strict `MAJOR.MINOR.PATCH`, ordered independently for
game and launcher releases on each target. They are separate from retail
`game.ver`, the Cargo package version, the Git build description, and GitHub
Release tags. A published prerelease can never be a stable updater offer.

## Version 1 wire format

Metadata is UTF-8 JSON with a detached signature of exactly 64 raw Ed25519
bytes. Sign the file's exact bytes; reformatting the JSON invalidates the
signature. The verifier reads exactly 32 raw public key bytes from a separately
selected file and rejects unsupported fields and schema versions.

Size limits are 1 MiB for metadata, 64 MiB for the game delivery manifest, and
2 MiB for accepted state.

| Field | Value |
|---|---|
| `schema_version` | `1` |
| `product` | `game` or `launcher` |
| `channel` | `stable` |
| `target` | `platform-independent` for game data; `windows-x86_64`, `linux-x86_64`, or `macos-universal` for a launcher package |
| `version` | Strict `MAJOR.MINOR.PATCH`, with no prerelease or build suffix |
| `artifact` | `object_key`, `format` (`zip` or `tar_gz`), byte `length`, and lowercase hex `sha256` |
| `inventory` | The product's identity or file entries, described below |

Launcher artifact keys use
`launcher/<version>/<target>/<sha256>.zip` on Windows and macOS, and
`launcher/<version>/<target>/<sha256>.tar.gz` on Linux. Game artifact keys must
match an archive object declared in the supplied delivery manifest. These
keys identify release objects, not local install paths.

For game data, inventory contains `kind: "game_delivery"`, `manifest_sha256`,
and `target_version`. The verifier checks the exact supplied delivery manifest,
its final-file catalog, and the selected declared archive object. The release
version orders updater offers; `target_version` retains its existing meaning
as the retail client version.

Launcher inventory contains `kind: "launcher"` and a `files` array. Each
entry supplies a relative `path`, byte `length`, lowercase hex `sha256`, and
`ownership` of `managed` or `seed`. It lists components maintained by the
launcher, including the Windows launcher helper and bundled files under
`plugins/dats/bahamut-dats-overlay/`. On Windows, `scripts/default.txt` must
be a seed. The exact official package manifest may also be a seed for
compatibility with older launcher receipts. The verifier rejects unsafe paths,
case aliases, file/directory collisions, and protected locations.

The macOS ZIP contains one top-level `Bahamut Launcher.app` directory. Every
inventory entry is managed, with a path beginning
`Bahamut Launcher.app/Contents/`. The inventory includes:

- `Info.plist`, `MacOS/bahamut-launcher`, `_CodeSignature/CodeResources`,
  `CodeResources` on a stapled build, and `Resources/icon.icns`.
- Under `Resources/`: the loader, DLL, two plugins, shipped addons, official
  overlay package, `scripts/default.txt`, `README.md`, and license notices.

Keep signed bundles sealed. Configuration, logs, screenshots, backups, and the
editable `scripts/default.txt` live outside the bundle in the state root:
`$BAHAMUT_LAUNCHER_HOME`, or `~/.bahamut-launcher` by default. See
[macOS app layout](configuration.md#macos-app-layout). A signed inventory
cannot own those writable paths.

The ZIP may contain directory entries that are ancestors of inventory files.
The verifier rejects all other directories, AppleDouble `._` entries,
`__MACOSX/` entries, symlinks, and paths outside the bundle.

Only Windows package replacement currently uses signed launcher metadata; see
[Portable updates](releasing.md#portable-updates). The published Linux tar.gz
does not meet the Linux launcher inventory rules in
[`release.rs`](../src/release.rs), so it has no signed Linux release. Those
rules accept only root-level `bahamut-launcher`, `LICENSE.md`, `README.md`,
and notices under `licenses/`, excluding the MinGW-w64 runtime notice.
The published tar.gz instead nests files under `bahamut-launcher/`; that
directory entry collides with the `bahamut-launcher` inventory path.

## Offline publisher and verifier

Prepare and review the metadata JSON, artifact, and inventory before signing.
The publisher hashes the named local artifact and checks its declared length
and digest. For game releases, also supply the delivery manifest so it can
check the manifest's exact identity and authoritative final-file list.

Signing reads a raw 32-byte Ed25519 private seed from an external file or
standard input. Never put secret bytes in command arguments, logs, metadata,
or source control. Replace these example paths with your own:

```powershell
cargo run --bin release-metadata -- sign --metadata C:/Release/metadata.json --signature-out C:/Release/metadata.sig --key-file C:/Private/signing-seed.bin --artifact C:/Release/package.zip
cargo run --bin release-metadata -- verify --metadata C:/Release/metadata.json --signature C:/Release/metadata.sig --public-key C:/Trust/publisher.pub --product launcher --channel stable --target windows-x86_64 --artifact C:/Release/package.zip
```

For product `game`, add `--delivery-manifest C:/Release/game-delivery.json`.
The verifier checks the signature and exact local artifact before considering
the offer. Choose the trusted public key separately; do not fetch it from
the release host. The owner must create and distribute production keys through
a separate deployment process.

After verifying an explicitly trusted starting release, use these commands:

- `trust`: create the state file for that scope.
- `accept`: advance the product and target's remote acceptance boundary.
- `record-installed`: record a previously accepted local package for rollback
  without lowering the remote boundary.

Each takes the same verification arguments plus `--state FILE`. Bootstrap also
requires `--starting-version MAJOR.MINOR.PATCH` to match the verified starting
release. Select a trusted local path for the state file. Metadata and remote
catalogs cannot choose it.

## Managed ownership

The `base.final_files` catalog in
[`game-delivery.json`](../manifests/game-delivery.json) remains the authoritative
game file list. A signed game descriptor pins that catalog's identity rather
than supplying another list. The launcher inventory also owns its explicitly
listed official overlay files.

Every inventory path is relative to the managed root selected by the caller.
Metadata cannot choose an arbitrary local directory, move files into another
product's root, or gain replacement rights merely by including files in an
archive.

Managed entries can replace only their explicitly owned paths. Seed entries
can be installed if absent but never authorize replacement. On Windows,
initial installation seeds `scripts/default.txt` and preserves an existing
copy. The macOS bundle's copy is managed and read-only inside the sealed app.

A signed launcher inventory can replace only listed files in the official
`plugins/dats/bahamut-dats-overlay` package, under `Contents/Resources/` on
macOS. Custom overlays, configuration, authentication and profile state, logs,
and screenshots remain outside its replacement authority.

## Ordering and recovery

Start from an explicitly trusted version. Across restarts, the verifier retains
the highest accepted remote version and its metadata identity. It rejects older
remote offers and changed metadata for an already accepted version. Rechecking
identical accepted metadata is safe.

A later replacement transaction can restore a previously trusted local package
without lowering the remote acceptance boundary. There is no remote downgrade
switch.

A missing state file can be bootstrapped only from the explicitly trusted
starting version. A malformed existing state fails closed. The owner must
control recovery from lost trusted state or signing keys; downloaded metadata
cannot repair trust.

State changes hold a system lock on a persistent adjacent `.lock` file from
the read through atomic publication. Keep both files in a trusted local
directory, and retain the lock file between invocations. If publication fails,
the previous valid state file remains in place.

This contract defines verification and inventory authority. It does not fetch
release catalogs, publish artifacts, replace installed files, or provide an
updater UI. Production key custody and provisioning, hosted platform validation,
and live delivery still need separate release checks.
