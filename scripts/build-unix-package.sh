#!/usr/bin/env bash
# Build and stage the launcher package on macOS or Linux: cross-compile the
# Win32 client module with llvm-mingw, build the Tauri shell, then publish the
# Unix release manifest into out/dev (default) or out/release/<version>.
#
#   scripts/build-unix-package.sh [--release] [--test] [--skip-build]
#                                 [--llvm-mingw <dir>] [--wine <binary>]
#                                 [--wine-prefix <dir>]
#
# --release     Release client build and release shell, published under
#               out/release/<version>; the default is Debug under out/dev.
# --test        Run the client module tests under Wine through
#               client/tools/run-under-wine.sh before publishing. The Wine
#               binary defaults to the launcher's managed macOS engine, then
#               wine on PATH. The prefix defaults to out/wine-test-prefix; a
#               new prefix takes minutes to initialize on its first run, and
#               the loader test needs a GUI session.
# --skip-build  Publish from the existing client and shell build outputs.
#
# The llvm-mingw root comes from --llvm-mingw, then LLVM_MINGW_ROOT, then
# ~/.local/llvm-mingw, then i686-w64-mingw32-clang on PATH.
#
# Publishing keeps an existing scripts/default.txt and never deletes files, so
# local settings, custom packages, and startup commands survive an update.
set -euo pipefail

script_dir="$(cd "$(dirname -- "$0")" && pwd -P)"
repo_root="$(cd "$script_dir/.." && pwd -P)"
out_root="$repo_root/out"

release=0
run_tests=0
skip_build=0
llvm_mingw_root="${LLVM_MINGW_ROOT:-}"
wine_binary=""
wine_prefix=""

usage() {
    sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --release) release=1 ;;
        --test) run_tests=1 ;;
        --skip-build) skip_build=1 ;;
        --llvm-mingw) llvm_mingw_root="${2:?--llvm-mingw needs a directory}"; shift ;;
        --wine) wine_binary="${2:?--wine needs a binary}"; shift ;;
        --wine-prefix) wine_prefix="${2:?--wine-prefix needs a directory}"; shift ;;
        -h | --help) usage; exit 0 ;;
        *) echo "build-unix-package.sh: unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

fail() {
    echo "build-unix-package.sh: $*" >&2
    exit 1
}

require_tool() {
    command -v "$1" > /dev/null 2>&1 || fail "$1 is required; $2"
}

require_tool cmake "install CMake 3.25 or newer"
require_tool cargo "install the Rust toolchain from rust-toolchain.toml"

if [[ -z "$llvm_mingw_root" && -x "$HOME/.local/llvm-mingw/bin/i686-w64-mingw32-clang++" ]]; then
    llvm_mingw_root="$HOME/.local/llvm-mingw"
fi
if [[ -z "$llvm_mingw_root" ]] && command -v i686-w64-mingw32-clang++ > /dev/null 2>&1; then
    llvm_mingw_root="$(cd "$(dirname -- "$(command -v i686-w64-mingw32-clang++)")/.." && pwd -P)"
fi
if [[ $skip_build -eq 0 && ! -x "$llvm_mingw_root/bin/i686-w64-mingw32-clang++" ]]; then
    fail "llvm-mingw was not found; install a release from https://github.com/mstorsjo/llvm-mingw/releases and pass --llvm-mingw <dir>"
fi

if [[ $release -eq 1 ]]; then
    version="$(cargo metadata --manifest-path "$repo_root/Cargo.toml" --no-deps --format-version 1 |
        sed -n 's/.*"name":"bahamut-launcher-shell","version":"\([^"]*\)".*/\1/p' | head -n 1)"
    [[ -n "$version" ]] || fail "could not resolve the launcher package version"
    destination="$out_root/release/$version"
    client_build="$out_root/client-mingw"
    client_config="Release"
    cargo_profile="release"
    cargo_flags=(--release)
else
    destination="$out_root/dev"
    client_build="$out_root/client-mingw-debug"
    client_config="Debug"
    cargo_profile="debug"
    cargo_flags=()
fi
launcher_binary="$repo_root/target/$cargo_profile/bahamut-launcher-shell"
emulator="$repo_root/client/tools/run-under-wine.sh"

if [[ $skip_build -eq 0 ]]; then
    cmake -S "$repo_root/client" -B "$client_build" \
        -DCMAKE_TOOLCHAIN_FILE="$repo_root/client/cmake/llvm-mingw-i686.cmake" \
        -DCMAKE_BUILD_TYPE="$client_config" \
        -DLLVM_MINGW_ROOT="$llvm_mingw_root" \
        -DCMAKE_CROSSCOMPILING_EMULATOR="$emulator"
    cmake --build "$client_build" --parallel
    # bash 3.2 treats an empty array as unbound under set -u.
    (cd "$repo_root" && cargo build -p bahamut-launcher-shell ${cargo_flags[@]+"${cargo_flags[@]}"})
fi

for artifact in bahamut-loader.exe bahamut.dll screenshot.dll discord-rpc.dll; do
    [[ -f "$client_build/$artifact" ]] || fail "missing client build output: $client_build/$artifact"
done
[[ -x "$launcher_binary" ]] || fail "missing launcher build output: $launcher_binary"

if [[ $run_tests -eq 1 ]]; then
    managed_runtime="$HOME/Library/Application Support/com.BahamutXIV.Launcher/runtime"
    if [[ -z "$wine_binary" && -x "$managed_runtime/wswine.bundle/bin/wine" ]]; then
        wine_binary="$managed_runtime/wswine.bundle/bin/wine"
        export BAHAMUT_WINE_DYLD_FALLBACK="$managed_runtime/Frameworks:$managed_runtime/Frameworks/GStreamer.framework/Versions/Current/lib:$managed_runtime/wswine.bundle/lib:/usr/local/lib:/usr/lib"
    fi
    if [[ -z "$wine_binary" ]]; then
        wine_binary="$(command -v wine || true)"
    fi
    [[ -n "$wine_binary" ]] || fail "no Wine binary for --test; start the launcher once to install the managed engine, or pass --wine"
    export BAHAMUT_WINE="$wine_binary"
    export WINEPREFIX="${wine_prefix:-$out_root/wine-test-prefix}"
    export WINEDEBUG="${WINEDEBUG:--all}"
    mkdir -p "$WINEPREFIX"
    ctest --test-dir "$client_build" --output-on-failure --timeout 900
fi

mkdir -p "$out_root"
staging="$(mktemp -d "$out_root/package-staging.XXXXXX")"
cleanup() {
    rm -rf "$staging"
}
trap cleanup EXIT

"$script_dir/stage-unix-release.sh" \
    --launcher "$launcher_binary" \
    --client-build "$client_build" \
    --destination "$staging"

case "$destination" in
    "$out_root"/*) ;;
    *) fail "package destination escaped the repository output root: $destination" ;;
esac
[[ -z "$(find "$staging" -type l -print -quit)" ]] || fail "staging tree contains a symbolic link"

mkdir -p "$destination"
find "$staging" -type d | while IFS= read -r directory; do
    mkdir -p "$destination/${directory#"$staging"}"
done
find "$staging" -type f | while IFS= read -r file; do
    relative="${file#"$staging"/}"
    target="$destination/$relative"
    if [[ "$relative" == "scripts/default.txt" && -f "$target" ]]; then
        continue
    fi
    [[ ! -d "$target" ]] || fail "package file path is a directory: $target"
    cp -p "$file" "$target"
done

echo "Bahamut Launcher package: $destination"
echo "Run: $destination/bahamut-launcher"
