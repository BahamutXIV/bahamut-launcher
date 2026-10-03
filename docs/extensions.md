# Extensions

[Back to the documentation index](README.md)

Use Extensions to manage Lua addons, Screenshot, DiscordRPC, and DAT packages.
Extended Draw Distance and Extended Camera Zoom are in Settings > Graphics.
Selections apply on the next game launch.

The x86 Win32 client module hosts Lua addons. Screenshot and DiscordRPC are
separate DLLs in `plugins/`, Dats-Overlay is managed by the launcher, and the
two Graphics options use optional client hooks. The runtime's private native
ABI loads only Screenshot and DiscordRPC. It does not discover arbitrary
DLLs or provide a public plugin SDK.

The first sections cover player-facing behavior. [Package layout](#package-layout),
[Lua lifecycle](#lua-lifecycle), and [Host API](#host-api) are the addon author
reference.

Distance, Target HP, and Draw Distance credit AuroraFlare for their addon and
plugin designs.

## Client compatibility

The client module starts only when all three retail identity values match:

- `ffxivgame.exe` SHA-256:
  `9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9`
- `ffxivgame.exe` length: `15996808` bytes
- `game.ver`: `2012.09.19.0001`

An addon's `supported_client_builds` field is display metadata. It cannot
bypass this check or change which addons the player has enabled.

## Platform support

Every release includes `bahamut-loader.exe`, `bahamut.dll`, and the
repository's plugins and addons. Loading them depends on the platform:

| Platform | Client module at launch |
|---|---|
| Windows | Implemented. The loader starts the client and loads `bahamut.dll` as described in the [Handshake](handshake.md#windows-launch). |
| macOS | Implemented, unverified against a live client. If `bahamut-loader.exe` and `bahamut.dll` are in the launcher [install root](configuration.md#macos-app-layout), the loader runs under the managed Sikarugir Wine engine. The install root is `Contents/Resources` in the app or beside the executable in a portable tree. |
| Linux | The [Flatpak package](flatpak.md) supports SteamOS and Steam Deck. If `bahamut-loader.exe` and `bahamut.dll` are in the launcher [install root](configuration.md#linux-package-layout), the loader runs under the [managed Wine engine](configuration.md#linux-wine-engine) or the Wine selected by `BAHAMUT_WINE`. If the engine cannot be installed or the host is not x86_64, it uses system `wine`. The install root is beside the executable in both the Linux package and a portable tree. |

See [Wine extension launch](handshake.md#wine-extension-launch) for the launch
transaction, logs, and fallback when either loader file is missing.

On macOS and Linux:

- DiscordRPC uses the Windows named pipe `\\.\pipe\discord-ipc-N`. There is no
  bridge to native Discord, so presence is not expected to appear. This is
  unverified.
- Screenshot capture on macOS under Wine is unverified. Its default `print_screen`
  key is absent from Apple keyboards; on macOS, choose a function key as
  described in [Screenshot settings](configuration.md#screenshot-settings).
- Enabled DAT packages, including custom packages, apply whenever the launch
  loads the module.
- Launcher updates and the signed managed inventory cover Windows only.
  Shipped files in the macOS app and Linux package change when a newer release
  is extracted or installed. Put custom addons and DAT packages under
  `~/.bahamut-launcher`, not beside the executable. A custom package with a
  shipped package's ID is skipped.

## Packaged addons

Hold Shift and drag an unlocked overlay to move it. Addon-specific settings
persist under `config/addons/<id>/settings.ini`; positions are shared by all
characters in the launcher installation.

### FPS

FPS displays the host's rounded presentation rate as integer text in the upper
right corner. It starts red at size 13.

- `/fps` toggles the display and saves its visibility.
- `/fps lock` toggles whether Shift-dragging is allowed, saves the choice, and
  reports it in chat.
- `/fps help` lists the commands.

### Position

The Pos overlay appears when the addon is loaded and a player snapshot is
available. It starts at the lower left edge and displays
`X <value> | Y <value> | Z <value> | R <value> | Zone <value>`.

`/pos` writes and copies `{ X, Z, Y, rotation }, -- !pos X Y Z zone`, with
coordinates to three decimal places. The heading is converted from radians
to the server command's rounded `0..255` rotation.

- `/pos lock` toggles whether Shift-dragging is allowed, saves the choice, and
  reports it in chat.
- `/pos help` lists the position and lock commands.

### Distance

Distance displays the selected enemy, NPC, or friendly actor's horizontal X/Z
range in yalms, to one decimal place. It uses unboxed text and hides it when
targeting yourself or when the target's position is unknown. Positions observed
during zone entry are
retained for the matching player zone snapshot. Copied target and position
state is cleared on zone changes, logout, and target despawn.

- `/distance lock` saves whether Shift-dragging is allowed.
- `/distance help` lists the commands.

### Target HP

Target HP displays current HP, maximum HP, and percentage as unboxed text when
those values are known, and hides the text otherwise. Target and health state
are cleared on zone changes, logout, and actor despawn.

- `/targethp lock` saves whether Shift-dragging is allowed.
- `/targethp help` lists the commands.

### Text color and size

FPS, Distance, and Target HP accept `/fps color #RRGGBB` and `/fps size N`.
Replace `fps` with the addon's own command name. Color requires six hexadecimal
RGB digits; size must be an integer from 8 to 48. Each addon saves these
choices in its settings file. FPS starts red; Distance and Target HP start
white. All three start at size 13.

### Zone name

The `zonename` addon is enabled by default. It shows an uppercase region
heading above a larger italic map name in a shaded, centered panel with a
divider. The panel appears for six seconds, fading in over 0.4 seconds and out
over the final 1.4 seconds. It uses Georgia when installed, or the overlay
font otherwise. Shift-dragging moves the panel.

Unknown area names are not displayed. Local subarea labels are deferred.

### Wiki

- `/wiki` opens the Bahamut wiki home page.
- `/wiki <query>` searches its MediaWiki pages.
- `/wiki help` prints concise usage.

Search text is validated as UTF-8 and percent-encoded before reaching the
host. Both successful and rejected attempts report their status in chat.

### Chatlogs

Chatlogs records retail chat while loaded. It writes daily UTF-8 files under
`logs/chat/`, named `Character_Name_YYYY.MM.DD.log` using the current local
actor display name. Until a name is available, it uses `YYYY.MM.DD.log`.
The in-world display name can change during play and is not treated as an
immutable account identity. Each entry has a local `[HH:MM:SS]` timestamp.

Spaces in the filename become underscores, and characters forbidden by
Windows are replaced. If the named file cannot be opened, Chatlogs falls back
to the date-only filename.

Named dialogue is written as `Speaker: message`; system messages without a
source have no prefix. Chatlogs removes control characters and converts the
client's embedded line break marker to a normal line break.

### Packetlogger

Packetlogger is disabled by default. Enabling it starts a CSV capture session
under `logs/packets/`, named with the UTC date. Each row contains the host's
capture time, direction, frame size, and full frame hex. It does not emit or
modify outgoing packets.

**Captures may contain chat and other private game data. Keep them private
when sharing diagnostics.**

- `/packetlogger start` starts recording. Each start after a stop creates a
  new file.
- `/packetlogger stop` stops recording without disabling the addon. Frames
  already queued remain in their original session.
- `/packetlogger status` shows the current part filename, cumulative
  captured/written/dropped counts since enablement, current queue length, and
  highest observed queue length.

Each CSV part is capped at 16 MiB. Later parts use `-part0002.csv`,
`-part0003.csv`, and so on, with the same header. Rows are never split between
parts. Old parts are not deleted automatically. Disabling the addon stops
capture and discards queued messages. A callback fault stops recording.
See [Packet callbacks](#packet-callbacks) for capture limits.

### Combatparser

Combatparser is disabled by default. Its movable meter has no header, a dark
background, and a stable minimum width. The default DPS view ranks up to five
observed players by DPS and shows damage, DPS, accuracy, and class-colored
bars. When the selected mode has no positive results, the meter shows its
`DPS` or `HPS` heading.

- `/combatparser mode` switches between DPS and a separate healing view ranked
  by HPS.
- `/combatparser reset` clears accumulated player results.
- `/combatparser lock` saves whether Shift-dragging is allowed.
- `/combatparser` and `/combatparser help` list the commands.

While the addon stays loaded, results persist across fights, zone changes,
and temporary gaps in player state. Only recognized class or job IDs get a
class color; other rows use neutral bars. The current feed supplies class
and job IDs for the local actor only.

Queue overflow or bounded table capacity marks the display incomplete.
Displayed rates stay fixed between results. The rate denominator adds time
between result batches only when the gap is at most 10 seconds. Longer gaps
start separate active intervals while preserving totals.

The current Bahamut action-result sender returns player actions to the acting
session. Another player's actions therefore appear only if the server sends
those results to the observer. Healing counts positive results from known
Bahamut healing commands. Damage and accuracy count known hit, critical,
miss, evade, parry, and block text IDs, plus positive results without a text
ID from Bahamut's spell path. Unknown effects are ignored. The current feed
does not provide complete in-game coverage.

## DiscordRPC presence

DiscordRPC is a launcher plugin configured in `extensions.ini`. A bounded
worker sends a copied player snapshot to Discord's local RPC pipe with the
activity name `BahamutXIV`.

Player capture provides the character display name and combines bounded
incoming class, job, and level updates for the local actor. A known active
job takes precedence over the base class for the icon key and level tooltip.
The worker adds a session start timestamp. A committed zone ID selects the
English area name from the 1.23b client place name table. Unknown zones and
client placeholder names leave that line empty.

The session timestamp stays fixed across character and zone changes.
Unavailable fields are omitted rather than reused from an earlier character.
Logout or loss of player state clears character fields; the generic session
timer may remain. Disabling the plugin or closing the client clears the
activity. See [Platform support](#platform-support) for Wine limitations.

The native callback is a private launcher interface shared only by the
packaged Screenshot and DiscordRPC DLLs. It gives Lua addons no native ABI or
client memory API.

## Extended Draw Distance

Extended Draw Distance multiplies the supported 1.23b client's final object
distance result by 1.25x, 1.5x, 1.75x, or 2x. Off leaves it unchanged. Settings
> Graphics provides one stepped bar with Off and these multipliers. The
selection is saved in `config/extensions.ini` and applies on the next launch.

The native hook validates the retail function signature before installation.
It changes the object cutoff, not scenery LOD selection or server visibility.

## Extended Camera Zoom

Extended Camera Zoom raises the supported 1.23b client's camera distance
setter ceiling from 10 to a value from 11 through 15. Settings > Graphics
provides one stepped bar with Off and these ceilings. The selection is saved
in `config/extensions.ini` and applies on the next launch.

The hook validates the setter signature. Distances at or below 10 and
nonfinite inputs pass to the original setter. Larger finite inputs are capped
at the selected ceiling. The client's minimum and default distance are
unchanged, and the ceiling does not force the camera to that distance.
Zone, camera mode, and collision behavior at the extended range are outside
this contract.

## Package layout

Each addon is one direct child of the state root's `addons/` folder. In the
portable layout, that is `<launcher-dir>/addons/`. See
[macOS app layout](configuration.md#macos-app-layout) and
[Linux package layout](configuration.md#linux-package-layout) for the other
layouts.

```text
addons/
  example-addon/
    addon.toml
    main.lua
```

The launcher reads manifests without executing them when building Extensions.
It omits malformed manifests and those with a missing entry file. Duplicate
IDs within one addons root reject discovery rather than choosing a package.
In the macOS app and Linux package, a player addon with a shipped ID is
instead skipped with a warning, and the shipped addon is used.

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

`homepage` and `commands` are optional. All other fields are required.

- `id`: unique lowercase ASCII identifier, at most 64 characters, containing
  letters, digits, and hyphens
- `kind`: must be `addon`
- `entry`: one `.lua` file beside the manifest; paths and nested entries are
  rejected
- `api_version`, `minimum_runtime_version`, `supported_client_builds`,
  `capabilities`, and `commands`: metadata in the current host; they do not
  grant services or register arbitrary commands
- Each optional command record: nonempty `name`, `usage`, and `description`
  strings, with `name` beginning with `/`

The launcher passes enabled manifests to the module in `[addon.N]` row order
from `config/extensions.ini`. It also passes the installed manifest catalog,
so startup commands can load a package not selected for automatic loading.
Startup commands can append packages; unload and reload operations can change
the active order for that session. Packages cannot enable or reorder themselves.
See [Configuration](configuration.md#portable-launcher-tree) for startup
scripts and paths in each layout.

## Lua lifecycle

Each addon receives its own Lua 5.1 state. The host calls these globals if they
exist:

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

`update` receives elapsed frame time in seconds. `draw` runs on every rendered
frame while the addon is loaded. `chat` receives copied UTF-8 text on the next
update after retail inserts the line.

Reload closes the old state before loading its replacement. Disable closes
and removes the state. A load or callback error faults only that addon:
other addons keep running, the host stops calling the faulted addon's
callbacks, and `unload` is not called on the faulted state. The host attempts
to print a short error through the current chat sink.

### Packet callbacks

Only the enabled addon with ID `packetlogger` receives `packet`. It receives
copied, complete transport frames on a later update, never on the Winsock hook
thread.

- `direction` is `incoming` or `outgoing`.
- `capture_unix_ms_utc` is the host capture time in Unix-epoch milliseconds.
- `frame_hex` contains the original bytes as uppercase hex, including protocol
  headers and any compression.

The host observes synchronous `recv`, `WSARecv`, `send`, and `WSASend`
completions. It does not log asynchronous completions or unframed data.
The queue holds at most 256 frames and 4 MiB of data, delivers at most 32
frames per update, and drops new frames when full.

See [Packetlogger](#packetlogger) for recording commands, CSV parts, status,
and privacy warnings.

### Area changes

Only the enabled addon with ID `zonename` receives `area_changed`. After an
incoming `SetMap` message on the game connection, it receives the copied zone
ID, English area name, and English region heading. Region headings use
Bahamut's zone-to-region assignments; service and battle regions use `Eorzea`.

Repeated messages for the same zone, and transitions with the same displayed
region and map name, do not repeat the event. Unknown zone names are empty
and clear the popup.

### Chat command dispatch

The runtime recognizes the ASCII addon commands `/fps`, `/pos`, `/distance`,
`/targethp`, `/wiki`, `/packetlogger`, and `/combatparser`. Addons handle their
own arguments.

The host reads a bounded token vector from the retail parser. Malformed,
unsupported, overlong, or unreadable inputs pass to retail. Recognition occurs
only at the top-level retail lookup call; nested token lookups always pass
through.

The host calls each loaded addon's optional `command(command_name)` once in
load order until one returns `true`. A `true` result supplies the retail
unknown-command sentinel, which the validated retail path consumes without
sending the command as chat. A missing callback, `false`, or callback error
preserves the retail lookup path.

The command boundary is installed when any addon with a supported command is
loaded. Loading `chatlogs` also installs the existing retail chat insertion
boundary. The optional `BAHAMUT_TEST_COMMAND_BOUNDARY_RESULT_FILE` output is
an aggregate diagnostic snapshot.

## Host API

The global `bahamut` table exposes the functions below. The Wiki addon also
receives the bounded URL service.

### Settings and player state

- `bahamut.settings_get(key, fallback)` returns the stored string or the
  fallback.
- `bahamut.settings_set(key, value)` stores a string and returns whether the
  settings file was written successfully.
- `bahamut.player_state()` returns `nil` until a current player and committed
  zone are available. It then returns a copied table with numeric `x`, `y`,
  `z`, `rotation`, and `zone` fields. `zone` is the SetMap zone identifier,
  not its secondary argument. A pending zone change clears the old snapshot.
- `bahamut.target_distance()` is available only to the packaged `distance`
  addon. It returns the selected target's horizontal range, resolved name,
  and actor ID. Range is `nil` when targeting yourself or if the position is
  unavailable. All three values are `nil` if no target is selected.
- `bahamut.target_hp()` is available only to the packaged `targethp` addon.
  It returns current HP, maximum HP, resolved name, and actor ID. Both HP
  values are `nil` until a complete health update is available; all four are
  `nil` if no target is selected.
- `bahamut.combat_events()` is available only to the packaged `combatparser`
  addon. It drains up to 256 validated records with `source_id`, `amount`,
  `kind`, `source_name`, `source_class_id`, and `source_job_id` fields. Class
  and job IDs are available for the local actor; unknown actors use zero.
  It also returns the cumulative queue drop count and current local actor ID.
  Only known damage, healing, and miss outcomes enter the queue.

Setting keys contain 1-64 ASCII letters, digits, hyphens, underscores, or dots.
Values are limited to 1024 bytes and cannot contain line breaks. Settings are
saved in `config/addons/<id>/settings.ini`.

### Drawing

These functions submit content during `draw`:

- `bahamut.combat_meter(mode, rows, locked, incomplete)` is available only to
  the packaged `combatparser` addon. `mode` is `dps` or `hps`. Each row
  supplies a player name, metric total, rate, accuracy label, and `#RRGGBB`
  bar color. Empty rows show the selected mode heading. Position and
  Shift-drag locking use the same overlay layout as other addon windows.
- `bahamut.window(title, text, locked)` submits one host-owned text window.
  An empty title selects the compact headerless style. Combatparser initially
  anchors at mid-left; other compact windows start at the lower left edge.
  `locked = true` disables Shift-dragging. Otherwise, moved positions persist
  for the launcher installation.
- `bahamut.raw_text(text, red, green, blue, alpha, locked, size)` submits text
  with no window chrome or background. Color components are numeric ImGui
  RGBA values. Optional `size` is a font size from 8 to 48. Raw text starts
  at the upper right edge, except Distance at mid-right and Target HP at the
  upper left quarter. Distance and Target HP keep existing saved positions.
  `locked = true` disables Shift-dragging. Otherwise, moved positions persist
  for the launcher installation.

### Chat and packet logs

- `bahamut.chat_print(text)` writes nonempty UTF-8 text of at most 512 bytes
  through the pinned retail local-log insertion path. It returns whether the
  write succeeded, or `false` until retail supplies a live log receiver on
  the current command thread.
- `bahamut.chatlog_write(text)` is available only to the packaged `chatlogs`
  addon. It appends nonempty valid UTF-8 text of at most 8192 bytes to the
  current daily log after cleaning display control characters.
- `bahamut.packetlog_write(line)` is available only to the enabled addon with
  ID `packetlogger`, and only during its `packet` callback. It appends one
  printable ASCII CSV row of at most 131200 bytes to the frame's capture
  session part under `logs/packets/`, returning whether the write succeeded.
  It accepts no file path. Session files use the header
  `capture_unix_ms_utc,direction,size,frame_hex`.
- `bahamut.packetlog_start()` and `bahamut.packetlog_stop()` control recording
  for the enabled Packetlogger and return whether the operation succeeded.
- `bahamut.packetlog_status()` returns `recording`, `file`, `captured`,
  `written`, `dropped`, `queued`, and `queue_high_water`. The start, stop,
  and status functions are available only to the addon with ID `packetlogger`.

### Clipboard and URLs

- `bahamut.clipboard_set(text)` replaces the Windows Unicode clipboard while
  the game has the foreground window. It accepts nonempty valid UTF-8 text
  of at most 4096 bytes and returns whether the write succeeded.
- `bahamut.open_url(url)` opens valid UTF-8 HTTPS URLs of at most 2048 bytes
  with the exact origin `https://bahamut.miraheze.org`. Other schemes, hosts,
  ports, malformed URLs, and invalid UTF-8 return `false` without reaching
  the browser sink.

### Lua restrictions and package trust

The host opens only Lua's base, table, string, and math libraries. It removes
`dofile`, `loadfile`, `load`, `loadstring`, `print`, and `require`. It provides
no Lua operating system, I/O, package, debug, network, D3D, process, client
memory, or general packet APIs.

Packet observation and log writing are restricted by addon ID to
`packetlogger`. **The host does not authenticate addon files. Enabling a
replacement package with that ID grants it this access.**

## Unsupported surfaces

The API does not provide target or party adapters, chat modification, general
event subscriptions, callback budgets, raw input logging, arbitrary hooks,
packet mutation, or a public native plugin ABI.

The retail chat hook recognizes only the command patterns implemented in
`client/src/command_boundary.cpp`. Manifest metadata cannot add patterns or
make unsupported host services available.
