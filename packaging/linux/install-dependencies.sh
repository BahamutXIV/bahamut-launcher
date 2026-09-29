#!/usr/bin/env bash
# Check, and on request install, the host libraries and Wine the packaged
# launcher needs.
set -euo pipefail

name="install-dependencies.sh"

usage() {
    cat <<'EOF'
usage: install-dependencies.sh [--check | --install] [--launcher PATH] [--yes]

  --check          report what is missing (the default)
  --install        print the package command for this distribution, confirm,
                   and run it through sudo or doas
  --launcher PATH  the launcher to inspect; default: bahamut-launcher beside
                   this script
  --yes            with --install, do not ask for confirmation

Exit codes: 0 all satisfied, 1 something is missing, 2 usage error,
3 cannot check on this host, 4 a glibc-based distribution is required,
5 the host glibc is too old. --install also exits 1 when the package manager
fails or its prompt is declined.
EOF
}

usage_error() {
    echo "$name: $*" >&2
    echo "Run '$name --help' for usage." >&2
    exit 2
}

script_dir="$(cd "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
mode="check"
launcher=""
assume_yes=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --check) mode="check"; shift ;;
        --install) mode="install"; shift ;;
        --yes | -y) assume_yes=1; shift ;;
        --launcher)
            if [ "$#" -lt 2 ] || [ -z "$2" ]; then
                usage_error "--launcher requires a value"
            fi
            launcher="$2"
            shift 2
            ;;
        --launcher=*) launcher="${1#*=}"; shift ;;
        -h | --help) usage; exit 0 ;;
        *) usage_error "unknown argument: $1" ;;
    esac
done

[ -n "$launcher" ] || launcher="$script_dir/bahamut-launcher"
case "$launcher" in
    /*) ;;
    *) launcher="$PWD/$launcher" ;;
esac

glibc_loaders=(/lib64/ld-linux-x86-64.so.2 /lib/ld-linux-aarch64.so.1)

has_glibc_loader() {
    local loader
    for loader in "${glibc_loaders[@]}"; do
        [ -e "$loader" ] && return 0
    done
    return 1
}

# lib_state: ok, missing, too_old, cannot, arch, or musl.
lib_state=""
lib_note=""
missing_libs=()
loader_errors=()
too_old=()

# requires_launcher LINE: the version error names the launcher, or no file.
requires_launcher() {
    local re='\(required by ([^)]*)\)' by
    [[ $1 =~ $re ]] || return 0
    by="${BASH_REMATCH[1]}"
    [ "$by" = "$launcher" ] || [ "$by" = "$(readlink -f -- "$launcher" 2>/dev/null)" ]
}

parse_listing() {
    local line soname
    while IFS= read -r line; do
        case "$line" in
            *"version \`GLIBC_"*)
                if requires_launcher "$line"; then
                    too_old+=("$line")
                else
                    loader_errors+=("$line")
                fi
                ;;
            *"version \`"*"not found"*)
                loader_errors+=("$line")
                ;;
            *" => not found"*)
                read -r soname _ <<< "$line"
                missing_libs+=("$soname")
                ;;
            *"not found"*)
                loader_errors+=("$line")
                ;;
        esac
    done <<< "$1"
    if [ "${#too_old[@]}" -gt 0 ]; then
        lib_state="too_old"
    elif [ "${#missing_libs[@]}" -gt 0 ] || [ "${#loader_errors[@]}" -gt 0 ]; then
        lib_state="missing"
    else
        lib_state="ok"
    fi
}

listing_usable() {
    case "$1" in
        *"=>"* | *"not found"*) return 0 ;;
    esac
    return 1
}

probe_libraries() {
    local out loader
    if [ ! -f "$launcher" ]; then
        lib_state="cannot"
        lib_note="the launcher was not found at $launcher"
        return
    fi
    if [ "$(head -c 4 -- "$launcher" 2>/dev/null | od -An -tx1 | tr -d ' \n')" != 7f454c46 ]; then
        lib_state="cannot"
        lib_note="$launcher is not an ELF executable"
        return
    fi
    local machine built host
    machine="$(od -An -tx1 -j18 -N2 -- "$launcher" 2>/dev/null | tr -d ' \n')"
    case "$machine" in
        3e00) built="x86_64" ;;
        b700) built="aarch64" ;;
        *) built="ELF machine 0x${machine:2:2}${machine:0:2}" ;;
    esac
    host="$(uname -m 2>/dev/null || echo unknown)"
    if [ "$built" != "$host" ]; then
        lib_state="arch"
        lib_note="the launcher is built for $built; this host is $host"
        return
    fi
    if compgen -G '/lib/ld-musl-*' >/dev/null && ! has_glibc_loader; then
        lib_state="musl"
        return
    fi
    if command -v ldd >/dev/null 2>&1; then
        out="$(ldd "$launcher" 2>&1)" || true
        if listing_usable "$out"; then
            parse_listing "$out"
            return
        fi
    fi
    for loader in "${glibc_loaders[@]}"; do
        [ -x "$loader" ] || continue
        out="$("$loader" --list "$launcher" 2>&1)" || true
        if listing_usable "$out"; then
            parse_listing "$out"
            return
        fi
    done
    lib_state="cannot"
    lib_note="neither ldd nor a glibc ELF loader could list the libraries of $launcher"
}

# A clean listing can still hide a symbol or version error the loader only
# reports at startup.
truth_test() {
    local out status=0 line runner=()
    if command -v timeout >/dev/null 2>&1; then
        runner=(timeout 20)
    fi
    # Without a display, a launcher that ignored --version exits instead of
    # opening a window.
    out="$(${runner[@]+"${runner[@]}"} env -u DISPLAY -u WAYLAND_DISPLAY "$launcher" --version 2>&1 < /dev/null)" ||
        status=$?
    [ "$status" -ne 0 ] || return 0
    while IFS= read -r line; do
        case "$line" in
            *"error while loading shared libraries"* | *"symbol lookup error"* | *"cannot open shared object"* | *"version \`"*"not found"*)
                loader_errors+=("$line")
                ;;
        esac
    done <<< "$out"
    if [ "${#loader_errors[@]}" -gt 0 ]; then
        lib_state="missing"
    else
        lib_note="'bahamut-launcher --version' exited $status without a loader error"
    fi
}

wine_state=""
wine_note=""

wine_version() {
    local runner=()
    if command -v timeout >/dev/null 2>&1; then
        runner=(timeout 20)
    fi
    ${runner[@]+"${runner[@]}"} "$1" --version 2>/dev/null < /dev/null | head -n 1 || true
}

probe_wine() {
    local wine_cmd wine_path wine_root candidate version
    # BAHAMUT_WINE is used as given, like the launcher does.
    if [ -n "${BAHAMUT_WINE+set}" ]; then
        wine_cmd="$BAHAMUT_WINE"
        if [ ! -f "$wine_cmd" ]; then
            wine_state="missing"
            wine_note="BAHAMUT_WINE is set but $wine_cmd is not a file"
            return
        fi
    else
        wine_cmd="$(command -v wine 2>/dev/null || true)"
    fi
    if [ -z "$wine_cmd" ]; then
        wine_state="missing"
        wine_note="wine was not found on PATH (set BAHAMUT_WINE to use another binary)"
        return
    fi
    wine_path="$(readlink -f -- "$wine_cmd" 2>/dev/null || true)"
    if [ -z "$wine_path" ] || [ ! -f "$wine_path" ]; then
        wine_state="missing"
        wine_note="$wine_cmd does not exist"
        return
    fi
    wine_root="$(dirname -- "$(dirname -- "$wine_path")")"
    # lib*/*/wine covers Debian multiarch; wine/ covers Gentoo's slotted roots.
    for candidate in "$wine_root"/lib*/wine/i386-windows/ntdll.dll \
        "$wine_root"/lib*/*/wine/i386-windows/ntdll.dll \
        "$wine_root"/wine/i386-windows/ntdll.dll; do
        if [ -f "$candidate" ]; then
            wine_state="ok"
            wine_note="$wine_path"
            return
        fi
    done
    # Wine before 7 keeps ntdll.dll directly in the wine library directory.
    for candidate in "$wine_root"/lib*/wine/ntdll.dll* "$wine_root"/lib*/*/wine/ntdll.dll*; do
        if [ -f "$candidate" ]; then
            version="$(wine_version "$wine_path")"
            wine_state="old"
            wine_note="Wine ${version:-(unknown version)} is older than 7; the launcher needs Wine 7 or newer ($wine_path)"
            return
        fi
    done
    wine_state="no32"
    wine_note="$wine_path lacks 32-bit support (no i386-windows/ntdll.dll under $wine_root)"
}

distro="unknown"

detect_distro() {
    local id="" like="" key value word words=()
    [ -r /etc/os-release ] || return 0
    while IFS='=' read -r key value; do
        value="${value#[\"\']}"
        value="${value%[\"\']}"
        case "$key" in
            ID) id="$value" ;;
            ID_LIKE) like="$value" ;;
        esac
    done < /etc/os-release
    read -r -a words <<< "$id $like"
    for word in ${words[@]+"${words[@]}"}; do
        case "$word" in
            gentoo | debian | ubuntu | fedora | arch) distro="$word"; return 0 ;;
            opensuse* | suse) distro="opensuse"; return 0 ;;
        esac
    done
}

install_command() {
    case "$distro" in
        gentoo) echo "sudo emerge --ask --noreplace net-libs/webkit-gtk:4.1 x11-libs/gtk+:3 virtual/wine" ;;
        debian | ubuntu) echo "sudo dpkg --add-architecture i386 && sudo apt update && sudo apt install libwebkit2gtk-4.1-0 wine wine32:i386" ;;
        fedora) echo "sudo dnf install webkit2gtk4.1 wine" ;;
        arch) echo "sudo pacman -S webkit2gtk-4.1 wine" ;;
        opensuse) echo "sudo zypper install libwebkit2gtk-4_1-0 wine" ;;
    esac
}

print_distro_notes() {
    case "$distro" in
        gentoo)
            echo "  Wine needs 32-bit support: enable the abi_x86_32 or wow64 USE flag on"
            echo "  app-emulation/wine-vanilla or app-emulation/wine-staging (for example in"
            echo "  /etc/portage/package.use), then check the active Wine with 'eselect wine list'."
            ;;
    esac
}

print_wine_upgrade() {
    echo "  Debian and Ubuntu: install Wine 7 or newer from the WineHQ packages,"
    echo "  https://gitlab.winehq.org/wine/wine/-/wikis/Debian-Ubuntu"
}

print_file_search() {
    echo "  Find the package that provides each missing library with your"
    echo "  distribution's file search, for example 'apt-file search <library>',"
    echo "  'dnf provides */<library>', 'pkgfile <library>', or 'e-file <library>'."
}

glibc_version() {
    getconf GNU_LIBC_VERSION 2>/dev/null || echo "unknown glibc"
}

required_glibc() {
    local line re='version .(GLIBC_[0-9.]+)'
    local seen=""
    for line in "${too_old[@]}"; do
        if [[ $line =~ $re ]]; then
            case " $seen " in
                *" ${BASH_REMATCH[1]} "*) ;;
                *) seen="${seen:+$seen }${BASH_REMATCH[1]}" ;;
            esac
        fi
    done
    printf '%s' "${seen:-a newer glibc}"
}

run_checks() {
    lib_state=""
    lib_note=""
    missing_libs=()
    loader_errors=()
    too_old=()
    probe_libraries
    if [ "$lib_state" = "ok" ]; then
        truth_test
    fi
    probe_wine
    detect_distro
}

# installable: a distribution package can fix something the report found.
installable() {
    [ "$lib_state" = "missing" ] || [ "$wine_state" = "missing" ] || [ "$wine_state" = "no32" ]
}

# Prints the report and sets result to the exit code.
result=0

report() {
    local item
    echo "Bahamut Launcher dependency check"
    echo "launcher: $launcher"
    case "$lib_state" in
        ok)
            echo "libraries: all resolved"
            ;;
        missing)
            echo "libraries: missing"
            for item in ${missing_libs[@]+"${missing_libs[@]}"} ${loader_errors[@]+"${loader_errors[@]}"}; do
                echo "  $item"
            done
            ;;
        too_old)
            echo "libraries: the launcher requires $(required_glibc); this host has $(glibc_version)"
            echo "  A newer release of your distribution is needed; installing packages cannot fix this."
            ;;
        cannot)
            echo "libraries: cannot check on this host: $lib_note"
            ;;
        arch)
            echo "libraries: $lib_note"
            ;;
        musl)
            echo "libraries: this host uses musl libc; the launcher needs a glibc-based distribution"
            ;;
    esac
    if [ "$lib_state" = "ok" ] && [ -n "$lib_note" ]; then
        echo "  note: $lib_note"
    fi
    case "$wine_state" in
        ok) echo "wine: $wine_note (32-bit support present)" ;;
        missing) echo "wine: missing: $wine_note" ;;
        no32) echo "wine: Wine lacks 32-bit support: $wine_note" ;;
        old)
            echo "wine: $wine_note"
            print_wine_upgrade
            ;;
    esac
    echo "distribution: $distro"

    case "$lib_state" in
        musl) result=4 ;;
        cannot | arch) result=3 ;;
        too_old) result=5 ;;
        *)
            if [ "$lib_state" = "missing" ] || [ "$wine_state" != "ok" ]; then
                result=1
            else
                result=0
            fi
            ;;
    esac

    if { [ "$result" -eq 1 ] || [ "$result" -eq 3 ]; } && [ "$lib_state" != "arch" ] && installable; then
        if [ "$distro" = "unknown" ]; then
            echo "to install: no package command is known for this distribution"
            print_file_search
        else
            echo "to install: $(install_command)"
            print_distro_notes
        fi
    fi
    case "$result" in
        0) echo "result: all dependencies are satisfied" ;;
        1) echo "result: something is missing (exit 1)" ;;
        3)
            if [ "$lib_state" = "arch" ]; then
                echo "result: the launcher does not match this host's architecture (exit 3)"
            else
                echo "result: cannot check the libraries on this host (exit 3)"
            fi
            ;;
        4) echo "result: a glibc-based distribution is required (exit 4)" ;;
        5) echo "result: the host glibc is too old (exit 5)" ;;
    esac
}

elevate=()

choose_elevation() {
    if [ "$EUID" -eq 0 ]; then
        elevate=()
    elif command -v sudo >/dev/null 2>&1; then
        elevate=(sudo)
    elif command -v doas >/dev/null 2>&1; then
        elevate=(doas)
    else
        echo "$name: installing packages needs root, and neither sudo nor doas was found" >&2
        exit 1
    fi
}

as_root() {
    echo "+ ${elevate[*]+${elevate[*]} }$*"
    ${elevate[@]+"${elevate[@]}"} "$@"
}

# Each step returns on failure: set -e does not apply inside an if condition.
run_install() {
    local yes_flag=()
    case "$distro" in
        gentoo)
            if [ "$assume_yes" -eq 1 ]; then
                as_root emerge --noreplace net-libs/webkit-gtk:4.1 x11-libs/gtk+:3 virtual/wine || return 1
            else
                as_root emerge --ask --noreplace net-libs/webkit-gtk:4.1 x11-libs/gtk+:3 virtual/wine || return 1
            fi
            ;;
        debian | ubuntu)
            [ "$assume_yes" -eq 0 ] || yes_flag=(-y)
            as_root dpkg --add-architecture i386 || return 1
            as_root apt update || return 1
            as_root apt install ${yes_flag[@]+"${yes_flag[@]}"} libwebkit2gtk-4.1-0 wine wine32:i386 || return 1
            ;;
        fedora)
            [ "$assume_yes" -eq 0 ] || yes_flag=(-y)
            as_root dnf install ${yes_flag[@]+"${yes_flag[@]}"} webkit2gtk4.1 wine || return 1
            ;;
        arch)
            [ "$assume_yes" -eq 0 ] || yes_flag=(--noconfirm)
            as_root pacman -S ${yes_flag[@]+"${yes_flag[@]}"} webkit2gtk-4.1 wine || return 1
            ;;
        opensuse)
            [ "$assume_yes" -eq 0 ] || yes_flag=(-y)
            as_root zypper install ${yes_flag[@]+"${yes_flag[@]}"} libwebkit2gtk-4_1-0 wine || return 1
            ;;
    esac
}

run_checks
report

if [ "$mode" = "check" ]; then
    exit "$result"
fi

case "$result" in
    0)
        echo "$name: nothing to install"
        exit 0
        ;;
    4 | 5)
        exit "$result"
        ;;
esac
if [ "$lib_state" = "arch" ]; then
    echo "$name: the launcher does not match this host; nothing to install"
    exit 3
fi
if ! installable; then
    if [ "$result" -eq 3 ]; then
        echo "$name: cannot check the libraries; nothing to install"
        exit 3
    fi
    echo "$name: the distribution packages cannot fix what the report found; nothing to install"
    exit 1
fi

if [ "$distro" = "unknown" ]; then
    echo "$name: install the missing packages with your distribution's package manager" >&2
    exit 1
fi

echo "$name: this will run: $(install_command)"
if [ "$assume_yes" -eq 0 ]; then
    if [ ! -t 0 ]; then
        echo "$name: no terminal to confirm on; pass --yes to install without asking" >&2
        exit 2
    fi
    read -r -p "Run it now? [y/N] " answer || { echo; answer=""; }
    case "$answer" in
        y | Y | yes | YES) ;;
        *)
            echo "$name: nothing was installed"
            exit 1
            ;;
    esac
fi

choose_elevation
if ! run_install; then
    echo "$name: the package command failed" >&2
    exit 1
fi

echo
run_checks
report
exit "$result"
