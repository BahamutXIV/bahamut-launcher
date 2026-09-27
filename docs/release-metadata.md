# Signed release metadata

[Back to the documentation index](README.md)

Signed release metadata authorizes a stable updater offer only after an Ed25519
signature verifies against an explicitly trusted key. Metadata cannot add trust
keys. The signature covers the original bytes, not a reserialized JSON value.
Artifacts carry a length and SHA-256 identity. Inventory entries are signed
inline, or the game catalog is pinned by its manifest hash. A digest beside
untrusted remote bytes does not establish publisher authority.

Stable versions use strict `MAJOR.MINOR.PATCH`. Game and launcher versions are
ordered independently per target. This version is separate from retail
`game.ver`, the Cargo package version, and the Git build description. GitHub Release tags
are separate too: a published prerelease is never a stable updater offer.

## Version 1 wire format

The metadata file is UTF-8 JSON. Its detached signature contains exactly 64 raw
Ed25519 bytes over the file's exact bytes. Reformatting JSON invalidates the
signature. Metadata is limited to 1 MiB, the game delivery manifest to 64 MiB,
and accepted state to 2 MiB. The verifier rejects unsupported fields and schema
versions and reads exactly 32 raw public key bytes from a separately selected
file. Metadata cannot authorize its own key.

The version 1 fields are:

| Field | Value |
|---|---|
| `schema_version` | `1` |
| `product` | `game` or `launcher` |
| `channel` | `stable` |
| `target` | `platform-independent` for game data, or `windows-x86_64`, `linux-x86_64`, or `macos-universal` for a launcher package |
| `version` | Strict `MAJOR.MINOR.PATCH`, without a prerelease or build suffix |
| `artifact` | `object_key`, `format` (`zip` or `tar_gz`), byte `length`, and lowercase hex `sha256` |
| `inventory` | The identity or file entries for the product below |

Launcher artifact keys have the form
`launcher/<version>/<target>/<sha256>.zip` on Windows and
`launcher/<version>/<target>/<sha256>.tar.gz` on Linux and macOS. Game
artifact keys must match an archive object in the supplied delivery manifest. These keys
identify release objects. They are not local installation paths.

The game inventory uses `kind: "game_delivery"`, `manifest_sha256`, and
`target_version`. The verifier checks the exact supplied delivery manifest,
its catalog of final files, and the selected declared archive object. The release
version orders updater offers. `target_version` identifies the retail client
and has its existing meaning.

Launcher inventories use `kind: "launcher"` and a `files` array. Each entry declares a relative `path`, byte
`length`, lowercase hex `sha256`, and `ownership` of `managed` or `seed`.
The launcher inventory names managed components maintained by the launcher. On
Windows, that includes the launcher helper and bundled files under
`plugins/dats/bahamut-dats-overlay/`. The Windows `scripts/default.txt` can only
be a seed. The exact official package manifest
may also be a seed for compatibility with older launcher receipts.
The verifier rejects unsafe paths, case aliases, file/directory collisions,
and protected locations.

## Offline publisher and verifier

Prepare and review the metadata JSON, artifact and inventory before signing.
The publisher hashes the named local artifact and checks its declared length
and digest. For a game release, pass the delivery manifest so
the publisher can check its exact identity and authoritative list of final files.
Signing reads a raw 32-byte Ed25519 private seed from an external file or
standard input. Secret bytes must not appear in command arguments, logs,
metadata or source control. These examples use paths chosen by the operator:

```powershell
cargo run --bin release-metadata -- sign --metadata C:/Release/metadata.json --signature-out C:/Release/metadata.sig --key-file C:/Private/signing-seed.bin --artifact C:/Release/package.zip
cargo run --bin release-metadata -- verify --metadata C:/Release/metadata.json --signature C:/Release/metadata.sig --public-key C:/Trust/publisher.pub --product launcher --channel stable --target windows-x86_64 --artifact C:/Release/package.zip
```

Add `--delivery-manifest C:/Release/game-delivery.json` when the product is
`game`. The verifier checks the signature and exact local artifact before
considering an offer. The public key file is a trust input, not something to
fetch from the release host. Production key creation and distribution remain
deployment steps controlled by the owner outside this command.

After verifying an explicitly trusted starting release, `trust` creates its
scope's state file. `accept` advances that product and target's remote
acceptance boundary. `record-installed` records a previously accepted local
package for rollback while retaining the remote boundary. These commands
take the same verification arguments plus `--state FILE`. Bootstrap also
requires `--starting-version MAJOR.MINOR.PATCH`, equal to the verified starting
release. A state file belongs to a trusted local path selected by the caller.
Neither metadata nor a remote catalog can choose it.

## Managed ownership

The existing [`game-delivery.json`](../manifests/game-delivery.json)
`base.final_files` catalog remains the authoritative game file list. A signed
game descriptor pins that catalog's identity. It does not carry a second game
file list. The launcher inventory also owns the explicitly listed official
overlay files. Every inventory path is relative to the managed root chosen by its caller. Metadata
cannot select an arbitrary local directory, move a file to another product's
root, or gain replacement authority from archive membership alone.

Managed entries can replace only their explicit owned paths. Seed entries can
be installed when absent but are never replacement authority. The launcher
package seeds `scripts/default.txt` on initial installation and preserves an
existing copy. A signed launcher inventory can replace only the listed files
in the official `plugins/dats/bahamut-dats-overlay` package. Custom overlay
packages remain outside that authority. Configuration, authentication and
profile state, logs, and screenshots also remain outside managed replacement
authority.

## Ordering and recovery

The verifier needs an explicit trusted starting version and retains the
highest accepted remote version and metadata identity across restarts. It
rejects an older remote offer and different metadata under an already accepted
version. Rechecking identical accepted metadata is safe. A previously trusted
local package may be restored during a later replacement transaction without
lowering the remote acceptance boundary. There is no remote downgrade switch.

A missing state file can only be bootstrapped from the explicitly trusted
starting version. A malformed existing state fails closed. Recovery from lost
trusted state or a lost signing key requires a process controlled by the owner.
Downloaded metadata cannot repair trust by itself.

State changes take a system lock on a persistent adjacent `.lock`
file through the read and atomic publication. Keep the state and lock in a
trusted local directory and retain the lock file across invocations. A failed
publication leaves the previously valid state file in place.

The release metadata contract supplies verification and inventory authority only.
It does not fetch release catalogs, publish artifacts, replace installed files,
or provide an updater UI. Production key custody and provisioning, hosted platform
validation, and live delivery remain separate release gates.
