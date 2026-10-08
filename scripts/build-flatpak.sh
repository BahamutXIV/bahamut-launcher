#!/usr/bin/env bash
# Build the S0 Flatpak tester from an exact committed source tree.
#
#   scripts/build-flatpak.sh [--source-ref <commit>] [--output <dir>]
#                            [--label <asset name>] [--release-tag <tag>]
#                            [--work-dir <empty dir>] [--keep-work]
#
# The source archive is made with git archive, so uncommitted UI files cannot
# enter the package. The Flatpak bundle contains the app only; its runtime and
# SDK must already be available from the configured Flatpak remote. The bundle
# is <output>/<label>.flatpak with .sha256 and .identity.json sidecars; the
# label defaults to bahamut-launcher-tester-s0-<version>. --release-tag
# defaults to BAHAMUT_RELEASE_TAG and makes the packaged launcher report that
# tag from --version.
set -euo pipefail

script_dir="$(cd "$(dirname -- "$0")" && pwd -P)"
repo_root="$(cd "$script_dir/.." && pwd -P)"
prepare_script="$script_dir/prepare-flatpak.py"
app_id="io.github.BahamutXIV.Launcher.Tester"
branch="s0"
runtime_repo_url="https://flathub.org/repo/flathub.flatpakrepo"
source_ref="HEAD"
output="$repo_root/out/flatpak-s0"
label=""
release_tag="${BAHAMUT_RELEASE_TAG:-}"
keep_work=0
requested_work=""

fail() {
    echo "build-flatpak.sh: $*" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --source-ref)
            [[ $# -ge 2 ]] || fail "--source-ref needs a commit"
            source_ref="$2"
            shift 2
            ;;
        --output)
            [[ $# -ge 2 ]] || fail "--output needs a directory"
            output="$2"
            shift 2
            ;;
        --label)
            [[ $# -ge 2 && -n "$2" ]] || fail "--label needs an asset name"
            label="$2"
            shift 2
            ;;
        --release-tag)
            [[ $# -ge 2 ]] || fail "--release-tag needs a tag"
            release_tag="$2"
            shift 2
            ;;
        --keep-work)
            keep_work=1
            shift
            ;;
        --work-dir)
            [[ $# -ge 2 ]] || fail "--work-dir needs an empty directory"
            requested_work="$2"
            shift 2
            ;;
        -h | --help)
            sed -n '2,14p' "$0" | sed 's/^# //'
            exit 0
            ;;
        *)
            fail "unknown argument: $1"
            ;;
    esac
done

case "$label" in
    .* | -* | *[!A-Za-z0-9._-]*) fail "the label must be a file name of letters, digits, '.', '_', and '-', not starting with '.' or '-': $label" ;;
esac

for tool in git python3 cargo flatpak flatpak-builder sha256sum; do
    command -v "$tool" >/dev/null 2>&1 || fail "$tool is required"
done

source_commit="$(git -C "$repo_root" rev-parse --verify "$source_ref^{commit}")" ||
    fail "source ref is not a commit: $source_ref"
source_tree="$(git -C "$repo_root" rev-parse --verify "$source_commit^{tree}")" ||
    fail "could not resolve source tree: $source_commit"

if [[ -L "$output" ]]; then
    fail "output is a symbolic link: $output"
fi
mkdir -p "$output"
output="$(cd "$output" && pwd -P)"

if [[ -n "$requested_work" ]]; then
    if [[ -L "$requested_work" ]]; then
        fail "work directory is a symbolic link: $requested_work"
    fi
    if [[ -e "$requested_work" && ! -d "$requested_work" ]]; then
        fail "work path is not a directory: $requested_work"
    fi
    mkdir -p "$requested_work"
    [[ -z "$(find "$requested_work" -mindepth 1 -print -quit)" ]] ||
        fail "work directory is not empty: $requested_work"
    work="$(cd "$requested_work" && pwd -P)"
else
    mkdir -p "$repo_root/out"
    work="$(mktemp -d "$repo_root/out/flatpak-s0-build.XXXXXX")"
fi
cleanup() {
    if [[ "$keep_work" -eq 0 ]]; then
        rm -rf "$work"
    fi
}
trap cleanup EXIT

prepare_args=(
    --repo-root "$repo_root"
    --source-ref "$source_commit"
    --work-dir "$work"
    --vendor-cargo
)
if [[ -n "$release_tag" ]]; then
    prepare_args+=(--release-tag "$release_tag")
fi
python3 "$prepare_script" "${prepare_args[@]}"

result_file="$work/prepare-result.json"
version="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["version"])' "$result_file")"
manifest="$work/io.github.BahamutXIV.Launcher.Tester.yml"
build_dir="$work/build"
repo_dir="$work/repo"
if [[ -z "$label" ]]; then
    label="bahamut-launcher-tester-s0-$version"
fi
bundle="$output/$label.flatpak"
identity="$bundle.identity.json"
checksum="$bundle.sha256"

for path in "$bundle" "$identity" "$checksum"; do
    [[ ! -e "$path" && ! -L "$path" ]] || fail "refusing to overwrite existing output: $path"
done

flatpak-builder --state-dir "$work/state" --keep-build-dirs \
    --repo "$repo_dir" "$build_dir" "$manifest"
flatpak build-bundle --runtime-repo="$runtime_repo_url" "$repo_dir" "$bundle" "$app_id" "$branch"
[[ -f "$bundle" ]] || fail "flatpak build-bundle did not create $bundle"

processed_manifest="$build_dir/files/manifest.json"
[[ -f "$processed_manifest" ]] || fail "processed build manifest is missing: $processed_manifest"
runtime_commit="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["runtime-commit"])' "$processed_manifest")"
sdk_commit="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["sdk-commit"])' "$processed_manifest")"
[[ "$runtime_commit" =~ ^[0-9a-f]{64}$ && "$sdk_commit" =~ ^[0-9a-f]{64}$ ]] ||
    fail "processed manifest has invalid runtime or SDK commit identities"
toolchain_file="$build_dir/files/share/bahamut-launcher/s0-build-toolchain.txt"
[[ -f "$toolchain_file" ]] || fail "SDK toolchain receipt is missing: $toolchain_file"

python3 "$prepare_script" \
    --finalize \
    --base-identity "$work/source/packaging/flatpak/build-identity.json" \
    --bundle "$bundle" \
    --output "$identity" \
    --runtime-commit "$runtime_commit" \
    --sdk-commit "$sdk_commit" \
    --toolchain-file "$toolchain_file"
(cd "$output" && sha256sum -- "$(basename "$bundle")" > "$(basename "$checksum")")

echo "Flatpak bundle: $bundle"
echo "SHA-256 sidecar: $checksum"
echo "Build identity: $identity"
echo "Source commit: $source_commit"
echo "Source tree: $source_tree"
