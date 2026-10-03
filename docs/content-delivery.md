# Game content delivery

[Back to the documentation index](README.md)

The launcher downloads immutable game ZIPs over HTTPS from an owner-controlled
content host. It verifies each object against metadata shipped with the
launcher. Content requests never include account credentials, session tokens,
or storage keys. A verified ZIP download can resume without changing its
pinned identity.

[Installation](#installation) explains what players can expect. The remaining
sections describe the trusted metadata and publisher workflow.

## Shipped identities

[`game-delivery.json`](../manifests/game-delivery.json) defines the delivery
host and optional base package. If the base package is missing, new game
installation is disabled with an explicit message. If the host is missing,
downloads are disabled unless `launcher.content_root` supplies an HTTPS host.
Changing the host changes where known objects are fetched, never which bytes
are trusted.

The base package specifies:

- Target version and immutable object keys
- Archive lengths and SHA-256 identities
- Complete extracted-file inventories and final-file checks
- A staging-space reservation

The shipped package is one full final-client ZIP. Its schema 3 manifest pins
`xiv1point0.zip` on the configured R2 host. The launcher accepts only schema 3.
Clients other than the final 1.23b build are offered a fresh final installation.
Normal builds and tests require neither game files nor storage credentials.

An offline [signed release descriptor](release-metadata.md) can pin this
manifest's identity for a stable game offer. Its game inventory references
`base.final_files`; it neither creates a second file list nor permits remote
replacement.

## Installation

Install requires a new or empty destination. Home shows that destination;
use PATH to change it. Install checks disk space first and reports failures
in the strip. The worker repeats the checks before changing the destination.

The worker extracts into owned staging on the destination volume, verifies
the ZIP inventory, and checks the final files and version before publishing
the directory. It never clears a nonempty destination to make room. The
selected client changes only after installation succeeds.
[Repair Install](troubleshooting.md#repair-install) verifies an installed
client's managed files and replaces damaged files from the same ZIP.

### Progress, pause, and cancellation

A single operation controls download, verification, and extraction. Its
reservation blocks conflicting launch, backup, restore, and HUD reset
operations. Configuration changes cannot retarget an active worker. Closing
the launcher requests cancellation and waits for the worker to finish.

Pause is acknowledged only at a durable download checkpoint or safe disk
operation boundary. A file already being written finishes before pause or
cancellation takes effect.

### Resuming downloads and installs

Partial transfers are tied to an expected length and SHA-256 identity. Before
appending, resume validates the server's byte-range response. If the server
ignores a range, the download restarts. Truncated, oversized, differently
encoded, or corrupt responses are rejected.

Only complete, verified objects enter the cache. Cached objects are verified
again before reuse. After an interrupted base install, extraction can restart
from verified archives once the staging receipt and path containment pass
validation. A folder with an install receipt cannot become Ready until the
installation completes; Retry Install verifies it first.

### ZIP layout and validation

The shipped ZIP uses the explicit `final-fantasy-xiv-wrapper` layout. The
installer strips exactly `FINAL FANTASY XIV/`, checks source `boot.ver` and
`game.ver` against the final identities, then writes its own version stamps.
It preserves `patch.ver`, selected game files, and declared empty directories.

Only source exclusions pinned in the manifest and paired `__MACOSX` Apple
metadata are skipped. Other paths, aliases, links, special entries, collisions,
or missing inventory members fail installation.

The current ZIP omits `backtrace.txt` and a distributed
`ffxivgame.patched.exe`. Linux and macOS launches without loader files create
the patched executable from `ffxivgame.exe`; see
[Wine launch](handshake.md#wine-launch). The
[Wine extension launch](handshake.md#wine-extension-launch) instead patches
the original in process memory and writes no patched copy.

## Publisher and intake workflows

### Build a delivery package

Run [`package-game-content.py`](../scripts/package-game-content.py) with an
explicit final client source, its inventory, and a measured staging-space
requirement. It emits deterministic archives and a delivery manifest using
explicit file allowlists with one relative path per line.

Review the inventory before adopting the metadata. Keep credentials, character
settings, macros, logs, caches, and local overrides out of the input.

Prepare and identify base and final trees whose payloads differ only in
`boot.ver` and `game.ver`, then run:

```powershell
python scripts/package-game-content.py --input-root C:/Content/Base --base-allowlist C:/Content/base-files.txt --final-root C:/Content/Final --final-allowlist C:/Content/final-files.txt --output-dir C:/Content/Output --staging-bytes MEASURED_PEAK --max-archive-uncompressed-bytes SELECTED_LIMIT
```

Replace the uppercase placeholders with measured byte counts. Base and final
payloads must match. The base allowlist excludes `boot.ver` and `game.ver`;
the final allowlist includes their exact target values. Copy the generated
metadata into the tracked manifest and set its public host before building a
release with delivery enabled.

Publish those exact objects to an owner-controlled R2 bucket behind an HTTPS
custom domain. Keep keys immutable and preserve their lengths and SHA-256
identities. Packager objects use `game/<sha256>/<archive>.zip`. Upload
credentials belong in the operator's secret store and are not launcher inputs.

### Intake an existing full-client ZIP

For the pinned full 1.23b object,
[`intake-full-client.py`](../scripts/intake-full-client.py) rechecks the entire
archive's length and SHA-256, every ZIP entry and selected file hash, exact
source exclusions, version stamps, and empty directories. It emits schema 3
metadata without rewriting the object or publishing game bytes.

`--staging-bytes` must cover the staged inventory. Installer preflight also
accounts for file and directory allocation on the destination volume. The
normalized publisher above can also use other explicitly prepared inputs.

### Validate production delivery

Before using a production manifest, test complete downloads, byte ranges,
identity encoding, hashes, and a fresh installation through the actual host.
Check cache behavior for the chosen object sizes and Cloudflare plan.
Synthetic tests cover the implementation only; production delivery and
platform installation still require live checks.

The optional smoke command reads tracked identities and performs HEAD,
one-byte range, and complete SHA-256 checks without saving the payload:

```powershell
python scripts/check-content-delivery.py --object-key xiv1point0.zip
```

Repeat for the selected archive sizes. Record the reported cache headers
alongside the fresh installation result. This command is opt-in and never
uploads or publishes objects.
