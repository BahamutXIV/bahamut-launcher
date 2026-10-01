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

The direct-launch test also checks that launching without extensions neither
loads `bahamut.dll` or `screenshot.dll` nor creates the event environment.

## Limits and related docs

These tests use the bundled stub client. Results do not establish retail client
behavior or Wine support beyond the tested build and prefix.

See [Development](../docs/development.md) for workspace checks, package staging,
and coverage. [Handshake](../docs/handshake.md) and
[Extensions](../docs/extensions.md) define the launch inputs, runtime contracts,
and platform status.

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
