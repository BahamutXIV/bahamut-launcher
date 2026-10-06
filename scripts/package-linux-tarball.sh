#!/usr/bin/env bash
# Package the Linux launcher as <label>.tar.gz: the tree stage-unix-release.sh
# stages plus the install scripts, the desktop entry, and the icons, checked
# against a fixed file manifest before and after archiving. The archive's
# README.md is packaging/linux/README.md; the README.md the stage script copies
# is dropped.
#
#   scripts/package-linux-tarball.sh --launcher <elf> --client-build <dir>
#                                    --label <asset name> --output <dir>
#
# Writes <output>/<label>.tar.gz with the single top-level directory
# bahamut-launcher/ and <output>/<label>.tar.gz.sha256 in shasum -a 256
# format. The staged empty writable directories are dropped: the launcher
# keeps its writable state in ~/.bahamut-launcher when the package marker is
# present. The archive mtime is SOURCE_DATE_EPOCH, else the HEAD commit time.
set -euo pipefail

name="package-linux-tarball.sh"
usage="usage: $name --launcher <elf> --client-build <dir> --label <asset name> --output <dir>"

fail() {
    echo "$name: $*" >&2
    exit 1
}

script_dir="$(cd "$(dirname -- "$0")" && pwd -P)"
repo_root="$(cd "$script_dir/.." && pwd -P)"

launcher_binary=""
client_build_dir=""
label=""
output=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --launcher | --client-build | --label | --output)
            if [ "$#" -lt 2 ] || [ -z "$2" ]; then
                fail "$1 requires a value"
            fi
            case "$1" in
                --launcher) launcher_binary="$2" ;;
                --client-build) client_build_dir="$2" ;;
                --label) label="$2" ;;
                --output) output="$2" ;;
            esac
            shift 2
            ;;
        *)
            fail "unknown argument: $1"
            ;;
    esac
done

if [ -z "$launcher_binary" ] || [ -z "$client_build_dir" ] || [ -z "$label" ] || [ -z "$output" ]; then
    echo "$usage" >&2
    exit 1
fi

case "$label" in
    .* | *[!A-Za-z0-9._-]*) fail "the label must be a file name of letters, digits, '.', '_', and '-': $label" ;;
esac

if tar --version 2>/dev/null | head -n 1 | grep -q 'GNU tar'; then
    gnu_tar="tar"
elif command -v gtar >/dev/null 2>&1 && gtar --version 2>/dev/null | head -n 1 | grep -q 'GNU tar'; then
    gnu_tar="gtar"
else
    fail "GNU tar is required (as tar or gtar) for a normalized archive"
fi
command -v gzip >/dev/null 2>&1 || fail "gzip is required"
if command -v shasum >/dev/null 2>&1; then
    sha256=(shasum -a 256)
elif command -v sha256sum >/dev/null 2>&1; then
    sha256=(sha256sum)
else
    fail "shasum or sha256sum is required"
fi

linux="$repo_root/packaging/linux"
icon_sizes=(48x48 128x128 256x256)
extras_src=(
    "$linux/README.md"
    "$linux/package-marker.txt"
    "$linux/install.sh"
    "$linux/install-dependencies.sh"
    "$linux/Makefile"
    "$linux/bahamut-launcher.desktop"
)
extras_dst=(
    "README.md"
    ".bahamut-launcher-package"
    "install.sh"
    "install-dependencies.sh"
    "Makefile"
    "share/applications/bahamut-launcher.desktop"
)
for size in "${icon_sizes[@]}"; do
    extras_src+=("$linux/icons/hicolor/$size/apps/bahamut-launcher.png")
    extras_dst+=("share/icons/hicolor/$size/apps/bahamut-launcher.png")
done
for source in "${extras_src[@]}"; do
    if [ -L "$source" ] || [ ! -f "$source" ]; then
        case "$source" in
            */icons/*) fail "icon is missing: $source (generate it with: cargo run --release --manifest-path tools/icon-gen/Cargo.toml)" ;;
            *) fail "required repository file is missing or not a regular file: $source" ;;
        esac
    fi
done

if [ -n "${SOURCE_DATE_EPOCH:-}" ]; then
    epoch="$SOURCE_DATE_EPOCH"
else
    epoch="$(git -C "$repo_root" log -1 --format=%ct 2>/dev/null)" ||
        fail "set SOURCE_DATE_EPOCH: the HEAD commit time could not be read from $repo_root"
fi
case "$epoch" in
    '' | *[!0-9]*) fail "SOURCE_DATE_EPOCH is not a whole number of seconds: $epoch" ;;
esac

if [ -L "$output" ]; then
    fail "output is a symbolic link: $output"
fi
if [ -e "$output" ] && [ ! -d "$output" ]; then
    fail "output exists and is not a directory: $output"
fi
archive_name="$label.tar.gz"
mkdir -p "$output"
output="$(cd "$output" && pwd -P)"
for existing in "$output/$archive_name" "$output/$archive_name.sha256"; do
    if [ -e "$existing" ] || [ -L "$existing" ]; then
        fail "refusing to overwrite $existing"
    fi
done

work="$(mktemp -d "${TMPDIR:-/tmp}/package-linux-tarball.XXXXXX")"
trap 'rm -rf "$work"' EXIT

stage="$work/stage"
bash "$script_dir/stage-unix-release.sh" \
    --launcher "$launcher_binary" \
    --client-build "$client_build_dir" \
    --destination "$stage"

if [ -L "$stage/bahamut-launcher" ] || [ ! -f "$stage/bahamut-launcher" ]; then
    fail "staged release tree has no regular bahamut-launcher"
fi

top="bahamut-launcher"
tree="$work/root/$top"
mkdir -p "$tree"

# The expected manifest is independent of what the stage produced.
addon_ids=(chatlogs zonename packetlogger combatparser distance targethp fps pos wiki targetlines)
license_files=(
    MinHook-LICENSE.txt Dear-ImGui-LICENSE.txt Lua-COPYRIGHT.txt Miniz-LICENSE.txt
    Inter-OFL.txt Cinzel-OFL.txt JetBrainsMono-OFL.txt MinGW-w64-runtime-COPYING.txt
)
expected_list=(
    bahamut-launcher bahamut-loader.exe bahamut.dll
    plugins/screenshot.dll plugins/discord-rpc.dll
    LICENSE.md scripts/default.txt
)
for license in "${license_files[@]}"; do
    expected_list+=("licenses/$license")
done
for addon in "${addon_ids[@]}"; do
    expected_list+=("addons/$addon/addon.toml" "addons/$addon/$addon.lua")
done
expected_list+=("${extras_dst[@]}")
overlay_source="$repo_root/plugins/dats/bahamut-dats-overlay"
newline=$'\n'
if [ -d "$overlay_source" ]; then
    while IFS= read -r -d '' entry; do
        rel="${entry#"$overlay_source"/}"
        case "$rel" in
            *"$newline"*) fail "overlay file name contains a newline: $rel" ;;
        esac
        expected_list+=("plugins/dats/bahamut-dats-overlay/$rel")
    done < <(find "$overlay_source" -type f -print0)
fi

# Only files are copied, so the empty skeleton directories never reach the
# archive while their non-empty siblings (scripts/) do. The staged README.md
# is skipped; packaging/linux/README.md is copied in its place below.
while IFS= read -r -d '' entry; do
    rel="${entry#"$stage"/}"
    if [ -L "$entry" ]; then
        fail "staged release tree contains a symbolic link: $rel"
    fi
    if [ -d "$entry" ]; then
        continue
    fi
    if [ ! -f "$entry" ]; then
        fail "staged release tree entry is not a regular file: $rel"
    fi
    if [ "$rel" = "README.md" ]; then
        continue
    fi
    mkdir -p "$tree/$(dirname -- "$rel")"
    cp "$entry" "$tree/$rel"
done < <(find "$stage" -mindepth 1 -print0)

i=0
while [ "$i" -lt "${#extras_src[@]}" ]; do
    rel="${extras_dst[$i]}"
    if [ -e "$tree/$rel" ]; then
        fail "staged release tree collides with a packaging file: $rel"
    fi
    mkdir -p "$tree/$(dirname -- "$rel")"
    cp "${extras_src[$i]}" "$tree/$rel"
    i=$((i + 1))
done

find "$tree" -type d -exec chmod 0755 {} +
find "$tree" -type f -exec chmod 0644 {} +
chmod 0755 "$tree/bahamut-launcher" "$tree/install.sh" "$tree/install-dependencies.sh"

irregular="$(find "$tree" ! -type f ! -type d)"
if [ -n "$irregular" ]; then
    echo "$name: package tree contains a symbolic link or non-regular file:" >&2
    printf '%s\n' "$irregular" >&2
    exit 1
fi

empty_dirs="$(find "$tree" -type d -empty)"
if [ -n "$empty_dirs" ]; then
    echo "$name: package tree contains an empty directory:" >&2
    printf '%s\n' "$empty_dirs" >&2
    exit 1
fi

assert_same() {
    local label_text="$1" expected="$2" actual="$3" missing unexpected
    if [ "$expected" = "$actual" ]; then
        return 0
    fi
    missing="$(comm -23 <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") || true)"
    unexpected="$(comm -13 <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") || true)"
    echo "$name: $label_text mismatch" >&2
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

expected_sorted="$(printf '%s\n' "${expected_list[@]}" | LC_ALL=C sort)"
actual_sorted="$(
    find "$tree" -type f -print0 |
        while IFS= read -r -d '' f; do
            printf '%s\n' "${f#"$tree"/}"
        done | LC_ALL=C sort
)"
assert_same "package file manifest" "$expected_sorted" "$actual_sorted"

executables="bahamut-launcher
install-dependencies.sh
install.sh"
actual_modes="$(
    find "$tree" -type f -perm -0100 -print0 |
        while IFS= read -r -d '' f; do
            printf '%s\n' "${f#"$tree"/}"
        done | LC_ALL=C sort
)"
assert_same "executable file set" "$executables" "$actual_modes"

partial="$work/$archive_name"
(
    cd "$work/root"
    LC_ALL=C "$gnu_tar" --sort=name --owner=0 --group=0 --numeric-owner \
        --mode=go-w,a+rX --mtime="@$epoch" -cf - "$top"
) | gzip -9n > "$partial"

listed="$(
    "$gnu_tar" --quoting-style=literal -tzf "$partial" | while IFS= read -r member; do
        case "$member" in
            "$top"/*/) ;;
            "$top"/) ;;
            "$top"/*) printf '%s\n' "${member#"$top"/}" ;;
            *) fail "archive member outside $top/: $member" ;;
        esac
    done | LC_ALL=C sort
)"
assert_same "archive member" "$expected_sorted" "$listed"

mv "$partial" "$output/$archive_name"
(
    cd "$output"
    "${sha256[@]}" "$archive_name" > "$archive_name.sha256"
)

file_count="$(printf '%s\n' "$expected_sorted" | wc -l | tr -d ' ')"
echo "$name: PASS ($file_count files): $output/$archive_name"
