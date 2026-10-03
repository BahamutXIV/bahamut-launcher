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
