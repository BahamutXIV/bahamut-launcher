#!/usr/bin/env bash
# Package the macOS launcher as "Bahamut Launcher.app" around the exact release
# manifest that stage-unix-release.sh stages.
#
#   scripts/package-macos-app.sh --launcher <universal mach-o>
#                                --client-build <dir>
#                                --destination <empty or missing dir>
#                                [--sign <identity>]
#
# The launcher becomes Contents/MacOS/bahamut-launcher and every other staged
# file keeps its relative path under Contents/Resources. The staged empty
# writable directories are dropped: the bundle is read-only, and the launcher
# keeps its writable state in ~/.bahamut-launcher.
#
# --sign  Developer ID identity, signed with the hardened runtime, a secure
#         timestamp, and packaging/macos/entitlements.plist, inner executable
#         first. The default identity "-" signs ad hoc.
#
# macOS only. The script neither builds the inputs nor checks their
# architectures.
set -euo pipefail

name="package-macos-app.sh"
usage="usage: $name --launcher <universal mach-o> --client-build <dir> --destination <empty or missing dir> [--sign <identity>]"

fail() {
    echo "$name: $*" >&2
    exit 1
}

if [ "$(uname -s)" != "Darwin" ]; then
    fail "macOS only: codesign, plutil, and xattr are required"
fi

script_dir="$(cd "$(dirname -- "$0")" && pwd -P)"
repo_root="$(cd "$script_dir/.." && pwd -P)"

launcher_binary=""
client_build_dir=""
destination=""
identity="-"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --launcher | --client-build | --destination | --sign)
            if [ "$#" -lt 2 ] || [ -z "$2" ]; then
                fail "$1 requires a value"
            fi
            case "$1" in
                --launcher) launcher_binary="$2" ;;
                --client-build) client_build_dir="$2" ;;
                --destination) destination="$2" ;;
                --sign) identity="$2" ;;
            esac
            shift 2
            ;;
        *)
            fail "unknown argument: $1"
            ;;
    esac
done

if [ -z "$launcher_binary" ] || [ -z "$client_build_dir" ] || [ -z "$destination" ]; then
    echo "$usage" >&2
    exit 1
fi

plist_template="$repo_root/packaging/macos/Info.plist.in"
entitlements="$repo_root/packaging/macos/entitlements.plist"
icon="$repo_root/src-tauri/icons/icon.icns"
tauri_conf="$repo_root/src-tauri/tauri.conf.json"
for input in "$plist_template" "$entitlements" "$icon" "$tauri_conf"; do
    [ -f "$input" ] || fail "required repository file is missing: $input"
done

if [ -L "$destination" ]; then
    fail "destination is a symbolic link: $destination"
fi
if [ -e "$destination" ]; then
    [ -d "$destination" ] || fail "destination exists and is not a directory: $destination"
    if [ -n "$(ls -A "$destination")" ]; then
        fail "destination is not empty: $destination"
    fi
fi

version="$(python3 -c '
import json, re, sys
with open(sys.argv[1], encoding="utf-8") as f:
    doc = json.load(f)
value = doc.get("version") if isinstance(doc, dict) else None
if not isinstance(value, str) or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", value):
    sys.exit("package-macos-app.sh: tauri.conf.json .version is not MAJOR.MINOR.PATCH: %r" % (value,))
print(value)
' "$tauri_conf")"

work="$(mktemp -d "${TMPDIR:-/tmp}/package-macos-app.XXXXXX")"
app=""
cleanup() {
    status=$?
    rm -rf "$work"
    # A failed run leaves no partial bundle behind.
    if [ "$status" -ne 0 ] && [ -n "$app" ]; then
        rm -rf "$app"
    fi
}
trap cleanup EXIT

stage="$work/stage"
bash "$script_dir/stage-unix-release.sh" \
    --launcher "$launcher_binary" \
    --client-build "$client_build_dir" \
    --destination "$stage"

if [ -L "$stage/bahamut-launcher" ] || [ ! -f "$stage/bahamut-launcher" ]; then
    fail "staged release tree has no regular bahamut-launcher"
fi

mkdir -p "$destination"
destination="$(cd "$destination" && pwd -P)"
app="$destination/Bahamut Launcher.app"
contents="$app/Contents"
resources="$contents/Resources"
mkdir -p "$contents/MacOS" "$resources"

expected_files="Contents/Info.plist
Contents/MacOS/bahamut-launcher
Contents/Resources/icon.icns"

# Only files are copied, so the empty skeleton directories never reach the
# bundle while their non-empty siblings (scripts/) do.
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
    if [ "$rel" = "bahamut-launcher" ]; then
        continue
    fi
    mkdir -p "$resources/$(dirname -- "$rel")"
    cp "$entry" "$resources/$rel"
    expected_files="$expected_files
Contents/Resources/$rel"
done < <(find "$stage" -mindepth 1 -print0)

mv "$stage/bahamut-launcher" "$contents/MacOS/bahamut-launcher"
chmod 755 "$contents/MacOS/bahamut-launcher"
if [ -e "$resources/icon.icns" ]; then
    fail "staged release tree collides with the bundle icon: icon.icns"
fi
cp "$icon" "$resources/icon.icns"

sed "s/@VERSION@/$version/g" "$plist_template" > "$contents/Info.plist"
if grep -q '@[A-Z_]*@' "$contents/Info.plist"; then
    fail "Info.plist has an unresolved placeholder"
fi
plutil -lint "$contents/Info.plist" >/dev/null

irregular="$(find "$app" -mindepth 1 ! -type f ! -type d)"
if [ -n "$irregular" ]; then
    echo "$name: app bundle contains a symbolic link or non-regular file:" >&2
    printf '%s\n' "$irregular" >&2
    exit 1
fi

# BSD cp carries Finder info and other extended attributes, which codesign
# rejects in a strict seal.
xattr -cr "$app"

expected_sorted="$(printf '%s\n' "$expected_files" | LC_ALL=C sort)"
actual_sorted="$(
    find "$app" -type f -print0 |
        while IFS= read -r -d '' f; do
            printf '%s\n' "${f#"$app"/}"
        done | LC_ALL=C sort
)"
if [ "$expected_sorted" != "$actual_sorted" ]; then
    missing="$(comm -23 <(printf '%s\n' "$expected_sorted") <(printf '%s\n' "$actual_sorted") || true)"
    unexpected="$(comm -13 <(printf '%s\n' "$expected_sorted") <(printf '%s\n' "$actual_sorted") || true)"
    echo "$name: app bundle file manifest mismatch" >&2
    if [ -n "$missing" ]; then
        echo "missing:" >&2
        printf '%s\n' "$missing" >&2
    fi
    if [ -n "$unexpected" ]; then
        echo "unexpected:" >&2
        printf '%s\n' "$unexpected" >&2
    fi
    exit 1
fi

empty_dirs="$(find "$app" -type d -empty)"
if [ -n "$empty_dirs" ]; then
    echo "$name: app bundle contains an empty directory:" >&2
    printf '%s\n' "$empty_dirs" >&2
    exit 1
fi

# Never --deep. Developer ID signs inside out so the executable itself carries
# the hardened runtime and entitlements.
if [ "$identity" = "-" ]; then
    codesign --force --sign - "$app"
    signature="ad hoc"
else
    codesign --force --options runtime --timestamp --entitlements "$entitlements" \
        --sign "$identity" "$contents/MacOS/bahamut-launcher"
    codesign --force --options runtime --timestamp --entitlements "$entitlements" \
        --sign "$identity" "$app"
    signature="Developer ID"
fi
codesign --verify --strict --verbose=2 "$app"

file_count="$(find "$app" -type f | wc -l | tr -d ' ')"
echo "$name: PASS ($file_count files, $signature signature): $app"
