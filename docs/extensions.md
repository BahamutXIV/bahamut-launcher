# Extensions

[Back to the documentation index](README.md)

The x86 Win32 client module hosts Lua addons. Screenshot and DiscordRPC are
separate native DLLs under `plugins/`. Dats-Overlay is launcher-managed, while
Extended Draw Distance and Extended Camera Zoom are optional client hooks. The
runtime loads only Screenshot and DiscordRPC through a private native ABI. It
does not discover arbitrary DLLs or expose a public plugin SDK.

See [Platform support](#platform-support) for where the launcher loads these
components.

Distance, Target HP, and Draw Distance credit AuroraFlare for their addon and
plugin designs.

## Client compatibility

The client module starts only when all three retail identity values match:

- `ffxivgame.exe` SHA-256:
  `9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9`
- `ffxivgame.exe` length: `15996808` bytes
- `game.ver`: `2012.09.19.0001`

An addon's `supported_client_builds` field is display metadata. It does not
override this host identity check or the player's selection of enabled addons.

## Platform support

Every release archive carries `bahamut-loader.exe`, `bahamut.dll`, and the
repository's plugins and addons. Whether a launch loads them depends on the
platform:

| Platform | Client module at launch |
|---|---|
| Windows | Implemented. The loader starts the client and loads `bahamut.dll` as described in the [Handshake](handshake.md#windows-launch). |
| macOS | Implemented, unverified against a live client. When `bahamut-loader.exe` and `bahamut.dll` are in the launcher [install root](configuration.md#macos-app-layout) (`Contents/Resources` in the app, beside the executable in a portable tree), the loader runs under the managed Sikarugir Wine engine. |
| Linux | The [Flatpak package](flatpak.md) supports SteamOS and Steam Deck. When `bahamut-loader.exe` and `bahamut.dll` are in the launcher [install root](configuration.md#linux-package-layout) (beside the executable, in the Linux package and in a portable tree), the loader runs under the [managed Wine engine](configuration.md#linux-wine-engine), or the Wine that `BAHAMUT_WINE` selects. When the engine cannot be installed or the host is not x86_64, it runs under the system `wine`. |

The [Handshake](handshake.md#wine-extension-launch) defines the Wine launch
transaction, its logs, and the fallback used when either file is missing.

These limits apply on macOS and Linux:

- DiscordRPC connects to Discord's Windows named pipe
  `\\.\pipe\discord-ipc-N`. The launcher provides no bridge from that pipe
  to a native Discord client, so presence is not expected to appear. This is
  unverified.
- Screenshot's default hotkey is `print_screen`, which Apple keyboards do not
  have. On macOS, choose a function key as described in
  [Screenshot settings](configuration.md#screenshot-settings). Capture
  on macOS under Wine is unverified.
- Enabled DAT packages, including custom packages, apply to every macOS or
  Linux launch that loads the module.
- Launcher updates and the signed managed inventory cover the Windows package
  only. The shipped files inside the macOS app bundle and in the Linux
  package change only when a newer archive is extracted or installed. A
  custom addon or DAT package for the macOS app or the Linux package goes
  under `~/.bahamut-launcher` instead of beside the executable, and one whose
  id matches a shipped package is skipped.

## Package layout

Each addon occupies one direct child of the state root's `addons/` folder
(`<launcher-dir>/addons/` in the portable layout; see
[macOS app layout](configuration.md#macos-app-layout) and
[Linux package layout](configuration.md#linux-package-layout)):

```text
addons/
  example-addon/
    addon.toml
    main.lua
```

The launcher reads but does not execute manifests while building the Extensions
page. Malformed manifests and manifests whose entry file is missing are omitted.
Duplicate ids inside one addons root reject discovery rather than selecting
one package implicitly. In the macOS app and the Linux package, a player
addon that reuses a shipped id is skipped with a warning instead, and the
shipped addon is used.

The manifest schema is:

```toml
manifest_schema_version = 1
id = "example-addon"
kind = "addon"
name = "Example Addon"
author = "Example Author"
version = "1.0.0"
description = "Shows an example window."
homepage = "https://example.com/project"
entry = "main.lua"
api_version = "0.1"
minimum_runtime_version = "0.1.0"
supported_client_builds = ["2012.09.19.0001"]
capabilities = ["ui.draw"]
commands = []
```

`homepage` and `commands` are optional. Every other field is required.

- `id` is a unique lowercase ASCII identifier of at most 64 characters. It may
  contain letters, digits, and hyphens.
- `kind` must be `addon`.
- `entry` must name one `.lua` file beside the manifest. Paths and nested entry
  files are rejected.
- `api_version`, `minimum_runtime_version`, `supported_client_builds`,
  `capabilities`, and `commands` are metadata in the current host. They do not
  grant services or register arbitrary commands.
- Each optional command record has nonempty `name`, `usage`, and `description`
  strings, and `name` must begin with `/`.

The launcher passes enabled manifests to the client module in the order of
their `[addon.N]` rows in `config/extensions.ini`. This is the automatic load
order. It also passes the installed manifest catalog so client runtime startup
commands can load a package that was not selected automatically. Startup
commands can append another package, and unload or reload operations can change
the active order for that session. A package cannot enable or reorder itself.
See [Configuration](configuration.md#portable-launcher-tree) for startup
script semantics and the launcher tree's paths in each layout.

## Lua lifecycle

Every addon receives its own Lua 5.1 state. The host invokes these global
callbacks when they exist:

```lua
function load()
end

function update(delta_seconds)
end

function draw()
end

function command(command_name)
    return false
end

function chat(message)
end

function area_changed(zone_id, area_name, region_name)
end

function packet(direction, capture_unix_ms_utc, size, frame_hex)
end

function unload()
end
```

`update` receives the elapsed frame time in seconds. `draw` runs every rendered
frame while the addon is loaded. `chat` receives copied UTF-8 text on the next
update after retail inserts the line. `packet` is dispatched only to the enabled
addon whose id is `packetlogger`. It receives copied, complete transport frames
on a later update, not on the Winsock hook thread. Direction is `incoming` or
`outgoing`. The original frame bytes are uppercase hex, including protocol
headers and any compression. `capture_unix_ms_utc` is the host's capture time in
Unix-epoch milliseconds. The host observes synchronous
`recv`, `WSARecv`, `send`, and `WSASend` completions. Asynchronous completions
and unframed data are not logged. The packet queue holds at most 256
frames and 4 MiB of data, delivers at most 32 per update, and drops new
frames when full. Enabling `packetlogger` starts a CSV capture session. Its
`/packetlogger start`, `/packetlogger stop`, and `/packetlogger status` commands
control recording without changing addon enablement. Each start after a stop
creates a new file named with the UTC date under `logs/packets/`. Each CSV part
is capped at 16 MiB. Later parts use `-part0002.csv`, `-part0003.csv`, and so on,
with the same header. A row is never split between parts. Frames already
queued at stop remain in their original session. Status shows the current
part filename, cumulative captured/written/dropped counts since enablement,
the current queue length, and the highest observed queue length. Old parts are
not deleted automatically. A callback fault stops recording. The host attempts
to print a short error through the current chat sink. Reload closes the old
state before loading a replacement. Disable closes and removes the state.

`area_changed` is dispatched only to the enabled addon whose id is `zonename`.
It receives the copied zone ID, English area name, and English region heading
after an incoming `SetMap` message on the game connection. The region heading comes
from Bahamut's zone-to-region assignments. Service and battle regions use
`Eorzea`. Repeated messages for the same zone, or transitions with
the same displayed region and map name, do not repeat the event.
Unknown zone names are empty and clear the popup.

A load or callback error faults only that addon. The host stops invoking that
addon's callbacks while other addons keep running and does not call `unload` on
a faulted state.

The runtime recognizes the ASCII addon command names `/fps`, `/pos`,
`/distance`, `/targethp`, `/wiki`, `/packetlogger`, and `/combatparser`. Each
addon handles its own arguments.
The host reads a bounded token vector from the retail parser and forwards
malformed, unsupported, overlong, or unreadable inputs to retail. Recognition
is limited to the top-level retail lookup call. Nested token lookups always
pass through. It invokes each loaded addon's optional
`command(command_name)` callback once, in load order, until a callback
returns `true`. A callback returning `true` supplies the retail unknown-command
sentinel, which the validated retail path consumes without forwarding the
command as chat. A missing callback, `false` result, or callback error preserves
the retail lookup path.

The optional `BAHAMUT_TEST_COMMAND_BOUNDARY_RESULT_FILE` output is an aggregate
diagnostic snapshot. The command boundary is installed when any addon with a
supported command is loaded. The existing retail chat insertion boundary is
also installed when `chatlogs` is loaded.

The packaged FPS addon displays the rounded presentation rate observed by the
host as raw integer text, red by default, in the upper right corner. `/fps` toggles
that text and persists its visibility setting. `/fps lock` toggles and persists
whether Shift-dragging that overlay is allowed, and reports the new state in chat.
`/fps help` lists its commands.

The packaged Pos addon writes and copies
`{ X, Z, Y, rotation }, -- !pos X Y Z zone`, with coordinates to three decimal
places and the heading converted from radians to the rounded `0..255` rotation used by
the server command. `/pos help` prints the position and lock commands.
`/pos lock` toggles and persists whether Shift-dragging the position overlay is
allowed, and reports the new state in chat. The Position window appears when
the addon is loaded and a player snapshot is available. It initially anchors
at the lower left edge.
Its display is `X <value> | Y <value> | Z <value> | R <value> | Zone <value>`.
The `/pos` chat and clipboard output retain the server command format above.

The packaged Distance addon shows the selected enemy, NPC, or friendly actor's
horizontal X/Z range in yalms, to one decimal place, as unboxed text when its
position is known. It hides the text otherwise. Positions observed during
zone entry are retained for the matching player zone snapshot.
Its copied target and position state is cleared on zone changes, logout, and
target despawn. `/distance lock` persists whether Shift-dragging its overlay is
allowed. `/distance help` lists its commands.

FPS, Distance, and Target HP each accept `/fps color #RRGGBB` and `/fps size N`
with their own command name in place of `fps`. Color uses six hexadecimal RGB
digits. Size is an integer from 8 to 48. Each addon's choices persist in its
settings file. FPS starts red, while Distance and Target HP start white. All
three start at size 13.

The packaged Target HP addon shows current and maximum HP plus percentage as
unboxed text when those values are known. It hides the text otherwise. Target
and health state are cleared on zone changes, logout, and actor despawn.
`/targethp lock` persists whether Shift-dragging its overlay is allowed.
`/targethp help` lists its commands.

The packaged `zonename` addon is enabled by default. It displays an uppercase
region heading above the larger italic map name in a shaded, centered panel
with a divider. It shows for six seconds, fading in over 0.4 seconds and out
over the final 1.4 seconds. Georgia is used when installed. The overlay font
is the fallback. Shift-dragging can move the panel. It does not display an
unknown area name. Local subarea labels are deferred.

The packaged Wiki addon opens the Bahamut wiki home page with `/wiki`, searches
its MediaWiki pages with `/wiki <query>`, and prints concise usage with
`/wiki help`. Search text is UTF-8 validated and encoded with percent escapes
before the host receives it. Successful and rejected attempts report status in chat.

The packaged Chatlogs addon records retail chat while it is loaded. It writes
daily UTF-8 files named `Character_Name_YYYY.MM.DD.log` under `logs/chat/`,
using the current local actor display name. Until that name is available it
uses `YYYY.MM.DD.log`. The in world display name can change while the client is
running. It is not treated as immutable account identity. Each entry has a
local `[HH:MM:SS]` timestamp.
Spaces become underscores, and characters Windows forbids in filenames are
replaced. If a named file cannot be opened, Chatlogs uses the file named only
with the date.
Named dialogue uses the form `Speaker: message`. System messages without a source
remain unprefixed. It removes control characters and converts the client's
embedded line break marker to a normal line break.

The packaged Packetlogger addon is disabled by default. When enabled, it
writes observed transport frames to CSV sessions named with the UTC date under
`logs/packets/`.
Each CSV row contains the host capture time, direction, frame size, and
full frame hex. Captures can contain chat and other private
game data. Keep them private when sharing diagnostics. Disabling the addon
stops capture and discards its queued messages. No outgoing packet is emitted
or modified by this addon.

The packaged `combatparser` addon is disabled by default. Its headerless,
movable meter with a dark background keeps a stable minimum width and shows
the selected `DPS` or `HPS` heading while that mode has no positive results.
The default DPS view ranks up to five observed players by DPS, with damage, DPS,
accuracy, and bars colored by class. `/combatparser mode` toggles to a separate
healing view ranked by HPS, then back to DPS. Bare `/combatparser` lists the
commands.
While the addon remains loaded, player results remain across fights, zone
changes, and transient gaps in player state until `/combatparser reset` clears
them. `/combatparser lock` persists whether Shift-dragging is allowed, and
`/combatparser help` lists its commands.
Only recognized class or job IDs receive a class color. Rows without either
use a neutral bar. The current result feed supplies class and job IDs for the
local actor only. Queue overflow and bounded table capacity mark the display
incomplete. Displayed rates stay fixed between results. The denominator adds
time between result batches only when the gap is at most 10 seconds. Longer
gaps separate active intervals while keeping the accumulated totals.

The current Bahamut action-result sender returns player actions to the acting
session, so another player's actions will not appear until the
server delivers those results to the observer. Healing counts only positive
results from known Bahamut healing commands. Damage and accuracy count known
hit, critical, miss, evade, parry, and block text IDs, plus positive action
results with no text ID from Bahamut's spell path. Unknown effects are ignored.
The current feed does not claim complete in-game coverage.

## DiscordRPC presence

The launcher DiscordRPC plugin is configured through `extensions.ini`, not
through a Lua addon. It sends a copied player snapshot to Discord's local RPC
pipe from a bounded worker. The activity name is `BahamutXIV`. Current player
capture supplies the character display name and joins bounded incoming
class, job, and level updates for the local actor. A known active job takes
precedence over base class for the icon key and level tooltip. The worker
supplies a session start timestamp. A committed zone ID selects its English
area display name from the 1.23b client place name table. Unknown zones and
client placeholder names leave the area line empty.
The session timestamp is stable across character and zone changes. The plugin
omits unavailable fields rather than carrying them over from an earlier
character. Logging out or losing player state clears fields for the character.
The generic session timer may remain.
Disabling the plugin or shutting down the client clears the activity.
Presence under Wine is covered in [Platform support](#platform-support).

The native callback is a private launcher boundary shared only by the
packaged Screenshot and DiscordRPC DLLs. It does not grant Lua addons a native
ABI or an API for client memory.

## Extended Draw Distance

The optional Extended Draw Distance setting scales the supported 1.23b client's
final object distance result by a selected 1.25x, 1.5x, 1.75x, or 2x. It validates
the retail function signature before installing its native hook. Disabling the
setting leaves the client's object distance unchanged. Settings > Graphics offers
one stepped bar with Off and the supported multipliers. Its selection is saved in
`config/extensions.ini`
and applies on the next game launch. This changes the object cutoff, not scenery
LOD selection or server visibility.

## Extended Camera Zoom

Extended Camera Zoom raises the supported 1.23b client's camera distance setter
ceiling from 10 to a selected 11 through 15. The native hook validates the setter
signature and passes distances at or below 10, and nonfinite inputs, to the
original setter. It caps finite larger inputs at the selected ceiling without
changing the client's minimum or default distance. Settings > Graphics offers one
stepped bar with Off and the supported ceilings. Its selection is saved in
`config/extensions.ini`
and applies on the next game launch. The ceiling does not force the camera to that
distance. Zone, camera mode, and collision behavior at the extended range are
outside this contract.

## Host API

The global `bahamut` table provides the following functions. The Wiki addon
also receives the bounded URL service:

- `bahamut.settings_get(key, fallback)` returns a stored string or the fallback.
- `bahamut.settings_set(key, value)` stores a string and returns whether the
  settings file was written successfully.
- `bahamut.player_state()` returns `nil` until a current player and committed zone are
  available. Otherwise it returns a copied table with numeric `x`, `y`, `z`,
  `rotation`, and `zone` fields. `zone` is the SetMap zone identifier, not the
  secondary SetMap argument. A pending zone change clears the previous snapshot.
- `bahamut.target_distance()` is available only to the packaged `distance`
  addon. It returns the selected target's horizontal range, resolved name, and
  actor ID. The range is `nil` when its position is unavailable. All three
  values are `nil` when no target is selected.
- `bahamut.target_hp()` is available only to the packaged `targethp` addon.
  It returns current HP, maximum HP, resolved name, and actor ID. Both HP
  values are `nil` until a complete health update is available. All four
  values are `nil` when no target is selected.
- `bahamut.combat_events()` is available only to the packaged `combatparser`
  addon. It drains up to 256 validated result records with `source_id`,
  `amount`, `kind`, `source_name`, `source_class_id`, and `source_job_id` fields.
  Class and job IDs are available for the local actor. Unknown actors use zero.
  It also returns the cumulative queue drop count and the current local actor ID.
  Only known damage, healing,
  and miss outcomes enter the queue.
- `bahamut.combat_meter(mode, rows, locked, incomplete)` is available only to
  the packaged `combatparser` addon during `draw`. The mode is `dps` or `hps`.
  Each row supplies a player name, metric total, rate, accuracy label, and
  `#RRGGBB` bar color. Empty rows show the selected mode heading. Its position
  and Shift-drag lock use the same overlay layout as other addon windows.
- `bahamut.window(title, text, locked)` submits one text window owned by the host
  during `draw`. An empty title requests the compact headerless presentation. The
  combat parser initially anchors at mid-left. Other compact windows initially
  anchor at the lower left edge.
  When `locked` is true, Shift-dragging is disabled. Otherwise Shift-dragged
  positions persist for the launcher installation.
- `bahamut.raw_text(text, red, green, blue, alpha, locked, size)` submits text without
  visible window chrome or background during `draw`. Color components are numeric
  ImGui RGBA values. Optional `size` is a font size from 8 to 48. Raw text
  initially anchors at the upper right edge, except Distance at mid-right and
  Target HP at the upper left quarter. Both keep their existing saved
  positions. When `locked` is true, Shift-dragging is disabled. Otherwise
  Shift-dragged positions persist for the launcher installation.
- `bahamut.chat_print(text)` writes nonempty UTF-8 text of at most 512 bytes
  through the pinned retail local-log insertion path and returns whether it was
  written. It returns `false` until retail has supplied a live log receiver on
  the current command thread.
- `bahamut.chatlog_write(text)` is available only to the packaged `chatlogs`
  addon. It appends nonempty valid UTF-8 text of at most 8192 bytes to the
  current daily chat log after cleaning display control characters.
- `bahamut.packetlog_write(line)` is available only to the enabled addon whose
  id is `packetlogger`. It appends one printable ASCII CSV row of at most
  131200 bytes to the frame's capture session part under `logs/packets/` and
  returns whether the write succeeded. It is valid only during the `packet`
  callback and does not accept a file path. Session files use the
  `capture_unix_ms_utc,direction,size,frame_hex` header.
- `bahamut.packetlog_start()` and `bahamut.packetlog_stop()` control the
  enabled packetlogger's recording state and return whether the operation
  succeeded. `bahamut.packetlog_status()` returns `recording`, `file`,
  `captured`, `written`, `dropped`, `queued`, and `queue_high_water`. These
  functions are available only to the addon whose id is `packetlogger`.
- `bahamut.clipboard_set(text)` replaces the Windows Unicode clipboard with
  nonempty valid UTF-8 text of at most 4096 bytes while the game owns the
  foreground window, and returns whether it was written.
- `bahamut.open_url(url)` opens a valid UTF-8 HTTPS URL of at most 2048 bytes
  only when its exact origin is `https://bahamut.miraheze.org`. Other schemes,
  hosts, ports, malformed URLs, and invalid UTF-8 return `false` without
  reaching the browser sink.

Setting keys are 1 to 64 ASCII letters, digits, hyphens, underscores, or dots.
Values are at most 1024 bytes and cannot contain line breaks. Addon state is
kept under `config/addons/<id>/settings.ini`.

The host opens only Lua's base, table, string, and math libraries. It removes
`dofile`, `loadfile`, `load`, `loadstring`, `print`, and `require`. It does not
open Lua operating system, I/O, package, debug, network, D3D, process,
client memory, or general packet APIs. Packet observation and log writing are
restricted by addon id to `packetlogger`. The host does not authenticate addon
files. Enabling a replacement package with that id grants it this access.

## Unsupported surfaces

The current API does not provide target or party adapters, chat modification,
general event subscriptions, callback budgets, raw input logging, arbitrary
hooks, packet mutation, or a public native plugin ABI. The retail chat hook
recognizes only the command patterns implemented in `client/src/command_boundary.cpp`.
Manifest metadata alone cannot add another pattern.
Manifest metadata describing those concepts must not be treated as an
available host service.
