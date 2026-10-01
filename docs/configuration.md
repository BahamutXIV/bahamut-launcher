# Configuration

[Back to the documentation index](README.md)

In the portable layout, backend configuration lives beside the executable.
The Windows package uses this layout, as does any Linux or macOS launcher tree
without the Linux package marker:

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

In a portable Linux or macOS tree the executable is `bahamut-launcher`. The
rest of the layout is the same. The macOS app splits this tree, as described in
[macOS app layout](#macos-app-layout), and so does the Linux package, as
described in [Linux package layout](#linux-package-layout).
[Platform support](extensions.md#platform-support) defines which platforms load
the packaged loader and DLLs.

Each file has one writable responsibility. `bahamut.ini` owns launcher, mapped
game, developer, and server settings. `extensions.ini` owns plugin and addon
selection. `dats.ini` owns ordered DAT packages. Extensions own settings below
`config/plugins/` or `config/addons/`. Tracked starters live under
[`configs/`](../configs/). Missing live files are created from current defaults.
Saves validate the data, write a temporary file beside the destination, and
replace it only after a successful sync. A failed write preserves the previous
file.

On Windows the launcher uses no Bahamut-specific AppData roots, and WebView2
creates `data/EBWebView/` under the launcher's writable `data/` root. On macOS
and Linux the managed Wine prefix, `wine.log`, and `helper.log` live in the
launcher data directory: `$BAHAMUT_LAUNCHER_HOME` when that variable holds an
absolute path, otherwise `~/.bahamut-launcher/`. On Linux, `runtime/` in that
same directory holds the managed Wine engine and the verified DXVK cache; on
macOS it holds the managed Wine engine. Verified downloads and partial
transfers use the configured download cache folder.

Only the sections shown below are valid in `bahamut.ini`. Screenshot, plugin,
addon, and DAT settings belong in their dedicated files.

## macOS app layout

When the launcher runs from a macOS app bundle, it never writes inside the
bundle. It reads the loader, the client module, native plugins, shipped addons,
the official DAT package, and the `scripts/default.txt` seed from
`Bahamut Launcher.app/Contents/Resources/`. The launcher's own writable files
live under the state root, which is the launcher data directory above:

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

WebKit keeps the WebView's session, theme, and gamepad storage under
`~/Library/WebKit` keyed by the bundle identifier, not under `data/`.

Load order comes from `extensions.ini` and `dats.ini`. A player package that
reuses a shipped addon or DAT package ID is skipped with a warning, and the
shipped package wins that id collision. The official DAT package loads only
from `Contents/Resources/`. A launcher binary outside an app bundle follows
the [Linux package layout](#linux-package-layout) when the package marker sits
beside it, and the portable layout otherwise.

## Linux package layout

The Linux archive ships the marker file `.bahamut-launcher-package` beside
`bahamut-launcher`. When that regular file sits beside the running executable,
the launcher splits its roots like the macOS app. The executable's directory
is the install root, which holds the loader, the client module, native
plugins, shipped addons, the official DAT package, and the
`scripts/default.txt` seed. The launcher never writes there, so the install
root can be a read-only system directory. The state root is the launcher data
directory above. The launcher resolves symbolic links to its executable
first, so the `bahamut-launcher` command that
[`install.sh`](../packaging/linux/install.sh) links into `<prefix>/bin` finds
the payload in `<prefix>/lib/bahamut-launcher`.

The state tree matches the macOS app's:

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

A player package that reuses a shipped addon or DAT package ID is skipped with
a warning, and the official DAT package loads only from the install root, as
in the macOS app.

In a folder you extracted yourself, removing `.bahamut-launcher-package`
restores the portable layout for that tree: configuration, backups, logs,
WebView data, screenshots, scripts, and player packages live beside the
launcher again. The Wine prefix, the DXVK cache, `wine.log`, and `helper.log`
stay in the launcher data directory in both layouts. The launcher does not
move files between the two layouts; see
[Moving a beside-the-launcher tree](troubleshooting.md#moving-a-beside-the-launcher-tree).

Keep the marker in a copy that `install.sh` installed. `install.sh` replaces
that whole directory on the next install and deletes it on `--uninstall`. It
refuses to do either when the marker is missing, the directory holds entries
the package does not ship, or a shipped file is changed.

### Linux Wine engine

On Linux x86_64 the first game launch downloads a pinned Wine into
`runtime/wine-<version>-<sha256>/` in the launcher data directory: upstream
Wine 11.18, WoW64 build, as built by the
[Kron4ek/Wine-Builds](https://github.com/Kron4ek/Wine-Builds) project. The
download is 99,305,644 bytes and unpacks to 836,996,788 bytes. The size and
SHA-256 are pinned in
[`runtime_archive.rs`](../src/platform/runtime_archive.rs), and the archive
must match both before it is unpacked. The build is WoW64: it ships the 32-bit
Windows libraries the client needs and no 32-bit host libraries. Unpacking it
needs `tar` and `xz`; the launcher checks for both before it downloads and
names a missing one.

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

The launcher chooses a Wine in this order:

1. The file named by the `BAHAMUT_WINE` environment variable, used as given.
2. The managed engine, on an x86_64 host.
3. `wine` on `PATH`, when the engine cannot be installed or the host is not
   x86_64. The launcher log names the reason.

Installing a newer engine removes superseded `wine-*` directories that carry
the launcher's `.bahamut-sha256` marker. A `wine-*` directory without the
marker is kept, except the engine's own path, which an install replaces.
`runtime/.wine-engine.lock` serializes installs, and an install removes
`.wine-stage-*` directories that an interrupted install left behind. To
download the engine again, delete `runtime/wine-*`; see
[Wine engine download](troubleshooting.md#wine-engine-download).

## `bahamut.ini`

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

Blank launcher paths and endpoints mean "use automatic detection or the
shipped default." `close_on_game_start` closes the launcher only after the game
starts successfully and defaults to `true` when absent. When the launcher stays
open, Play remains disabled until that client process exits. The backend also
rejects a second launch during startup or play. `content_root` overrides the
HTTPS host in the shipped delivery manifest without changing trusted content
identities. The default download cache directory is
`<Documents>/XIVLegacy_Downloads`. Setting `download_cache_dir` replaces that
default. The packaged loader and `bahamut.dll`
are launcher components included by default rather than settings for players.
`native_resolution_override` belongs to the launcher and defaults to `false` when
absent. When enabled, Play uses the physical resolution of the explicitly
selected monitor in Borderless mode while preparing retail `config.sys`.
In other display modes, or with no borderless destination selected, it uses
the monitor holding the launcher window.
It does not change the saved `[game]` width and height, which remain the Off
fallback. The monitor resolution comes from the operating system and is not
accepted from frontend input.

On Windows, General's Borderless Monitor choice stores an OS device identity
in `borderless_monitor`, not a list position. Blank selects Default Monitor,
which uses the display nearest the game window for borderless placement. A
disconnected selection remains saved and falls back to the primary display,
then the first available display if no primary is reported. Explicit selection
controls both borderless placement and the native resolution override. The
choice is disabled in other display modes and unavailable on Linux and macOS.
`news.toml` remains separate from the writable launcher state. A private
distribution may replace that announcement feed without rebuilding the
launcher.

The `[game]`, `[game.graphics]`, and `[game.audio]` sections own the mapped
retail client switches. They must either all be present or all be absent.
Settings edits these sections through General and Graphics. Audio switches and
hardware mouse are part of General. Home owns initial install selection and
the active installation lifecycle: download, cancellation, progress, and
terminal recovery. The `download_cache_dir` configuration value remains an
optional override for the verified complete client download cache. Misc keeps the
Install Location path row, manual User Settings and Macros, Extensions backups,
the backup and state folder actions, and the action for the screenshots
folder. Launcher gamepad options remain on the dedicated Gamepad page. On
Windows, matching XIV Config actions on Gamepad and Misc open the installed
retail configuration utility for settings outside the launcher's mapped surface.
Display mode accepts `windowed`, `borderless`, or `fullscreen`. Multisampling
accepts `none`, `2x`, `4x`, or `8x`. On Windows, Borderless uses the retail
client's windowed configuration value and enables the packaged runtime's
borderless window behavior at launch. Linux and macOS start Borderless as an
ordinary window, including when either loads the client module. General
quality is 1 through 10,
background quality is 1 through 5, and the enum values use the lowercase
labels shown in the starter INI. Width and height must form one of the retail
utility's supported pairs.
Those pairs are `1024x768`, `1152x864`, `1280x720`, `1280x768`, `1280x800`,
`1280x854`, `1280x960`, `1280x1024`, `1360x768`, `1366x768`, `1368x768`,
`1400x1050`, `1440x900`, `1440x1050`, `1440x1080`, `1600x900`, `1600x1024`,
`1600x1050`, `1600x1200`, `1680x1050`, `1920x1080`, `1920x1200`,
`1920x1440`, `2048x1536`, `2560x1440`, `2560x1600`, and `2560x2048`.
The native override is the only exception at launch: it may write the
physical monitor size, including a pair outside this saved fallback list.

When a new `bahamut.ini` is created, it starts with the verified retail
defaults shown above. If the existing
`<Documents>/My Games/FINAL FANTASY XIV/config.sys` validates, the launcher
imports its mapped values into the new INI and marks them initialized without
rewriting the retail file. Missing or invalid retail state leaves the defaults
pending with `initialized = false`. A later config load retries the import
after a valid file appears. Play validates an existing retail file, and
initialized settings replace only the mapped words before any client process
is created. Every unmapped byte is preserved. A changed file receives a
known-good `config.sys.bak`. A save with no changes does not rewrite either file.

When no `config.sys` exists, Play creates one before the client starts: the
mapped words come from the INI `[game]` values, every other word carries the
documented retail configuration utility default, the screenshot directory is
left empty, and the INI is marked initialized. Creation never replaces an
existing file and writes no `.bak`.
An unexpected 684-byte layout or stamp, unsupported mapped values, an
unreadable path, and failed writes stop launch instead of falling back to
defaults. The launcher writes its mapped settings directly. Opening
`ffxivconfig.exe` from the Gamepad page does not replace validation during launch.

## Backups and restore

Misc creates compressed `UserSettings_` and `Extensions_` backups under
`backups/` in the state root: `<launcher-dir>/backups/` in the portable layout
and `~/.bahamut-launcher/backups/` in the macOS app and the Linux package.
Names include a sortable local timestamp. The launcher keeps the newest five
of each kind.
`Open Backup Folder` opens that directory. `Open Install Folder` opens the state
root, not the selected game directory or the install root.

User Settings and Macros backups contain only these known FFXIV 1.x files from
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

Retail screenshots, per-character `log/` directories, unknown files, and the
launcher's `config.sys.bak` recovery sidecar are excluded. Restore replaces
only files present in the newest matching archive, so excluded and newer unrelated
files remain in place. When the archive contains `config.sys`, its mapped game
values are also written to the current `bahamut.ini`. The next Play therefore
does not overwrite the restored retail choices with stale launcher values.

Extensions backups read the state root. They contain installed Lua addons under
`addons/`, installed DAT packages under `plugins/dats/`, `scripts/default.txt`,
`config/extensions.ini`, `config/dats.ini`, and writable settings below
`config/addons/` and `config/plugins/`. They exclude native DLLs, `bahamut.ini`,
credentials, logs, screenshots, download and cache data, WebView state, and the
shipped packages in the install root of the macOS app or the Linux package.
Extensions restore treats each included directory as a snapshot: packages or
settings added below one of those owned directories after the backup are
removed when that archive is restored. Excluded runtime files remain
untouched.

Restore selects the newest archive of the requested kind and requires
confirmation. It is blocked while this launcher owns a starting or running
game, and it reserves the operation until complete. Close games or configuration
utilities started outside this launcher first. Backup, restore, and HUD reset
run on workers while the window stays responsive, with only one such operation
at a time. HUD reset also reserves against game launch. Closing the launcher
waits for them and for launch preparation.
Restore stages data on the same volume, rejects entries outside the allowlist,
validates owned files, and keeps rollback storage until replacement succeeds.
A failed replacement restores prior files. A failed rollback reports its
recovery directory instead of deleting it. Configuration writes use the same
staged, atomic replacement, so a failed reconciliation leaves the prior file
intact.

Before replacing live data, restore rejects symbolic links and Windows reparse
points at the destination root or along replacement paths inside it. This
check also covers launcher settings when restoring retail settings requires
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
load order within each package kind, and toggling a package does not reorder
its row. IDs must be unique lowercase ASCII identifiers within that kind. The
shipped defaults enable every packaged plugin and addon except Combatparser and
Packetlogger. New addons without a row are disabled. Existing selections stay
in place, while rows for missing packages are ignored at launch.

The supported `extensions.ini` plugin IDs are `screenshot`, `discord-rpc`,
`object-distance`, and `camera-zoom`. Screenshot and DiscordRPC appear in
Extensions. Extended Draw Distance and Extended Camera Zoom are controlled from
Settings > Graphics.
Settings presents one stepped bar per control: Off or a Draw Distance multiplier of
125, 150, 175, or 200 percent of the client's object cutoff. Off or an extended
camera zoom ceiling from 11 through 15. Each selection updates its plugin's
`enabled` row and the corresponding `[graphics]` value together. Existing
files without `[graphics]` use 200 percent and 15. Selections apply on the next
game launch.
Other plugin IDs are rejected because this
file does not create a public native plugin loader or ABI. Screenshot and
DiscordRPC use packaged DLLs. The two Graphics controls use optional hooks inside
client runtime. DiscordRPC publishes `BahamutXIV`, the copied character name,
the English area name for a known committed zone, and a session timer. When
class or job and level are available, it also sends
the corresponding icon key and level tooltip. The icon requires that asset to
be registered with the Discord application. Missing state is omitted. Logging
out or losing player state clears fields for that character. The generic
session timer may remain. Disabling the plugin or shutting down the client
clears the activity. Presence under Wine on macOS or Linux is not expected. See
[Platform support](extensions.md#platform-support).

## Screenshot settings

`config/plugins/screenshot/settings.ini` owns Screenshot behavior:

```ini
[screenshot]
format = png
hide_overlays = true
hotkey = print_screen
```

`format` accepts `png` or `bmp`. `hide_overlays` controls whether Bahamut
overlay content is omitted from the captured frame. `hotkey` accepts
`print_screen`, `insert`, or `f1` through `f9` as the bootstrap fallback. The
launcher UI does not edit the key. Put `/bind <key> /screenshot [hide]` in
`scripts/default.txt` to select the key for a game session. On Windows, the
active key requests one capture from the final D3D9 backbuffer and writes it
under the state root's `screenshots/` directory. Capture is unavailable when
Screenshot is disabled in `extensions.ini` or the Bahamut client module is
missing, and a failed write does not terminate the game. Apple keyboards have
no Print Screen or Insert key, so on macOS choose one of `f1` through `f9` in
`hotkey` or a `/bind` line. macOS may require holding fn for a function key.
`insert` suits an external PC keyboard.
[Platform support](extensions.md#platform-support) records capture
status under Wine.

## Client controls

The packaged `scripts/default.txt` binds F11 to `/fillmode` and F12 to
`/fps`. These fixed active-window bindings override the client's original
F11 and F12 actions. They are not a configurable hotkey system.

F11 switches game-world rendering between Solid and Wireframe. Addon content
remains solid and readable. A device
reset preserves the selected mode. Disabling Wireframe restores ordinary solid
rendering without restarting the client.

F12 toggles the FPS addon when it is loaded. If the addon is disabled or
unavailable, the key remains reserved and does nothing. Hold Shift and drag
unlocked addon content with the left mouse button to reposition it.

## `dats.ini`

For a complete package walkthrough, see
[Creating a DAT overlay](dat-overlays.md).

```ini
[dats]

[dat.1]
id = example-pack
enabled = true
```

Numbered DAT rows select packages directly under `plugins/dats/`. Their order
uses priority based on first match. Dats-Overlay is part of the launcher and has no
enable switch. An empty file selects no replacement packages. Enable and order changes
take effect on the next launch. Rows for removed packages are retained but
ignored, and newly discovered packages begin disabled.

Each package is one direct child whose directory name matches its manifest ID:

```text
plugins/dats/example-pack/
  overlay.toml
  data/2A/08/00/17.DAT
```

The version 1 manifest owns identity and presentation metadata only:

```toml
manifest_schema_version = 1
id = "example-pack"
name = "Example Pack"
author = "Example Author"
version = "1.0.0"
description = "Replaces selected local DAT files."
homepage = "https://example.com/project"
```

`homepage` is optional. IDs use 1-64 lowercase ASCII letters, digits, or
hyphens. `dats-overlay` is reserved for the launcher service. All other fields
are required and the manifest rejects unknown fields.
The remaining package tree mirrors normalized paths relative to the game.
Package manifests cannot enable or order themselves.

The launcher rejects malformed packages, duplicate IDs, unsafe relative paths,
and package trees containing links or reparse points. At launch it passes only
enabled package roots in INI order. The exact client build substitutes
the first existing package file for a safe relative request and otherwise calls
the original client open with its original path. It never copies or modifies
the installed DAT tree. Packages apply only to launches that load the client
module, as listed in [Platform support](extensions.md#platform-support).
The Dats-Overlay detail editor shows package controls and the match order. If
enabled packages contain the same path, the first package in match order
supplies it; the launcher does not report path overlaps.

Server sections must be contiguous from `[server.1]`, display names must be
unique, and `[servers] selected` must match one display name exactly. `host` is
a bare hostname or IP. `use_https` is explicit so a LAN host cannot silently
switch protocols. Profiles, reached from the Home login card, owns selection
and editing. Login,
registration, session validation, and Play all resolve the same profile.

## Portable launcher tree

In the portable layout, extension packages, writable state, logs, scripts, and
screenshots live beside the executable. The whole launcher directory can move
without changing paths. The macOS app keeps the shipped files of this tree in
`Contents/Resources/` and the Linux package keeps them beside
`bahamut-launcher`. Both keep the writable files under the state root, as
described in [macOS app layout](#macos-app-layout) and
[Linux package layout](#linux-package-layout):

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

Each session starts a fresh `bahamut-launcher.log` and moves the preceding
transcript to `bahamut-launcher.previous.log`. Help and Support copy only a
bounded portion of the current log and apply secret redaction at the UI boundary.

The launcher passes the persistent selection from `extensions.ini` and
Screenshot settings directly to the injected runtime. It then executes the
optional `scripts/default.txt` in source order. These commands affect only the
current session and never rewrite persistent configuration.

Blank lines and comments whose line contains only `#` are ignored. Supported forms are
`/load screenshot`, `/unload screenshot`, `/bind <key> /screenshot [hide]`,
the fixed `/bind f11 /fillmode` and `/bind f12 /fps` overrides,
`/addon load <id>`, `/addon unload <id>`, and `/addon reload <id>`. A Screenshot
binding changes its key and whether overlays are hidden. It does not enable the
plugin by itself. Addon commands may address any installed package,
including one not selected for automatic loading. Missing files are treated as
an empty script. Unknown packages and malformed commands terminate the launch
before the game's primary thread resumes and report the physical line.

Other plugin command forms remain reserved and rejected because no native
plugin package loader exists. `/screenshot` and `/fillmode` are
implemented only as startup binding targets. Typing them into retail chat is
not supported. `/fps` uses the same addon command whether invoked from F12 or
retail chat. This startup interface is separate from addon commands entered in
game chat.

Every release archive carries repository addons under `addons/`, including
`chatlogs` for daily game chat logs and `wiki` for the Bahamut wiki and
MediaWiki search. When the client module loads (see [Platform support](extensions.md#platform-support)),
the launcher reads that directory without copying or rewriting it. A valid `addon.toml`
describes package identity, metadata, compatibility, and one Lua entry file.
It cannot enable itself or choose its load position. `extensions.ini` supplies
the enabled manifests in row order. Duplicate IDs are rejected and malformed
manifests are omitted. Client build listings remain display metadata. The host's
exact identity gate decides whether the module may start. Addon settings live
under `config/addons/<id>/`. Shift-dragged positions are shared by all
characters in the installation and persist in `config/addons/layout.ini`.

See also [Authentication](auth.md),
[Handshake](handshake.md),
[Troubleshooting](troubleshooting.md), and the
[documentation index](README.md).
