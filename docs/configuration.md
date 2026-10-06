# Configuration

[Back to the documentation index](README.md)

Use Settings to change launcher and game options, Extensions to choose addons,
and the Dats-Overlay editor to choose DAT packages. This page explains where
those choices are saved, how backups work, and the configuration file formats.

## Configuration files and storage

Each file has a separate purpose:

| File | Contents |
|---|---|
| `config/bahamut.ini` | Launcher, mapped game, developer, and server settings |
| `config/extensions.ini` | Plugin and addon selection and load order |
| `config/dats.ini` | DAT package selection and priority |
| `config/plugins/` and `config/addons/` | Settings owned by individual extensions |
| `config/addons/layout.ini` | Addon overlay positions shared by all characters |
| `config/news.toml` | Announcements shown on Home |

Tracked starter files are in [`configs/`](../configs/). The launcher creates
missing live files from the current defaults. When saving, it validates the
data, writes a temporary file beside the destination, syncs it, and then replaces
the destination. A failed write leaves the previous file intact.

In the portable layout, configuration lives beside the executable. Windows uses
this layout, as does a Linux or macOS launcher tree without the Linux package
marker:

```text
<launcher-dir>/
  bahamut-launcher.exe
  bahamut-loader.exe
  bahamut.dll
  plugins/screenshot.dll                <- packaged Screenshot implementation
  plugins/discord-rpc.dll               <- packaged DiscordRPC implementation
  backups/                              <- bounded manual backup archives
  data/                                 <- portable WebView profile root and local storage
  config/
    bahamut.ini                         <- launcher, game, and server settings
    extensions.ini                      <- plugin/addon enablement and order
    dats.ini                            <- DAT package enablement and priority order
    addons/layout.ini                   <- shared addon overlay positions
    plugins/screenshot/settings.ini     <- Screenshot-owned settings
    news.toml                           <- announcements rendered on the Home screen
```

The executable in a portable Linux or macOS tree is `bahamut-launcher`; the
rest of the layout is the same. The [macOS app](#macos-app-layout) and
[Linux package](#linux-package-layout) separate installed files from writable
state. See [Platform support](extensions.md#platform-support) for which
platforms load the packaged loader and DLLs.

Windows uses no Bahamut-specific AppData folders. WebView2 stores its profile
in `data/EBWebView/` under the launcher's writable `data/` folder.

On Linux and macOS, the launcher data directory is `$BAHAMUT_LAUNCHER_HOME`
when it contains an absolute path, or `~/.bahamut-launcher/` otherwise. The
managed Wine prefix lives in this directory. Wine and helper logs are saved as
`logs/wine.log` and `logs/helper.log` beneath it. The `runtime/` folder holds
the managed Wine engine and, on Linux, the verified DXVK cache. Verified
downloads and partial transfers use the configured download cache folder.

## macOS app layout

The macOS app never writes inside its bundle. It reads the loader, client
module, native plugins, shipped addons, official DAT package, and
`scripts/default.txt` seed from `Bahamut Launcher.app/Contents/Resources/`.
Writable files live in the state root, which is the launcher data directory
described above:

```text
~/.bahamut-launcher/
  config/                               <- every settings file listed above
  backups/  data/                       <- as in the portable layout, except WebView storage (below)
  logs/                                 <- launcher, chat, and packet logs, wine.log, helper.log
  addons/<addon-id>/                    <- player-installed addons
  plugins/dats/<package-id>/            <- player-installed DAT packages
  screenshots/
  scripts/default.txt                   <- copied from the shipped seed when missing
  prefix/  runtime/                     <- managed Wine prefix and engine
```

WebKit stores the WebView's session, theme, and gamepad data under
`~/Library/WebKit`, keyed by the bundle identifier, rather than under `data/`.

`extensions.ini` and `dats.ini` set the load order. If a player-installed addon
or DAT package uses a shipped package's ID, the launcher skips the player
package, logs a warning, and uses the shipped package. The official DAT package
loads only from `Contents/Resources/`.

A launcher binary outside an app bundle uses the
[Linux package layout](#linux-package-layout) if the package marker is beside
it, or the portable layout otherwise.

## Linux package layout

The Linux download includes `.bahamut-launcher-package` beside
`bahamut-launcher`. When this regular file is beside the running executable,
the launcher separates installed files from writable state, as the macOS app
does.

The executable's directory is the install root. It contains the loader, client
module, native plugins, shipped addons, official DAT package, and
`scripts/default.txt` seed. The launcher never writes there, so it can be a
read-only system directory. The state root is the launcher data directory
described above.

The launcher resolves symbolic links to its executable before finding the
install root. The `bahamut-launcher` command that
[`install.sh`](../packaging/linux/install.sh) links into `<prefix>/bin`
therefore finds its files in `<prefix>/lib/bahamut-launcher`.

```text
~/.bahamut-launcher/
  config/                               <- every settings file listed above
  backups/  data/                       <- as in the portable layout
  logs/                                 <- launcher, chat, and packet logs, wine.log, helper.log
  addons/<addon-id>/                    <- player-installed addons
  plugins/dats/<package-id>/            <- player-installed DAT packages
  screenshots/
  scripts/default.txt                   <- copied from the shipped seed when missing
  prefix/                               <- managed Wine prefix
  runtime/                              <- managed Wine engine and DXVK cache
```

As in the macOS app, a player package that reuses a shipped addon or DAT
package ID is skipped with a warning. The official DAT package loads only
from the install root.

In a folder you extracted yourself, removing `.bahamut-launcher-package`
restores the portable layout. Configuration, backups, logs, WebView data,
screenshots, scripts, and player packages then live beside the launcher.
The Wine prefix and DXVK cache stay in the launcher data directory in either
layout, with `wine.log` and `helper.log` in its `logs/` subdirectory. The
launcher does not move existing files between layouts. See
[Moving a beside-the-launcher tree](troubleshooting.md#moving-a-beside-the-launcher-tree).

Keep the marker in a copy installed by `install.sh`. The script replaces the
entire directory on the next install and deletes it on `--uninstall`. It
refuses either operation if the marker is missing, the directory contains
files the package does not ship, or a shipped file has changed.

### Linux Wine engine

On Linux x86_64, the first game launch downloads a pinned Wine engine to
`runtime/wine-<version>-<sha256>/` in the launcher data directory. This is
upstream Wine 11.18, built in WoW64 mode by
[Kron4ek/Wine-Builds](https://github.com/Kron4ek/Wine-Builds).

The download is 99,305,644 bytes and extracts to 836,996,788 bytes. Its size and
SHA-256 are pinned in
[`runtime_archive.rs`](../src/platform/runtime_archive.rs); both must match
before extraction. The WoW64 build includes the 32-bit Windows libraries the
client needs and no 32-bit host libraries. Extraction requires `tar` and `xz`.
The launcher checks for both before downloading and names any missing tool.

The engine loads these host libraries:

| Group | Libraries |
|---|---|
| Display and fonts | `libX11.so.6`, `libXext.so.6`, `libXcomposite.so.1`, `libXcursor.so.1`, `libXfixes.so.3`, `libXi.so.6`, `libXinerama.so.1`, `libXrandr.so.2`, `libXrender.so.1`, `libXxf86vm.so.1`, `libGL.so.1`, `libEGL.so.1`, `libxkbregistry.so.0`, `libfreetype.so.6`, `libfontconfig.so.1` |
| Wayland | `libwayland-client.so.0`, `libwayland-egl.so.1`, `libxkbcommon.so.0`, `libxkbregistry.so.0` |
| Audio | `libpulse.so.0` or `libasound.so.2` |
| Vulkan | `libvulkan.so.1` |
| Game controllers | `libudev.so.1`, `libSDL2-2.0.so.0` |

`install-dependencies.sh --check` reports missing display, font, and audio
libraries without changing its exit status.

The launcher chooses Wine in this order:

1. The file named by `BAHAMUT_WINE`, used as given.
2. The managed engine, on x86_64.
3. `wine` on `PATH`, if the engine cannot be installed or the host is not
   x86_64. The launcher log explains the fallback.

Installing a newer engine removes superseded `wine-*` directories that contain
the launcher's `.bahamut-sha256` marker. Directories without that marker are
kept, except the engine's own destination, which installation replaces.
`runtime/.wine-engine.lock` prevents concurrent installs. An install also
removes `.wine-stage-*` directories left by interrupted installs.

To download the engine again, delete `runtime/wine-*`. See
[Wine engine download](troubleshooting.md#wine-engine-download).

## `bahamut.ini`

Only the sections below are valid in `bahamut.ini`. Screenshot, plugin, addon,
and DAT settings belong in their dedicated files.

```ini
[launcher]
close_on_game_start = true
native_resolution_override = false
borderless_monitor =
game_location =
content_root =
download_cache_dir =

[game]
initialized = false
display_mode = windowed
width = 1280
height = 720

[game.graphics]
multisampling = none
general_quality = 8
background_quality = 5
shadow_detail = standard
ambient_occlusion = false
depth_of_field = false
cutscene_effects = true
hardware_mouse = true
texture_quality = standard
texture_filtering = standard

[game.audio]
enabled = true
play_in_background = false

[developer]
enable_verbose_wine_debug = false

[servers]
selected = Bahamut

[server.1]
display_name = Bahamut
host = bahamut.stegall.me
auth_port = 443
lobby_port = 54994
use_https = true

[server.2]
display_name = Bahamut Local
host = 127.0.0.1
auth_port = 8080
lobby_port = 54994
use_https = false
```

### Launcher options

Blank paths and endpoints mean automatic detection or the shipped default.

- `close_on_game_start` closes the launcher after a successful game start. It
  defaults to `true` if absent. If the launcher stays open, Play remains
  disabled until the client exits. The backend also rejects a second launch
  during startup or play.
- `game_location` records the selected game installation.
- `content_root` overrides the HTTPS host in the shipped delivery manifest.
  It does not change the trusted content identities.
- `download_cache_dir` overrides the verified complete client download cache
  location. Install and Repair default to a sibling of the game folder with
  ` Downloads` appended to its name: `D:\Games\FINAL FANTASY XIV` uses
  `D:\Games\FINAL FANTASY XIV Downloads`. Without a selected game folder,
  the fallback is `<Documents>/XIVLegacy_Downloads`, or the launcher data
  directory's `XIVLegacy_Downloads` when Documents is unavailable.
- `native_resolution_override` defaults to `false`. When enabled, Play writes
  the physical monitor resolution to retail `config.sys` without changing the
  saved `[game]` width and height. Those saved values remain the fallback when
  the override is Off. The resolution comes from the operating system, never
  frontend input.

In Borderless mode, the native resolution override uses the explicitly
selected Borderless Monitor. In other modes, or if no borderless destination
is selected, it uses the monitor containing the launcher window.

On Windows, General's Borderless Monitor choice saves an OS device identity
in `borderless_monitor`, not a list position. Blank means Default Monitor,
which places the borderless game on the display nearest its window. A
selection remains saved if the display disconnects; placement falls back to
the primary display, or the first available display if no primary is reported.
An explicit selection controls both borderless placement and the native
resolution override. The control is disabled in other display modes and is
unavailable on Linux and macOS.

The packaged loader and `bahamut.dll` are included launcher components, not
player configuration options. `news.toml` remains separate from writable
launcher state, so a private distribution can replace the announcement feed
without rebuilding the launcher.

### Game options and Settings pages

`[game]`, `[game.graphics]`, and `[game.audio]` contain the mapped retail client
settings. Either all three sections must be present or all three must be
absent.

Settings divides these controls as follows:

- General and Graphics edit the mapped game settings. General includes audio
  and hardware mouse options.
- Home handles the initial install selection and active installation:
  downloading, cancellation, progress, and recovery after completion or failure.
- Misc contains the Install Location path, manual User Settings and Macros
  backups, Extensions backups, folder actions for backups and launcher state,
  and the screenshots folder action.
- Gamepad contains launcher gamepad options. On Windows, XIV Config actions on
  both Gamepad and Misc open the installed retail configuration utility for
  options outside the launcher's mapped settings.

Valid values are:

| Setting | Values |
|---|---|
| Display mode | `windowed`, `borderless`, `fullscreen` |
| Multisampling | `none`, `2x`, `4x`, `8x` |
| General quality | 1 through 10 |
| Background quality | 1 through 5 |
| Other enum settings | Lowercase labels shown in the starter INI |

On Windows, Borderless saves the retail client's windowed value and enables
the packaged runtime's borderless behavior at launch. Linux and macOS start
Borderless as an ordinary window, including when they load the client module.

Saved width and height must be one of the retail utility's supported pairs:
`1024x768`, `1152x864`, `1280x720`, `1280x768`, `1280x800`, `1280x854`,
`1280x960`, `1280x1024`, `1360x768`, `1366x768`, `1368x768`, `1400x1050`,
`1440x900`, `1440x1050`, `1440x1080`, `1600x900`, `1600x1024`, `1600x1050`,
`1600x1200`, `1680x1050`, `1920x1080`, `1920x1200`, `1920x1440`, `2048x1536`,
`2560x1440`, `2560x1600`, or `2560x2048`. At launch, the native resolution
override may use a physical monitor size outside this list.

### Importing and writing retail settings

A new `bahamut.ini` starts with the verified retail defaults shown above.
If `<Documents>/My Games/FINAL FANTASY XIV/config.sys` exists and validates,
the launcher imports its mapped values, marks them initialized, and leaves
the retail file unchanged. Missing or invalid retail state leaves the defaults
pending with `initialized = false`. Later configuration loads retry the import
when a valid file appears.

Before creating a client process, Play validates an existing retail file and
writes initialized settings to its mapped words only. Every unmapped byte is
preserved. A changed file receives a known-good `config.sys.bak`; a save with
no changes rewrites neither file.

If `config.sys` is missing, Play creates it before starting the client. Mapped
words come from the INI's `[game]` values, every other word uses the documented
retail configuration utility default, and the screenshot directory is empty.
The INI is then marked initialized. Creation never replaces an existing file
and does not write a `.bak`.

An unexpected 684-byte layout or stamp, unsupported mapped values, an
unreadable path, or a failed write stops launch. The launcher does not fall
back to defaults in these cases. It writes mapped settings directly; opening
`ffxivconfig.exe` from Gamepad does not replace launch-time validation.

### Server profiles

Server sections must be contiguous from `[server.1]`. Display names must be
unique, and `[servers] selected` must exactly match one of them. `host` is a
bare hostname or IP address. `use_https` is explicit so a LAN host cannot
silently change protocols.

Use Profiles on the Home login card to select and edit a server. Login,
registration, session validation, and Play all use the same selected profile.

## Backups and restore

Misc creates compressed `UserSettings_` and `Extensions_` backups in the state
root's `backups/` folder. In the portable layout, this is
`<launcher-dir>/backups/`; the macOS app and Linux package use
`~/.bahamut-launcher/backups/`. Filenames include a sortable local timestamp.
The launcher keeps the newest five backups of each kind.

`Open Backup Folder` opens this directory. `Open Install Folder` opens the state
root, not the selected game directory or the install root.

### User Settings and Macros

These backups contain only the following known FFXIV 1.x files from
`<Documents>/My Games/FINAL FANTASY XIV`:

```text
config.lng
config.pad
config.rgn
config.sys
user/<eight-hex-character-id>/game
user/<eight-hex-character-id>/mcr0
user/<eight-hex-character-id>/ui
user/<eight-hex-character-id>/*.cmb
```

They exclude retail screenshots, per-character `log/` directories, unknown
files, and the launcher's `config.sys.bak` recovery file. Restore replaces only
files present in the newest matching backup. Excluded files and newer unrelated
files stay in place.

If the backup includes `config.sys`, restore also writes its mapped game
settings to the current `bahamut.ini`. This prevents the next Play from
replacing the restored choices with stale launcher values.

### Extensions

Extensions backups read the state root and include:

- Installed Lua addons under `addons/`
- Installed DAT packages under `plugins/dats/`
- `scripts/default.txt`
- `config/extensions.ini` and `config/dats.ini`
- Writable settings under `config/addons/` and `config/plugins/`

They exclude native DLLs, `bahamut.ini`, credentials, logs, screenshots,
download and cache data, WebView state, and packages shipped in the macOS app
or Linux package install root.

**Restoring Extensions removes packages and settings added after the backup
inside any directory that backup includes.** Each included directory is
restored as a snapshot. Excluded runtime files are left untouched.

### Restore safeguards

Restore selects the newest backup of the requested kind and asks for
confirmation. It cannot run while this launcher owns a starting or running
game, and it reserves the operation until it finishes. Close games and
configuration utilities started outside this launcher before restoring.

Backup, restore, and HUD reset run on workers so the window stays responsive.
Only one can run at a time. HUD reset also blocks game launch. Closing the
launcher waits for these operations and launch preparation to finish.

Restore stages files on the same volume, rejects entries outside the
allowlist, validates owned files, and keeps rollback storage until replacement
succeeds. If replacement fails, it restores the previous files. If rollback
also fails, it reports the recovery directory and keeps it. Configuration
writes use the same staged, atomic replacement, so failed reconciliation
leaves the prior configuration intact.

Before replacing live data, restore rejects symbolic links and Windows
reparse points at the destination root or along replacement paths inside it.
This check includes launcher settings when restored retail settings require
reconciliation. Parent folder aliases outside those roots are preserved.

## `extensions.ini`

```ini
[graphics]
object_distance_percent = 200
camera_zoom_limit = 15

[plugins]

[plugin.1]
id = screenshot
enabled = true

[plugin.2]
id = discord-rpc
enabled = true

[plugin.3]
id = object-distance
enabled = true

[plugin.4]
id = camera-zoom
enabled = true

[addons]

[addon.1]
id = chatlogs
enabled = true

[addon.2]
id = combatparser
enabled = false

[addon.3]
id = distance
enabled = true

[addon.4]
id = fps
enabled = true

[addon.5]
id = packetlogger
enabled = false

[addon.6]
id = pos
enabled = true

[addon.7]
id = targethp
enabled = true

[addon.8]
id = wiki
enabled = true

[addon.9]
id = zonename
enabled = true
```

Each numbered row records an installed or previously installed package.
`enabled` controls automatic loading on the next launch. Section order sets
load order within each package kind; toggling a package leaves its row in
place. IDs must be unique lowercase ASCII identifiers within that kind.

The shipped defaults enable every packaged plugin and addon except
Combatparser and Packetlogger. New addons without a row start disabled.
Existing selections are preserved, and rows for missing packages are ignored
at launch.

### Plugins and graphics controls

Supported plugin IDs are `screenshot`, `discord-rpc`, `object-distance`, and
`camera-zoom`. Other IDs are rejected: this file does not provide a public
native plugin loader or ABI.

Screenshot and DiscordRPC appear in Extensions and use packaged DLLs.
Extended Draw Distance and Extended Camera Zoom use optional hooks in the
client runtime and are controlled from Settings > Graphics:

- Draw Distance: Off, or 125, 150, 175, or 200 percent of the client's object
  cutoff
- Camera Zoom: Off, or a ceiling from 11 through 15

Each control is one stepped bar. A selection saves the plugin's `enabled` row
and its `[graphics]` value together, then applies on the next game launch.
Files without `[graphics]` use 200 percent and 15.

DiscordRPC publishes `BahamutXIV`, the copied character name, the English area
name for a known committed zone, and a session timer. If class or job and level
are available, it also sends the matching icon key and level tooltip. The
icon appears only if the asset is registered with the Discord application.
Unavailable fields are omitted. Logout or loss of player state clears the
character fields; the generic session timer may remain. Disabling the plugin
or closing the client clears the activity. Presence under Wine on Linux or
macOS is not expected. See [Platform support](extensions.md#platform-support).

## Screenshot settings

Screenshot stores its settings in `config/plugins/screenshot/settings.ini`:

```ini
[screenshot]
format = png
hide_overlays = true
hotkey = print_screen
```

- `format`: `png` or `bmp`
- `hide_overlays`: whether to omit Bahamut overlay content from the capture
- `hotkey`: `print_screen`, `insert`, or `f1` through `f9`, used as the
  bootstrap fallback

The launcher UI does not edit the key. Add
`/bind <key> /screenshot [hide]` to `scripts/default.txt` to choose a key for
the game session.

On Windows, the active key requests one capture of the final D3D9 backbuffer
and saves it under the state root's `screenshots/` folder. Capture is
unavailable if Screenshot is disabled in `extensions.ini` or the Bahamut
client module is missing. A failed save does not terminate the game.

Apple keyboards have no Print Screen or Insert key. On macOS, choose `f1`
through `f9` in `hotkey` or a `/bind` line; you may need to hold fn. `insert`
suits an external PC keyboard. See
[Platform support](extensions.md#platform-support) for capture status under
Wine.

## Client controls

The packaged `scripts/default.txt` binds F11 to `/fillmode` and F12 to `/fps`.
These fixed bindings work in the active game window and override the client's
original F11 and F12 actions. They are not a configurable hotkey system.

- F11 switches the game world between Solid and Wireframe. Addon content stays
  solid and readable. The selected mode survives a device reset, and turning
  Wireframe off restores normal rendering without restarting.
- F12 toggles the FPS addon if it is loaded. The key stays reserved and does
  nothing if the addon is disabled or unavailable.
- Hold Shift and drag unlocked addon content with the left mouse button to
  reposition it.

## `dats.ini`

See [Creating a DAT overlay](dat-overlays.md) for a package walkthrough.

```ini
[dats]

[dat.1]
id = example-pack
enabled = true
```

Numbered rows select DAT packages directly under `plugins/dats/`. Order is
first-match priority. Dats-Overlay is part of the launcher and has no enable
switch. An empty file selects no replacement packages. Selection and order
changes apply on the next launch.

Rows for removed packages are retained but ignored. Newly discovered packages
start disabled.

Each package is one direct child whose directory name matches its manifest ID:

```text
plugins/dats/example-pack/
  overlay.toml
  data/2A/08/00/17.DAT
```

The version 1 manifest defines identity and display metadata only:

```toml
manifest_schema_version = 1
id = "example-pack"
name = "Example Pack"
author = "Example Author"
version = "1.0.0"
description = "Replaces selected local DAT files."
homepage = "https://example.com/project"
```

`homepage` is optional; every other field is required. Unknown fields are
rejected. IDs contain 1-64 lowercase ASCII letters, digits, or hyphens.
`dats-overlay` is reserved for the launcher service.

The rest of the package mirrors normalized paths relative to the game.
A manifest cannot enable its package or choose its priority. Malformed
packages, duplicate IDs, unsafe relative paths, and trees containing links
or reparse points are rejected.

At launch, the launcher passes enabled package roots in INI order. The exact
client build uses the first existing package file for a safe relative request.
If none exists, it calls the original client open with its original path.
Installed DAT files are never copied or changed. Packages apply only when a
launch loads the client module; see
[Platform support](extensions.md#platform-support).

The Dats-Overlay detail editor shows package controls and match order. If
several enabled packages contain the same path, the first supplies it. The
launcher does not report overlaps.

## Portable launcher tree

In the portable layout, extension packages, writable state, logs, scripts,
and screenshots live beside the executable. You can move the whole launcher
directory without changing paths.

The macOS app keeps shipped files in `Contents/Resources/`; the Linux package
keeps them beside `bahamut-launcher`. Both put writable files in the state
root. See [macOS app layout](#macos-app-layout) and
[Linux package layout](#linux-package-layout).

```text
bahamut-launcher.exe
bahamut-loader.exe
bahamut.dll
addons/<addon-id>/addon.toml
addons/<addon-id>/<entry>.lua
config/addons/<addon-id>/settings.ini
config/bahamut.ini
config/dats.ini
config/extensions.ini
config/plugins/screenshot/settings.ini
logs/chat/YYYY-MM-DD.log
logs/packets/YYYY-MM-DD.csv
logs/launcher/bahamut-launcher.log
logs/launcher/bahamut-launcher.previous.log
plugins/dats/
plugins/dats/<package-id>/overlay.toml
plugins/dats/<package-id>/data/<DAT paths relative to the game>
plugins/discord-rpc.dll
plugins/screenshot.dll
screenshots/
scripts/default.txt
```

Each session starts a fresh `bahamut-launcher.log` and moves the previous
session's log to `bahamut-launcher.previous.log`. Help and Support copy only a
bounded portion of the current log and redact secrets before showing it in
the UI.

### Startup scripts

The launcher passes persistent selections from `extensions.ini` and
Screenshot settings directly to the injected runtime. It then runs the
optional `scripts/default.txt` in source order. Script commands affect only
that session and never rewrite persistent configuration.

Blank lines and lines whose first non-whitespace character is `#` are ignored.
Supported forms are:

- `/load screenshot`
- `/unload screenshot`
- `/bind <key> /screenshot [hide]`
- The fixed `/bind f11 /fillmode` and `/bind f12 /fps` overrides
- `/addon load <id>`
- `/addon unload <id>`
- `/addon reload <id>`

A Screenshot binding sets its key and whether overlays are hidden; it does
not enable the plugin. Addon commands can address any installed package,
including one not selected for automatic loading. A missing script is treated
as empty. Unknown packages or malformed commands stop launch before the
game's primary thread resumes and report the physical line number.

Other plugin command forms are reserved and rejected because there is no
native plugin package loader. `/screenshot` and `/fillmode` work only as
startup binding targets, not as retail chat commands. `/fps` uses the same
addon command from F12 and retail chat. The startup interface is separate from
addon commands typed in game chat.

### Addon discovery and settings

Every release includes the repository's addons under `addons/`, including
`chatlogs` for daily game chat logs and `wiki` for the Bahamut wiki and
MediaWiki search. When the client module loads, the launcher reads this
folder without copying or rewriting it. See
[Platform support](extensions.md#platform-support).

A valid `addon.toml` describes the package identity, metadata, compatibility,
and one Lua entry file. It cannot enable the addon or choose its load order.
`extensions.ini` supplies enabled manifests in row order. Duplicate IDs are
rejected and malformed manifests are omitted. Client build lists are display
metadata; the host's exact identity check decides whether the module can start.

Addon settings live under `config/addons/<id>/`. Shift-dragged positions are
shared by all characters in the installation and saved in
`config/addons/layout.ini`.

See also [Authentication](auth.md), [Handshake](handshake.md),
[Troubleshooting](troubleshooting.md), and the [documentation index](README.md).
