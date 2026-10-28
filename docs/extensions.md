# Extensions

[Back to the documentation index](README.md)

Use Extensions to manage Lua addons, Screenshot, DiscordRPC, and DAT packages.
Extended Draw Distance and Extended Camera Zoom are in Settings > Graphics.
Selections apply on the next game launch. In game, `/addon` and `/load`
change loaded addons and plugins for the current session; see
[Loading in game](#loading-in-game).

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

## Loading in game

These chat commands work in every session where the client module starts:

| Command | Effect |
|---|---|
| `/addon load <id>` | Loads an installed addon by its manifest `id`. |
| `/addon unload <id>` | Unloads a loaded addon. |
| `/addon reload <id>` | Reloads a loaded addon, or loads it if it is not loaded. |
| `/addon list` | Lists loaded addons in load order; faulted addons are marked. |
| `/load <plugin>` | Loads or re-enables `screenshot` or `discord-rpc`. |
| `/unload <plugin>` | Disables the plugin. |
| `/plugins` | Lists both plugins and whether each is loaded. |

Any installed addon can be loaded, including one not selected in Extensions.
Changes last until the game closes; they never rewrite `extensions.ini`, and
the next launch uses the Extensions selection and the
[startup script](configuration.md#startup-scripts). DAT packages and the
Graphics options are applied at launch and have no in-game command.

Commands take effect immediately, even when chat feedback is unavailable.
When available, chat shows results such as `Loaded addon: fps`,
`Addon is not installed: foo`, or `fps is an addon; use /addon load fps`.
Other `/addon` and `/load` forms print usage.

Unloading an addon closes its Lua state and stops its overlay and commands.
Native services an addon uses stay installed and pass through: the addon's
behavior ends, but the host does not remove client hooks during play.
Unloading a plugin disables it without unloading its DLL; DiscordRPC clears
the activity and Screenshot stops reacting to its key.

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

### Enemy Cast

Enemy Cast (`enemycast`) is experimental and disabled by default. Enable it
in Extensions before the next game launch. Existing saved selections are
preserved. `/enemycast lock` saves whether Shift-dragging is allowed.

The overlay shows up to four copied enemy cast or engaged-idle rows. Unfinished
casts take priority, followed by completed casts and then idle rows. Names fall
back to an actor ID and `Ability <command ID>` when unknown. Incoming battle
results supply cast-start, completion, and interruption cues. A matching
interruption clears the cast; an engaged enemy can remain as an idle row.
Passive/dead state, actor removal or ID reuse, and the implementation's despawn
message clear that actor's rows. Clearing all actors, player identity or zone
changes, and unavailable player state clear all rows.

Progress is an estimate from the embedded Bahamut command catalog, with a
3500 ms fallback and a minimum visible duration of 1800 ms. `Done` can mean
either a matching completion message or that the estimate elapsed; it does
not prove an observed cast duration. Completed rows remain for five seconds.
Engaged-idle rows expire after fifteen seconds without relevant traffic.
Live cast timing, interruption, actor reuse, transitions, and Wine behavior
remain unverified. See [Packet evidence](#packet-evidence) for the opcode limits.

### Custom Timer

Custom Timer (`customtimer`) is experimental and disabled by default. Enable
it in Extensions before the next game launch. Existing saved selections are
preserved. `/customtimer lock` saves whether Shift-dragging is allowed.

It displays one title, objective, and countdown from Bahamut-specific tagged
incoming GenericData messages. It does not infer duty timers from chat or
ordinary retail GenericData traffic. An open replaces the current timer;
an objective update preserves its deadline; a close or elapsed deadline hides
it. Disabling the addon hides its overlay but does not clear the native timer.
The service has no zone/logout reset, so matching server close events are
required to avoid showing a still-active timer after a transition or reload.
Tagged delivery, duty cleanup, and transition behavior require live acceptance.

The launcher accepts these application payloads, up to 192 bytes:

| Bahamut tag | Parameters after the tag |
|---|---|
| `0x49575401` | Open: text and positive finish timestamp in Unix seconds |
| `0x49575402` | Close: no parameters |
| `0x49575403` | Update: objective text |

Integers use type byte `0x00` and a signed 32-bit big-endian value. Strings
use type byte `0x02`, valid UTF-8 of at most 64 bytes, and a NUL terminator.
The parameter list ends with `0x0F` and optional zero padding. Unknown tags,
malformed parameters, and truncated input leave the current timer unchanged.
Open text may use `title|objective`; absent or empty components use
`Instanced Content` and `Custom Objective` as appropriate. Whitespace is
normalized. Remaining time starts at finish time minus the message timestamp,
then follows a monotonic clock and rounds up to whole seconds. These tags,
parameters, and limits are the [launcher consumer contract](../client/src/custom_timer.cpp),
not retail timer facts.

### Packet evidence

The catalog reference is
[`XIVLegacy/xivl-opcodes:opcodes.json`](https://github.com/XIVLegacy/xivl-opcodes/blob/main/opcodes.json),
in `MapClientbound`. Its `0x0133` GenericData record identifies the carrier;
it does not establish Bahamut's timer tags. The `0x0134` actor-state byte is
documented in
[`XIVLegacy/xivl-opcodes:data/achievement_lifecycle.json`](https://github.com/XIVLegacy/xivl-opcodes/blob/main/data/achievement_lifecycle.json),
record `s2c-0134-actor-state`.

The `0x0139` through `0x013C` battle-result routes and row capacities are
documented in
[`XIVLegacy/xivl-opcodes:data/battle_result_semantics.json`](https://github.com/XIVLegacy/xivl-opcodes/blob/main/data/battle_result_semantics.json),
the `rows` records for X01, X10, X18, and X00. The X18 (`0x013B`) record is
static evidence with no retained capture. These records do not prove the
addon's cast-start heuristics or duration estimates. Synthetic
service tests establish consumer behavior, not live retail acceptance.

### Zone name

The `zonename` addon is enabled by default. It shows an uppercase region
heading above a larger italic map name in a shaded, centered panel with a
divider. The panel appears for six seconds, fading in over 0.4 seconds and out
over the final 1.4 seconds. It uses Georgia when installed, or the overlay
font otherwise. Shift-dragging moves the panel.

Unknown area names are not displayed. Local subarea labels are unavailable.

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

Packetlogger is enabled by default on new installs; existing selections are
kept. While enabled, each game launch starts a CSV capture session under
`logs/packets/`, named with the UTC date. Turn it off on the Extensions page.
Each row contains the host's capture time, direction, frame size, and frame
hex or a stub. It does not emit or modify outgoing packets.

**Captures contain in-game names, positions, and party data. Keep them private
when sharing diagnostics.**

#### Capture scope

By default Packetlogger records only game-world connections. The host
identifies the connection from its first frames; the lobby and login
connection is never written. Frames seen before the connection is identified
are skipped and counted as `skipped`. Game-world frames are written as full hex
rows, except that handshake and control records, chat, tell, party, and
linkshell messages, mail-compose messages, search profile updates, and
search results with comments are written as stub rows. A stub row keeps the
original frame size and carries only item markers instead of bytes:

```text
1700000000001,outgoing,77,stub:op=0003
1700000000002,incoming,310,stub:op=00C8;t=0001
```

The stub payload is `stub:` followed by `;`-separated items in frame order:
`op=XXXX` for a game message with that opcode and `t=XXXX` for any other
subpacket type, each as four uppercase hex digits. Stub rows count as written.

Setting `capture_scope=full` in `config/addons/packetlogger/settings.ini`
records every observed TCP frame as hex, including the login connection and
the session ticket. Use it only when a developer asks for it, and do not share
those captures. The scope is read when the addon loads or reloads.

- `/packetlogger start` starts recording. Each start after a stop creates a
  new file.
- `/packetlogger stop` stops recording without disabling the addon. Frames
  already queued are written to their original session first.
- `/packetlogger split` closes the current part with a `split` end row and
  continues in the next part of the same session, with the same session id and
  new meta rows. It does nothing while capture is stopped.
- `/packetlogger status` shows the current part filename (with the part number
  after the first part), cumulative captured/written/dropped counts since
  enablement, current queue length, highest observed queue length, and the
  number of frames skipped by the capture scope.

The host writes rows on its own writer thread, so the addon does no per-frame
work. Rows reach the file within about 250 ms. A session file can be read while
recording. Each CSV part is capped at 8 MiB. Later parts use `-part0002.csv`,
`-part0003.csv`, and so on, with the same header and meta rows, which count
toward the cap. Rows are never split between parts. Disabling the addon stops
capture after the queued frames are written.

When capture starts, the first update after chat accepts it prints `Packet
capture: on, logs/packets, <N> MiB used. Pause with /packetlogger stop. Turn
off on the launcher Extensions page`, where `<N>` is the size of the capture
files rounded to whole MiB. The notice is retried each update until chat
accepts it, then not printed again for that load of the addon.

The first Lua fault in Packetlogger closes the session with a `fault` end row,
restarts the addon in a new session, and prints `Packet capture: recording again
after an addon error`. A second fault stops recording and prints `Packet capture:
stopped after an addon error`. Enabling, reloading, or disabling the addon resets the
count. The restart runs at the end of the host update on the game thread;
packet hooks never call into Lua.

Captures under `logs/packets/` are limited to 256 MiB in total and the newest
10 sessions. When a session starts or a part rotates, the host deletes the
oldest whole sessions first. If the active session alone exceeds 256 MiB, its
oldest parts are deleted. The active part is never deleted. The host deletes
only files that match its own capture names, never directories or links, and
skips pruning if `logs/packets` is itself a link. A session that wrote no
frames is removed when it stops, and older header-only single-part sessions
are removed at the next start. A file that cannot be deleted is skipped.

If a write fails, capture continues once in a fresh session and prints
`Packet capture: recording again after a save error`. A second failure stops
recording and prints `Packet capture: stopped after a save error`.
See [Packet callbacks](#packet-callbacks) for capture limits.

### Combatparser

Combatparser is enabled by default on new installs. Its movable meter has no
header, a dark background, and a stable minimum width. The default DPS view
ranks up to five observed players by DPS and shows damage, DPS, accuracy, and
class-colored bars. When the selected mode has no positive results, the meter
shows its `DPS` or `HPS` heading.

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

### Targetlines

Targetlines is disabled by default. Enable it in Addons for the next game
launch. It draws a raised glowing red curve to the selected recognized
mob and a green curve during a supported local heal or buff cast on another
ally. Self and ordinary friendly selection draw no curve. The green curve
keeps the accepted cast target when selection changes, fades on completion,
and disappears on interruption.

Targetlines supports reviewed Bahamut mob spawn profiles and known healing
or buff commands with a cast time. Unknown profiles and instant abilities
are outside its coverage. It uses the shared overlay visibility control
and has no slash commands or adjustable visual settings. See the
[native Targetlines contract](../client/README.md#targetlines) for projection and testing limits.

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
distance result by 1.25x, 1.5x, 1.75x, or 2x. Off, the shipped default, leaves
it unchanged. Settings > Graphics provides one stepped bar with Off and these
multipliers. The selection is saved in `config/extensions.ini` and applies on
the next launch.

The native hook validates the retail function signature before installation.
It changes the object cutoff, not scenery LOD selection or server visibility.

## Extended Camera Zoom

Extended Camera Zoom raises the supported 1.23b client's camera distance
setter ceiling from 10 to a value from 11 through 15. Off, the shipped default,
keeps the client's ceiling. Settings > Graphics provides one stepped bar with
Off and these ceilings. The selection is saved in `config/extensions.ini` and
applies on the next launch.

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
- `api_version`, `minimum_runtime_version`, `supported_client_builds`, and
  `capabilities`: metadata in the current host; they do not grant services
- Each optional command record: nonempty `name`, `usage`, and `description`
  strings, with `name` beginning with `/`. The first word of each `name`
  registers that chat command; see
  [Chat command dispatch](#chat-command-dispatch).

The launcher passes enabled manifests to the module in `[addon.N]` row order
from `config/extensions.ini`. It also passes the installed manifest catalog,
so startup commands can load a package not selected for automatic loading.
Startup and in-game commands can append packages; unload and reload
operations can change the active order for that session. Packages cannot
enable or reorder themselves.
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

Addons receive no packet callback. The host copies complete transport frames
on the Winsock hook thread and the Packetlogger writer thread formats them, so
no addon code runs per frame.

The host observes synchronous `recv`, `WSARecv`, `send`, and `WSASend`
completions. It does not log asynchronous completions or unframed data.
The writer queue holds at most 4096 frames and 4 MiB of frame data, and drops
new frames when full; stub rows count by the length of their text. Each part
starts with the header `capture_unix_ms_utc,direction,size,frame_hex`, and
each row holds:

- `capture_unix_ms_utc`, the host capture time in Unix-epoch milliseconds.
- `direction`, `incoming` or `outgoing`.
- `size`, the frame length in bytes.
- `frame_hex`, the original bytes as uppercase hex, including protocol
  headers and any compression, or a `stub:` payload as described under
  [Capture scope](#capture-scope).

Every row has four columns of printable ASCII and no commas inside a column.
Besides frame rows, a part holds three other kinds of row, which carry a fixed
word in the `direction` column and `0` in `size`:

- Meta rows, written right after the header at the top of every part, one
  `key=value` per row in this order: `format=2`, `session=` (16 lowercase hex
  characters, shared by all parts of one session), `part=` (the part number),
  `launcher=` (the launcher version, or `unknown`), `runtime_api=`,
  `client=2012.09.19.0001`, `os=windows-<major>.<minor>.<build>`,
  `wine=` (the Wine version, or `none`), and `scope=standard` or `scope=full`.
  Values have commas and non-printable characters replaced by spaces.
- Zone rows, written when the game reports a map change while recording:
  `<ms>,zone,0,id=<decimal zone id>;name=<area name>`. The name is empty for
  zones without a known name.
- End rows, written as the last row of a part when capture stops
  (`reason=stop`), the addon is disabled or reloaded (`disable`), the part
  fills and rotates (`rotate`), `/packetlogger split` closes it (`split`), or a
  write error or Lua fault stops it
  (`fault`):
  `<ms>,end,0,reason=<reason>;captured=<n>;written=<n>;skipped=<n>;dropped=<n>;queue_high_water=<n>`.
  The counts are the cumulative status counters since enablement at the moment
  the row is written. A part without an end row is incomplete, because a game
  exit is not observed.

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

The runtime reserves `/addon`, `/load`, `/unload`, and `/plugins` for
[Loading in game](#loading-in-game). Every installed addon's manifest
`commands` names register their first word, such as `/fps`, when the module
starts. Names are read from inline tables in a `commands` array or from
`[[commands]]` tables. A word must be `/` followed by 1 to 31 lowercase ASCII letters,
digits, hyphens, or underscores. Reserved words, malformed words, and words
already registered by an addon whose ID sorts earlier are ignored. A registered word is
dispatched only while an addon that handles it is loaded.

The host reads a bounded token vector from the retail parser. Malformed,
unsupported, overlong, or unreadable inputs pass to retail. Recognition occurs
only at the top-level retail lookup call; nested token lookups always pass
through.

The host joins the tokens with single spaces, up to 512 bytes, and calls each
loaded addon's optional `command(command_name)` once in load order until one
returns `true`. Addons validate their own arguments and return `false` for
forms they do not support. A `true` result supplies the retail unknown-command
sentinel, which the validated retail path consumes without sending the
command as chat. A missing callback, `false`, or callback error preserves the
retail lookup path.

The command boundary and the retail chat insertion boundary are installed in
every session. Packet observation is installed when any installed addon
consumes it. The optional `BAHAMUT_TEST_COMMAND_BOUNDARY_RESULT_FILE` output
is an aggregate diagnostic snapshot.

## Host API

The global `bahamut` table exposes the functions below. The Wiki addon also
receives the bounded URL service.

### NoLock movement policy

The NoLock movement service is unavailable in the retail runtime, and no addon
API exposes it. Native host admission requires a healthy loaded instance, a
current verified session and an independently qualified movement boundary.
Manifest metadata grants no admission. The host revokes policy before reload,
disable or unload, and on callback faults. Revocation does not retire a native
hook or establish call quiescence.

Native candidate admission also checks copied evidence for one client clip
path through the primary movement wrapper and setter. The target preflight
requires exact executable/version identity and mapped instruction windows;
it accepts no test-stub identity exception. The call check requires a current
service session, qualified recovery and synchronous borrowed lifetime, matching
owner/controller, primary vptr/gate, thread, callers and duration, and an
actor ID equal to the validated player-state active actor ID.
These are native-owner inputs; no retail producer currently supplies them.
A clip class name or matching pointer does not establish gameplay eligibility.
Any reduction that rounds a positive input to zero needs its own qualified
native outcome. Missing evidence forwards the original duration once.

The [native admission declaration](../client/src/action_movement_admission.h)
cites the exact-client static source. Its setter return is `0x0065ED59`; the
client clip calls the wrapper at `0x008284A9` and returns at `0x008284AE`.
The byte windows were checked against the executable identity in
[runtime_contract.h](../client/api/runtime_contract.h) using pefile 2024.8.26
and Capstone 5.0.7. This is exact-binary validation of the candidate route,
not a retail recovery classification or movement result.

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

### Experimental addon state

- `bahamut.custom_timer()` is available only to addon ID `customtimer`. It
  returns `nil` when no timer is active or the service is unavailable;
  otherwise it returns `instance_name`, `objective`, and integer
  `remaining_seconds`. See [Custom Timer](#custom-timer) for the tag and
  lifetime limits.
- `bahamut.enemy_casts()` is available only to addon ID `enemycast`. It
  returns an array of at most four rows, or an empty table when no rows or
  player state are available. Each row has `actor_id`, `command_id`, `name`,
  `actor_name`, `remaining_seconds`, `duration_seconds`, `progress`, `casting`,
  and `completed`. Unfinished casts sort by remaining time before completed
  casts, followed by engaged-idle rows. `progress` ranges from 0 to 1;
  `casting = false` denotes an idle row.
  `completed` also becomes true when the estimate expires, as described in
  [Enemy Cast](#enemy-cast).

These functions expose copied state, not client memory or raw packets. Like
other addon ID restrictions, they do not authenticate replacement packages.

### Drawing

These functions submit content during `draw`:

- `bahamut.combat_meter(mode, rows, locked, incomplete)` is available only to
  the packaged `combatparser` addon. `mode` is `dps` or `hps`. Each row
  supplies a player name, metric total, rate, accuracy label, and `#RRGGBB`
  bar color. Empty rows show the selected mode heading. Position and
  Shift-drag locking use the same overlay layout as other addon windows.
- `bahamut.targetlines()` submits the current native arcs. It takes no
  arguments and is available only to the `targetlines` addon during `draw`.
  It does not expose actor positions, camera matrices, or arbitrary world
  drawing.
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
- `bahamut.packetlog_start()`, `bahamut.packetlog_stop()`, and
  `bahamut.packetlog_split()` control recording for the enabled Packetlogger
  and return whether the operation succeeded.
  Each waits up to 2 seconds for the writer thread. Starting while recording
  keeps the current session.
- `bahamut.packetlog_status()` returns `recording`, `file`, `part`, `captured`,
  `written`, `dropped`, `skipped`, `queued`, `queue_high_water`,
  `bytes_on_disk`, and `error`. `bytes_on_disk` is the size of the capture
  files after the last retention scan plus bytes written since. `file` is the
  current part filename only, and `error` is an empty string unless a write
  error stopped capture. The start, stop, and status functions are available
  only to the addon with ID `packetlogger`. Addons cannot write capture rows
  or choose capture paths.

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

Manifest command names register chat command words only. They cannot make
unsupported host services available.
