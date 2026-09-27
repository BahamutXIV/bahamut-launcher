//! Discovery and first-hit selection for DAT replacement packages.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::extension_config::{ExtensionPreference, OFFICIAL_DAT_OVERLAY_PACKAGE_ID};

pub const OVERLAY_MANIFEST_FILE_NAME: &str = "overlay.toml";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct OverlayManifest {
    manifest_schema_version: u32,
    id: String,
    name: String,
    author: String,
    version: String,
    description: String,
    homepage: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayPackage {
    pub manifest_path: PathBuf,
    pub root_path: PathBuf,
    pub id: String,
    pub name: String,
    pub author: String,
    pub version: String,
    pub description: String,
    pub homepage: Option<String>,
    /// Game-relative files below this package, excluding `overlay.toml`.
    pub payload_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayConflict {
    pub relative_path: PathBuf,
    pub package_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlaySelection {
    pub roots: Vec<PathBuf>,
    pub conflicts: Vec<OverlayConflict>,
}

#[derive(Debug, thiserror::Error)]
pub enum OverlayDiscoveryError {
    #[error("could not read DAT overlay root {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("DAT overlay root is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("DAT overlay package path is unsafe: {0}")]
    UnsafePath(PathBuf),
    #[error("DAT overlay package {package} is malformed: {message}")]
    InvalidManifest { package: PathBuf, message: String },
    #[error("could not parse DAT overlay manifest {path}: {source}")]
    ManifestParse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("DAT overlay package id {id:?} is declared more than once ({first} and {second})")]
    DuplicateId {
        id: String,
        first: PathBuf,
        second: PathBuf,
    },
}

#[derive(Debug, thiserror::Error)]
enum OverlayPathError {
    #[error("overlay path is empty")]
    Empty,
    #[error("overlay path is not relative: {0}")]
    NotRelative(PathBuf),
    #[error("overlay path contains a parent traversal: {0}")]
    ParentTraversal(PathBuf),
    #[error("overlay path contains an invalid component: {0}")]
    InvalidComponent(PathBuf),
}

/// Discover every direct package child under `root`.
///
/// A child directory is a package and must contain a valid `overlay.toml`.
/// An invalid launcher-owned package is ignored so it cannot block Play.
pub fn discover_overlay_packages(
    root: &Path,
) -> Result<Vec<OverlayPackage>, OverlayDiscoveryError> {
    if matches!(
        fs::symlink_metadata(root),
        Err(ref error) if error.kind() == io::ErrorKind::NotFound
    ) {
        return Ok(Vec::new());
    }
    let root = canonical_directory(root)?;
    let mut entries = read_dir(&root)?;
    entries.sort_by_key(|left| left.file_name());

    let mut packages = Vec::new();
    let mut ids = BTreeMap::<String, PathBuf>::new();
    for entry in entries {
        let official = entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case(OFFICIAL_DAT_OVERLAY_PACKAGE_ID);
        let package = match discover_overlay_package(&root, entry) {
            Ok(Some(package)) => package,
            Ok(None) => continue,
            Err(_) if official => continue,
            Err(error) => return Err(error),
        };
        let package_root = package.root_path.clone();
        let id = package.id.clone();
        if let Some(first) = ids.insert(id.clone(), package_root.clone()) {
            return Err(OverlayDiscoveryError::DuplicateId {
                id,
                first,
                second: package_root,
            });
        }
        packages.push(package);
    }
    Ok(packages)
}

fn discover_overlay_package(
    root: &Path,
    entry: fs::DirEntry,
) -> Result<Option<OverlayPackage>, OverlayDiscoveryError> {
    let path = entry.path();
    let file_type = entry
        .file_type()
        .map_err(|source| OverlayDiscoveryError::Io {
            path: path.clone(),
            source,
        })?;
    if file_type.is_symlink() || is_reparse_point(&path) {
        return Err(OverlayDiscoveryError::UnsafePath(path));
    }
    if !file_type.is_dir() {
        return Ok(None);
    }

    let package_root = canonical_directory(&path)?;
    ensure_contained(root, &package_root)?;
    let manifest_path = package_root.join(OVERLAY_MANIFEST_FILE_NAME);
    let metadata = fs::symlink_metadata(&manifest_path).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            OverlayDiscoveryError::InvalidManifest {
                package: package_root.clone(),
                message: format!("missing {OVERLAY_MANIFEST_FILE_NAME}"),
            }
        } else {
            OverlayDiscoveryError::Io {
                path: manifest_path.clone(),
                source,
            }
        }
    })?;
    if metadata.file_type().is_symlink() || is_reparse_metadata(&metadata) || !metadata.is_file() {
        return Err(OverlayDiscoveryError::UnsafePath(manifest_path));
    }
    let manifest_text =
        fs::read_to_string(&manifest_path).map_err(|source| OverlayDiscoveryError::Io {
            path: manifest_path.clone(),
            source,
        })?;
    let manifest: OverlayManifest =
        toml::from_str(&manifest_text).map_err(|source| OverlayDiscoveryError::ManifestParse {
            path: manifest_path.clone(),
            source: Box::new(source),
        })?;
    validate_manifest(&package_root, &manifest)?;

    let payload_files = collect_payload_files(&package_root)?;
    Ok(Some(OverlayPackage {
        manifest_path,
        root_path: package_root,
        id: manifest.id,
        name: manifest.name,
        author: manifest.author,
        version: manifest.version,
        description: manifest.description,
        homepage: manifest.homepage,
        payload_files,
    }))
}

/// Select enabled package roots in the exact order stored in `dats.ini`.
/// Rows for packages that are no longer installed are intentionally ignored.
pub fn select_overlay_packages(
    packages: &[OverlayPackage],
    preferences: &[ExtensionPreference],
) -> OverlaySelection {
    let mut roots = Vec::new();
    let mut selected = Vec::new();
    let official = ExtensionPreference {
        id: OFFICIAL_DAT_OVERLAY_PACKAGE_ID.into(),
        enabled: true,
    };
    for preference in std::iter::once(&official).chain(
        preferences
            .iter()
            .filter(|preference| preference.id != OFFICIAL_DAT_OVERLAY_PACKAGE_ID),
    ) {
        if !preference.enabled {
            continue;
        }
        if let Some(package) = packages.iter().find(|package| package.id == preference.id) {
            roots.push(package.root_path.clone());
            selected.push(package);
        }
    }

    let mut paths = BTreeMap::<String, (PathBuf, Vec<String>)>::new();
    for package in selected {
        for relative_path in &package.payload_files {
            let key = path_key(relative_path);
            let entry = paths
                .entry(key)
                .or_insert_with(|| (relative_path.clone(), Vec::new()));
            entry.1.push(package.id.clone());
        }
    }
    let conflicts = paths
        .into_values()
        .filter_map(|(relative_path, package_ids)| {
            (package_ids.len() > 1).then_some(OverlayConflict {
                relative_path,
                package_ids,
            })
        })
        .collect();
    OverlaySelection { roots, conflicts }
}

fn validate_manifest(
    package_root: &Path,
    manifest: &OverlayManifest,
) -> Result<(), OverlayDiscoveryError> {
    if manifest.manifest_schema_version != 1 {
        return Err(invalid_manifest(
            package_root,
            format!(
                "manifest_schema_version must be 1, got {}",
                manifest.manifest_schema_version
            ),
        ));
    }
    if !valid_package_id(&manifest.id) {
        return Err(invalid_manifest(
            package_root,
            "id must be 1-64 lowercase ASCII letters, digits, or hyphens and may not be dats-overlay",
        ));
    }
    let directory_id = package_root.file_name().and_then(|name| name.to_str());
    if directory_id != Some(manifest.id.as_str()) {
        return Err(invalid_manifest(
            package_root,
            "id must match the direct package directory name",
        ));
    }
    for (field, value) in [
        ("name", &manifest.name),
        ("author", &manifest.author),
        ("version", &manifest.version),
        ("description", &manifest.description),
    ] {
        if value.trim().is_empty() || value.contains(['\r', '\n']) {
            return Err(invalid_manifest(
                package_root,
                format!("{field} must be non-empty and may not contain line breaks"),
            ));
        }
    }
    if let Some(homepage) = &manifest.homepage
        && (homepage.trim().is_empty() || homepage.contains(['\r', '\n']))
    {
        return Err(invalid_manifest(
            package_root,
            "homepage must be non-empty and may not contain line breaks",
        ));
    }
    Ok(())
}

fn collect_payload_files(package_root: &Path) -> Result<Vec<PathBuf>, OverlayDiscoveryError> {
    let mut files = Vec::new();
    collect_payload_files_inner(package_root, package_root, &mut files)?;
    files.sort_by_key(|left| path_key(left));
    Ok(files)
}

fn collect_payload_files_inner(
    package_root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), OverlayDiscoveryError> {
    let mut entries = read_dir(directory)?;
    entries.sort_by_key(|left| left.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| OverlayDiscoveryError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() || is_reparse_metadata(&metadata) {
            return Err(OverlayDiscoveryError::UnsafePath(path));
        }
        if metadata.is_dir() {
            collect_payload_files_inner(package_root, &path, files)?;
        } else if metadata.is_file() && {
            let manifest_path = package_root.join(OVERLAY_MANIFEST_FILE_NAME);
            if cfg!(windows) {
                !path
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&manifest_path.to_string_lossy())
            } else {
                path != manifest_path
            }
        } {
            let canonical =
                fs::canonicalize(&path).map_err(|source| OverlayDiscoveryError::Io {
                    path: path.clone(),
                    source,
                })?;
            ensure_contained(package_root, &canonical)?;
            let relative = canonical
                .strip_prefix(package_root)
                .map(Path::to_path_buf)
                .map_err(|_| OverlayDiscoveryError::UnsafePath(canonical.clone()))?;
            normalize_relative_path(&relative)
                .map_err(|_| OverlayDiscoveryError::UnsafePath(canonical.clone()))?;
            files.push(relative);
        }
    }
    Ok(())
}

fn normalize_relative_path(path: &Path) -> Result<PathBuf, OverlayPathError> {
    let raw = path.as_os_str().to_string_lossy();
    if raw.is_empty() {
        return Err(OverlayPathError::Empty);
    }
    // The client path is Windows-shaped even when the launcher is compiled for
    // another host, so normalize both separators before inspecting components.
    let normalized = raw.replace('\\', "/");
    let bytes = normalized.as_bytes();
    if (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || normalized.starts_with("//")
    {
        return Err(OverlayPathError::NotRelative(path.to_path_buf()));
    }
    let mut output = PathBuf::new();
    for component in normalized.split('/') {
        if component.is_empty() || component == "." {
            return Err(OverlayPathError::InvalidComponent(path.to_path_buf()));
        }
        if component == ".." {
            return Err(OverlayPathError::ParentTraversal(path.to_path_buf()));
        }
        if component.bytes().any(|byte| {
            !(0x21..=0x7e).contains(&byte)
                || matches!(byte, b':' | b'*' | b'?' | b'"' | b'<' | b'>' | b'|')
        }) {
            return Err(OverlayPathError::InvalidComponent(path.to_path_buf()));
        }
        output.push(component);
    }
    if output.as_os_str().is_empty() {
        return Err(OverlayPathError::Empty);
    }
    Ok(output)
}

fn canonical_directory(path: &Path) -> Result<PathBuf, OverlayDiscoveryError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OverlayDiscoveryError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || is_reparse_metadata(&metadata) {
        return Err(OverlayDiscoveryError::UnsafePath(path.to_path_buf()));
    }
    if !metadata.is_dir() {
        return Err(OverlayDiscoveryError::NotDirectory(path.to_path_buf()));
    }
    let canonical = fs::canonicalize(path).map_err(|source| OverlayDiscoveryError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(canonical)
}

fn read_dir(path: &Path) -> Result<Vec<fs::DirEntry>, OverlayDiscoveryError> {
    fs::read_dir(path)
        .map_err(|source| OverlayDiscoveryError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OverlayDiscoveryError::Io {
            path: path.to_path_buf(),
            source,
        })
}

fn ensure_contained(root: &Path, path: &Path) -> Result<(), OverlayDiscoveryError> {
    if is_contained(root, path) {
        Ok(())
    } else {
        Err(OverlayDiscoveryError::UnsafePath(path.to_path_buf()))
    }
}

fn is_contained(root: &Path, path: &Path) -> bool {
    #[cfg(windows)]
    {
        let root = root
            .to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase();
        let path = path
            .to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase();
        path == root || path.starts_with(&(root + "\\"))
    }
    #[cfg(not(windows))]
    {
        path == root || path.starts_with(root)
    }
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn invalid_manifest(package: &Path, message: impl Into<String>) -> OverlayDiscoveryError {
    OverlayDiscoveryError::InvalidManifest {
        package: package.to_path_buf(),
        message: message.into(),
    }
}

fn valid_package_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id != "dats-overlay"
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn is_reparse_point(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| is_reparse_metadata(&metadata))
        .unwrap_or(false)
}

#[cfg(windows)]
fn is_reparse_metadata(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_metadata(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::extension_config::ExtensionPreference;

    fn write_package(root: &Path, directory_id: &str, id: &str, payload: &[(&str, &str)]) {
        let package = root.join(directory_id);
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join(OVERLAY_MANIFEST_FILE_NAME),
            format!(
                "manifest_schema_version = 1\nid = \"{id}\"\nname = \"{id}\"\nauthor = \"Tester\"\nversion = \"1.0.0\"\ndescription = \"Test package\"\n"
            ),
        )
        .unwrap();
        for (path, contents) in payload {
            let path = package.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
    }

    #[test]
    fn official_seed_manifest_is_a_valid_empty_noop_package() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("bahamut-dats-overlay");
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join(OVERLAY_MANIFEST_FILE_NAME),
            include_str!("../../plugins/dats/bahamut-dats-overlay/overlay.toml"),
        )
        .unwrap();
        write_package(root.path(), "custom", "custom", &[]);

        let packages = discover_overlay_packages(root.path()).unwrap();
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].id, "bahamut-dats-overlay");
        assert!(packages[0].payload_files.is_empty());
        let selection = select_overlay_packages(
            &packages,
            &[
                ExtensionPreference {
                    id: "custom".into(),
                    enabled: true,
                },
                ExtensionPreference {
                    id: "bahamut-dats-overlay".into(),
                    enabled: false,
                },
            ],
        );
        assert_eq!(
            selection.roots,
            vec![packages[0].root_path.clone(), packages[1].root_path.clone()]
        );
        assert!(selection.conflicts.is_empty());
    }

    #[test]
    fn missing_or_malformed_official_overlay_does_not_block_custom_packages() {
        for malformed in [false, true] {
            let root = tempfile::tempdir().unwrap();
            write_package(root.path(), "custom", "custom", &[("data/custom.DAT", "x")]);
            if malformed {
                let official = root.path().join(OFFICIAL_DAT_OVERLAY_PACKAGE_ID);
                fs::create_dir_all(&official).unwrap();
                fs::write(
                    official.join(OVERLAY_MANIFEST_FILE_NAME),
                    "invalid manifest",
                )
                .unwrap();
            }

            let packages = discover_overlay_packages(root.path()).unwrap();
            assert_eq!(packages.len(), 1);
            assert_eq!(packages[0].id, "custom");
            let selection = select_overlay_packages(
                &packages,
                &[ExtensionPreference {
                    id: "custom".into(),
                    enabled: true,
                }],
            );
            assert_eq!(selection.roots, vec![packages[0].root_path.clone()]);
        }
    }

    #[test]
    fn malformed_case_alias_of_official_overlay_does_not_block_custom_packages() {
        let root = tempfile::tempdir().unwrap();
        write_package(root.path(), "custom", "custom", &[]);
        let official_alias = root.path().join("Bahamut-Dats-Overlay");
        fs::create_dir_all(&official_alias).unwrap();
        fs::write(
            official_alias.join(OVERLAY_MANIFEST_FILE_NAME),
            "invalid manifest",
        )
        .unwrap();

        let packages = discover_overlay_packages(root.path()).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].id, "custom");
    }

    #[test]
    fn missing_dat_root_is_an_empty_inventory() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            discover_overlay_packages(&root.path().join("missing"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn discovery_requires_exact_schema_and_package_directory_identity() {
        let root = tempfile::tempdir().unwrap();
        write_package(root.path(), "alpha", "alpha", &[("data/2A/08.DAT", "a")]);
        let package = &discover_overlay_packages(root.path()).unwrap()[0];
        assert_eq!(package.id, "alpha");
        assert_eq!(package.payload_files, vec![PathBuf::from("data/2A/08.DAT")]);

        fs::write(
            root.path().join("alpha").join(OVERLAY_MANIFEST_FILE_NAME),
            "manifest_schema_version = 1\nid = \"alpha\"\nname = \"alpha\"\nauthor = \"Tester\"\nversion = \"1\"\ndescription = \"ok\"\nextra = true\n",
        )
        .unwrap();
        assert!(matches!(
            discover_overlay_packages(root.path()),
            Err(OverlayDiscoveryError::ManifestParse { .. })
        ));
    }

    #[test]
    fn package_id_must_match_its_directory_name() {
        let root = tempfile::tempdir().unwrap();
        write_package(root.path(), "alpha", "alpha", &[]);
        write_package(root.path(), "beta", "beta", &[]);
        fs::write(
            root.path().join("beta").join(OVERLAY_MANIFEST_FILE_NAME),
            "manifest_schema_version = 1\nid = \"alpha\"\nname = \"beta\"\nauthor = \"Tester\"\nversion = \"1\"\ndescription = \"ok\"\n",
        )
        .unwrap();
        assert!(matches!(
            discover_overlay_packages(root.path()),
            Err(OverlayDiscoveryError::InvalidManifest { package, .. })
                if package.ends_with("beta")
        ));
    }

    #[test]
    fn stale_rows_are_ignored_and_enabled_order_drives_conflicts() {
        let root = tempfile::tempdir().unwrap();
        write_package(root.path(), "alpha", "alpha", &[("data/a.DAT", "a")]);
        write_package(root.path(), "beta", "beta", &[("data/a.DAT", "b")]);
        let packages = discover_overlay_packages(root.path()).unwrap();
        let selection = select_overlay_packages(
            &packages,
            &[
                ExtensionPreference {
                    id: "missing".into(),
                    enabled: true,
                },
                ExtensionPreference {
                    id: "beta".into(),
                    enabled: true,
                },
                ExtensionPreference {
                    id: "alpha".into(),
                    enabled: true,
                },
            ],
        );
        assert_eq!(selection.roots[0], packages[1].root_path);
        assert_eq!(selection.conflicts[0].package_ids, vec!["beta", "alpha"]);
    }

    #[test]
    fn enabled_packages_with_disjoint_payloads_have_no_conflicts() {
        let root = tempfile::tempdir().unwrap();
        write_package(root.path(), "alpha", "alpha", &[("data/a.DAT", "a")]);
        write_package(root.path(), "beta", "beta", &[("data/b.DAT", "b")]);
        let packages = discover_overlay_packages(root.path()).unwrap();
        let selection = select_overlay_packages(
            &packages,
            &[
                ExtensionPreference {
                    id: "alpha".into(),
                    enabled: true,
                },
                ExtensionPreference {
                    id: "beta".into(),
                    enabled: true,
                },
            ],
        );
        assert_eq!(
            packages
                .iter()
                .find(|package| package.id == "alpha")
                .unwrap()
                .payload_files,
            vec![PathBuf::from("data/a.DAT")]
        );
        assert_eq!(
            packages
                .iter()
                .find(|package| package.id == "beta")
                .unwrap()
                .payload_files,
            vec![PathBuf::from("data/b.DAT")]
        );
        assert!(selection.conflicts.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn manifest_filename_is_excluded_case_insensitively() {
        let root = tempfile::tempdir().unwrap();
        write_package(
            root.path(),
            "alpha",
            "alpha",
            &[("data/positive.DAT", "payload")],
        );
        let package_root = root.path().join("alpha");
        fs::rename(
            package_root.join(OVERLAY_MANIFEST_FILE_NAME),
            package_root.join("OVERLAY.TOML"),
        )
        .unwrap();
        assert!(
            fs::read_dir(&package_root)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .any(|name| name == "OVERLAY.TOML")
        );

        let package = &discover_overlay_packages(root.path()).unwrap()[0];
        assert_eq!(
            package.payload_files,
            vec![PathBuf::from("data/positive.DAT")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_payload_is_rejected_before_selection() {
        let root = tempfile::tempdir().unwrap();
        write_package(root.path(), "alpha", "alpha", &[]);
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("alpha/data")).unwrap();
        assert!(matches!(
            discover_overlay_packages(root.path()),
            Err(OverlayDiscoveryError::UnsafePath(_))
        ));
    }

    #[test]
    fn windows_absolute_paths_are_rejected_on_every_host() {
        assert!(matches!(
            normalize_relative_path(Path::new(r"C:\\game\\data\\file.DAT")),
            Err(OverlayPathError::NotRelative(_))
        ));
        assert!(matches!(
            normalize_relative_path(Path::new("data/./file.DAT")),
            Err(OverlayPathError::InvalidComponent(_))
        ));
    }

    #[test]
    fn reserved_service_id_and_invalid_payload_names_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        write_package(root.path(), "dats-overlay", "dats-overlay", &[]);
        assert!(matches!(
            discover_overlay_packages(root.path()),
            Err(OverlayDiscoveryError::InvalidManifest { .. })
        ));

        fs::remove_dir_all(root.path().join("dats-overlay")).unwrap();
        write_package(
            root.path(),
            "alpha",
            "alpha",
            &[("data/bad name.DAT", "bad")],
        );
        assert!(matches!(
            discover_overlay_packages(root.path()),
            Err(OverlayDiscoveryError::UnsafePath(_))
        ));
    }
}
