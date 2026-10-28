# Win32 client module

Build the x86 Win32 loader, `bahamut.dll`, Screenshot and DiscordRPC DLLs, and
the D3D9 test client from this directory. The runtime embeds Lua 5.1.5 for
isolated addons. Screenshot and DiscordRPC use a private native boundary;
there is no public plugin ABI or general client memory API.

Windows releases use MSVC. Linux and macOS releases use llvm-mingw and run
the same x86 module under Wine.

## Build on Windows

Run these commands in a Visual Studio Developer PowerShell using the x86
generator:

```powershell
cmake -S client -B out/client-debug -G "Visual Studio 17 2022" -A Win32
cmake --build out/client-debug --config Debug
ctest --test-dir out/client-debug -C Debug --output-on-failure
```

## Cross-compile on macOS or Linux

Use [llvm-mingw](https://github.com/mstorsjo/llvm-mingw/releases), the supported
non-MSVC toolchain. CI pins a tested release. GCC-based MinGW-w64 toolchains
are unsupported.

```bash
cmake -S client -B out/client-mingw \
  -DCMAKE_TOOLCHAIN_FILE="$PWD/client/cmake/llvm-mingw-i686.cmake" \
  -DCMAKE_BUILD_TYPE=Release \
  -DLLVM_MINGW_ROOT=<dir>
cmake --build out/client-mingw
```

[`llvm-mingw-i686.cmake`](cmake/llvm-mingw-i686.cmake) looks for the toolchain
root in this order: `-DLLVM_MINGW_ROOT`, `LLVM_MINGW_ROOT`, then the first
`i686-w64-mingw32-clang` on `PATH`. Outputs go directly into the build
directory, without a `Release/` subdirectory. The binaries statically link
the llvm-mingw C++ library, unwinder, and mingw-w64 CRT. They import only
Windows system DLLs and UCRT API-set DLLs.

Keep the decorated names expected by `link.exe` in tracked `.def` files.
CMake rewrites them for `lld` during configuration.

Use the guarded-code macros in [`fault_guard.h`](src/fault_guard.h) rather
than raw `__try`. MSVC expands them to `__try`/`__except`; the i686 mingw
build uses an explicit handler frame.

### Tests under Wine

Configure the emulator, build, and run CTest:

```bash
cmake -S client -B out/client-mingw \
  -DCMAKE_TOOLCHAIN_FILE="$PWD/client/cmake/llvm-mingw-i686.cmake" \
  -DCMAKE_BUILD_TYPE=Release \
  -DLLVM_MINGW_ROOT=<dir> \
  -DCMAKE_CROSSCOMPILING_EMULATOR=$PWD/client/tools/run-under-wine.sh
cmake --build out/client-mingw
ctest --test-dir out/client-mingw --output-on-failure
```

[`run-under-wine.sh`](tools/run-under-wine.sh) reads these variables:

| Variable | Use |
|---|---|
| `BAHAMUT_WINE` | Wine binary; defaults to `wine` on `PATH`. |
| `WINEPREFIX` | Wine prefix; defaults to `~/.wine`. |
| `BAHAMUT_WINE_DYLD_FALLBACK` | macOS `DYLD_FALLBACK_LIBRARY_PATH` for the managed engine. See the launcher's list in [`macos.rs`](../src/platform/macos.rs). |
| `WINEDLLOVERRIDES` | Preserves caller overrides and appends `xinput1_3=n,b` unless already set, so the test client's stub takes priority over Wine's builtin. |
| `WINEDEBUG` | Wine debug channels; defaults to `-all`. |

The script maps absolute Unix path arguments to drive letters from
`<prefix>/dosdevices`. A cold prefix may take several minutes to initialize.
The `bahamut-loader` test opens a D3D9 window and requires a GUI session.

## Native test coverage

The native suite covers:

- Loader and runtime launch transactions, including launches without extensions.
- Screenshot loading, capture, and fault isolation.
- DiscordRPC presence and copied player state publication.
- DAT opening and resolution for the exact supported client build.
- D3D9 device, swap-chain `Present`, `Reset`, and input forwarding.
- Telemetry and the isolated Lua addon lifecycle against the bundled test client.
- NoLock movement policy and addon admission, including typed Lua requests,
  application of 50% on load or update, faults, reload, disable and host shutdown.
  Offline fixtures check the absence of NoLock commands and handling of missing
  older-host APIs. Exact setter, wrapper and producer-call byte fixtures cover
  target verification; synthetic test-client identity is accepted for ordinary
  runtime checks but rejected by the movement target check.
  A synthetic actor, fake vtable and stub wrapper exercise the binding to
  `FUN_0065ED30` and caller return `0x008284AE`, guarded capture,
  stale-session fallback, rejection of a scaled duration of zero, installation
  and shutdown with a resident trampoline and detour. Native fixtures check
  x87/MXCSR, LastError, stack/register forwarding and an original that frees the
  receiver to detect reads after forwarding. MSVC Win32 also tests synthetic
  MinHook installation, rollback and cleanup failures in the dedicated
  movement test target.
  A test-only baseline observer records independent wrapper and setter entries
  in fixed storage, forwards unchanged and reads nothing after the original.
  Fixtures check caller filtering before reads and recording stops on a read
  fault, capacity limit, invalid object, failed clock, producer-thread change
  or collection reentry. Rows establish neither retail eligibility nor paired
  calls; the caller must stop and drain calls before reading or retiring storage.
  Its optional `--benchmark` measures fixture calls, not game performance.
  See [NoLock movement policy](../docs/extensions.md#nolock-movement-policy)
  for the runtime contract. These fixtures do not establish retail movement
  behavior; live acceptance of this build is pending.
- Gearsets records and controls through real host bindings, bounded exact UID
  resolution over synthetic equipment and bag memory, and whole-set transactions
  through a fake executor. Separate debug tests cover switch gates, receipts,
  and the single-item diagnostic transaction.

The direct-launch test also checks that launching without extensions neither
loads `bahamut.dll` or `screenshot.dll` nor creates the event environment.

## Autologin

The [autologin service](src/autologin.h) drives the retail Bootup title and
character-list actions without sending input. On each frame tick, the
[login reader](src/login_reader.h) copies the scene, phase and character-list
state and runs at most one pending request on that thread. After the title
settles for 4 s, it calls the Start confirmation's accept path. Once the
character list has been readable for 3 s, it applies the list's slot checks and
queues the select and confirm requests for the configured slot. The run
finishes when the client enters the world scene.

A slot is a list position from 1 to 8; the service has no character, account
or server identity. It stops with Needs user for a slot outside that range, an
empty slot, a slot requiring a warning or vetoed by the list, an unexpected
Bootup phase before title Start, a scene other than the world after selection,
client code that does not match the supported build, or a timeout. If the client
is already in the world when the run starts, it stays idle. Each game process
allows one run. Transitions and request results are appended to `autologin.log`
beside the client module.

The `bahamut-autologin` CTest drives the state machine with a fake clock and
recorded requests. The offsets, request selectors and function addresses are
specific to the retail 1.23b client; live behavior beyond the tested slot is
not established.

## Limits and related docs

These tests use the bundled stub client. Results do not establish retail client
behavior or Wine support beyond the tested build and prefix.

See [Development](../docs/development.md) for workspace checks, package staging,
and coverage. [Handshake](../docs/handshake.md) and
[Extensions](../docs/extensions.md) define the launch inputs, runtime contracts,
and platform status.

## Gearsets

The `gearsets` addon is disabled by default. `/gearsets` toggles the Gear Sets
window, and `/gs` is the short form of every `/gearsets` command with the same
arguments. Selection never equips; double-click a set, use Equip Set, or run
`/gs equip <set>` to apply it slot by slot, main hand first. Equip never
requests a Job Change.
Sets are stored per server and character in an editable `sets.lua` file parsed as
data and never run.
Controls and host services are described in
[Extensions](../docs/extensions.md#gearsets).

`equipment_state.cpp` reads the equipped slots from the client's item store
without calling client code. Its layout comes from static reading of the
supported `ffxivgame.exe`; [Client evidence](#client-evidence) records its
SHA-256, offsets and supporting instructions. On the supported
Windows client, a bounded reader check observed eight equipped instance IDs
and the body slot becoming Empty and returning to the same instance after
manual removal and restoration. This does not qualify every slot label,
bag movement, or another character.
Synthetic memory tests establish reader and resolver logic only.
While the addon is loaded and has not faulted, `gearsets_runtime.cpp` reads
equipment and the accessible bag on the game thread after the hooked
player-state update returns, at most every 100 ms and on every update while
an Equip is active, and publishes them to `bahamut.equipment_snapshot()`.
Otherwise the running game publishes no equipment snapshot. When the MyPlayer
control changes or disappears, the snapshot is replaced or cleared on that
update. An unreadable slot is reported as
`unknown`, never as `empty`. A store that lacks the equipment or bag package
produces no snapshot.
Item resolution copies equipment and the accessible bag twice and requires
agreement. The bag search has a 1024-position safety bound, not a claimed
client capacity. A saved instance is never replaced by another copy of its
item type. A set entry naming only an item type resolves to the instance worn
in that slot, else to the single copy found; two copies are ambiguous and the
entry is refused. Inaccessible items are never retrieved.
`item_names.inc` maps item-list names to item numbers for set files. It is
generated from `bahamut:data/enums/item.yaml`; its header records the source
digest.

The launcher passes `BAHAMUT_RUNTIME_SERVER_ID`, the selected server profile's
stable ID: 16 hex digits of SHA-256 over its normalized lobby host and port.
Bahamut assigns the player actor ID from the character's database ID
(`bahamut:src/map/map_runtime.cpp` checks a player action's `ActorId()`
against the session's `characterId`). The reader copies class, job and, when
the player-state capture succeeded on the same game frame, that actor ID into
each snapshot; a failed capture leaves the snapshot without a character. A
snapshot for another character starts a new equipment session.
`AddonHost::SetProfileKeyProvider` supplies `<server id>-<character id>` from
the current snapshot, with the character ID as 8 hex digits. Record calls
carry the key the addon's store was created for and are refused once it is
no longer current. Saving uses only a snapshot from the addon's session.
Without a valid server ID or a snapshot naming the character, the key is
absent and the addon keeps its sets in memory only.
`addon_records.cpp` stores `.lua` records under the key; the addon keeps all
sets in one record. Session loss or replacement clears copied equipment and
session-only sets.

`gearsets_transaction.cpp` copies immutable whole-set intent, preflights every
requested slot, and accepts ordering, restriction checks, a confirmation
deadline, and a settling window only from its executor. The fake-executor
tests cover sequential operations, explicit Empty/unequip, moved items, an
observed class change, dependent slots cleared by class transitions,
deferred submission, cancellation, and partial or unconfirmed outcomes. A
queued request is not success; `complete` requires a fresh full match.
Cancellation stops unsent work and accounts for an accepted request that
subsequently confirms. Sent requests are never retried or rolled back, and
saved job metadata never requests a Job Change.

`gearsets_native_executor.cpp` submits each operation as the client to server
0x012D `commandRequest` for EquipCommand (12009) that the retail Gear window
sends, encoded by `gearsets_equip_request.cpp`, through the client's own send
call on the game thread. No game Lua runs and no local event is installed;
the server's inventory replies apply the change. A request is sent only while
the connection is established, no event is in progress or installed, and the
command reference resolves; otherwise the step waits and stops with
`native-not-ready` after 5 seconds. The main hand goes first, and a set
without one is refused. Each operation must be seen in a fresh snapshot within
5 seconds. A main-hand change can change class and job and clear other
slots; the transaction waits one second after it is seen, accepts the class
it observes without predicting it, and re-evaluates the remaining slots. The
game can report that class later; while the main hand is still the one the
transaction equipped, the late change settles the same way. Any other change
stops the transaction as `external-change` with the slot it saw, unless the
whole set is then worn, which completes. A
reader failure stops an active Equip only after a second without a good read.
On the supported Windows client against a Bahamut server, this request
equipped a body item and the reader confirmed it.

Live acceptance still requires unequip, same-class and class-changing whole
sets, moved-UID durability, stable server/character identity,
missing/restricted items, cancellation, and staged overlay/input checks.
Windows fixtures do not establish Wine or other platform behavior.

### Client evidence

The static observations below concern retail `ffxivgame.exe` 1.23b,
SHA-256 `9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9`,
image base `0x00400000`.

#### Item store layout

The item store layout was read with a disassembler. It is only partly
qualified by the bounded Windows reader observations [above](#gearsets).

MyPlayer + 0xD8 is the item store: 0x006F51A0 returns [ecx+0xD8] and
allocates it when null (0x006F51C5). This reader never calls it.

Store + 8 is a `std::map<uint32_t package code, Package*>` in MSVC 9 layout:
0x0071D420 is its lower_bound (head [map+4], root [head+4], left +0,
right +8, key +0xC, isnil byte +0x15), and 0x006EE3E0 reads the value at
node +0x10.

Package + 0x10 is a `std::vector<Entry*>` (begin +0x14, end +0x18);
0x0052F7F0 indexes it with a bounds check.

Entry: +8 item*, +0xC redirect package code, +0x10 redirect slot, both -1
when unset. 0x006EE3E0 follows the redirect recursively.

Item: instance id as two words at +0x10 (low) and +0x14 (high), from the
ItemRef encoder at 0x006F52CB-0x006F52D0; catalog id at +0x24, the field
`ItemBase._getCatalogID` (impl 0x00701820) reads.

Package code 0xFE holds 35 equipment positions: push 0xFE at 0x00702558 and
the position count [0x00FE052C] used at 0x00702361. Retail captures show
its positions linking to positions of package 0 (the bag).

#### Player lookup

Same pointer traversal as `ReadDisplayName` in
[`client/src/player_state.cpp`](src/player_state.cpp):
container -> waiting -> engine -> holder -> holder state -> player.
That the final pointer is the MyPlayer control whose +0xD8 is the item
store produced the observed equipped identities and body changes described
in [Gearsets](#gearsets). 0x0070A010 takes MyPlayer as ECX and reads
[ecx+0xD8] through 0x006F51A0; broader session coverage remains unqualified.

#### Equipment slots

Slot indexes are 0-based positions in the client's equipment package
(code 0xFE, 35 positions). The supported set follows the Bahamut server's
equipment slot enumeration (`scripts/enum/equipment_slot.lua`). Only the
main hand, head, and body positions are corroborated by retail captures;
the remaining positions and every label are UNVERIFIED against the client.

The bag package (code 0) is the source package of every observed equip request.

#### Static actor lookup

Tools: Capstone 5 and Ghidra 12.1.3 DumpVAs.

`_getStaticActor` worker 0x0078CC30, numeric argument: 0x00CC70E0 ->
0x00CD8050 -> 0x00D03A50 on [engine+0x1C4] builds the reference
[entry+0xC] + id from the category entry for byte [0x0130D426], then
0x00CC7A50 tests it. When 0x00D035D0 holds (the reference's category entry
exists, the masked reference equals its base, and the category is
[0x0130D426]), 0x00CD81D0 looks the reference up in the dense table at
[engine+0x17C]+0x60 (0x00D0A3D0); otherwise 0x00CD8160 uses +0x1C, which
these reads do not follow. A found bucket's value V is loaded when V and
[V] are nonzero; [V] is the control the binding pushes.

`stdext::hash_map<char, base>` at actor-ids+4 (find 0x00D03DC0); every
element is on its std::list, whose head node is [actor-ids+0xC]. Node:
+0 next, +8 key byte, +0xC base.

Dense table (find 0x00CEBCA0, probe 0x00CEB9C0): 8-byte {key, value}
buckets; quadratic probe until the empty key.

#### Engine and send path

Engine: container+0x518 -> waiting, waiting+4 -> GameEngine; engine+0x1CC
error reporter, halt byte +8. `_doServerOnCommand` (0x006E2FB0) reaches the
sender state as [[engine+0x220]+4] (0x00CC7510 -> 0x00CD7910 returns
engine+0x220); 0x0075D460 copies its actor [+0x10] and state word [+0x14]
into the request, and 0x0075E1C0 sends through network [[state]].

0x004D6D30 (thiscall on the network, ret 0xC) returns false unless
session [net+0x174EC], [session+0x234] and its +0x70 connection exist;
the connection (0x00DAE010) queues an 0x012D record only in state 3
([+0x8C]) and copies its [+4] - 0x10 payload bytes from +0x18 before
returning, reading nothing else of the record but the opcode at +0.

The game sends the record laid out by builder 0x00776760 from an
uninitialized stack buffer. The native executor zeroes the unread bytes.

commandRequest's descriptor reads its event type through slot 5 from
[0x0134C3FF]; the captured request carried 0.

`_executeCommand` 0x0070A010 refuses unless both player event slots
+0x128/+0x12C hold [0x0130C778] and manager [player+0xF8] byte +0x1E is 0.
The manager's client-side (+0x08) and server-order (+0x0C) block events
must also be empty: every s2c EndEvent teardown tests them for null first
(0x008934A0, 0x00893520, 0x008935B0, 0x00893800), so the server's
EndEvent for this request then changes nothing.

#### Equip request layout

c2s 0x012D EventStart application payload (184 bytes) as the retail
builder 0x00776760 lays it out: u32 actor, u32 owner reference, u32 state
word, u32 CRC32 of the 128-byte parameter tail, u8 event type, 32-byte
event name, 128-byte tail, then seven bytes the builder leaves unwritten.

commandRequest(item, nil, 1, nil x5, equipSlot, itemDBIds) with the
parameter tags of the captured requests: 07 item reference (owner actor,
bag slot, package 0), 05 nil, 00 i32, 09 two u64 unique IDs, 0f end. An
unequip sends a nil item and no unique IDs.

#### Game-frame ordering

ffxivgame.exe (Capstone 5): the Rapture update FUN_004B3C50 calls packet
dispatch at 0x004B3CCC and the input step at 0x004B3DCC, then 0x004DAA10 at
0x004B3DD4, whose tail jmp at 0x004DAB41 enters the hooked 0x004DA680.

### Debug switches

Two environment variables enable diagnostics for a live check. Both are off
by default, and each acts only while the `gearsets` addon is loaded and has
not faulted. Set them before starting `out/dev/bahamut-launcher.exe`; the game
inherits them. The module reads them once at startup. When neither is set
no debug service is constructed or installed: no receipt or diagnostic
transaction runs. The ordinary reader and Gear Sets controls remain
available when the addon is enabled. Debug commands pass through to the game
without their switch.

| Variable | Value | Effect |
|---|---|---|
| `BAHAMUT_TEST_GEARSETS_SNAPSHOT_FILE` | Absolute path of a new file | Reads equipment and writes the snapshot receipt |
| `BAHAMUT_TEST_GEARSETS_EQUIP` | `1` | Reads equipment and enables the debug equip command |

Any other value leaves a switch off. While a switch is on, the debug service
also reads the equipment package on the game thread after the hooked
player-state update returns, at most every 100 ms and on every update while a
debug equip request is active, and publishes it to `bahamut.equipment_snapshot()`. When
the MyPlayer control changes or disappears, the published snapshot is
replaced or cleared on that update. Each equipment or bag read is taken twice
and must agree.

The receipt is created only when the addon is first active and covers that
one addon session; an existing file is never replaced, and an open failure
disables the receipt. The render thread writes it; the game thread only
queues records. It holds a header with the schema version (1), the client
SHA-256, and the process ID, then CSV records, then a footer:

```text
# bahamut-gearsets-receipt schema=1
# client_sha256=<sha256>
# process_id=<pid>
# snapshot: record,sequence,frame,generation,complete,slot,state,uid,catalog
# unavailable: record,sequence,frame
# equip: record,sequence,frame,request,phase,reason,generation,bag_slot,equip_slot,uid,catalog,invocations,return_value,submit_frame,invoke_frame,finish_frame
snapshot,1,40,1,1,0,item,0000001200000001,<catalog>
...
# end records=<n> dropped=<n> reason=addon-inactive
```

A snapshot record is written only when its content changes, as one row per
supported slot. `frame` counts game-thread updates while the addon is
active. `unavailable` marks a read that found no player, no readable store,
or two passes that disagree. The receipt stops at 4096 records with reason
`limit`, and ends with reason `addon-inactive` at the first unload, reload,
or fault of the addon. At module shutdown it ends with reason `shutdown`
unless another thread holds the receipt lock; then it is closed without
waiting and without a footer. A receipt without a footer ends at its last
complete row.

With `BAHAMUT_TEST_GEARSETS_EQUIP=1`, the addon accepts:

```text
/gearsets debug-equip <bag slot> <equip slot> [uid]
/gearsets debug-status
```

The bag slot is the 0-based position in the bag package, and the equip slot
is the 1-based slot of the game's Equip command (11 is the body). The
optional uid is 1 to 16 hex digits. One request is active at a time. On the
game thread the module rechecks the bag position on the frame that would act:
an unreadable or empty position, a different uid, or a different player
control rejects the request without any native call. Every load, reload,
unload, or fault of the addon ends an active request at once. Without a uid,
the first frame records the position's uid and a later frame rechecks it. A
slot that already holds the item completes without a native call. Results
are printed only by `/gearsets debug-status`, because `bahamut.chat_print`
succeeds only on the command thread.

The debug command has no native invocation, so a request that passes every
check ends `refused` with `invoke-unavailable` and changes nothing. The
transaction would report `complete` only when a fresh snapshot shows the uid
in the slot within 5 seconds, `unconfirmed` on a timeout or session end
after the call, and `rejected` when the call returns false.

## Targetlines

The `targetlines` addon is disabled by default. Enable it in Addons before
the next game launch. The normal client build includes its native renderer
and needs no capture file or diagnostic environment variable. The addon
calls `bahamut.targetlines()` during `draw`. Camera, projection, and
relationship data remain inside the native host.

With the overlay visible, a raised red curve with a soft glow follows the
selected mob until selection clears or changes. Mob recognition requires an
exact reviewed class/path pair and the complete Bahamut ambient enemy spawn
profile; unknown actors and ally profiles stay hidden. The reviewed opening
Jellyfish profile is also supported. Self and ordinary friendly selection
draw no arc.

A green curve follows the target of an accepted local heal or buff cast,
even when selection changes during the cast. The command allowlist covers 26
magic commands with a cast time from Bahamut's action catalog. Matching
completion fades it over 250 ms; matching interruption clears it immediately.
A verified cast-end property supplies its deadline, with a bounded catalog
timeout when absent. Instant abilities and unknown commands are outside its coverage.
Actor creation or removal, zone changes, and logout clear relationships.
Disabling or faulting the addon stops drawing and clears its active cast.

Projection requires an observed scene shader and matching copied camera
matrices. It accepts the back buffer or an offscreen render target of the
same size with a viewport covering the full surface. No scaling is applied,
and overlay pixel dimensions must match the back buffer. Arcs stay hidden
after a device reset or in a frame without a supported draw. Endpoints are
lifted by a fixed amount above the copied XYZ positions. The red selection
and green cast behavior were tested in-game on Windows. Broader scenes,
clipping, window and reset recovery, transitions, and Wine behavior need
further testing.

### Optional diagnostic capture

For camera alignment checks, configure a separate build with capture enabled:

```powershell
cmake -S client -B out/client-targetlines-probe -G "Visual Studio 17 2022" -A Win32 -DBAHAMUT_TARGETLINES_PROBE=ON
cmake --build out/client-targetlines-probe --config Debug --target bahamut
```

Stage the Windows package as described in [Development](../docs/development.md#staged-windows-package),
then replace its `bahamut.dll` with the diagnostic build while the launcher and
game are closed. Set `BAHAMUT_TARGETLINES_PROBE_FILE` to an absolute capture
filename before starting `out/dev/bahamut-launcher.exe`. The requested filename
is used when it is new. If it already exists, the probe creates a unique sibling
named with the game process ID and a bounded retry number. It never overwrites
an existing capture. The runtime must be enabled. The complete client identity
gate and camera entry signature still apply. Capture requires both this build
option and the environment variable. The normal build ignores the variable.

The probe records the last two copied camera records and samples up to
four render surfaces per frame whose vertex constants exactly contain the
observed view matrix. Draw sampling waits for a valid player/selected-target
position pair so login screens do not use the draw capture budget.
Each sample contains the active viewport, render target and back buffer
dimensions, whether the render target is the back buffer, vertex constants,
shader bytecode, and the copied player/target XYZ. Reset starts a new capture
generation.
Capture stops at 128 MiB; the enabled addon keeps updating after recording stops.

After closing the game, summarize the capture with:

```powershell
python client/tools/read-targetlines-probe.py <capture-file>
```

The reader accepts `BTLP0001` and `BTLP0002`. Version 2 records the actual
render target and back buffer comparison. Version 1 reports that comparison as
unknown, since dimensions alone cannot establish surface identity.

The binary capture is private local evidence and includes shader bytecode;
keep it outside Git. These samples do not prove final screen alignment or
complete draw coverage. The diagnostic reads game state and forwards
the original camera and draw calls. It adds synchronous capture overhead, so
use it for short camera alignment checks.
