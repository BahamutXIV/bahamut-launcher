// Release jobs select the exact tag; local builds carry the latest tag, a hyphen,
// and the short commit hash. Cargo package versions are metadata, not the runtime identity.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=BAHAMUT_RELEASE_TAG");
    watch_release_inputs();
    // Several tags can identify one commit, so git describe alone cannot select the release tag.
    let describe = std::env::var("BAHAMUT_RELEASE_TAG")
        .ok()
        .filter(|tag| !tag.is_empty())
        .unwrap_or_else(local_identity);
    println!("cargo:rustc-env=BAHAMUT_GIT_DESCRIBE={describe}");
}

/// `<latest tag>-<short hash>`, the bare hash without a reachable tag, or "unknown" outside Git.
/// A `-dirty` suffix marks uncommitted changes to tracked files.
fn local_identity() -> String {
    let Some(hash) = git_stdout(&["rev-parse", "--short=7", "HEAD"]) else {
        return "unknown".to_owned();
    };
    let mut identity = match git_stdout(&["describe", "--tags", "--abbrev=0"]) {
        Some(tag) => format!("{tag}-{hash}"),
        None => hash,
    };
    if git_stdout(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty())
    {
        identity.push_str("-dirty");
    }
    identity
}

fn watch_release_inputs() {
    // Only tracked edits affect `--dirty`; directory watches also pick up ignored settings
    // and generated output. The index watch below refreshes this list after additions/removals.
    if let Ok(output) = Command::new("git").args(["ls-files", "-z"]).output()
        && output.status.success()
    {
        for path in output
            .stdout
            .split(|byte| *byte == 0)
            // Cargo already tracks the build script itself.
            .filter(|path| !path.is_empty() && *path != b"build.rs")
        {
            println!("cargo:rerun-if-changed={}", String::from_utf8_lossy(path));
        }
    }

    // Git resolves per-worktree HEAD/index and shared refs correctly. Watching refs also
    // catches newly created tags and loose refs disappearing into packed-refs, without logs.
    for relative in ["HEAD", "index", "packed-refs", "refs"] {
        if let Some(path) = git_path(&["rev-parse", "--git-path", relative])
            && path.exists()
        {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

fn git_path(args: &[&str]) -> Option<PathBuf> {
    git_stdout(args).map(|path| Path::new(&path).to_path_buf())
}

/// Trimmed stdout of a successful git command; `None` on failure or empty output.
fn git_stdout(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!text.is_empty()).then_some(text)
}
