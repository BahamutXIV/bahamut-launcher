#!/usr/bin/env bash
# CTest emulator for the cross-compiled client module: run a Win32 test
# executable under Wine, converting absolute Unix path arguments to the
# prefix's drive letters so argv-driven tests see Windows paths.
#
#   cmake ... -DCMAKE_CROSSCOMPILING_EMULATOR=$PWD/client/tools/run-under-wine.sh
#
# BAHAMUT_WINE selects the wine binary (default: wine on PATH). WINEPREFIX
# selects the prefix (default: Wine's own default, ~/.wine). The drive map is
# read from <prefix>/dosdevices and the longest matching drive root wins.
# BAHAMUT_WINE_DYLD_FALLBACK is exported as DYLD_FALLBACK_LIBRARY_PATH for
# the macOS managed engine.
set -euo pipefail

wine="${BAHAMUT_WINE:-wine}"
prefix="${WINEPREFIX:-$HOME/.wine}"
if [[ -n "${BAHAMUT_WINE_DYLD_FALLBACK:-}" ]]; then
    export DYLD_FALLBACK_LIBRARY_PATH="$BAHAMUT_WINE_DYLD_FALLBACK"
fi
export WINEDEBUG="${WINEDEBUG:--all}"
# The input tests load the stub xinput1_3.dll beside the test client; Wine
# prefers its builtin unless the override says native first. A caller's
# overrides are kept and this entry is appended when it is absent.
case ";${WINEDLLOVERRIDES:-};" in
    *";xinput1_3="* | *",xinput1_3="*) ;;
    *) export WINEDLLOVERRIDES="${WINEDLLOVERRIDES:+$WINEDLLOVERRIDES;}xinput1_3=n,b" ;;
esac

letters=()
roots=()
if [[ ! -d "$prefix/dosdevices" ]]; then
    # A prefix Wine has not created yet gets its default z: link to /.
    letters=(Z)
    roots=(/)
fi
if [[ -d "$prefix/dosdevices" ]]; then
    for link in "$prefix"/dosdevices/[a-z]:; do
        [[ -L "$link" ]] || continue
        target="$(cd "$(dirname "$link")" 2>/dev/null && cd "$(readlink "$link")" 2>/dev/null && pwd -P)" || continue
        letter="$(basename "$link")"
        letters+=("$(printf '%s' "${letter:0:1}" | tr '[:lower:]' '[:upper:]')")
        roots+=("$target")
    done
fi

to_dos() {
    local unix="$1" best_index=-1 best_length=-1 i root rest
    for i in ${roots[@]+"${!roots[@]}"}; do
        root="${roots[$i]}"
        if [[ "$unix" == "$root" || "$unix" == "$root/"* || "$root" == "/" ]]; then
            if (( ${#root} > best_length )); then
                best_index=$i
                best_length=${#root}
            fi
        fi
    done
    if (( best_index < 0 )); then
        printf '%s' "$unix"
        return
    fi
    root="${roots[$best_index]}"
    if [[ "$root" == "/" ]]; then
        rest="$unix"
    else
        rest="${unix#"$root"}"
    fi
    rest="${rest//\//\\}"
    printf '%s:%s' "${letters[$best_index]}" "$rest"
}

args=()
for arg in "$@"; do
    if [[ "$arg" == /* ]]; then
        args+=("$(to_dos "$arg")")
    else
        args+=("$arg")
    fi
done

exec "$wine" "${args[@]}"
