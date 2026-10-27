# Test authoring

A test earns its place by detecting a distinct defect. Choose the cheapest
fixture that can detect it, while preserving the boundary the claim depends on.
Use [Development](../development.md) for execution commands and platform limits.

## Behavior and assertions

Add the narrowest regression test for a demonstrated bug, or cover a new
branch, byte layout, persisted state, failure path, or ownership boundary that
existing tests do not reach. A second input through the same branch with the
same assertions does not justify another expensive scenario.

Keep expected values independent of the function or registry under test.
Authored data rows, exact bytes, inventories, and hand-placed literals are
specifications. Do not replace them with values derived from production code.

A guard-specific failure must start with otherwise valid input and identify
the intended rejection. An unrelated error must not let the test pass when
that guard is removed. The target-version case in
[`game_inventory_pins_delivery_manifest_archive_and_final_files`](../../src/release.rs)
keeps its version mismatch separate from archive-identity failure.

## Choose sufficient inputs

Start with pure functions, decoders, or metadata when their consumers need no
artifact. Metadata validation can use a valid synthetic artifact identity;
retain real signing, verification, locks, and persisted-state operations when
those are under test. Do not fabricate verified state to bypass them.

Use literal bytes for a failure rejected before archive parsing, provided
all earlier checks pass. Keep real archives and contents-derived identities
for extraction, hashing, archive-format validation, and package consumers.
Keep exact staged-tree and archive inventories; they detect different omissions.
[`src/release.rs`](../../src/release.rs) separates metadata fixtures from
archive fixtures; the Wine engine's
[`bad_digest_leaves_no_engine_and_no_staging`](../../src/platform/wine_engine.rs)
uses bytes that reach digest rejection without an unused valid tar archive.

Use real filesystem trees for publication, permissions, alias handling,
preservation, recovery, and cleanup. Preserve every distinct durable state:
equal final files do not make interrupted journal, pointer, receipt, payload,
or backup states equivalent. The crash boundaries in
[`tests/update_helper_process.rs`](../../tests/update_helper_process.rs)
exercise different recovery inputs.

## Share setup without sharing mutations

Reuse existing local helpers before adding scaffolding. Share immutable
baselines, or combine read-only assertions after the same successful input,
when consumers cannot alter the shared result. Keep each test's writable
roots, environments, command stubs, and mutation fixtures separate. Register
cleanup before fallible setup and cover failure cleanup.

[`scripts/test-linux-package.py`](../../scripts/test-linux-package.py) shares
a filtered command directory while retaining per-test tool stubs and roots.
[`scripts/test-macos-app-package.py`](../../scripts/test-macos-app-package.py)
checks manifest, architecture, and plist values after one successful package.
Keep separate outputs when comparing determinism, and separate writable
fixtures for tampering, refusal, installation, and rollback.

Retain assertions at the state boundary they protect. A receipt check before
apply is not replaced by one after apply; see
[`metadata_check_is_download_free_and_apply_downloads_before_handoff`](../../src-tauri/src/launcher_updates.rs).

## Preserve isolation and timing

Keep native executables, DLL loading, locks, child-process protocols, crash
boundaries, and teardown where they establish behavior. Share compiled objects
only when compile options, definitions, and dependencies match; preserve
separate DLL export/probe variants and process lifetimes. The test object
libraries in [`client/CMakeLists.txt`](../../client/CMakeLists.txt) follow this
boundary.

Keep browser scenarios in separate processes and profiles, including their
origins, pre-document injection, explicit reboots, and cleanup. Reuse
[`setupBrowserTest`](../../src-tauri/ui/browser-test.mjs) for common lifecycle
setup. A shared helper alone does not save browser starts.

Use bounded readiness predicates for positive readiness. Keep elapsed-time
assertions, negative observation windows, late-event races, countdowns, expiry,
and cleanup coverage. A ready state does not establish focus or layout
settling. Shrinking deliberate deadlines, payload sizes, thresholds, or stress
iterations alone is not an equivalent optimization.

## Review cost and coverage

For an expensive fixture, name the defect it detects and the setup it needs:
archive construction, full-tree copies, compilation, signing, process starts,
or waits. Before duplicating that work, check for a cheaper seam or an existing
same-input success fixture. Avoid a new framework for a small local helper.

Keep all authored data rows and independent literals. Sampling a matrix or
merging process, filesystem, or timing boundaries requires explicit review of
the exact coverage lost and the surviving detector; representative sampling
is not a general launcher rule.

Verify consolidations against the relevant platform and preserve assertions,
inputs, durable states, and cleanup. Distinguish removed setup operations from
measured elapsed savings. Synthetic package checks, real staged-artifact checks,
and target-platform execution establish different facts; apply
[Evidence and claims](evidence-and-claims.md) when reporting results.
