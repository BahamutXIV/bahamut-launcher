#!/usr/bin/env bash
# Check a zipped "Bahamut Launcher.app" the way the release publishes it: every
# entry sits under the bundle, no AppleDouble or __MACOSX metadata, and an
# extracted copy carries a strict code seal and both CPU slices.
#
#   scripts/check-macos-app-zip.sh --zip <archive> [--signed]
#
# --signed  Also require a stapled notarization ticket and Gatekeeper acceptance
#           (xcrun stapler validate, spctl -a -t exec) on the extracted copy.
#
# macOS only.
set -euo pipefail

name="check-macos-app-zip.sh"
usage="usage: $name --zip <archive> [--signed]"

fail() {
    echo "$name: $*" >&2
    exit 1
}

if [ "$(uname -s)" != "Darwin" ]; then
    fail "macOS only: ditto, codesign, and lipo are required"
fi

zip=""
signed=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        --zip)
            if [ "$#" -lt 2 ] || [ -z "$2" ]; then
                fail "--zip requires a value"
            fi
            zip="$2"
            shift 2
            ;;
        --signed)
            signed=1
            shift
            ;;
        *)
            fail "unknown argument: $1"
            ;;
    esac
done
if [ -z "$zip" ]; then
    echo "$usage" >&2
    exit 1
fi
[ -f "$zip" ] || fail "archive not found: $zip"

work="$(mktemp -d "${TMPDIR:-/tmp}/check-macos-app-zip.XXXXXX")"
trap 'rm -rf "$work"' EXIT

entries="$work/entries.txt"
unzip -Z1 "$zip" > "$entries"
if [ ! -s "$entries" ]; then
    fail "archive has no entries: $zip"
fi
bad=0
while IFS= read -r entry; do
    case "$entry" in
        "Bahamut Launcher.app/"*) ;;
        *)
            echo "$name: entry outside the app bundle: $entry" >&2
            bad=1
            ;;
    esac
    case "/$entry" in
        */__MACOSX/* | */._*)
            echo "$name: entry is macOS metadata: $entry" >&2
            bad=1
            ;;
    esac
done < "$entries"
if [ "$bad" -ne 0 ]; then
    exit 1
fi

ditto -x -k "$zip" "$work/extract"
app="$work/extract/Bahamut Launcher.app"
[ -d "$app" ] || fail "extraction did not produce Bahamut Launcher.app"
codesign --verify --strict --verbose=2 "$app"

archs="$(lipo -archs "$app/Contents/MacOS/bahamut-launcher")"
echo "$name: universal archs: $archs"
for arch in arm64 x86_64; do
    case " $archs " in
        *" $arch "*) ;;
        *) fail "extracted app is missing the $arch slice" ;;
    esac
done

if [ "$signed" -eq 1 ]; then
    xcrun stapler validate "$app"
    spctl -a -vvv -t exec "$app"
fi

echo "$name: PASS ($(wc -l < "$entries" | tr -d ' ') entries): $zip"
