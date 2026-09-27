// Release jobs select the exact tag; local builds derive an identity from Git.
// Cargo package versions are metadata, not the runtime identity.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=BAHAMUT_RELEASE_TAG");
    watch_release_inputs();
    // Several tags can identify one commit, so git describe alone cannot select the release tag.
    let describe = std::env::var("BAHAMUT_RELEASE_TAG")
        .ok()
        .filter(|tag| !tag.is_empty())
        .unwrap_or_else(|| {
            Command::new("git")
                .args(["describe", "--tags", "--always", "--dirty"])
                .output()
                .ok()
                .filter(|out| out.status.success())
                .and_then(|out| String::from_utf8(out.stdout).ok())
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "unknown".to_owned())
        });
    println!("cargo:rustc-env=BAHAMUT_GIT_DESCRIBE={describe}");
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
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!path.is_empty()).then(|| Path::new(&path).to_path_buf())
}
