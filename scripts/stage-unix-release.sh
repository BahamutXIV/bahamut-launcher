#!/usr/bin/env bash
# Stage a Linux or macOS release tree with stage-windows-release.ps1's
# manifest minus bahamut-update-helper.exe, plus the llvm-mingw runtime's
# licenses/MinGW-w64-runtime-COPYING.txt; bahamut-launcher.exe loses its
# extension.
# The expected file and directory lists mirror
# windows_release_archive_manifest_and_cleanliness.ps1's Assert-ExactManifest,
# including the official DAT overlay's dynamic entries.
set -euo pipefail

script_dir="$(cd "$(dirname -- "$0")" && pwd -P)"
repo_root="$(cd "$script_dir/.." && pwd -P)"

launcher_binary=""
client_build_dir=""
destination=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --launcher)
            launcher_binary="${2:-}"
            shift 2
            ;;
        --client-build)
            client_build_dir="${2:-}"
            shift 2
            ;;
        --destination)
            destination="${2:-}"
            shift 2
            ;;
        *)
            echo "stage-unix-release.sh: unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

if [ -z "$launcher_binary" ] || [ -z "$client_build_dir" ] || [ -z "$destination" ]; then
    echo "usage: stage-unix-release.sh --launcher <binary> --client-build <dir> --destination <empty dir>" >&2
    exit 1
fi

if [ ! -f "$launcher_binary" ]; then
    echo "stage-unix-release.sh: launcher binary not found: $launcher_binary" >&2
    exit 1
fi
if [ ! -d "$client_build_dir" ]; then
    echo "stage-unix-release.sh: client build directory not found: $client_build_dir" >&2
    exit 1
fi

abs_file() {
    dir="$(cd "$(dirname -- "$1")" && pwd -P)"
    base="$(basename -- "$1")"
    printf '%s/%s\n' "$dir" "$base"
}
abs_dir() {
    (cd "$1" && pwd -P)
}

launcher_binary="$(abs_file "$launcher_binary")"
client_build_dir="$(abs_dir "$client_build_dir")"

if [ -e "$destination" ]; then
    if [ ! -d "$destination" ]; then
        echo "stage-unix-release.sh: destination exists and is not a directory: $destination" >&2
        exit 1
    fi
    if [ -n "$(ls -A "$destination" 2>/dev/null)" ]; then
        echo "stage-unix-release.sh: release staging destination is not empty: $destination" >&2
        exit 1
    fi
fi
mkdir -p "$destination"
destination="$(abs_dir "$destination")"

# Skeleton directories, mirroring stage-windows-release.ps1's New-Item loop.
skeleton_dirs=(
    "addons/chatlogs"
    "addons/zonename"
    "addons/packetlogger"
    "addons/combatparser"
    "addons/distance"
    "addons/targethp"
    "addons/fps"
    "addons/pos"
    "addons/wiki"
    "licenses"
    "plugins/dats"
    "config/addons"
    "config/plugins/screenshot"
    "scripts"
    "logs/launcher"
    "logs/chat"
    "logs/packets"
    "screenshots"
)
for d in "${skeleton_dirs[@]}"; do
    mkdir -p "$destination/$d"
done

copy_sources=()
copy_dests=()
add_copy() {
    copy_sources+=("$1")
    copy_dests+=("$2")
}

add_copy "$launcher_binary" "$destination/bahamut-launcher"
add_copy "$client_build_dir/bahamut-loader.exe" "$destination/bahamut-loader.exe"
add_copy "$client_build_dir/bahamut.dll" "$destination/bahamut.dll"
add_copy "$client_build_dir/screenshot.dll" "$destination/plugins/screenshot.dll"
add_copy "$client_build_dir/discord-rpc.dll" "$destination/plugins/discord-rpc.dll"
add_copy "$repo_root/LICENSE.md" "$destination/LICENSE.md"
add_copy "$repo_root/docs/getting-started.md" "$destination/README.md"
add_copy "$repo_root/client/vendor/minhook/LICENSE.txt" "$destination/licenses/MinHook-LICENSE.txt"
add_copy "$repo_root/client/vendor/imgui/LICENSE.txt" "$destination/licenses/Dear-ImGui-LICENSE.txt"
add_copy "$repo_root/client/vendor/lua/COPYRIGHT" "$destination/licenses/Lua-COPYRIGHT.txt"
add_copy "$repo_root/client/vendor/miniz/LICENSE" "$destination/licenses/Miniz-LICENSE.txt"
add_copy "$repo_root/src-tauri/ui/assets/licenses/Inter-OFL.txt" "$destination/licenses/Inter-OFL.txt"
add_copy "$repo_root/src-tauri/ui/assets/licenses/Cinzel-OFL.txt" "$destination/licenses/Cinzel-OFL.txt"
add_copy "$repo_root/src-tauri/ui/assets/licenses/JetBrainsMono-OFL.txt" "$destination/licenses/JetBrainsMono-OFL.txt"
add_copy "$repo_root/src-tauri/ui/assets/licenses/MinGW-w64-runtime-COPYING.txt" "$destination/licenses/MinGW-w64-runtime-COPYING.txt"

addon_names="chatlogs zonename packetlogger combatparser distance targethp fps pos wiki"
for name in $addon_names; do
    add_copy "$repo_root/addons/$name/addon.toml" "$destination/addons/$name/addon.toml"
    add_copy "$repo_root/addons/$name/$name.lua" "$destination/addons/$name/$name.lua"
done

add_copy "$repo_root/scripts/default.txt" "$destination/scripts/default.txt"

i=0
while [ "$i" -lt "${#copy_sources[@]}" ]; do
    src="${copy_sources[$i]}"
    dst="${copy_dests[$i]}"
    if [ ! -f "$src" ]; then
        echo "stage-unix-release.sh: release staging source is missing: $src" >&2
        exit 1
    fi
    mkdir -p "$(dirname -- "$dst")"
    cp "$src" "$dst"
    i=$((i + 1))
done

chmod +x "$destination/bahamut-launcher"

# Official DAT overlay: copy recursively, rejecting any symlink in the
# source tree (mirrors stage-windows-release.ps1's reparse-point checks).
overlay_source="$repo_root/plugins/dats/bahamut-dats-overlay"
overlay_dest="$destination/plugins/dats/bahamut-dats-overlay"

if [ -L "$overlay_source" ]; then
    echo "stage-unix-release.sh: official overlay package is a symlink: $overlay_source" >&2
    exit 1
fi
if [ ! -d "$overlay_source" ]; then
    echo "stage-unix-release.sh: official overlay package is missing: $overlay_source" >&2
    exit 1
fi
if [ ! -f "$overlay_source/overlay.toml" ]; then
    echo "stage-unix-release.sh: official overlay manifest is missing: $overlay_source" >&2
    exit 1
fi

mkdir -p "$overlay_dest"

overlay_expected_files=()
overlay_expected_dirs=("plugins/dats/bahamut-dats-overlay/")

while IFS= read -r -d '' entry; do
    if [ "$entry" = "$overlay_source" ]; then
        continue
    fi
    if [ -L "$entry" ]; then
        echo "stage-unix-release.sh: official overlay package contains a symlink: $entry" >&2
        exit 1
    fi
    relative="${entry#"$overlay_source"/}"
    dest_entry="$overlay_dest/$relative"
    if [ -d "$entry" ]; then
        mkdir -p "$dest_entry"
        overlay_expected_dirs+=("plugins/dats/bahamut-dats-overlay/$relative/")
    else
        mkdir -p "$(dirname -- "$dest_entry")"
        cp "$entry" "$dest_entry"
        overlay_expected_files+=("plugins/dats/bahamut-dats-overlay/$relative")
    fi
done < <(find "$overlay_source" -print0)

# Reject a symlink anywhere in the finished staged tree, not just the overlay.
staged_symlinks="$(find "$destination" -type l)"
if [ -n "$staged_symlinks" ]; then
    echo "stage-unix-release.sh: staged release tree contains a symlink:" >&2
    printf '%s\n' "$staged_symlinks" >&2
    exit 1
fi

expected_files=(
    'bahamut-launcher'
    'bahamut-loader.exe'
    'bahamut.dll'
    'plugins/screenshot.dll'
    'plugins/discord-rpc.dll'
    'LICENSE.md'
    'README.md'
    'licenses/MinHook-LICENSE.txt'
    'licenses/Dear-ImGui-LICENSE.txt'
    'licenses/Lua-COPYRIGHT.txt'
    'licenses/Inter-OFL.txt'
    'licenses/Cinzel-OFL.txt'
    'licenses/JetBrainsMono-OFL.txt'
    'licenses/Miniz-LICENSE.txt'
    'licenses/MinGW-w64-runtime-COPYING.txt'
    'addons/chatlogs/addon.toml'
    'addons/chatlogs/chatlogs.lua'
    'addons/zonename/addon.toml'
    'addons/zonename/zonename.lua'
    'addons/packetlogger/addon.toml'
    'addons/packetlogger/packetlogger.lua'
    'addons/combatparser/addon.toml'
    'addons/combatparser/combatparser.lua'
    'addons/distance/addon.toml'
    'addons/distance/distance.lua'
    'addons/targethp/addon.toml'
    'addons/targethp/targethp.lua'
    'addons/fps/addon.toml'
    'addons/fps/fps.lua'
    'addons/pos/addon.toml'
    'addons/pos/pos.lua'
    'addons/wiki/addon.toml'
    'addons/wiki/wiki.lua'
    'scripts/default.txt'
)
if [ "${#overlay_expected_files[@]}" -gt 0 ]; then
    expected_files+=("${overlay_expected_files[@]}")
fi

expected_dirs=(
    "addons/"
    "addons/chatlogs/"
    "addons/zonename/"
    "addons/packetlogger/"
    "addons/combatparser/"
    "addons/distance/"
    "addons/targethp/"
    "addons/fps/"
    "addons/pos/"
    "addons/wiki/"
    "licenses/"
    "config/"
    "config/addons/"
    "logs/packets/"
    "config/plugins/"
    "config/plugins/screenshot/"
    "logs/"
    "logs/chat/"
    "logs/launcher/"
    "plugins/"
    "plugins/dats/"
    "screenshots/"
    "scripts/"
)
if [ "${#overlay_expected_dirs[@]}" -gt 0 ]; then
    expected_dirs+=("${overlay_expected_dirs[@]}")
fi

assert_exact_manifest() {
    label="$1"
    expected_sorted="$2"
    actual_sorted="$3"
    if [ "$expected_sorted" = "$actual_sorted" ]; then
        return 0
    fi
    missing="$(comm -23 <(printf '%s\n' "$expected_sorted") <(printf '%s\n' "$actual_sorted") || true)"
    unexpected="$(comm -13 <(printf '%s\n' "$expected_sorted") <(printf '%s\n' "$actual_sorted") || true)"
    echo "stage-unix-release.sh: $label manifest mismatch" >&2
    if [ -n "$missing" ]; then
        echo "missing:" >&2
        printf '%s\n' "$missing" >&2
    fi
    if [ -n "$unexpected" ]; then
        echo "unexpected:" >&2
        printf '%s\n' "$unexpected" >&2
    fi
    exit 1
}

# Informational listing, mirroring stage-windows-release.ps1's final output.
find "$destination" -type f -print0 |
    while IFS= read -r -d '' f; do
        rel="${f#"$destination"/}"
        size="$(wc -c < "$f" | tr -d ' ')"
        printf '%s (%s bytes)\n' "$rel" "$size"
    done | LC_ALL=C sort

expected_files_sorted="$(printf '%s\n' "${expected_files[@]}" | LC_ALL=C sort)"
actual_files_sorted="$(
    find "$destination" -type f -print0 |
        while IFS= read -r -d '' f; do
            printf '%s\n' "${f#"$destination"/}"
        done | LC_ALL=C sort
)"
assert_exact_manifest "staging file" "$expected_files_sorted" "$actual_files_sorted"

expected_dirs_sorted="$(printf '%s\n' "${expected_dirs[@]}" | LC_ALL=C sort)"
actual_dirs_sorted="$(
    find "$destination" -mindepth 1 -type d -print0 |
        while IFS= read -r -d '' d; do
            printf '%s/\n' "${d#"$destination"/}"
        done | LC_ALL=C sort
)"
assert_exact_manifest "staging directory" "$expected_dirs_sorted" "$actual_dirs_sorted"

file_count="$(printf '%s\n' "$actual_files_sorted" | wc -l | tr -d ' ')"
dir_count="$(printf '%s\n' "$actual_dirs_sorted" | wc -l | tr -d ' ')"
echo "stage-unix-release.sh: PASS ($file_count files, $dir_count directories)"
