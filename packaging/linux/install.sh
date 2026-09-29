#!/usr/bin/env bash
# Install the packaged launcher into PREFIX, or remove it again with
# --uninstall. Run it from the extracted archive; --uninstall also works from
# the installed payload copy in PKGDIR.
set -euo pipefail
umask 022

name="install.sh"
marker=".bahamut-launcher-package"
manifest_name=".bahamut-launcher-install-manifest"
files_name=".bahamut-launcher-install-files"
installer_key="X-Bahamut-Launcher-Installer"
move_doc="https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/troubleshooting.md#moving-a-beside-the-launcher-tree"
icon_sizes=(48x48 128x128 256x256)
payload_exec=(bahamut-launcher install.sh install-dependencies.sh)
payload_files=("$marker" bahamut-loader.exe bahamut.dll LICENSE.md README.md)
payload_dirs=(plugins addons scripts licenses share)
# Top-level names a managed pkgdir may hold; anything else may be state.
managed_names=("${payload_exec[@]}" "${payload_files[@]}" "${payload_dirs[@]}" "$manifest_name" "$files_name" Makefile)

usage() {
    cat <<'EOF'
usage: install.sh [--prefix DIR] [--pkgdir DIR] [--destdir DIR] [--skip-checks] [--force] [--uninstall] [--help]

Installs the launcher payload into PKGDIR, links PREFIX/bin/bahamut-launcher
to it, and adds an application menu entry and icons under PREFIX/share.

  --prefix DIR    install prefix; default ~/.local, or /usr/local as root
  --pkgdir DIR    payload directory; default PREFIX/lib/bahamut-launcher
  --destdir DIR   staging root prepended to every written path; a relative
                  DIR is resolved against the current directory
  --skip-checks   skip the install-dependencies.sh --check run
  --force         replace or remove a payload directory install.sh created
                  even when it no longer matches its install records;
                  everything in that directory is deleted
  --uninstall     remove what the install manifest in PKGDIR records; run
                  from an installed copy with no --prefix or --pkgdir, it
                  removes that copy's own install
  --help          show this help

PREFIX and DESTDIR are also read from the environment. The launcher keeps its
state in ~/.bahamut-launcher, which install and uninstall never touch.
EOF
}

fail() {
    echo "$name: $*" >&2
    exit 1
}

usage_error() {
    echo "$name: $*" >&2
    echo "Run '$name --help' for usage." >&2
    exit 2
}

warn() {
    echo "$name: warning: $*" >&2
}

prefix="${PREFIX:-}"
pkgdir=""
destdir="${DESTDIR:-}"
skip_checks=0
force=0
mode="install"
explicit=0
[ -z "$prefix" ] || explicit=1

while [ "$#" -gt 0 ]; do
    case "$1" in
        --prefix | --pkgdir | --destdir)
            [ "$#" -ge 2 ] || usage_error "$1 requires a value"
            case "$1" in
                --prefix) prefix="$2"; explicit=1 ;;
                --pkgdir) pkgdir="$2"; explicit=1 ;;
                --destdir) destdir="$2" ;;
            esac
            shift 2
            ;;
        --prefix=*) prefix="${1#*=}"; explicit=1; shift ;;
        --pkgdir=*) pkgdir="${1#*=}"; explicit=1; shift ;;
        --destdir=*) destdir="${1#*=}"; shift ;;
        --skip-checks) skip_checks=1; shift ;;
        --force) force=1; shift ;;
        --uninstall) mode="uninstall"; shift ;;
        -h | --help) usage; exit 0 ;;
        *) usage_error "unknown argument: $1" ;;
    esac
done

command -v sha256sum >/dev/null 2>&1 || fail "sha256sum (coreutils) is required"

newline=$'\n'
for pair in "prefix:$prefix" "pkgdir:$pkgdir" "destdir:$destdir"; do
    case "$pair" in
        *"$newline"*) fail "the ${pair%%:*} path contains a newline" ;;
    esac
done

# tidy PATH: collapse repeated slashes and drop a trailing one.
tidy() {
    local p="$1"
    while [ "${p#*//}" != "$p" ]; do
        p="${p//\/\//\/}"
    done
    [ "$p" = "/" ] || p="${p%/}"
    printf '%s' "$p"
}

# resolve_relative PATH: PATH resolved against PWD lexically; "." is dropped
# and ".." removes the preceding component.
resolve_relative() {
    local part parts=() out=()
    IFS=/ read -r -a parts <<< "$PWD/$1"
    for part in ${parts[@]+"${parts[@]}"}; do
        case "$part" in
            "" | .) ;;
            ..) [ "${#out[@]}" -eq 0 ] || out=("${out[@]:0:${#out[@]}-1}") ;;
            *) out+=("$part") ;;
        esac
    done
    local joined
    joined="$(printf '/%s' ${out[@]+"${out[@]}"})"
    printf '%s' "${joined:-/}"
}

# normalize LABEL PATH: an absolute, tidy path with no . or .. component, so
# every manifest entry is one --uninstall accepts.
normalize() {
    local p
    case "$2" in
        /*) ;;
        *) fail "the $1 must be an absolute path: $2" ;;
    esac
    p="$(tidy "$2")"
    case "$p/" in
        */./* | */../*) fail "the $1 must not contain a . or .. component: $2" ;;
    esac
    printf '%s' "$p"
}

src_dir="$(cd "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
src_manifest="$src_dir/$manifest_name"
src_is_installed=0
if [ -f "$src_manifest" ] && [ ! -L "$src_manifest" ]; then
    src_is_installed=1
fi

# An installed copy run with --uninstall and no location removes itself.
from_manifest=0
if [ "$mode" = "uninstall" ] && [ "$explicit" -eq 0 ] && [ "$src_is_installed" -eq 1 ]; then
    mapfile -t recorded < "$src_manifest"
    [ "${#recorded[@]}" -gt 0 ] || fail "the install manifest is empty: $src_manifest"
    pkgdir="${recorded[${#recorded[@]} - 1]}"
    for entry in "${recorded[@]}"; do
        case "$entry" in
            */bin/bahamut-launcher)
                prefix="${entry%/bin/bahamut-launcher}"
                [ -n "$prefix" ] || prefix="/"
                from_manifest=1
                break
                ;;
        esac
    done
    [ "$from_manifest" -eq 1 ] || fail "the install manifest $src_manifest records no bin entry"
fi

if [ -z "$prefix" ]; then
    if [ "$EUID" -eq 0 ]; then
        prefix="/usr/local"
    else
        [ -n "${HOME:-}" ] || fail "HOME is not set; pass --prefix"
        prefix="$HOME/.local"
    fi
fi
prefix="$(normalize prefix "$prefix")"
root="${prefix%/}"

[ -n "$pkgdir" ] || pkgdir="$root/lib/bahamut-launcher"
pkgdir="$(normalize pkgdir "$pkgdir")"
[ "$pkgdir" != "/" ] || fail "the pkgdir cannot be /"

if [ -n "$destdir" ]; then
    case "$destdir" in
        /*) ;;
        *) destdir="$(resolve_relative "$destdir")" ;;
    esac
    destdir="$(normalize destdir "$destdir")"
    [ "$destdir" != "/" ] || destdir=""
fi

# Installed paths, recorded without DESTDIR.
bin_path="$root/bin/bahamut-launcher"
desktop_path="$root/share/applications/bahamut-launcher.desktop"
hicolor="$root/share/icons/hicolor"
icon_paths=()
for size in "${icon_sizes[@]}"; do
    icon_paths+=("$hicolor/$size/apps/bahamut-launcher.png")
done
target="$destdir$pkgdir"
pkg_parent="$(dirname -- "$target")"

# under PATH DIR: PATH is DIR or lies inside it.
under() {
    case "$1/" in
        "${2%/}/"*) return 0 ;;
    esac
    return 1
}

nearest_existing() {
    local p="$1"
    while [ ! -e "$p" ] && [ ! -L "$p" ]; do
        p="$(dirname -- "$p")"
    done
    printf '%s' "$p"
}

# physical_path PATH: PATH with its nearest existing ancestor resolved.
physical_path() {
    local p e rest phys
    p="$(tidy "$1")"
    e="$(nearest_existing "$p")"
    rest="${p#"$e"}"
    if [ -d "$e" ]; then
        phys="$(cd "$e" 2>/dev/null && pwd -P)" || phys="$e"
    else
        phys="$(cd "$(dirname -- "$e")" 2>/dev/null && pwd -P)" || phys="$(dirname -- "$e")"
        phys="${phys%/}/$(basename -- "$e")"
    fi
    phys="${phys%/}"
    case "$rest" in
        "" | /*) ;;
        *) rest="/$rest" ;;
    esac
    if [ -z "$phys" ]; then
        printf '%s' "${rest:-/}"
    else
        printf '%s%s' "$phys" "$rest"
    fi
}

# overlaps A B: either path is, or lies inside, the other.
overlaps() {
    local a b
    a="$(physical_path "$1")"
    b="$(physical_path "$2")"
    under "$1" "$2" || under "$2" "$1" || under "$a" "$b" || under "$b" "$a"
}

# passwd_home KEY FIELD: the home of the passwd entry whose FIELD (1 = name,
# 3 = uid) equals KEY.
passwd_home() {
    if command -v getent >/dev/null 2>&1; then
        getent passwd "$1" 2>/dev/null | cut -d: -f6 | head -n 1 || true
    elif [ -r /etc/passwd ]; then
        awk -F: -v key="$1" -v field="$2" '$field == key { print $6; exit }' /etc/passwd 2>/dev/null || true
    fi
}

# HOME, plus as root the home of the user who ran sudo, doas, or pkexec.
home_dirs=()
invoking_homes=()
unresolved_invokers=()
add_home() {
    case "$1" in
        / | "") return 0 ;;
        /*) home_dirs+=("$(tidy "$1")" "$(physical_path "$1")") ;;
    esac
}
add_invoker() {
    local home
    home="$(passwd_home "$2" "$3")"
    case "$home" in
        /*) invoking_homes+=("$home") ;;
        *) unresolved_invokers+=("$1=$2") ;;
    esac
}
add_home "${HOME:-}"
if [ "$EUID" -eq 0 ]; then
    [ -z "${SUDO_USER:-}" ] || add_invoker SUDO_USER "$SUDO_USER" 1
    [ -z "${DOAS_USER:-}" ] || add_invoker DOAS_USER "$DOAS_USER" 1
    [ -z "${PKEXEC_UID:-}" ] || add_invoker PKEXEC_UID "$PKEXEC_UID" 3
fi
for h in ${invoking_homes[@]+"${invoking_homes[@]}"}; do
    add_home "$h"
done

in_home() {
    local h physical
    physical="$(physical_path "$1")"
    for h in ${home_dirs[@]+"${home_dirs[@]}"}; do
        if under "$1" "$h" || under "$physical" "$h"; then
            return 0
        fi
    done
    return 1
}

if [ "$src_is_installed" -eq 1 ] && [ "$mode" = "uninstall" ] &&
    [ "$(physical_path "$target")" != "$src_dir" ]; then
    if [ "$from_manifest" -eq 1 ]; then
        fail "the install manifest beside this script records $pkgdir, but this script runs from $src_dir; pass --destdir for a staged install, or --prefix and --pkgdir"
    fi
    fail "this script belongs to the install in $src_dir, but the resolved pkgdir is $target; run the install.sh inside $target to remove that install"
fi

state_dirs=()
case "${BAHAMUT_LAUNCHER_HOME:-}" in
    /*) state_dirs+=("$BAHAMUT_LAUNCHER_HOME") ;;
    *) [ -z "${HOME:-}" ] || state_dirs+=("$HOME/.bahamut-launcher") ;;
esac
for h in ${invoking_homes[@]+"${invoking_homes[@]}"}; do
    case "$h" in
        / | "") ;;
        /*) state_dirs+=("$h/.bahamut-launcher") ;;
    esac
done
for s in ${state_dirs[@]+"${state_dirs[@]}"}; do
    if overlaps "$target" "$s"; then
        fail "the pkgdir $target overlaps the launcher state directory $s; choose another --pkgdir"
    fi
done

write_roots=("$destdir$root/bin" "$destdir$root/share" "$pkg_parent")

if [ -z "$destdir" ]; then
    if [ "$EUID" -eq 0 ]; then
        # Without the invoking user's home, only well-known system prefixes are safe.
        if [ "${#unresolved_invokers[@]}" -gt 0 ]; then
            for p in "$prefix" "$pkgdir"; do
                if ! under "$p" /usr && ! under "$p" /opt && ! under "$p" /usr/local; then
                    fail "could not resolve the home directory of ${unresolved_invokers[*]}, so $p cannot be checked against it; pass an explicit --prefix under /usr, /opt, or /usr/local"
                fi
            done
        fi
        for p in "${write_roots[@]}" "$target"; do
            if in_home "$p"; then
                fail "as root this script does not write under a user's home directory ($p); pass a system --prefix, or run it without sudo for a per-user install"
            fi
        done
    else
        for p in "${write_roots[@]}"; do
            existing="$(nearest_existing "$p")"
            if [ ! -w "$existing" ]; then
                if in_home "$prefix"; then
                    fail "$existing is not writable"
                fi
                fail "$existing is not writable; run with sudo for a system prefix ($prefix)"
            fi
        done
    fi
fi

echo "$name: prefix  $prefix"
echo "$name: pkgdir  $pkgdir"
echo "$name: destdir ${destdir:-(none)}"

# make_dirs DIR: create each missing component with mode 0755.
make_dirs() {
    local p="$1" missing=()
    while [ ! -e "$p" ] && [ ! -L "$p" ]; do
        missing=("$p" ${missing[@]+"${missing[@]}"})
        p="$(dirname -- "$p")"
    done
    [ -d "$p" ] || fail "not a directory: $p"
    for p in ${missing[@]+"${missing[@]}"}; do
        install -d -m 0755 "$p"
    done
}

refresh_icon_cache() {
    [ -z "$destdir" ] || return 0
    if [ -f "$hicolor/icon-theme.cache" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then
        if ! gtk-update-icon-cache -q -t -f "$hicolor"; then
            warn "gtk-update-icon-cache failed; the icon appears after the next icon cache refresh"
        fi
    elif [ -d "$hicolor" ]; then
        touch "$hicolor" 2>/dev/null || true
    fi
}

link_points_into_pkgdir() {
    local link resolved physical
    link="$(readlink -- "$1")"
    if under "$link" "$pkgdir"; then
        return 0
    fi
    if [ -z "$destdir" ] && [ -d "$target" ]; then
        resolved="$(readlink -f -- "$1" 2>/dev/null || true)"
        physical="$(cd "$target" && pwd -P)"
        if [ -n "$resolved" ] && under "$resolved" "$physical"; then
            return 0
        fi
    fi
    return 1
}

# name_list ITEM...: up to ten items joined with ", ", then "and N more".
name_list() {
    local listed="" count=0 item
    for item in "$@"; do
        [ "$count" -ge 10 ] || listed="${listed:+$listed, }$item"
        count=$((count + 1))
    done
    [ "$count" -le 10 ] || listed="$listed and $((count - 10)) more"
    printf '%s' "$listed"
}

# same_content A B: A and B hold the same bytes; the run fails when either
# cannot be read.
same_content() {
    local a b
    a="$({ sha256sum < "$1"; } 2>/dev/null)" || fail "could not read $1 to compare it with $2"
    b="$({ sha256sum < "$2"; } 2>/dev/null)" || fail "could not read $2 to compare it with $1"
    [ "${a%% *}" = "${b%% *}" ]
}

# refuse_unmanaged DIR: DIR has no install manifest, so it may be an older
# beside-the-launcher tree holding launcher state.
refuse_unmanaged() {
    local dir="$1" entry names=() held
    if [ ! -d "$dir" ] || [ -L "$dir" ]; then
        fail "$dir exists, is not a directory, and is not managed by install.sh; nothing was changed. Launcher state may live there (see $move_doc); choose another --pkgdir"
    fi
    while IFS= read -r -d '' entry; do
        names+=("${entry##*/}")
    done < <(find "$dir" -mindepth 1 -maxdepth 1 -print0 2>/dev/null | LC_ALL=C sort -z)
    held="it is empty"
    [ "${#names[@]}" -eq 0 ] || held="it holds $(name_list "${names[@]}")"
    fail "$dir is not managed by install.sh (it has no $manifest_name) and launcher state may live there; $held. Nothing was changed. To move a beside-the-launcher tree see $move_doc, or choose another --pkgdir"
}

# recorded_rel PATH: PATH is a plain relative payload path.
recorded_rel() {
    case "/$1/" in
        // | *//* | */./* | */../*) return 1 ;;
    esac
    case "$1" in
        "$manifest_name" | "$files_name" | *"\\"*) return 1 ;;
    esac
    return 0
}

# flag TAG REL: record REL once in check_managed's found list.
flag() {
    [ -z "${flagged["k/$2"]+x}" ] || return 0
    flagged["k/$2"]=1
    found+=("$1 $2")
}

# check_managed DIR: DIR carries both install records and the marker, holds
# nothing its file list does not record, and every recorded file is intact;
# otherwise nothing in it may be replaced or removed.
check_managed() {
    local dir="$1"
    if [ -L "$dir/$manifest_name" ] || [ ! -f "$dir/$manifest_name" ]; then
        refuse_unmanaged "$dir"
    fi
    local problems=() found=() entry rel base a line status=0 out
    local re='^[0-9a-f]{64}  (.+)$' list="$dir/$files_name" list_ok=1
    # Keys carry a "k/" prefix so names such as @ or * stay plain keys.
    local -A recorded=() ancestors=() flagged=() is_dir=() has_flagged_child=()
    if [ -L "$dir/$marker" ] || [ ! -f "$dir/$marker" ]; then
        problems+=("it has no $marker")
        flagged["k/$marker"]=1
    fi
    if [ -L "$list" ] || [ ! -f "$list" ] || [ ! -r "$list" ]; then
        problems+=("it has no readable $files_name")
        list_ok=0
    else
        while IFS= read -r line || [ -n "$line" ]; do
            if [[ $line =~ $re ]] && recorded_rel "${BASH_REMATCH[1]}"; then
                rel="${BASH_REMATCH[1]}"
                recorded["k/$rel"]=1
                while [[ $rel == */* ]]; do
                    rel="${rel%/*}"
                    ancestors["k/$rel"]=1
                done
            else
                list_ok=0
            fi
        done < "$list"
        [ "$list_ok" -eq 1 ] || problems+=("its $files_name is malformed")
    fi

    if [ "$list_ok" -eq 0 ]; then
        while IFS= read -r -d '' entry; do
            base="${entry##*/}"
            for a in "${managed_names[@]}"; do
                [ "$base" != "$a" ] || continue 2
            done
            flag added "$base"
        done < <(find "$dir" -mindepth 1 -maxdepth 1 -print0 2>/dev/null | LC_ALL=C sort -z)
    else
        while IFS= read -r -d '' entry; do
            rel="${entry#"$dir"/}"
            if [ -d "$entry" ] && [ ! -L "$entry" ]; then
                if [ -n "${ancestors["k/$rel"]+x}" ] && [ -r "$entry" ] && [ -x "$entry" ]; then
                    continue
                fi
                is_dir["k/$rel"]=1
            elif [ "$rel" = "$manifest_name" ] || [ "$rel" = "$files_name" ]; then
                continue
            elif [ -n "${recorded["k/$rel"]+x}" ]; then
                if [ -f "$entry" ] && [ ! -L "$entry" ]; then
                    continue
                fi
                flag changed "$rel"
                continue
            fi
            if [ -n "${recorded["k/$rel"]+x}" ] || [ -n "${ancestors["k/$rel"]+x}" ]; then
                flag changed "$rel"
            else
                flag added "$rel"
            fi
        done < <(find "$dir" -mindepth 1 -print0 2>/dev/null | LC_ALL=C sort -z)

        out="$(cd "$dir" && LC_ALL=C sha256sum -c --quiet -- "$files_name" 2>/dev/null)" || status=$?
        local before="${#found[@]}"
        while IFS= read -r line; do
            case "$line" in
                *": FAILED open or read") flag missing "${line%: FAILED open or read}" ;;
                *": FAILED") flag changed "${line%: FAILED}" ;;
            esac
        done <<< "$out"
        if [ "$status" -ne 0 ] && [ "${#found[@]}" -eq "$before" ]; then
            problems+=("sha256sum could not verify its $files_name")
        fi
    fi

    # A directory is named only when nothing inside it is.
    local names=() item
    for item in ${found[@]+"${found[@]}"}; do
        rel="${item#* }"
        while [[ $rel == */* ]]; do
            rel="${rel%/*}"
            has_flagged_child["k/$rel"]=1
        done
    done
    for item in ${found[@]+"${found[@]}"}; do
        rel="${item#* }"
        if [ -n "${is_dir["k/$rel"]+x}" ] && [ -n "${has_flagged_child["k/$rel"]+x}" ]; then
            continue
        fi
        names+=("$item")
    done

    if [ "${#problems[@]}" -gt 0 ] || [ "${#names[@]}" -gt 0 ]; then
        local detail=""
        for item in ${problems[@]+"${problems[@]}"}; do
            detail="${detail:+$detail; }$item"
        done
        if [ "${#names[@]}" -gt 0 ]; then
            detail="${detail:+$detail; }$(name_list "${names[@]}")"
        fi
        if [ "$force" -eq 1 ]; then
            warn "$dir does not match its install records: $detail; --force replaces or removes it anyway"
            return 0
        fi
        fail "$dir does not match its install records: $detail; nothing was changed. Launcher state or player packages may live there; move them out (see $move_doc), then restore the marker and any changed or missing shipped file from the archive and run install.sh again, or pass --force to replace or remove the directory with everything in it"
    fi
}

uninstall() {
    [ ! -L "$target" ] || fail "$target is a symbolic link; not removing it"
    [ -d "$target" ] || fail "no installed launcher at $target"
    check_managed "$target"
    local manifest="$target/$manifest_name"
    local entries=()
    mapfile -t entries < "$manifest"
    [ "${#entries[@]}" -gt 0 ] || fail "the install manifest is empty: $manifest"
    local last="${entries[${#entries[@]} - 1]}"
    if [ "$last" != "$pkgdir" ]; then
        fail "the install manifest records $last, not $pkgdir; pass the pkgdir it was installed with"
    fi

    # Compare the icons before removing anything, so an unreadable icon stops
    # the run with nothing removed.
    local entry path size shipped i kept=0
    local -A identical_icons=()
    for ((i = 0; i < ${#entries[@]} - 1; i++)); do
        entry="${entries[$i]}"
        case "$entry" in
            /*/share/icons/hicolor/48x48/apps/bahamut-launcher.png | \
                /*/share/icons/hicolor/128x128/apps/bahamut-launcher.png | \
                /*/share/icons/hicolor/256x256/apps/bahamut-launcher.png) ;;
            *) continue ;;
        esac
        path="$destdir$entry"
        size="${entry%/apps/bahamut-launcher.png}"
        size="${size##*/}"
        shipped="$target/share/icons/hicolor/$size/apps/bahamut-launcher.png"
        if [ -f "$path" ] && [ ! -L "$path" ] && [ -f "$shipped" ] && same_content "$path" "$shipped"; then
            identical_icons["k/$entry"]=1
        fi
    done

    # The payload may hold this script; leave it before removing it.
    cd /

    for ((i = 0; i < ${#entries[@]} - 1; i++)); do
        entry="${entries[$i]}"
        case "$entry" in
            /*/../* | /*/./* | /../* | /./*)
                warn "ignoring an install manifest entry that is not a plain path: $entry"
                continue
                ;;
            /*) ;;
            *)
                warn "ignoring a relative install manifest entry: $entry"
                continue
                ;;
        esac
        path="$destdir$entry"
        case "$entry" in
            /*/bin/bahamut-launcher | /bin/bahamut-launcher)
                if [ -L "$path" ]; then
                    if link_points_into_pkgdir "$path"; then
                        rm -f -- "$path"
                    else
                        echo "$name: kept $path: it points outside $pkgdir"
                        kept=1
                    fi
                elif [ -e "$path" ]; then
                    echo "$name: kept $path: it is not a symbolic link"
                    kept=1
                fi
                ;;
            */share/applications/bahamut-launcher.desktop)
                if [ -f "$path" ] && [ ! -L "$path" ] && grep -qx "$installer_key=true" "$path"; then
                    rm -f -- "$path"
                elif [ -e "$path" ] || [ -L "$path" ]; then
                    echo "$name: kept $path: it was not written by install.sh"
                    kept=1
                fi
                ;;
            */share/icons/hicolor/48x48/apps/bahamut-launcher.png | \
                */share/icons/hicolor/128x128/apps/bahamut-launcher.png | \
                */share/icons/hicolor/256x256/apps/bahamut-launcher.png)
                if [ -n "${identical_icons["k/$entry"]+x}" ]; then
                    rm -f -- "$path"
                elif [ -e "$path" ] || [ -L "$path" ]; then
                    echo "$name: kept $path: it differs from the icon install.sh installed"
                    kept=1
                fi
                ;;
            *)
                warn "ignoring an unexpected install manifest entry: $entry"
                ;;
        esac
    done

    rm -rf -- "$target"
    refresh_icon_cache
    echo "$name: removed the launcher from $prefix"
    if [ "$kept" -ne 0 ]; then
        echo "$name: some paths were kept; see above"
    fi
    echo "$name: launcher state in ~/.bahamut-launcher was not touched"
}

if [ "$mode" = "uninstall" ]; then
    uninstall
    exit 0
fi

if [ "$(physical_path "$target")" = "$src_dir" ]; then
    fail "the source directory is the install target $pkgdir; run install.sh from the extracted archive"
fi
if under "$target" "$src_dir" || under "$(physical_path "$target")" "$src_dir"; then
    fail "the install target $target (destdir ${destdir:-(none)}, pkgdir $pkgdir) lies inside the source directory $src_dir"
fi

desktop_source="$src_dir/share/applications/bahamut-launcher.desktop"

for rel in "${payload_exec[@]}" "${payload_files[@]}"; do
    if [ -L "$src_dir/$rel" ] || [ ! -f "$src_dir/$rel" ]; then
        fail "$src_dir/$rel is missing or not a regular file; run install.sh from the extracted archive"
    fi
done
for rel in "${payload_dirs[@]}"; do
    if [ -L "$src_dir/$rel" ] || [ ! -d "$src_dir/$rel" ]; then
        fail "$src_dir/$rel is missing or not a directory"
    fi
    irregular="$(find "$src_dir/$rel" ! -type f ! -type d)"
    [ -z "$irregular" ] || fail "the archive holds a symbolic link or special file: $irregular"
done
share_sources=("$desktop_source")
for size in "${icon_sizes[@]}"; do
    share_sources+=("$src_dir/share/icons/hicolor/$size/apps/bahamut-launcher.png")
done
for source in "${share_sources[@]}"; do
    if [ -L "$source" ] || [ ! -f "$source" ]; then
        fail "$source is missing or not a regular file"
    fi
done

upgrade=0
if [ -L "$target" ]; then
    fail "$target is a symbolic link; not replacing it"
fi
if [ -e "$target" ]; then
    [ -d "$target" ] || refuse_unmanaged "$target"
    check_managed "$target"
    upgrade=1
fi
bin_dest="$destdir$bin_path"
if [ -L "$bin_dest" ]; then
    link_points_into_pkgdir "$bin_dest" ||
        fail "$bin_dest is a symbolic link that points outside $pkgdir; not replacing it"
elif [ -e "$bin_dest" ]; then
    fail "$bin_dest exists and is not a symbolic link; not replacing it"
fi
desktop_dest="$destdir$desktop_path"
if [ -L "$desktop_dest" ]; then
    fail "$desktop_dest is a symbolic link; not replacing it"
elif [ -e "$desktop_dest" ]; then
    if [ ! -f "$desktop_dest" ] || ! grep -qx "$installer_key=true" "$desktop_dest"; then
        fail "$desktop_dest exists and was not written by install.sh; not replacing it"
    fi
fi
for i in "${!icon_sizes[@]}"; do
    icon_dest="$destdir${icon_paths[$i]}"
    if [ -L "$icon_dest" ]; then
        fail "$icon_dest is a symbolic link; not replacing it"
    elif [ -e "$icon_dest" ]; then
        [ -f "$icon_dest" ] || fail "$icon_dest exists and is not a regular file; not replacing it"
        if [ "$upgrade" -eq 0 ] && ! same_content "$icon_dest" "${share_sources[$((i + 1))]}"; then
            fail "$icon_dest exists and differs from the launcher icon; not replacing it"
        fi
    fi
done

dep_warning=""
if [ "$skip_checks" -eq 0 ] && [ -z "$destdir" ]; then
    dep_status=0
    dep_output="$(bash "$src_dir/install-dependencies.sh" --check --launcher "$src_dir/bahamut-launcher" 2>&1)" ||
        dep_status=$?
    dep_silent=0
    case "$dep_status" in
        0) dep_silent=1 ;;
        3)
            case "$dep_output" in
                *"cannot check"*) dep_silent=1 ;;
            esac
            ;;
    esac
    if [ "$dep_silent" -eq 0 ]; then
        printf '%s\n' "$dep_output"
        dep_warning="install-dependencies.sh --check reported a problem (exit $dep_status); the launcher or the game may not start until it is fixed"
        warn "$dep_warning"
    fi
fi

new=""
aside=""
desktop_tmp=""
# An interrupted swap puts the previous payload back.
cleanup() {
    if [ -n "$aside" ]; then
        if [ ! -e "$target" ] && [ ! -L "$target" ] && [ -d "$aside/payload" ]; then
            mv -- "$aside/payload" "$target" 2>/dev/null || true
        fi
        if [ -e "$target" ]; then
            rm -rf -- "$aside"
        fi
    fi
    [ -z "$new" ] || rm -rf -- "$new"
    [ -z "$desktop_tmp" ] || rm -f -- "$desktop_tmp"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

make_dirs "$pkg_parent"
pkg_base="$(basename -- "$target")"
new="$(mktemp -d "$pkg_parent/.$pkg_base.new.XXXXXX")"
chmod 0755 "$new"

for rel in "${payload_exec[@]}"; do
    install -m 0755 "$src_dir/$rel" "$new/$rel"
done
for rel in "${payload_files[@]}"; do
    install -m 0644 "$src_dir/$rel" "$new/$rel"
done
for dir in "${payload_dirs[@]}"; do
    while IFS= read -r -d '' entry; do
        rel="${entry#"$src_dir"/}"
        if [ -d "$entry" ]; then
            install -d -m 0755 "$new/$rel"
        else
            install -m 0644 "$entry" "$new/$rel"
        fi
    done < <(find "$src_dir/$dir" -print0 | LC_ALL=C sort -z)
done

# The file list records every payload file (sha256sum format, C-sorted paths)
# so a later run can tell an untouched payload from one holding state.
payload_rels=()
while IFS= read -r -d '' entry; do
    rel="${entry#./}"
    case "$rel" in
        *"$newline"* | *"\\"*) fail "the archive holds a file name install.sh cannot record: $rel" ;;
    esac
    payload_rels+=("$rel")
done < <(cd "$new" && find . -type f -print0 | LC_ALL=C sort -z)
for rel in "${payload_rels[@]}"; do
    sum="$(sha256sum < "$new/$rel")"
    printf '%s  %s\n' "${sum%% *}" "$rel"
done > "$new/$files_name"
printf '%s\n' "$bin_path" "$desktop_path" "${icon_paths[@]}" "$pkgdir" > "$new/$manifest_name"
chmod 0644 "$new/$manifest_name" "$new/$files_name"

if [ -e "$target" ]; then
    # State may have appeared since the first check; look again right before the swap.
    check_managed "$target"
    aside="$(mktemp -d "$pkg_parent/.$pkg_base.old.XXXXXX")"
    mv -- "$target" "$aside/payload"
    if ! mv -- "$new" "$target"; then
        mv -- "$aside/payload" "$target"
        rm -rf -- "$aside"
        aside=""
        fail "could not move the new payload into $target; the previous install was restored"
    fi
    new=""
    rm -rf -- "$aside"
    aside=""
else
    mv -- "$new" "$target"
    new=""
fi
for leftover in "$pkg_parent/.$pkg_base.old."* "$pkg_parent/.$pkg_base.new."*; do
    if [ -d "$leftover" ] && [ ! -L "$leftover" ]; then
        rm -rf -- "$leftover"
    fi
done

make_dirs "$(dirname -- "$bin_dest")"
ln -sfn -- "$pkgdir/bahamut-launcher" "$bin_dest"

# Desktop Entry spec: an Exec argument holding a reserved character is
# double-quoted with ", `, $, and \ backslash-escaped; % doubles as %%.
exec_argument() {
    local s="$1" out="" ch i quote=0
    for ch in ' ' $'\t' '"' "'" "\\" '>' '<' '~' '|' '&' ';' '$' '*' '?' '#' '(' ')' '`'; do
        case "$s" in
            *"$ch"*) quote=1; break ;;
        esac
    done
    for ((i = 0; i < ${#s}; i++)); do
        ch="${s:i:1}"
        case "$ch" in
            '"' | '`' | '$' | "\\")
                if [ "$quote" -eq 1 ]; then
                    out+="\\"
                fi
                out+="$ch"
                ;;
            '%') out+="%%" ;;
            *) out+="$ch" ;;
        esac
    done
    if [ "$quote" -eq 1 ]; then
        out="\"$out\""
    fi
    printf '%s' "$out"
}

# String-value escaping, applied after Exec quoting.
string_value() {
    local s="$1" out="" ch i
    for ((i = 0; i < ${#s}; i++)); do
        ch="${s:i:1}"
        case "$ch" in
            "\\") out+="\\\\" ;;
            $'\t') out+='\t' ;;
            $'\r') out+='\r' ;;
            *) out+="$ch" ;;
        esac
    done
    printf '%s' "$out"
}

exec_value="$(string_value "$(exec_argument "$bin_path")")"
# TryExec is a plain path, not an Exec command line.
tryexec_value="$(string_value "$bin_path")"

make_dirs "$(dirname -- "$desktop_dest")"
desktop_tmp="$(mktemp "${TMPDIR:-/tmp}/bahamut-launcher.desktop.XXXXXX")"
# The installer key goes at the end of the [Desktop Entry] group.
group=""
key_written=0
while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
        "["*"]")
            if [ "$group" = "[Desktop Entry]" ] && [ "$key_written" -eq 0 ]; then
                printf '%s=true\n' "$installer_key"
                key_written=1
            fi
            group="$line"
            printf '%s\n' "$line"
            continue
            ;;
    esac
    if [ "$group" = "[Desktop Entry]" ]; then
        case "$line" in
            Exec=*) printf 'Exec=%s\n' "$exec_value"; continue ;;
            TryExec=*) printf 'TryExec=%s\n' "$tryexec_value"; continue ;;
            "$installer_key="*) continue ;;
        esac
    fi
    printf '%s\n' "$line"
done < "$desktop_source" > "$desktop_tmp"
if [ "$group" = "[Desktop Entry]" ] && [ "$key_written" -eq 0 ]; then
    printf '%s=true\n' "$installer_key" >> "$desktop_tmp"
fi
grep -qx "$installer_key=true" "$desktop_tmp" || fail "$desktop_source has no [Desktop Entry] group"
install -m 0644 "$desktop_tmp" "$desktop_dest"
rm -f -- "$desktop_tmp"
desktop_tmp=""

for i in "${!icon_sizes[@]}"; do
    icon_dest="$destdir${icon_paths[$i]}"
    make_dirs "$(dirname -- "$icon_dest")"
    install -m 0644 "${share_sources[$((i + 1))]}" "$icon_dest"
done

refresh_icon_cache

# The menu lists entries only from XDG data directories.
menu_listed=0
share_dir="$root/share"
data_dirs="$(tidy "${XDG_DATA_HOME:-${HOME:-/nonexistent}/.local/share}"):${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
IFS=: read -r -a data_dir_list <<< "$data_dirs"
for d in ${data_dir_list[@]+"${data_dir_list[@]}"}; do
    case "$d" in
        /*) [ "$(tidy "$d")" != "$share_dir" ] || menu_listed=1 ;;
    esac
done

echo "$name: installed the launcher into $destdir$pkgdir"
if [ -z "$destdir" ]; then
    on_path=0
    case ":${PATH:-}:" in
        *":$root/bin:"*) on_path=1 ;;
    esac
    if [ "$on_path" -eq 1 ] && [ "$menu_listed" -eq 1 ]; then
        echo "$name: start it with 'bahamut-launcher' or from the application menu (Bahamut Launcher)"
    elif [ "$on_path" -eq 1 ]; then
        echo "$name: start it with 'bahamut-launcher'"
    elif [ "$menu_listed" -eq 1 ]; then
        echo "$name: note: $root/bin is not on PATH; the application menu entry works regardless"
        echo "$name: start it from the application menu (Bahamut Launcher) or with $bin_path"
    else
        echo "$name: note: $root/bin is not on PATH"
        echo "$name: start it with $bin_path"
    fi
    if [ "$menu_listed" -eq 0 ]; then
        echo "$name: note: the menu entry is installed under $share_dir/applications, which needs $share_dir on XDG_DATA_DIRS to appear in menus"
    fi
fi
if [ -n "$dep_warning" ]; then
    warn "$dep_warning"
fi
