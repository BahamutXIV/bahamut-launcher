//! Shipped content identities; a download host cannot authorize different bytes.

use std::collections::HashSet;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::http::ObjectSpec;
use super::manifest::{PATCH_MANIFEST, expected_sha256};
use crate::version::FFXIV_GAME_VERSION;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryManifest {
    pub schema_version: u32,
    pub content_root: Option<String>,
    #[serde(default = "default_hosted_patches")]
    pub hosted_patches: bool,
    pub base: Option<BasePackage>,
}

fn default_hosted_patches() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BasePackage {
    pub baseline_version: String,
    pub target_version: String,
    pub transition: PatchTransition,
    pub archives: Vec<BaseArchive>,
    pub final_files: Vec<InstallFile>,
    /// Peak staged bytes including patch growth, measured by the publisher.
    pub staging_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PatchTransition {
    None,
    FullChain,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaseArchive {
    pub object: ObjectSpec,
    pub files: Vec<InstallFile>,
    #[serde(default)]
    pub layout: ArchiveLayout,
    #[serde(default)]
    pub excluded_files: Vec<InstallFile>,
    #[serde(default)]
    pub empty_directories: Vec<String>,
    #[serde(default)]
    pub apple_metadata_files: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchiveLayout {
    #[default]
    Flat,
    FinalFantasyXivWrapper,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallFile {
    pub path: String,
    pub length: u64,
    pub sha256: String,
}

static SHIPPED_MANIFEST: OnceLock<Result<DeliveryManifest, String>> = OnceLock::new();

pub fn shipped_manifest() -> Result<DeliveryManifest, String> {
    shipped_manifest_ref().cloned()
}

pub fn hosted_patches() -> Result<bool, String> {
    Ok(shipped_manifest_ref()?.hosted_patches)
}

fn shipped_manifest_ref() -> Result<&'static DeliveryManifest, String> {
    SHIPPED_MANIFEST
        .get_or_init(parse_shipped_manifest)
        .as_ref()
        .map_err(Clone::clone)
}

fn parse_shipped_manifest() -> Result<DeliveryManifest, String> {
    let manifest: DeliveryManifest =
        serde_json::from_str(include_str!("../../manifests/game-delivery.json"))
            .map_err(|error| format!("Invalid shipped delivery manifest: {error}"))?;
    if !matches!(manifest.schema_version, 1 | 2) {
        return Err("Unsupported delivery manifest version.".into());
    }
    if manifest.schema_version == 1 && !manifest.hosted_patches {
        return Err("Hosted patch availability requires delivery schema 2.".into());
    }
    if let Some(base) = &manifest.base {
        base.validate()?;
    }
    Ok(manifest)
}

pub fn patch_objects() -> Vec<ObjectSpec> {
    PATCH_MANIFEST
        .iter()
        .map(|entry| ObjectSpec {
            object_key: format!("patches/1.23b/{}", entry.path),
            length: entry.size,
            sha256: expected_sha256(entry.path)
                .expect("every public patch has a SHA-256 identity")
                .to_owned(),
        })
        .collect()
}

impl BasePackage {
    pub fn validate(&self) -> Result<(), String> {
        if self.baseline_version.is_empty()
            || self.target_version != FFXIV_GAME_VERSION
            || (self.transition == PatchTransition::None
                && self.baseline_version != self.target_version)
            || self.archives.is_empty()
        {
            return Err("Unsupported base package version or patch transition.".into());
        }
        let mut names = HashSet::new();
        let mut excluded_names = HashSet::new();
        let mut empty_names = HashSet::new();
        let mut object_keys = HashSet::new();
        let mut extracted_bytes = 0_u64;
        for archive in &self.archives {
            validate_relative_path(&archive.object.object_key)?;
            validate_hash(&archive.object.sha256)?;
            if archive.object.length == 0
                || archive.files.is_empty()
                || !object_keys.insert(archive.object.object_key.to_ascii_lowercase())
            {
                return Err("Empty or duplicate base archive.".into());
            }
            for file in &archive.files {
                validate_file(file, &mut names)?;
                extracted_bytes = extracted_bytes
                    .checked_add(file.length)
                    .ok_or("Base inventory length overflow.")?;
            }
            if archive.layout == ArchiveLayout::Flat {
                if !archive.excluded_files.is_empty()
                    || !archive.empty_directories.is_empty()
                    || archive.apple_metadata_files != 0
                {
                    return Err("Flat archives cannot declare wrapped-source exclusions.".into());
                }
            } else if self.transition != PatchTransition::None {
                return Err("The wrapped final client requires a no-patch transition.".into());
            }
            if archive.layout == ArchiveLayout::FinalFantasyXivWrapper && self.archives.len() != 1 {
                return Err("The wrapped final client requires one archive.".into());
            }
            for file in &archive.excluded_files {
                validate_file(file, &mut excluded_names)?;
                if names.contains(&file.path.to_ascii_lowercase())
                    || matches!(
                        file.path.to_ascii_lowercase().as_str(),
                        "boot.ver" | "game.ver"
                    )
                {
                    return Err(format!(
                        "Excluded source collides with a managed file: {}",
                        file.path
                    ));
                }
            }
            for directory in &archive.empty_directories {
                validate_relative_path(directory)?;
                if !empty_names.insert(directory.to_ascii_lowercase())
                    || names.contains(&directory.to_ascii_lowercase())
                    || excluded_names.contains(&directory.to_ascii_lowercase())
                {
                    return Err(format!(
                        "Empty directory is duplicated or collides with a file: {directory}"
                    ));
                }
            }
        }
        if !names.is_disjoint(&excluded_names) {
            return Err("An excluded source collides with a selected file.".into());
        }
        let source_names = names.union(&excluded_names).cloned().collect();
        validate_file_ancestors(&source_names)?;
        for directory in &empty_names {
            if source_names.contains(directory) {
                return Err(format!("Empty directory collides with a file: {directory}"));
            }
            for (index, _) in directory.match_indices('/') {
                if source_names.contains(&directory[..index]) {
                    return Err(format!("Empty directory is below a file: {directory}"));
                }
            }
        }
        let mut final_names = HashSet::new();
        let mut final_bytes = 0_u64;
        for file in &self.final_files {
            validate_file(file, &mut final_names)?;
            final_bytes = final_bytes
                .checked_add(file.length)
                .ok_or("Final inventory length overflow.")?;
        }
        validate_file_ancestors(&final_names)?;
        if self.staging_bytes < extracted_bytes.max(final_bytes) {
            return Err("Staging space is smaller than the base or final inventory.".into());
        }
        for required in ["ffxivboot.exe", "ffxivgame.exe"] {
            if !names.contains(required) || !final_names.contains(required) {
                return Err(format!(
                    "Base and final inventories must contain {required}."
                ));
            }
        }
        Ok(())
    }

    pub fn download_bytes(&self) -> Result<u64, String> {
        self.archives.iter().try_fold(0_u64, |sum, archive| {
            sum.checked_add(archive.object.length)
                .ok_or_else(|| "Base download length overflow.".into())
        })
    }
}

fn validate_file(file: &InstallFile, names: &mut HashSet<String>) -> Result<(), String> {
    validate_relative_path(&file.path)?;
    validate_hash(&file.sha256)?;
    if !names.insert(file.path.to_ascii_lowercase()) {
        return Err(format!("Duplicate inventory destination: {}", file.path));
    }
    Ok(())
}

fn validate_file_ancestors(names: &HashSet<String>) -> Result<(), String> {
    for name in names {
        for (index, _) in name.match_indices('/') {
            if names.contains(&name[..index]) {
                return Err(format!("Inventory file is also a directory: {name}"));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_hash(hash: &str) -> Result<(), String> {
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid SHA-256 identity.".into());
    }
    Ok(())
}

pub(crate) fn validate_relative_path(path: &str) -> Result<(), String> {
    if path.is_empty() || !path.is_ascii() || path.contains(['\\', ':']) {
        return Err(format!("Unsafe content path: {path:?}"));
    }
    for part in path.split('/') {
        let stem = part
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if part.is_empty()
            || matches!(part, "." | "..")
            || part.ends_with(['.', ' '])
            || part
                .bytes()
                .any(|byte| byte < 32 || b"<>\"|?*".contains(&byte))
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(format!("Unsafe content path: {path:?}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_manifest_and_derived_patches_are_consistent() {
        shipped_manifest().unwrap();
        for (object, patch) in patch_objects().iter().zip(PATCH_MANIFEST) {
            assert_eq!(object.length, patch.size);
            assert_eq!(object.sha256, expected_sha256(patch.path).unwrap());
            assert!(object.object_key.ends_with(patch.path));
        }
    }

    #[test]
    fn portable_paths_reject_aliases_and_escapes() {
        for path in [
            "../x", "/x", "a//b", "a/./b", "C:/x", "a\\b", "a.", "a ", "NUL.txt", "COM1", "a:b",
            "a\0b",
        ] {
            assert!(validate_relative_path(path).is_err(), "{path:?}");
        }
        validate_relative_path("game/data/file.dat").unwrap();
    }
}
