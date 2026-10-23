# Stock-DAT seasonal weather

The native runtime has an opt-in seasonal switch that reads installed stock
DATs. It selects retained atmosphere resources and adjusts verified seasonal
TimeLine conditions in memory. It writes no installed DATs. In-game visual and
reload acceptance is pending; the headless tests establish policy, record
classification, and the inline adapter ABI.

## Enable for testing

Use a separate launcher package built with this native module. Keep Runtime
enabled in the launcher. In PowerShell, set these variables before starting
that package yourself:

```powershell
$env:BAHAMUT_RUNTIME_STOCK_SEASONS = '1'
$env:BAHAMUT_RUNTIME_STOCK_SEASONS_LOG = 'C:\seasonal-test\stock-seasons.log'
```

Use an existing writable directory and an absolute log path. The log is
optional and appends up to 4096 lines per process. Only the exact value `1`
enables the switch. Remove the variables and start a fresh game session to
return to normal overlay behavior:

```powershell
Remove-Item Env:BAHAMUT_RUNTIME_STOCK_SEASONS
Remove-Item Env:BAHAMUT_RUNTIME_STOCK_SEASONS_LOG -ErrorAction SilentlyContinue
```

Stock mode bypasses **all runtime DAT overlay packages**, including custom
packages. It requires the supported executable and ten exact stock files:
the weather registry, three atmosphere DATs, and six layout DATs. A mismatch
fails native initialization with status 16 and a `stock seasons:` diagnostic;
it does not fall back to an overlay. The supported executable is build
`2012.09.19.0001`, SHA-256
`9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9`.
The file pins are in [the catalog generator](../client/tools/build-seasonal-catalog.py)
and [the runtime stock checks](../client/src/seasonal_weather.cpp).

## Presentation contract

| Server weather ID | Stock-mode behavior |
| ---: | --- |
| 8027 | Halloween; enable the catalogued Halloween conditions and suppress the catalogued winter conditions. |
| 8033 | Christmas; submit native ID 8027 with the retained snowy atmosphere, suppress Halloween and enable the catalogued winter conditions. |
| 8032 | Pass through unchanged and retain native conditions. |
| Other IDs | Pass through unchanged and retain native conditions. |

Translation applies to the known city roots: Limsa Lominsa 101, Gridania 103,
and Ul'dah 104. Gridania roots 106, 115, 122 and 123 share root 103. Unknown
regions retain native behavior.

| City | Halloween atmosphere key | Christmas atmosphere key |
| --- | --- | --- |
| Limsa Lominsa | `29D9001F` | `29B00020` |
| Gridania | `29D9001F` | `29B00020` |
| Ul'dah | `615A0023` | `29B00020` |

Gridania Halloween reuses Limsa's stock atmosphere. Its missing `dwev_01`
lookup is narrowly aliased to the selected Limsa atmosphere's `dwev_00`. This
is an authored stock-resource combination; it does not recover Gridania's original
2011 Halloween lighting. Christmas reuses Gridania's retained snow atmosphere
in all three cities. Cross-city atmosphere appearance still needs acceptance.

The catalog contains 130 TimeLine records: 58 public Halloween records, 62
interior Halloween records, and 10 winter records. The winter records cover
five retained interior show/hide pairs. This mode does not reconstruct missing
historical public Christmas placements. Every override requires the matching
city, native receiver lineage, relocated record/owner relationship, names,
weather vector and SCB fingerprint. Unknown records keep their native result.

## Updates and reloads

The service intercepts scene operations `0C` and `0D`, retaining the latest
deferred weather request. Replay runs after the original map-update callback
at `0062AA70`, with region lookup, weather enable, state, pending-operation,
and owner-ready guards. It does not issue native weather calls from the
requesting thread outside that callback.

Logical season survives owner reloads on the same map actor. A new or destroyed
map actor resets it. Switching between Halloween and Christmas clears the
current weather before reusing native ID 8027, avoiding the native same-ID
early return. Raw keys belonging to the active seasonal profile are rebound to
that profile's key for the current city. This handles a native mode-byte replay
queued with the previous city's key. Unrelated keys retain native behavior and
clear the logical mode. The server must supply the desired season after a new
map actor is created; this service does not synthesize a server weather update.

Weather-key substitution is limited to native callers `006241F2`, `007E8DD4`
and `007E8E58` during the original map-update callback. Each lookup reads the
incoming RegionInfo root at `+B0`; the actor's region can still refer to the
previous zone during transition. The DrawEnv alias is limited to the missing
lookup from `007DCEF5` for the current Gridania owner and selected Halloween key.

The condition adapter at `007FCCEA` changes only the saved low byte of EBX
before the original comparison. It preserves integer registers, flags and
the FXSAVE floating-point state. Native signatures, caller targets and method
cells are pinned by [seasonal-native.json](../client/contracts/seasonal-native.json).
The evidence and native operation order are documented in
[the stock-switch contract](https://github.com/XIVLegacy/xivl-decomp/blob/c913db63dcc854656e4757f31006b7f7d431e0e9/docs/resource/native-stock-seasonal-switch.md),
[the TimeLine record view](https://github.com/XIVLegacy/xivl-decomp/blob/c913db63dcc854656e4757f31006b7f7d431e0e9/docs/resource/timeline-weather-record-view.md)
and [the map selector contract](https://github.com/XIVLegacy/xivl-decomp/blob/c913db63dcc854656e4757f31006b7f7d431e0e9/docs/resource/map-layout-selector.md).

## Validation and manual acceptance

The self-contained checks require no retail assets:

```powershell
python client/tools/verify-seasonal-bindings.py
python client/tests/test_seasonal_catalog.py
ctest --test-dir out/client-stock-seasons -C Release --output-on-failure -R '^bahamut-(seasonal-policy|seasonal-catalog|seasonal-weather|dat-overlay)$' --parallel 1
```

The weather test installs an inline hook in a synthetic executable buffer. It
checks the real five-byte conditional branch relocation, unchanged pointer
registers, the selected BL result, MXCSR preservation and hook removal. It
does not launch the game or a window. The catalog and policy tests cover
unknown/malformed records, supported roots, mode changes and guarded replay.

With explicit local evidence inputs, `verify-seasonal-bindings.py --exe PATH`
checks the original PE digest and bindings. The catalog generator takes
`--client-root PATH --check` to compare freshly derived metadata with the
tracked catalog. No retail bytes are included in the generated catalog.

For in-game acceptance:

1. Launch the separate package with stock mode enabled. Confirm the log says
   `stock-DAT seasonal hooks enabled` and reports selected stock keys.
2. In each city, switch `8027 -> 8033 -> 8032 -> 8027`. Check atmosphere and
   decorations together in public areas and the retained interior families.
3. Repeat while changing zones, entering interiors and reloading the scene.
   Confirm the desired season returns after the server's weather update,
   without decorations from the other season.
4. Retain the diagnostic log, zone and weather-change sequence for any mismatch.
   Condition rejections identify a record that stayed native; overrides log
   mode 1 for Halloween or 2 for Christmas and role 1/2 for Halloween show/hide
   or 3/4 for winter show/hide.

Windows MSVC Win32 Release compilation and the headless checks passed. Retail
runtime acceptance and Wine validation remain open. Normal packaged overlays
are described in [DAT overlays](dat-overlays.md).
