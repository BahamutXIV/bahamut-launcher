# Win32 client module

This directory builds the x86 Win32 loader, `bahamut.dll`, the packaged
Screenshot and DiscordRPC DLLs, and the D3D9 test client. The runtime embeds
Lua 5.1.5 for isolated addons. Screenshot and DiscordRPC use a private native
boundary. This is not a public plugin ABI or general client memory API.

Windows archives use the MSVC build. Linux and macOS archives use the
llvm-mingw build and run the same x86 module under Wine.

## Build on Windows

Run from a Visual Studio Developer PowerShell with the x86 generator:

```powershell
cmake -S client -B out/client-debug -G "Visual Studio 17 2022" -A Win32
cmake --build out/client-debug --config Debug
ctest --test-dir out/client-debug -C Debug --output-on-failure
```

## Cross-compile on macOS or Linux

[llvm-mingw](https://github.com/mstorsjo/llvm-mingw/releases) is the supported
non-MSVC toolchain. CI pins a tested release. GCC-based MinGW-w64 toolchains
are not supported.

```bash
cmake -S client -B out/client-mingw \
  -DCMAKE_TOOLCHAIN_FILE=client/cmake/llvm-mingw-i686.cmake \
  -DCMAKE_BUILD_TYPE=Release \
  -DLLVM_MINGW_ROOT=<dir>
cmake --build out/client-mingw
```

[`llvm-mingw-i686.cmake`](cmake/llvm-mingw-i686.cmake) resolves the toolchain
root from `-DLLVM_MINGW_ROOT`, then `LLVM_MINGW_ROOT`, then the first
`i686-w64-mingw32-clang` on `PATH`. Outputs go directly to the build directory.
There is no `Release/` subdirectory. The binaries link the llvm-mingw C++
library, unwinder, and mingw-w64 CRT statically, and import only Windows system
DLLs and UCRT API-set DLLs.

Tracked `.def` files retain the decorated names expected by `link.exe`. CMake
rewrites them for `lld` at configure time.

Use the guarded-code macros in [`fault_guard.h`](src/fault_guard.h) instead of
raw `__try`. MSVC expands them to `__try`/`__except`. The i686 mingw build uses
an explicit handler frame.

### Tests under Wine

Configure with the emulator, then build and run CTest:

```bash
cmake -S client -B out/client-mingw \
  -DCMAKE_TOOLCHAIN_FILE=client/cmake/llvm-mingw-i686.cmake \
  -DCMAKE_BUILD_TYPE=Release \
  -DLLVM_MINGW_ROOT=<dir> \
  -DCMAKE_CROSSCOMPILING_EMULATOR=$PWD/client/tools/run-under-wine.sh
cmake --build out/client-mingw
ctest --test-dir out/client-mingw --output-on-failure
```

[`run-under-wine.sh`](tools/run-under-wine.sh) reads these variables:

| Variable | Use |
|---|---|
| `BAHAMUT_WINE` | Wine binary. Defaults to `wine` on `PATH`. |
| `WINEPREFIX` | Wine prefix. Defaults to `~/.wine`. |
| `BAHAMUT_WINE_DYLD_FALLBACK` | macOS `DYLD_FALLBACK_LIBRARY_PATH` for the managed engine. The launcher's list is in [`macos.rs`](../src/platform/macos.rs). |
| `WINEDLLOVERRIDES` | Caller overrides are kept. `xinput1_3=n,b` is appended unless already set, so the test client's stub wins over Wine's builtin. |
| `WINEDEBUG` | Wine debug channels. Defaults to `-all`. |

The script maps absolute Unix path arguments to drive letters from
`<prefix>/dosdevices`. A cold prefix may take several minutes to initialize.
The `bahamut-loader` test creates a D3D9 window and needs a GUI session.

## Native test coverage

The native suite covers:

- loader and runtime launch transactions, including the path without extensions.
- Screenshot loading, capture, and fault isolation.
- DiscordRPC presence and copied player state publication.
- DAT opening and resolution for the exact supported client build.
- D3D9 device, swap-chain `Present`, `Reset`, and input forwarding.
- telemetry and isolated Lua addon lifecycle against the bundled test client.

The direct-launch test also verifies that a launch without extensions does not
load `bahamut.dll`, `screenshot.dll`, or create its event environment.

## Limits and related docs

The suite uses the bundled stub client. It does not prove retail client
behavior or Wine support outside the tested build and prefix. See
[Development](../docs/development.md) for workspace checks, package staging,
and test coverage. [Handshake](../docs/handshake.md) and
[Extensions](../docs/extensions.md) own the launch inputs, runtime contracts,
and platform status.
