use std::fs::{self};
use std::path::{Component, Path, PathBuf};

pub(super) struct ResolvedPath {
    pub(super) path: PathBuf,
    pub(super) nearest_existing: PathBuf,
    pub(super) exists: bool,
}

pub(super) fn stage_path_for(destination: &Path) -> Result<PathBuf, String> {
    let leaf = destination
        .file_name()
        .ok_or_else(|| "The installation destination has no final path component.".to_string())?;
    let mut stage_leaf = leaf.to_os_string();
    stage_leaf.push(".bahamut-stage");
    Ok(destination.with_file_name(stage_leaf))
}

pub(super) fn ensure_destination_parent(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "The installation destination has no parent directory.".to_string())?;
    resolve_path(parent)?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create the installation parent: {error}"))?;
    let resolved = resolve_path(parent)?;
    if !resolved.exists || !resolved.path.is_dir() {
        return Err("The installation parent changed while it was being created.".into());
    }
    Ok(())
}

pub(super) fn reject_overlapping_paths(
    destination: &Path,
    cache: &Path,
    stage: &Path,
) -> Result<(), String> {
    for (left, right, label) in [
        (destination, cache, "destination and download cache"),
        (stage, cache, "staging directory and download cache"),
    ] {
        if paths_overlap(left, right) {
            return Err(format!("The {label} must be separate paths."));
        }
    }
    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    let left = folded_components(left);
    let right = folded_components(right);
    left.starts_with(&right) || right.starts_with(&left)
}

fn folded_components(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

pub(super) fn resolve_path(path: &Path) -> Result<ResolvedPath, String> {
    if !path.is_absolute() {
        return Err(format!("Expected an absolute path: {}", path.display()));
    }
    if path
        .as_os_str()
        .to_string_lossy()
        .split(['\\', '/'])
        .any(|part| matches!(part, "." | ".."))
    {
        return Err(format!(
            "The path contains a relative alias: {}",
            path.display()
        ));
    }
    let mut probe = path.to_path_buf();
    let mut missing = Vec::new();
    let deepest = loop {
        match fs::symlink_metadata(&probe) {
            Ok(metadata) => {
                check_reparse(&probe, &metadata)?;
                break probe;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let leaf = probe.file_name().ok_or_else(|| {
                    format!("The path has no existing parent: {}", path.display())
                })?;
                missing.push(leaf.to_os_string());
                if !probe.pop() {
                    return Err(format!("Could not resolve the path: {}", path.display()));
                }
            }
            Err(error) => return Err(format!("Could not inspect {}: {error}", probe.display())),
        }
    };
    if !fs::metadata(&deepest)
        .map_err(|error| format!("Could not inspect {}: {error}", deepest.display()))?
        .is_dir()
    {
        return Err(format!(
            "A path ancestor is not a directory: {}",
            deepest.display()
        ));
    }
    inspect_existing_ancestors(&deepest)?;
    let nearest_existing = fs::canonicalize(&deepest)
        .map_err(|error| format!("Could not resolve {}: {error}", deepest.display()))?;
    let mut resolved = nearest_existing.clone();
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(ResolvedPath {
        path: resolved,
        nearest_existing,
        exists: missing.is_empty(),
    })
}

fn inspect_existing_ancestors(path: &Path) -> Result<(), String> {
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let metadata = fs::symlink_metadata(&prefix)
            .map_err(|error| format!("Could not inspect {}: {error}", prefix.display()))?;
        check_reparse(&prefix, &metadata)?;
        if !metadata.is_dir() {
            return Err(format!(
                "A path ancestor is not a directory: {}",
                prefix.display()
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_no_reparse_ancestors(path: &Path) -> Result<(), String> {
    let resolved = resolve_path(path)?;
    if !resolved.exists || resolved.path != path {
        return Err(format!("The owned path changed: {}", path.display()));
    }
    Ok(())
}

pub(super) fn check_reparse(path: &Path, metadata: &fs::Metadata) -> Result<(), String> {
    if crate::atomic_fs::is_link_or_reparse(metadata) {
        return Err(format!(
            "Refusing a link or reparse point: {}",
            path.display()
        ));
    }
    Ok(())
}

pub(super) fn ensure_cache_directory(path: &Path) -> Result<PathBuf, String> {
    let before = resolve_path(path)?;
    fs::create_dir_all(&before.path)
        .map_err(|error| format!("Could not create the download cache: {error}"))?;
    let after = resolve_path(&before.path)?;
    if !after.exists || after.path != before.path {
        return Err("The download cache path changed while it was being created.".into());
    }
    Ok(after.path)
}

pub(super) fn validate_tree_has_no_reparse_points(root: &Path) -> Result<(), String> {
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("Could not inspect {}: {error}", root.display()))?;
    check_reparse(root, &root_metadata)?;
    if !root_metadata.is_dir() {
        return Err(format!("Expected a directory: {}", root.display()));
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("Could not inspect {}: {error}", directory.display()))?
        {
            let entry =
                entry.map_err(|error| format!("Could not inspect staging data: {error}"))?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
            check_reparse(&path, &metadata)?;
            if metadata.is_dir() {
                pending.push(path);
            } else if !metadata.is_file() {
                return Err(format!(
                    "Unexpected special file in owned staging: {}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn joined_content_path(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |mut path, part| {
            path.push(part);
            path
        })
}

pub(super) fn create_stage_parents(stage: &Path, directory: &Path) -> Result<(), String> {
    let relative = directory
        .strip_prefix(stage)
        .map_err(|_| "Staged file escaped its owned directory.".to_string())?;
    let mut current = stage.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("Invalid directory component in staged path.".into());
        };
        current.push(name);
        match fs::create_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&current).map_err(|error| {
                    format!(
                        "Could not inspect staged directory {}: {error}",
                        current.display()
                    )
                })?;
                check_reparse(&current, &metadata)?;
                if !metadata.is_dir() {
                    return Err(format!(
                        "Staged parent is not a directory: {}",
                        current.display()
                    ));
                }
            }
            Err(error) => {
                return Err(format!(
                    "Could not create staged directory {}: {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}
