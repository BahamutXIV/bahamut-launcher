# Troubleshooting

[Back to the documentation index](README.md)

For installation steps, see [Getting started](getting-started.md). Use
[Install failures](#install-failures), [Authentication failures](#authentication-failures),
or [Game closes after Play](#game-closes-after-play) for the problem you are seeing.
For further help, [join the Bahamut Discord](https://discord.gg/PxK5RJYQjm).
Build, test, and package instructions are in [Development](development.md).

## Launcher logs

The launcher writes its native structured transcript to
`<state-root>/logs/launcher/bahamut-launcher.log` (see
[macOS app layout](configuration.md#macos-app-layout) for the state root on
each platform). Use the Help button in the launcher to view the newest 256
KiB, copy the displayed snapshot, and see the canonical path. The support view
contains launcher events only. Wine output and game chat logs remain separate
sources. See the
[portable launcher tree](configuration.md#portable-launcher-tree) for the chat
log location.

The active log contains only the current launcher session. At startup, the
preceding transcript moves to `bahamut-launcher.previous.log`. Help and Copy
Logs never mix those older events into current support output.

Every launch records a startup banner, configuration phases, portable roots,
platform and bounded hardware details, disk space, required launcher/runtime
artifacts, attached displays, and Home lifecycle transitions. The launcher
keeps these INFO events even when a coarse inherited `RUST_LOG=warn` is set.
`RUST_LOG` directives for a target can still request additional detail.

Credentials, authorization values, bearer tokens, and launcher session IDs are
redacted before persistence and again before the Help view receives the log.
Review copied diagnostics because unrelated paths and environment details can
still identify the local machine.

## Game closes after Play

The supported 1.23b client can assert in `ThreadManager::CreateEffectThread_`
when its CPU mask contains 16 or more logical processors. Windows launch paths
limit the game process to 15 allowed logical processors before resuming it.
This addresses that known assertion but does not classify another crash.

Use the complete regular Windows package when validating a player report. The
`diagnostic-no-injection` feature is developer-only and uses a separate direct
launch transaction. It does not represent packaged native-runtime behavior.
Compare exact launcher revisions and package hashes before attributing a
difference to injection.

Check the executable hash, length and `game.ver` against the
[supported client identity](extensions.md#client-compatibility), rather
than relying on the version text alone. A matching identity and an access
violation do not identify the crash's cause or justify replacing game
files. Keep the launcher log, exact launcher/package identity, Windows faulting
module and offset, and any available call stack with the report. The final
numeric exit code in the launcher log records the process result. It does not
identify the failing instruction's cause.

The Wine stability patches are specific to the Wine backend. Applying them to
native Windows to suppress an assertion is not a diagnosis.

## Configuration

- Live files are under `<state-root>/config/`. Tracked files under `configs/` are
  annotated starter copies only.
- The launcher creates missing `bahamut.ini`, `extensions.ini`, `dats.ini`, and
  `plugins/screenshot/settings.ini` files from current defaults. Once present,
  each file is authoritative for its documented scope. Repair a malformed file
  directly. The launcher does not replace it silently.
- `bahamut.ini` owns launcher, mapped game, developer, and server settings.
  Screenshot options live in `plugins/screenshot/settings.ini`. Plugin and
  addon selection and order live in `extensions.ini`.
- An enabled Screenshot or DiscordRPC row requires its matching DLL under
  `<install-root>/plugins/`. A
  missing DLL, wrong architecture, incompatible private ABI, or identity
  mismatch stops launch before the client resumes and is reported as a native
  plugin bootstrap failure. Restore the DLL from the matching launcher package.
  Do not rename or copy `bahamut.dll` into its place. Disabling the plugin in
  `extensions.ini` leaves its DLL unloaded.
- Use Profiles from the Home login card to select and edit server profiles.
  Check `host`, `auth_port`, `lobby_port`, and `use_https` in the live INI when
  diagnosing a connection.

## Backup and restore failures

- User Settings and Macros backup requires at least one supported `config.*`
  or character-data file under
  `<Documents>/My Games/FINAL FANTASY XIV`. It does not create an empty retail
  folder when no supported data exists.
- Extensions backup requires at least one installed addon, DAT package,
  extension setting, or `scripts/default.txt`. Native plugin DLLs and generated
  launcher state are intentionally excluded.
- Restore uses the newest matching ZIP under `<state-root>/backups/`.
  `No backup exists yet` means that category has not completed a successful backup.
  `.partial` files are never selected.
- An unsafe-entry or invalid-configuration error occurs before live data is
  changed. Do not rename an unrelated ZIP to a launcher backup name.
- Close the retail configuration utility and the game before restoring. A
  locked file prevents replacement and leaves the current files unchanged when
  rollback succeeds.
- If an error names a retained rollback directory, stop and preserve that
  directory. It contains the prior live files that could not be moved back
  automatically.

## Retail game settings

Launcher load imports mapped values from a valid
`<Documents>/My Games/FINAL FANTASY XIV/config.sys` when `[game] initialized`
is false. Missing or invalid retail state leaves the seeded defaults pending.
Play creates a missing file from the INI values and validates an existing one
before it creates a client process. Initialized launches apply the INI values
while preserving all unmapped bytes.

- An invalid `config.sys` is a launch error. Move the file aside so Play
  recreates it from the INI values, or rewrite it with the retail
  configuration utility on Windows, then retry Play. Play reapplies the INI
  values to the mapped words either way.
- The Wine backends write the host user's Documents folder. The client reads
  it only through the prefix's `drive_c/users/<name>/Documents` link. A
  prefix whose Documents folder is a plain directory keeps a separate
  `config.sys` inside the prefix that the launcher does not manage.
- Keep all of `[game]`, `[game.graphics]`, and `[game.audio]`. Omitting all
  three requests triggers import on first adoption. Leaving only a subset is
  malformed.
- Use only values and resolution pairs listed on the Configuration page.
  Unknown encoded values are not normalized.
- A successful change leaves the previous file at `config.sys.bak`. A failed
  validation or staged write leaves both files unchanged.

## DAT overlays

Dats-Overlay is built into the client module and cannot be disabled as a
service. Its package selection is empty when no `[dat.N]` row is enabled.
Packages apply only to launches that load the module, as listed in
[Platform support](extensions.md#platform-support).

- Put each package directly under `plugins/dats/<package-id>/` and keep its
  `overlay.toml` ID identical to the directory name.
- Use the Dats-Overlay detail editor to enable packages and set their match order.
  Changes apply on the next game launch.
- A malformed custom manifest, duplicate package ID, unsafe relative path, or
  link or reparse point in a custom package tree rejects the overlay inventory.
  Fix the named custom package instead of changing the installed game DAT files.
- A request with no replacement continues through the original client path.
  To turn off custom overlays, disable their rows in `config/dats.ini` and
  restart the client. The official overlay stays enabled in slot 1.

## Install failures

The login and launch commands use the same install gate. A resolved game
directory must contain `ffxivboot.exe`. A ready install also needs
`ffxivgame.exe` and a `game.ver` containing the target 1.23b version.

- `no-install` or a `not-found` install state means the resolved directory is
  missing or does not contain `ffxivboot.exe`. A configured `game_location`
  takes precedence over Windows registry detection. Linux and macOS rely on
  the configured path. Select or replace that path from the install strip on
  Home or the Install Location path row in Settings > Misc.
- `not-patched` or `needs-patch` means the base install was found, but
  `game.ver` is not at the target version or `ffxivgame.exe` is absent. Fix
  the path first. The shipped manifest has no incremental remote retail patch
  objects.
  Choose Install Fresh on Home for another empty folder. The configured host
  must return the pinned final client archive for that download to succeed. See
  the [game content delivery reference](content-delivery.md) for verification.
- A download failure shows its cause in the fixed size recovery strip and
  launcher log. Press Retry Install after correcting the cause.
- Active download, extraction, and verification expose Pause or Resume beside
  Cancel. Progress belongs in the lifecycle strip. Downloads pause after a
  durable byte checkpoint. The strip shows `Pausing after the current step` until
  the worker reaches a safe boundary, then reports `Installation paused.` Resume also
  cancels a pending pause request.
- Closing the launcher during installation requests cancellation and waits for
  worker cleanup before exiting. This also applies to Alt-F4 and normal
  application quit. The closing message stays visible until the worker stops.
  Home shows install, patch, and repair progress.

### Repair Install

Open Settings > Misc > Install Location and choose Repair Install. Confirm to
check and restore missing or damaged managed game files. Progress appears in
the Home Install Strip, with Pause and Cancel controls. A verified ZIP of the
complete client can be reused. Without it, even one damaged file may require
downloading the complete archive.

An interrupted repair blocks login and Play until recovery finishes. Choose
Repair Install again and keep any recovery folder named in an error. A repair
running in another launcher must finish before a second repair can start.
The launcher verifies the managed inventory again before reporting success.

An unresolved read or access error is not permission to replace that file.
Resolve the reported access problem and retry Repair Install. Stop the game
before repair. Unknown files and player data are outside the managed inventory
and are preserved. Repair restores changed managed files, including intentional
changes to those files.

### Launcher updates

On Windows, the launcher checks signed update metadata quietly at startup.
Open Settings > Misc > Install Location and choose Check for Updates to check
again. If a release is available, the same button becomes Update Launcher. That action
downloads the signed package and restarts through the staged update helper
when other launcher work is idle.

If a restart is interrupted, startup uses the staged signed helper to confirm a
complete update or restore the previous signed release. If the helper is still
finishing, close the extra launcher window and retry after the restart completes.

If an update reports extra files in the legacy official overlay, remove those
files from `plugins/dats/bahamut-dats-overlay/` while leaving `overlay.toml`,
then retry. You can also extract the new launcher into an empty folder. Merely
extracting it over the existing folder leaves the extra files in place.

New installation requires a new or empty destination and configured base
metadata. A build without a pinned base package explains that limitation. Use
Settings > Misc > Install Location for an existing client. Interrupted
installations preserve owned staging for Retry and do not select a partial
client. See the
[game content delivery reference](content-delivery.md) for cache verification
and recovery. Existing local payloads are never deleted by launcher startup.

On Linux, the launcher looks for `wine` on `PATH` unless `BAHAMUT_WINE` is
set, initializes a managed prefix with `wineboot --init` when needed, and
falls back from an attempted DXVK setup to `wined3d`. On macOS, the managed
Sikarugir Wine engine is downloaded on first launch. Wine output goes to the
`wine.log` for that launch under the launcher's data directory, and the Wine
extension launch adds `helper.log` beside it (see
[Wine extension launch](#wine-extension-launch)).
`enable_verbose_wine_debug` widens the Linux Wine filter only.

When the client exits while the launcher is still open, the launcher appends
its exit status to `wine.log` (`=== ffxivgame exit: <status> after <elapsed> ===`
for the Linux launch from the working copy, `=== exit: <status> ===` for the Wine
extension launch) and records a nonzero status in the launcher log, with the
last lines of `wine.log` for the launch from the working copy. With
`close_on_game_start = true` (the default), the launcher closes before the
client exits and neither record is written. Set it to
`false` in `bahamut.ini` before reproducing a client that closes on its own.

The macOS wrapper and Wine engine archives and the Linux DXVK archive must
match the size and SHA-256 pinned in
[`runtime_archive.rs`](../src/platform/runtime_archive.rs) before extraction.
An archive verification failure leaves installed runtime components unchanged.
Check the launcher log for the named archive and retry after resolving a
failed download. Linux retains its `wined3d` fallback when DXVK setup fails.
Only DXVK caches created from a verified download are reused.

## Wine extension launch

Status and limits for players are in
[Platform support](extensions.md#platform-support). The launch
transaction is in the
[Handshake](handshake.md#wine-extension-launch). This
path writes two logs under the launcher data directory's `logs/` folder
(`~/.bahamut-launcher/logs/` on macOS and Linux, or
`$BAHAMUT_LAUNCHER_HOME/logs/` when that variable holds an absolute path):
`helper.log` holds the loader's `SUCCESS`, `ERROR`, and `EXIT` lines, and
`wine.log` holds Wine's own output.

- If addons, plugins, and DAT packages are all absent in game, check the
  launcher log for `client extensions are unavailable; launching without
  extensions`. Its reason names the missing `bahamut-loader.exe` or
  `bahamut.dll`, both read from the install root. Repair by reinstalling from
  the matching archive; on macOS, replace `Bahamut Launcher.app` itself rather
  than copying files into it, since editing the bundle invalidates its code
  signature. The Extensions page does not show this fallback.
- The fallback covers only those two files. An enabled Screenshot or
  DiscordRPC plugin whose DLL is missing from `plugins/` fails the launch
  instead, as described under [Configuration](#configuration).
- A launch that fails with `helper reported neither SUCCESS nor ERROR` timed
  out waiting for the loader's readiness line. The message names both logs.
  Check `wine.log` for Wine's diagnostics.
- A client identity mismatch stops the launch instead of falling back. Compare
  the install against the
  [supported client identity](extensions.md#client-compatibility).

## Authentication failures

Login is gated by the install check, so resolve `no-install` and
`not-patched` before diagnosing the server. Then check the selected profile on
Profiles from the Home login card or in `config/bahamut.ini`:

- `host` is a bare hostname or IP and is also the lobby host patched into the
  client. `auth_port` forms the HTTP auth base at `/api/v1`. `lobby_port` is
  validated but the launch patch changes only the host.
- HTTPS is required for non-loopback hosts. Plain HTTP server profiles are
  accepted only for `127.0.0.1` or `localhost`. A non-loopback HTTP entry is
  rejected before credentials are sent.
- The launcher makes one request with a 10-second timeout. A `network` error
  means no HTTP status was obtained, so check the address, connection, and
  TLS setup. `invalid-credentials` maps to the server's
  `invalid_credentials` response. `username-taken`, validation, and
  `rate-limited` responses are surfaced by their corresponding UI paths. A
  rate-limited response carries `Retry-After`. The login control stays
  disabled for that duration instead of repeatedly submitting.
- A malformed URL, malformed response body, unknown server error, or other
  protocol mismatch is surfaced as a server error. Compare the server's
  response with [`docs/auth.md`](auth.md).

For no-server launch testing, the developer-token path still validates a
token pasted by hand with 56 hexadecimal characters and bypasses the live auth
round-trip. It is a developer convenience, not a replacement for testing the
Bahamut auth service.

See also [Configuration](configuration.md),
[Authentication](auth.md), [Handshake](handshake.md),
and the [documentation index](README.md).
