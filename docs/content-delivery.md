# Game content delivery

[Back to the documentation index](README.md)

The launcher downloads immutable game archives over HTTPS from an owner controlled
content host and verifies every object against metadata shipped in the launcher.
Content requests never carry account credentials, session tokens, or storage
keys. Verified ZIP downloads can resume without changing their pinned identity.

## Shipped identities

[`game-delivery.json`](../manifests/game-delivery.json) owns the delivery
host and optional base package. A missing base package disables new game
installation with an explicit message. A missing host disables downloads unless
`launcher.content_root` specifies an HTTPS host. Host selection changes only
where known objects are fetched, never which bytes are trusted.

The base package records its target version, immutable object keys, archive
lengths and SHA-256 identities, complete extracted-file inventories, final-file
checks, and a staging-space reservation. The shipped package is one full
final-client archive. Normal builds and tests need neither game files nor
storage credentials.

The shipped schema 3 manifest pins `xiv1point0.zip` on the configured R2 host.
The launcher accepts only schema 3. A client that is not the final 1.23b build
is offered a fresh final installation.

An offline [signed release descriptor](release-metadata.md) can pin this
manifest's identity for a stable game offer. Its game inventory refers to
`base.final_files`. It does not create another file list or enable remote
replacement.

## Installation

Install accepts a new or empty destination. Home displays the destination and
uses PATH to change it. Install runs a disk-space preflight and reports failures
in the strip. The worker repeats space checks before modifying the destination.
It extracts into owned staging on the destination volume, verifies the archive
inventory, and checks the final files and version before publishing the
directory. A nonempty destination is never cleared to make it fit. The selected
client changes only after successful installation. Repair Install, described
under [Troubleshooting](troubleshooting.md#repair-install), verifies the managed
files of an installed client and replaces damaged ones from the same archive.

One operation owns downloading, verification, and extraction. Its reservation
prevents conflicting game launch, backup, restore, and HUD reset operations.
Changing configuration cannot retarget an active worker. Closing the launcher
requests cancellation and waits for the worker to finish. Pause is acknowledged
only at a durable download checkpoint or a safe disk operation boundary. A file
already being written finishes before cancellation or pause.

Partial transfers belong to a specific expected length and SHA-256 identity.
Resume checks the server's byte-range response before appending. A server that
ignores a range restarts the object. Truncated, oversized, differently encoded,
or corrupt responses are rejected. Only a complete verified object is published
to the cache, and cached objects are verified again before reuse.

An interrupted base install can restart extraction from verified archives after
its staging receipt and path containment are checked. A folder with an install
receipt cannot become Ready until it completes. Retry Install completes its
verification first.

This ZIP uses the explicit `final-fantasy-xiv-wrapper` layout. The installer
strips exactly `FINAL FANTASY XIV/`, checks the source `boot.ver` and `game.ver`
against the final identities, then writes its own version stamps. It preserves
`patch.ver`, the selected game files, and the declared empty directories.
Only source exclusions pinned in the manifest and paired `__MACOSX` Apple
metadata are skipped. Other paths, aliases, links, special entries, collisions,
and missing inventory members fail installation.

The current ZIP omits `backtrace.txt` and a distributed
`ffxivgame.patched.exe`. Linux and macOS launches without the loader files
create the patched executable from `ffxivgame.exe` as described in
[Wine launch](handshake.md#wine-launch). The
[Wine extension launch](handshake.md#wine-extension-launch) patches the
original in process memory and writes no patched copy.

## Publisher and intake workflows

Use an explicit final client source, its inventory, and measured staging
requirement with
[`package-game-content.py`](../scripts/package-game-content.py). The command
emits deterministic archives and a delivery manifest from explicit file
allowlists, one relative path per line. Review the inventory for
the intended game payload before adopting the metadata. Keep credentials,
character settings, macros, logs, caches, and local overrides outside the input.

For example, after preparing and identifying the base and final trees, whose
payloads are identical apart from `boot.ver` and `game.ver`:

```powershell
python scripts/package-game-content.py --input-root C:/Content/Base --base-allowlist C:/Content/base-files.txt --final-root C:/Content/Final --final-allowlist C:/Content/final-files.txt --output-dir C:/Content/Output --staging-bytes MEASURED_PEAK --max-archive-uncompressed-bytes SELECTED_LIMIT
```

Replace the uppercase values with the measured byte counts. The base and final
payloads must match. The base allowlist excludes `boot.ver` and `game.ver`. The
final allowlist includes their exact target values. Copy the generated metadata
into the tracked manifest and set its public host before building a release
with delivery enabled.

Publish those exact objects to an R2 bucket controlled by the owner behind an
HTTPS custom domain. Keep keys immutable and retain lengths and SHA-256
identities. Packager objects use `game/<sha256>/<archive>.zip`. Upload
credentials belong in the operator's secret store and are not launcher inputs.

For the pinned full 1.23b object, [`intake-full-client.py`](../scripts/intake-full-client.py)
rechecks the complete archive length and SHA-256, every ZIP entry and selected
file hash, the exact source exclusions, the version stamps, and empty directories.
It emits schema 3 metadata without rewriting the object. The `--staging-bytes`
value must cover the inventory being staged. Installer preflight also accounts
for file and directory allocation on the destination volume. The importer never
publishes game bytes. The normalized publisher above also accepts other
explicitly prepared inputs.

Before using a production manifest, check complete downloads, byte ranges,
identity encoding, hashes, and a fresh installation through the actual hostname.
Check cache behavior for the selected object sizes and Cloudflare plan.
Synthetic tests cover implementation only. Production delivery and platform
installation still need these live checks.

The optional smoke command reads the tracked identities and performs HEAD,
one-byte range, and complete SHA-256 checks without saving the payload:

```powershell
python scripts/check-content-delivery.py --object-key xiv1point0.zip
```

Repeat for the selected archive sizes and record the reported cache headers
with the fresh installation result. This command is opt-in. It never uploads
or publishes objects.
