# Troubleshooting

[Back to the documentation index](README.md)

Choose the problem you are seeing:

- [The Linux launcher will not open](#linux-startup)
- [Installation or repair fails](#install-failures)
- [Login fails](#authentication-failures)
- [The game closes after Play](#game-closes-after-play)
- [Settings or backups fail](#configuration)
- [Extensions are missing under Wine](#wine-extension-launch)

For setup, see [Getting started](getting-started.md). For help, use the
[Bahamut Discord](https://discord.gg/PxK5RJYQjm). Build and test instructions
are in [Development](development.md).

## Launcher logs

Open **Help** in the launcher, then choose **Copy Logs**. It refreshes the
current transcript and waits for pending failure entries before copying.
Review the text before sharing: credentials, authorization values, bearer
tokens, and launcher session IDs are redacted, but local paths and environment
details may still identify your machine.

**Open Logs** opens `<state-root>/logs/launcher/bahamut-launcher.log`. Help
shows the newest 256 KiB. To find your state root, see
[Windows portable files](configuration.md#portable-launcher-tree),
[macOS app layout](configuration.md#macos-app-layout), or
[Linux package layout](configuration.md#linux-package-layout).

The active log covers the current launcher session only. At startup, the
previous session moves to `bahamut-launcher.previous.log`; Help and Copy Logs
do not include it. Wine output and game chat logs are separate. See
[Wine extension launch](#wine-extension-launch) and the
[portable launcher tree](configuration.md#portable-launcher-tree) for their locations.

## Linux startup

Run `bahamut-launcher --version` first. It prints the version and exits without
opening a window, writing logs, or creating state. A version line confirms the
executable and its libraries load, but does not test the WebView.

### Missing libraries or an old glibc

For `error while loading shared libraries: <library>: cannot open shared object file`,
run `install-dependencies.sh --check` from the extracted or installed folder.
For a user installation, the command is
`~/.local/lib/bahamut-launcher/install-dependencies.sh --check`.
It names missing libraries and prints a package command for supported
distributions. Use `install-dependencies.sh --install` to run that command
after confirmation.

A missing `GLIBC_2.xx` version means your distribution's glibc is too old.
The release is built on Ubuntu 22.04 and requires glibc 2.35 or newer; use a
newer distribution release. The dependency installer cannot fix this.
A `GLIBCXX_` error, or a version error from another library, indicates
mismatched system packages. The script reports it as a missing library;
update your distribution's packages.

The [dependency script](../packaging/linux/install-dependencies.sh) returns:

| Exit status | Meaning |
|---|---|
| 0 | All checked libraries, and Wine when its requirement applies, are satisfied. |
| 1 | A library, `tar`, or `xz` is missing; a Wine selected through `BAHAMUT_WINE` or on a non-x86_64 host is missing, older than 7, or lacks 32-bit support; or the package command failed or was declined. |
| 2 | Invalid command line. |
| 3 | The host cannot be checked: no `ldd` or glibc loader, a non-ELF launcher, or a different CPU architecture. |
| 4 | The distribution uses musl; the launcher requires glibc. |
| 5 | glibc is older than the launcher requires. |

On x86_64 with `BAHAMUT_WINE` unset, the script reports the managed Wine
download and any system `wine` fallback. A missing, old, or 32-bit-less system
Wine does not affect its exit status. With `BAHAMUT_WINE` set, or on another
CPU architecture, it checks that Wine instead. `Wine lacks 32-bit support`
means the checked build lacks `i386-windows/ntdll.dll`. Select Wine 7 or newer
with 32-bit support.

Missing Vulkan loader (`libvulkan.so.1`) or driver support does not affect the
exit status either; the game uses slower OpenGL until a Vulkan driver is
installed. On x86_64 with `BAHAMUT_WINE` unset, the script also reports Wine's
display and font libraries as `engine libraries:` and `libpulse.so.0` or
`libasound.so.2` as `audio:`. Missing libraries in these reports do not change
the exit status. Without an audio library, the game has no sound. See the
[Linux Wine engine library list](configuration.md#linux-wine-engine).

### Distribution-specific checks

The script recognizes RHEL-family systems through `/etc/os-release` IDs
`rhel`, `centos`, `rocky`, `almalinux`, and `ol`. It does not run a package
command for them. On release 10 or newer, enable EPEL and install
`webkit2gtk4.1`; the script prints this as a manual step. Releases 9 and older
do not package WebKitGTK 4.1 and are unsupported. Derivatives such as Nobara
resolve to Fedora and receive its package command.

Read-only-root systems, including SteamOS and Bazzite, also receive no package
command. Use the system's own tooling for missing packages, such as
`rpm-ostree` or a distrobox container.

### Hyprland

For a blank window, try
`WEBKIT_DISABLE_DMABUF_RENDERER=1 bahamut-launcher`. This disables WebKitGTK's
DMA-BUF renderer. To apply the override to menu launches, add
`env = WEBKIT_DISABLE_DMABUF_RENDERER,1` to `hyprland.conf`. That setting affects
every application Hyprland starts.

`hyprctl clients` reports class `bahamut-launcher` on native Wayland and
`Bahamut-launcher` under XWayland. GTK uses XWayland when built without Wayland
support or when `GDK_BACKEND=x11` is set. To float either window class, use
`windowrule = match:class ^([Bb]ahamut-launcher)$, float on` in Hyprland 0.53 or
newer, or `windowrulev2 = float, class:^([Bb]ahamut-launcher)$` in 0.52 or older.

### Moving a beside-the-launcher tree

Close the launcher before moving its files. A Linux copy without
`.bahamut-launcher-package` stores state beside the executable. A packaged
copy does not import that state automatically.

Extract the new tar.gz into a new or empty directory, never over the old tree.
Its `bahamut-launcher/` directory conflicts with an older executable named
`bahamut-launcher`. For an existing packaged copy that runs in place, rename
or delete the old folder before extracting; its state already lives in
`~/.bahamut-launcher`. For an installed copy, rerun `./install.sh` from the
new release.

To migrate beside-the-launcher state, copy these paths into
`~/.bahamut-launcher`, or the absolute path in `$BAHAMUT_LAUNCHER_HOME`,
preserving their relative paths:

- `config/`
- `data/`
- `backups/`
- `logs/chat/` for chatlogs addon history
- `addons/<addon-id>/` for addons you installed yourself
- `plugins/dats/<package-id>/` for custom DAT packages, excluding `bahamut-dats-overlay`
- `scripts/default.txt`
- `screenshots/`

Shipped addons and the official DAT package stay in the install root. A copied
package with a shipped package's ID is skipped with a warning. The Wine
prefix, DXVK cache, `wine.log`, and `helper.log` already use the launcher data
directory in both layouts.

To keep state beside the executable, remove the marker only from a folder
you extracted yourself; see [Linux package layout](configuration.md#linux-package-layout).
Keep it in installed copies. `install.sh` replaces or deletes the whole
installation directory and refuses when the marker is missing, extra entries
are present, or a shipped file has changed.

If the script refuses, move the named files into the state root as above.
Restore a missing `.bahamut-launcher-package` from the matching release, then
retry. **Only after preserving everything you need**, use `install.sh --force`
to replace the directory or `install.sh --uninstall --force` to remove it.
Both delete everything in that directory.

## Install failures

Login and Play use the same install check. The game directory must contain
`ffxivboot.exe`; a ready installation also needs `ffxivgame.exe` and a
`game.ver` with the target 1.23b version.

- **`no-install` or `not-found`:** the directory is missing or lacks
  `ffxivboot.exe`. Select it on Home's install strip or in **Settings > Misc >
  Install Location**. A configured `game_location` overrides Windows registry
  detection; Linux and macOS use the configured path.
- **`outdated-client` or `outdated-install`:** the launcher found an
  installation, but its `game.ver` is wrong or `ffxivgame.exe` is missing.
  The diagnostics may show `STATE_OUTDATED_INSTALL` or game state `outdated`.
  Choose **Install** on Home with a new, empty folder. The configured host
  must serve the pinned final client ZIP; see [Game content delivery](content-delivery.md).
- **Download failure:** read the cause in the recovery strip and launcher log,
  fix it, then choose **Retry Install**.

A fresh installation needs a new or empty destination and configured base
metadata. A build without a pinned base package explains that limitation.
You can still select an existing client through **Install Location**.
Interrupted installs retain their owned staging files for Retry and never
select a partial client. Startup does not delete existing local payloads.
See [Game content delivery](content-delivery.md) for cache verification and recovery.

During download, extraction, and verification, **Pause** or **Resume** appears
beside **Cancel**. Downloads pause after a durable byte checkpoint. The strip
shows `Pausing after the current step` until a safe boundary, then
`Installation paused.` Resume also cancels a pending pause request.

Closing the launcher, including Alt-F4 or normal application quit, requests
cancellation and waits for worker cleanup. The closing message remains until
the worker stops. Home shows install and repair progress.

### Repair Install

Stop the game, then open **Settings > Misc > Install Location > Repair Install**.
Confirm to check and restore missing or damaged managed files. Home's Install
Strip shows progress, Pause, and Cancel.

Repair can reuse a verified ZIP of the complete client. Without one, even a
single damaged file may require downloading the full ZIP. Unknown files and
player data are preserved, but repair restores intentionally modified managed
files too.

An interrupted repair blocks login and Play until recovery finishes. Run
**Repair Install** again and keep any recovery folder named in an error. If
another launcher is repairing the same installation, let it finish first.
The launcher rechecks the managed inventory before reporting success.

Resolve read or access errors before retrying. An unreadable file is not
authorization to replace it.

### Launcher updates

After an interrupted restart, startup uses the staged signed helper to confirm
the complete update or restore the previous signed release. If that helper
is still working, close the extra launcher window and retry after it finishes.

If the updater reports extra files in the legacy official overlay, remove
those files from `plugins/dats/bahamut-dats-overlay/`, leaving `overlay.toml`,
then retry. Alternatively, extract the new release into an empty folder.
Extracting over the old copy leaves the extra files behind.

## Authentication failures

Resolve `no-install` or `outdated-client` before checking the server. Then open
**Profiles** from Home's login card, or inspect `config/bahamut.ini`:

- `host` must be a bare hostname or IP. It is also the lobby host patched into
  the client. `auth_port` sets the HTTP auth base at `/api/v1`; `lobby_port`
  is validated, but the launch patch changes only the host.
- Non-loopback hosts require HTTPS. Plain HTTP is accepted only for
  `127.0.0.1`, `localhost`, or `[::1]`; other HTTP profiles are rejected before
  credentials are sent.
- A `network` error means the single request, with its 10-second timeout,
  returned no HTTP status. Check the address, connection, and TLS setup.
- `invalid-credentials` maps to the server's `invalid_credentials` response.
  `username-taken` and validation errors appear in their matching UI flows.
  For `rate-limited`, the login control stays disabled for the duration in
  `Retry-After` rather than resubmitting.
- Malformed URLs or response bodies, unknown server errors, and other protocol
  mismatches appear as server errors. Compare the response with
  [Authentication](auth.md).

For developer testing without a server, a manually pasted token must contain
56 hexadecimal characters. That route skips live authentication, so it cannot
validate the Bahamut auth service.

## Game closes after Play

Keep the launcher open while reproducing a Wine crash: set
`close_on_game_start = false` in `bahamut.ini`. The default, `true`, closes the
launcher before the client exits and prevents exit-status logging.

For Windows, keep the launcher log, exact launcher version and package hash,
faulting module and offset, and any available call stack. Check the executable
hash, length, and `game.ver` against the
[supported client identity](extensions.md#client-compatibility). Version text
alone is insufficient. A matching identity and an access violation do not
identify a cause or justify replacing game files. The final numeric exit code
records the process result, not why the failing instruction failed.

The supported 1.23b client can assert in `ThreadManager::CreateEffectThread_`
with 16 or more logical processors in its CPU mask. Windows launch paths limit
the game to 15 allowed logical processors before resuming it. This addresses
that specific assertion, not every crash.

For developer comparisons, use the complete regular Windows package. The
`diagnostic-no-injection` feature uses a separate direct-launch transaction
and does not represent packaged native-runtime behavior. Compare exact
revisions and package hashes before attributing a difference to injection.
Wine stability patches apply only to Wine; applying them on Windows to suppress
an assertion does not establish its cause.

### Wine runtime and exit logs

Linux chooses the `BAHAMUT_WINE` file when set, otherwise managed Wine on
x86_64, otherwise `wine` on `PATH`. It initializes the managed prefix with
`wineboot --init` when needed and falls back to `wined3d` if DXVK setup fails.
macOS downloads managed Sikarugir Wine on first launch. See
[Linux Wine engine](configuration.md#linux-wine-engine).

Wine output goes to that launch's `logs/wine.log` under the launcher data
directory; extension launches add `helper.log` beside it. `enable_verbose_wine_debug`
widens the Linux Wine filter only.

If the client exits while the launcher remains open, the launcher appends:

- `=== ffxivgame exit: <status> after <elapsed> ===` for the Linux launch from
  the working copy
- `=== exit: <status> ===` for the Wine extension launch

A nonzero exit also goes into the launcher log. For the launch from the working
copy, it includes the last lines of `wine.log`. Neither record is written if
`close_on_game_start = true` closes the launcher first.

The macOS wrapper and Wine engine downloads, and the Linux DXVK download, must
match the size and SHA-256 pinned in
[`runtime_archive.rs`](../src/platform/runtime_archive.rs) before extraction.
A verification failure leaves installed runtime components unchanged. Check
the named download in the launcher log, resolve the failure, and retry. Linux
keeps its `wined3d` fallback, and reuses only DXVK caches from verified downloads.

### Wine engine download

Linux downloads its engine on first game launch and reuses it. A failed or
interrupted download leaves the previous engine and prefix untouched. If the
engine cannot be installed, the launcher tries `wine` on `PATH`. If that is
also missing, the error names the engine failure. Install Wine 7 or newer with
32-bit support, or set `BAHAMUT_WINE`.

To redownload the managed engine, delete `~/.bahamut-launcher/runtime/wine-*`
and start the game. Changing Wine does not change prefix rules: Wine updates a
prefix made by another Wine on first use. Stop any other Wine's `wineserver`
using that prefix first, with `wineserver -k` from that Wine. For a 32-bit-only
prefix you manage yourself, point `BAHAMUT_WINE` to the Wine that owns it.

## Configuration

Edit live files under `<state-root>/config/`; tracked `configs/` files are
annotated starter copies only.

- Missing `bahamut.ini`, `extensions.ini`, `dats.ini`, and
  `plugins/screenshot/settings.ini` are created from current defaults.
  Existing files control their documented settings. Repair malformed files
  directly; the launcher does not silently replace them.
- `bahamut.ini` owns launcher, mapped game, developer, and server settings.
  Screenshot options are in `plugins/screenshot/settings.ini`; plugin and
  addon selection and order are in `extensions.ini`.
- Use **Profiles** from Home's login card to select or edit servers. For a
  connection problem, check `host`, `auth_port`, `lobby_port`, and `use_https`
  in the live INI.

An enabled Screenshot or DiscordRPC plugin needs its matching DLL under
`<install-root>/plugins/`. A missing DLL, wrong architecture, incompatible
private ABI, or identity mismatch stops launch before the client resumes and
reports a native plugin bootstrap failure. Restore the DLL from the matching
launcher release. Do not rename or copy `bahamut.dll` into its place. Disable
the plugin in `extensions.ini` to leave its DLL unloaded.

## Backup and restore failures

Close the retail configuration utility and game before restoring.

- **User Settings and Macros backup has no data:** it needs a supported
  `config.*` or character-data file in `<Documents>/My Games/FINAL FANTASY XIV`.
  It does not create an empty retail folder when none exists.
- **Extensions backup has no data:** it needs an installed addon, DAT package,
  extension setting, or `scripts/default.txt`. It excludes native plugin DLLs
  and generated launcher state.
- **`No backup exists yet`:** the category has no completed backup. Restore
  selects the newest matching ZIP in `<state-root>/backups/`, never `.partial`
  files. Do not rename an unrelated ZIP to a launcher backup name.
- **Unsafe entry or invalid configuration:** validation failed before live
  data changed.
- **Locked file:** replacement cannot proceed. Current files remain unchanged
  if rollback succeeds.
- **Retained rollback directory:** stop and preserve the named directory.
  It holds prior live files that could not be restored automatically.

## Retail game settings

If `config.sys` is invalid, move it aside so Play can recreate it from the INI,
or rewrite it with the retail configuration utility on Windows. Retry Play;
it reapplies the INI values to mapped words in either case.

Keep these configuration rules in mind:

- On load, the launcher imports mapped values from a valid
  `<Documents>/My Games/FINAL FANTASY XIV/config.sys` if `[game] initialized`
  is false. Missing or invalid retail data leaves seeded defaults pending.
- Play creates a missing file from the INI and validates an existing file
  before creating the client process. After initialization, it applies INI
  values while preserving unmapped bytes.
- Wine writes the host user's Documents folder. The client reaches it through
  `drive_c/users/<name>/Documents` in the prefix. A plain directory there,
  rather than a link, holds a separate `config.sys` the launcher does not manage.
- Keep `[game]`, `[game.graphics]`, and `[game.audio]` together. Omitting all
  three triggers import on first adoption; keeping only some is malformed.
- Use only values and resolution pairs from [Configuration](configuration.md).
  Unknown encoded values are not normalized.
- A successful change saves the previous file as `config.sys.bak`. Failed
  validation or staged writes leave both files unchanged.

## DAT overlays

Use the **Dats-Overlay** detail editor to enable packages and set match order.
Changes take effect on the next game launch. See [DAT overlays](dat-overlays.md)
for the package format.

- Place each package directly in `plugins/dats/<package-id>/`, with a matching
  `overlay.toml` ID.
- A malformed custom manifest, duplicate ID, unsafe relative path, or link or
  reparse point in a custom package rejects the inventory. Fix the named
  package rather than editing the installed game DAT files.
- Requests without replacements use the original client path. To disable
  custom overlays, disable their rows in `config/dats.ini` and restart the
  client. The official overlay stays enabled in slot 1.

Dats-Overlay is built into the client module and cannot be disabled as a
service. No enabled `[dat.N]` rows means an empty package selection. Overlays
apply only when the launch loads the module; see
[Platform support](extensions.md#platform-support).

## Wine extension launch

Read [Platform support](extensions.md#platform-support) for current limits and
[Handshake](handshake.md#wine-extension-launch) for the launch transaction.
The logs are under `~/.bahamut-launcher/logs/` on macOS and Linux, or
`$BAHAMUT_LAUNCHER_HOME/logs/` when that variable is an absolute path:

- `helper.log`: the loader's `SUCCESS`, `ERROR`, and `EXIT` lines
- `wine.log`: Wine output

If addons, plugins, and DAT packages are all missing in game, look for
`client extensions are unavailable; launching without extensions` in the
launcher log. The reason names a missing `bahamut-loader.exe` or `bahamut.dll`
in the install root. Reinstall the matching launcher release. On macOS,
replace the whole `Bahamut Launcher.app`; editing files inside it invalidates
its code signature. The Extensions page does not display this fallback.

That fallback covers only those two files. A missing DLL for an enabled
Screenshot or DiscordRPC plugin stops launch instead; see
[Configuration](#configuration). A client identity mismatch also stops launch;
compare the installation with the [supported client identity](extensions.md#client-compatibility).

`helper reported neither SUCCESS nor ERROR` means the loader's readiness line
did not arrive before the timeout. The error names both logs; check
`wine.log` for Wine diagnostics.

## Diagnostic reference

Each startup records the version banner, configuration phases, install and
state roots, platform and bounded hardware details, disk space, required
launcher/runtime artifacts, displays, and Home lifecycle transitions. These
INFO events remain available even with a coarse inherited `RUST_LOG=warn`;
target-specific `RUST_LOG` directives can request more detail.

Frontend failures synchronously append ERROR events with the page, action,
displayed message, diagnostic, UI context, launcher version, and platform.
Redaction happens both before persistence and before Help receives the log.

Visible feedback behaves as follows:

- Two-column pages reserve one or two short lines at the bottom of the left
  card; stacked layouts use the affected card. Profiles uses the left card's
  spare space.
- Account creation shows messages above **Create Account**. Extensions shows
  them above the folder-button card in both layouts, without moving the
  library or package controls.
- Login and launch share the Account Login feedback area above **Log In** or
  **Log Out**, after **Remember Login**.
- Leaving a page or Settings tab clears visible messages. Pending operations
  can still log failures without restoring the message. Technical failures
  direct players to Help.
- Help shows its errors below the support text and copy confirmation below
  **Copy Logs** and **Open Logs**. Successful Settings saves are silent.
