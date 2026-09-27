#!/usr/bin/env bash
# Build, stage, and start the launcher on macOS or Linux.
#
#   scripts/build-and-run-unix.sh [build-unix-package.sh options]
#
# Every argument goes to scripts/build-unix-package.sh (for example --release,
# --test, or --skip-build to start the last build again). The published
# launcher then runs in the foreground from its package directory, so Play
# takes the extension launch and the launcher log lands beside it under
# logs/launcher. RUST_LOG is passed through when set.
set -euo pipefail

script_dir="$(cd "$(dirname -- "$0")" && pwd -P)"

for argument in "$@"; do
    case "$argument" in
        -h | --help)
            sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
            echo
            "$script_dir/build-unix-package.sh" --help
            exit 0
            ;;
    esac
done

# The build script's stdout is shown and kept so the package line can be read.
output="$("$script_dir/build-unix-package.sh" "$@" | tee /dev/stderr)"
package="$(printf '%s\n' "$output" | sed -n 's/^Bahamut Launcher package: //p' | tail -n 1)"
if [[ -z "$package" || ! -x "$package/bahamut-launcher" ]]; then
    echo "build-and-run-unix.sh: no published launcher to start" >&2
    exit 1
fi

cd "$package"
exec ./bahamut-launcher
