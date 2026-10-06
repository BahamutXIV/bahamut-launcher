#!/usr/bin/env python3
"""Prepare the S0 tester Flatpak build from an exact source revision."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import re
import shutil
import subprocess
import tarfile
from pathlib import Path


APP_ID = "io.github.BahamutXIV.Launcher.Tester"
BRANCH = "s0"
RUNTIME = "org.gnome.Platform"
RUNTIME_VERSION = "50"
SDK = "org.gnome.Sdk"
LLVM_ASSET = "llvm-mingw-20260922-ucrt-ubuntu-22.04-x86_64.tar.xz"
LLVM_URL = (
    "https://github.com/mstorsjo/llvm-mingw/releases/download/20260922/"
    + LLVM_ASSET
)
LLVM_SHA256 = "bb7bb7654b33d5aa8712acb837c963b2e0c56352560c76105270a3268c665c21"
RUST_VERSION = "1.95.0"
RUST_DIST_BASE = "https://static.rust-lang.org/dist/2026-04-16"
RUST_COMPONENTS = {
    "rustc": {
        "url": f"{RUST_DIST_BASE}/rustc-1.95.0-x86_64-unknown-linux-gnu.tar.xz",
        "sha256": "8426a3d170a5879f5682f5fbdd024a1779b3951e7baba685af2d6dc32a6dfc15",
    },
    "cargo": {
        "url": f"{RUST_DIST_BASE}/cargo-1.95.0-x86_64-unknown-linux-gnu.tar.xz",
        "sha256": "e74edd2cf7d0f1f1383b4f00eb90c843750bc489e2ccf7214e6476678a907425",
    },
    "rust-std": {
        "url": f"{RUST_DIST_BASE}/rust-std-1.95.0-x86_64-unknown-linux-gnu.tar.xz",
        "sha256": "047ea7098803d3500fa1072e9cee5392697e21525559e4458128a2bf874aa382",
    },
}


def run_git(repo: Path, *arguments: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *arguments],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    return result.stdout.strip()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_workspace_version(cargo_toml: Path) -> str:
    text = cargo_toml.read_text(encoding="utf-8")
    match = re.search(
        r"(?ms)^\[workspace\.package\]\s+.*?^version\s*=\s*\"([^\"]+)\"",
        text,
    )
    if match is None:
        raise SystemExit(f"could not read workspace package version from {cargo_toml}")
    return match.group(1)


def launcher_version_identity(repo: Path, commit: str) -> str:
    """The `<latest tag>-<short hash>` or bare-hash identity the root build.rs derives from Git."""
    short_hash = run_git(repo, "rev-parse", "--short=7", commit)
    describe = subprocess.run(
        ["git", "-C", str(repo), "describe", "--tags", "--abbrev=0", commit],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    tag = describe.stdout.strip()
    if describe.returncode == 0 and tag:
        return f"{tag}-{short_hash}"
    return short_hash


def extract_archive(repo: Path, commit: str, destination: Path) -> None:
    archive = subprocess.run(
        ["git", "-C", str(repo), "archive", "--format=tar", "--prefix=source/", commit],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    with tarfile.open(fileobj=io.BytesIO(archive.stdout), mode="r:") as tar:
        tar.extractall(destination)


def append_vendor_config(source: Path, output: str) -> None:
    marker = "[source.crates-io]"
    start = output.find(marker)
    if start < 0:
        raise SystemExit("cargo vendor did not return a source configuration")
    config_path = source / ".cargo" / "config.toml"
    existing = config_path.read_text(encoding="utf-8") if config_path.exists() else ""
    if marker not in existing:
        config_path.parent.mkdir(parents=True, exist_ok=True)
        config_path.write_text(
            existing.rstrip() + "\n\n" + output[start:].strip() + "\n",
            encoding="utf-8",
        )


def prepare(args: argparse.Namespace) -> None:
    repo = Path(args.repo_root).resolve()
    work = Path(args.work_dir).resolve()
    if work.exists():
        if any(work.iterdir()):
            raise SystemExit(f"refusing to reuse non-empty build context: {work}")
    else:
        work.mkdir(parents=True)

    commit = run_git(repo, "rev-parse", "--verify", f"{args.source_ref}^{{commit}}")
    tree = run_git(repo, "rev-parse", "--verify", f"{commit}^{{tree}}")
    launcher_version = launcher_version_identity(repo, commit)
    source = work / "source"
    extract_archive(repo, commit, work)

    archived_packaging = source / "packaging" / "flatpak"
    if not archived_packaging.is_dir():
        raise SystemExit(
            "selected source commit does not contain packaging/flatpak; "
            "commit the tester files before building"
        )

    if args.vendor_cargo:
        vendor = source / "vendor"
        if vendor.exists():
            raise SystemExit(f"source archive already contains a Cargo vendor directory: {vendor}")
        vendor_result = subprocess.run(
            ["cargo", "vendor", "--locked", "vendor"],
            cwd=source,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        append_vendor_config(source, vendor_result.stdout)

    cargo_lock = source / "Cargo.lock"
    rust_toolchain = source / "rust-toolchain.toml"
    stage_script = source / "scripts" / "stage-unix-release.sh"
    marker = source / "packaging" / "linux" / "package-marker.txt"
    required_files = [cargo_lock, rust_toolchain, stage_script, marker]
    missing = [str(path) for path in required_files if not path.is_file()]
    if missing:
        raise SystemExit("source archive is missing required inputs: " + ", ".join(missing))

    version = read_workspace_version(source / "Cargo.toml")
    identity = {
        "schema": 1,
        "app_id": APP_ID,
        "branch": BRANCH,
        "version": version,
        "source": {
            "commit": commit,
            "launcher_version": launcher_version,
            "tree": tree,
            "worktree_changes_excluded": True,
        },
        "runtime": {
            "platform": RUNTIME,
            "version": RUNTIME_VERSION,
            "sdk": SDK,
            "graphics_extension_branches": ["25.08", "25.08-extra", "1.4"],
        },
        "toolchain": {
            "rust_required": RUST_VERSION,
            "rust_components": RUST_COMPONENTS,
            "rust_toolchain_sha256": sha256_file(rust_toolchain),
            "cargo_lock_sha256": sha256_file(cargo_lock),
            "llvm_mingw_asset": LLVM_ASSET,
            "llvm_mingw_url": LLVM_URL,
            "llvm_mingw_sha256": LLVM_SHA256,
            "cargo_vendor": bool(args.vendor_cargo),
        },
        "payload": {
            "stage_script_sha256": sha256_file(stage_script),
            "package_marker_sha256": sha256_file(marker),
        },
    }
    identity_path = archived_packaging / "build-identity.json"
    identity_path.write_text(json.dumps(identity, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    manifest = archived_packaging / f"{APP_ID}.yml"
    if not manifest.is_file():
        raise SystemExit(f"Flatpak manifest is missing: {manifest}")
    shutil.copy2(manifest, work / manifest.name)
    (work / "prepare-result.json").write_text(
        json.dumps(
            {
                "app_id": APP_ID,
                "branch": BRANCH,
                "commit": commit,
                "launcher_version": launcher_version,
                "tree": tree,
                "version": version,
                "manifest": str(work / manifest.name),
                "source": str(source),
                "identity": str(identity_path),
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    print(json.dumps({"commit": commit, "tree": tree, "version": version}))


def finalize(args: argparse.Namespace) -> None:
    base_path = Path(args.base_identity).resolve()
    bundle = Path(args.bundle).resolve()
    output = Path(args.output).resolve()
    identity = json.loads(base_path.read_text(encoding="utf-8"))
    identity["artifact"] = {
        "filename": bundle.name,
        "format": "flatpak",
        "sha256": sha256_file(bundle),
    }
    identity["runtime"]["platform_commit"] = args.runtime_commit
    identity["runtime"]["sdk_commit"] = args.sdk_commit
    if args.graphics_commits:
        identity["runtime"]["graphics_extension_commits"] = args.graphics_commits
    toolchain_path = Path(args.toolchain_file)
    if toolchain_path.is_file():
        identity["toolchain"]["actual"] = {
            line.split("=", 1)[0]: line.split("=", 1)[1]
            for line in toolchain_path.read_text(encoding="utf-8").splitlines()
            if "=" in line
        }
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(identity, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--finalize", action="store_true", help="write final bundle identity metadata")
    parser.add_argument("--repo-root")
    parser.add_argument("--source-ref", default="HEAD")
    parser.add_argument("--work-dir")
    parser.add_argument("--vendor-cargo", action=argparse.BooleanOptionalAction, default=True)
    parser.add_argument("--base-identity")
    parser.add_argument("--bundle")
    parser.add_argument("--output")
    parser.add_argument("--runtime-commit")
    parser.add_argument("--sdk-commit")
    parser.add_argument("--graphics-commits", nargs="*", default=[])
    parser.add_argument("--toolchain-file")
    args = parser.parse_args()
    if args.finalize:
        required = {
            "--base-identity": args.base_identity,
            "--bundle": args.bundle,
            "--output": args.output,
            "--runtime-commit": args.runtime_commit,
            "--sdk-commit": args.sdk_commit,
            "--toolchain-file": args.toolchain_file,
        }
        missing = [name for name, value in required.items() if not value]
        if missing:
            parser.error("finalize requires " + ", ".join(missing))
        finalize(args)
        return
    if not args.repo_root or not args.work_dir:
        parser.error("prepare requires --repo-root and --work-dir")
    prepare(args)


if __name__ == "__main__":
    main()
