//! Read-only discovery of local Lua addon manifests and configured launch selection.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::layout::same_directory;
use crate::config::extension_config::AddonPreference;

pub const ADDON_MANIFEST_FILE_NAME: &str = "addon.toml";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AddonCommand {
    pub name: String,
    pub usage: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonPackage {
    pub manifest_path: PathBuf,
    pub id: String,
    pub name: String,
    pub author: String,
    pub version: String,
    pub description: String,
    pub homepage: Option<String>,
    pub supported_client_builds: Vec<String>,
    pub capabilities: Vec<String>,
    pub commands: Vec<AddonCommand>,
}

#[derive(Debug, Deserialize)]
struct AddonManifest {
    manifest_schema_version: u32,
    id: String,
    kind: String,
    name: String,
    author: String,
    version: String,
    description: String,
    homepage: Option<String>,
    entry: String,
    api_version: String,
    minimum_runtime_version: String,
    supported_client_builds: Vec<String>,
    capabilities: Vec<String>,
    #[serde(default)]
    commands: Vec<AddonCommand>,
}

#[derive(Debug, thiserror::Error)]
pub enum AddonDiscoveryError {
    #[error("could not inspect addon directory {path}: {source}")]
    ReadDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("addon id {id:?} is present in both {first} and {second}")]
    DuplicateId {
        id: String,
        first: PathBuf,
        second: PathBuf,
    },
}

pub fn select_addon_manifests(
    packages: &[AddonPackage],
    preferences: &[AddonPreference],
) -> Vec<PathBuf> {
    let mut selected = Vec::new();
    for preference in preferences.iter().filter(|preference| preference.enabled) {
        let Some(package) = packages.iter().find(|package| package.id == preference.id) else {
            continue;
        };
        selected.push(package.manifest_path.clone());
    }
    selected
}

pub fn discover_addons(root: &Path) -> Result<Vec<AddonPackage>, AddonDiscoveryError> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(root).map_err(|source| AddonDiscoveryError::ReadDirectory {
        path: root.to_path_buf(),
        source,
    })?;
    let mut manifests = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path().join(ADDON_MANIFEST_FILE_NAME))
        .filter(|manifest| manifest.is_file())
        .collect::<Vec<_>>();
    manifests.sort();

    let mut packages = BTreeMap::new();
    for manifest_path in manifests {
        let Ok(text) = std::fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = toml::from_str::<AddonManifest>(&text) else {
            continue;
        };
        if !validate_manifest(&manifest_path, &manifest) {
            continue;
        }
        let Ok(manifest_path) = std::fs::canonicalize(&manifest_path) else {
            continue;
        };
        let package = AddonPackage {
            manifest_path,
            id: manifest.id,
            name: manifest.name,
            author: manifest.author,
            version: manifest.version,
            description: manifest.description,
            homepage: manifest.homepage,
            supported_client_builds: manifest.supported_client_builds,
            capabilities: manifest.capabilities,
            commands: manifest.commands,
        };
        if let Some(first) = packages.insert(package.id.clone(), package.clone()) {
            return Err(AddonDiscoveryError::DuplicateId {
                id: package.id,
                first: first.manifest_path,
                second: package.manifest_path,
            });
        }
    }
    Ok(packages.into_values().collect())
}

/// Discover shipped addons and then the player's addons.
///
/// A player package that reuses a shipped id is skipped with a warning; duplicates inside one
/// root are still rejected. Equal roots are discovered once.
pub fn discover_addons_layered(
    shipped: &Path,
    user: &Path,
) -> Result<Vec<AddonPackage>, AddonDiscoveryError> {
    if same_directory(shipped, user) {
        return discover_addons(shipped);
    }
    let mut packages = discover_addons(shipped)?
        .into_iter()
        .map(|package| (package.id.clone(), package))
        .collect::<BTreeMap<_, _>>();
    for package in discover_addons(user)? {
        if let Some(shipped_package) = packages.get(&package.id) {
            tracing::warn!(
                id = %package.id,
                shipped = %shipped_package.manifest_path.display(),
                skipped = %package.manifest_path.display(),
                "user addon reuses a shipped addon id; the shipped package is used"
            );
            continue;
        }
        packages.insert(package.id.clone(), package);
    }
    Ok(packages.into_values().collect())
}

fn validate_manifest(path: &Path, manifest: &AddonManifest) -> bool {
    let entry = Path::new(&manifest.entry);
    let valid_entry = entry.file_name().is_some()
        && entry.parent() == Some(Path::new(""))
        && entry.extension().and_then(|value| value.to_str()) == Some("lua");
    let valid_text = [
        manifest.name.as_str(),
        manifest.author.as_str(),
        manifest.version.as_str(),
        manifest.description.as_str(),
        manifest.api_version.as_str(),
        manifest.minimum_runtime_version.as_str(),
    ]
    .into_iter()
    .all(|value| !value.trim().is_empty());
    let valid_commands = manifest.commands.iter().all(|command| {
        command.name.starts_with('/')
            && !command.usage.trim().is_empty()
            && !command.description.trim().is_empty()
    });
    manifest.manifest_schema_version == 1
        && manifest.kind == "addon"
        && valid_package_id(&manifest.id)
        && valid_entry
        && valid_text
        && valid_commands
        && !manifest.supported_client_builds.is_empty()
        && path.parent().unwrap_or(Path::new("")).join(entry).is_file()
}

pub(super) fn valid_package_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_addon(root: &Path, id: &str, builds: &[&str]) -> PathBuf {
        write_addon_at(root, id, id, builds)
    }

    fn write_addon_at(root: &Path, directory_name: &str, id: &str, builds: &[&str]) -> PathBuf {
        let directory = root.join(directory_name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("main.lua"), "function draw() end\n").unwrap();
        let builds = builds
            .iter()
            .map(|build| format!("\"{build}\""))
            .collect::<Vec<_>>()
            .join(", ");
        std::fs::write(
            directory.join(ADDON_MANIFEST_FILE_NAME),
            format!(
                "manifest_schema_version = 1\nid = \"{id}\"\nkind = \"addon\"\nname = \"{id}\"\nauthor = \"Tester\"\nversion = \"1.0.0\"\ndescription = \"Test addon.\"\nentry = \"main.lua\"\napi_version = \"0.1\"\nminimum_runtime_version = \"0.1.0\"\nsupported_client_builds = [{builds}]\ncapabilities = []\n"
            ),
        )
        .unwrap();
        directory.join(ADDON_MANIFEST_FILE_NAME)
    }

    #[test]
    fn selection_uses_configuration_enablement_and_order() {
        let root = tempfile::tempdir().unwrap();
        let first = write_addon(root.path(), "alpha", &["2012.09.19.0001"]);
        let second = write_addon(root.path(), "zeta", &["2012.09.19.0001"]);
        write_addon(root.path(), "disabled", &["2012.09.19.0001"]);
        let packages = discover_addons(root.path()).unwrap();
        let found = select_addon_manifests(
            &packages,
            &[
                AddonPreference {
                    id: "zeta".into(),
                    enabled: true,
                },
                AddonPreference {
                    id: "missing".into(),
                    enabled: true,
                },
                AddonPreference {
                    id: "disabled".into(),
                    enabled: false,
                },
                AddonPreference {
                    id: "alpha".into(),
                    enabled: true,
                },
            ],
        );
        assert_eq!(
            found,
            vec![
                std::fs::canonicalize(second).unwrap(),
                std::fs::canonicalize(first).unwrap()
            ]
        );
    }

    #[test]
    fn inventory_returns_manifest_metadata_without_executing_the_addon() {
        let root = tempfile::tempdir().unwrap();
        write_addon(root.path(), "fps", &["2012.09.19.0001"]);
        let found = discover_addons(root.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "fps");
        assert_eq!(found[0].author, "Tester");
        assert_eq!(found[0].description, "Test addon.");
    }

    #[test]
    fn first_party_fps_presentation_matches_its_package() {
        let manifest =
            toml::from_str::<AddonManifest>(include_str!("../../addons/fps/addon.toml")).unwrap();
        assert_eq!(manifest.id, "fps");
        assert_eq!(manifest.name, manifest.id);
        assert_eq!(manifest.author, "Aeshur");
        assert_eq!(manifest.version, "1.0");
    }

    #[test]
    fn first_party_wiki_presentation_matches_its_package() {
        let manifest =
            toml::from_str::<AddonManifest>(include_str!("../../addons/wiki/addon.toml")).unwrap();
        assert_eq!(manifest.id, "wiki");
        assert_eq!(manifest.name, "wiki");
        assert_eq!(manifest.author, "Aeshur");
        assert_eq!(
            manifest.homepage.as_deref(),
            Some("https://bahamut.miraheze.org/wiki/Main_Page")
        );
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].usage, "/wiki");
    }

    #[test]
    fn first_party_chatlogs_presentation_matches_its_package() {
        let manifest =
            toml::from_str::<AddonManifest>(include_str!("../../addons/chatlogs/addon.toml"))
                .unwrap();
        assert_eq!(manifest.id, "chatlogs");
        assert_eq!(manifest.name, manifest.id);
        assert_eq!(manifest.author, "Aeshur");
        assert!(manifest.commands.is_empty());
    }

    #[test]
    fn targetlines_is_discoverable_but_default_off() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("addons");
        let packages = discover_addons(&root).unwrap();
        let targetlines = packages
            .iter()
            .find(|package| package.id == "targetlines")
            .expect("targetlines package should be shipped");

        assert_eq!(targetlines.name, "targetlines");
        assert_eq!(targetlines.author, "Aeshur");
        assert_eq!(targetlines.version, "0.1.0");
        assert_eq!(targetlines.capabilities, ["ui.draw"]);
        assert!(targetlines.commands.is_empty());
        assert!(select_addon_manifests(std::slice::from_ref(targetlines), &[]).is_empty());
    }

    #[test]
    fn packetlogger_is_a_discoverable_opt_in_addon() {
        let manifest =
            toml::from_str::<AddonManifest>(include_str!("../../addons/packetlogger/addon.toml"))
                .unwrap();
        assert_eq!(manifest.id, "packetlogger");
        assert_eq!(manifest.name, manifest.id);
        assert_eq!(manifest.author, "Aeshur, AuroraFlare");
        assert_eq!(manifest.version, "1.0");
        assert_eq!(
            manifest
                .commands
                .iter()
                .map(|command| command.usage.as_str())
                .collect::<Vec<_>>(),
            ["/packetlogger", "/packetlogger start", "/packetlogger stop"]
        );
        assert!(manifest.capabilities.contains(&"chat.print".to_string()));
    }

    #[test]
    fn first_party_zonename_presentation_matches_its_package() {
        let manifest =
            toml::from_str::<AddonManifest>(include_str!("../../addons/zonename/addon.toml"))
                .unwrap();
        assert_eq!(manifest.id, "zonename");
        assert_eq!(manifest.name, manifest.id);
        assert!(manifest.commands.is_empty());
    }

    #[test]
    fn gameplay_overlay_lock_commands_are_discoverable() {
        for (id, text) in [
            ("distance", include_str!("../../addons/distance/addon.toml")),
            ("targethp", include_str!("../../addons/targethp/addon.toml")),
        ] {
            let manifest = toml::from_str::<AddonManifest>(text).unwrap();
            assert_eq!(manifest.id, id);
            assert!(
                manifest
                    .commands
                    .iter()
                    .any(|command| command.usage == format!("/{id} lock"))
            );
            assert!(manifest.capabilities.contains(&"chat.print".to_string()));
        }
    }

    #[test]
    fn combatparser_is_a_discoverable_opt_in_addon() {
        let manifest =
            toml::from_str::<AddonManifest>(include_str!("../../addons/combatparser/addon.toml"))
                .unwrap();
        assert_eq!(manifest.id, "combatparser");
        assert_eq!(manifest.name, manifest.id);
        assert_eq!(manifest.author, "Aeshur");
        assert_eq!(manifest.version, "1.0");
        assert_eq!(
            manifest
                .commands
                .iter()
                .map(|command| command.usage.as_str())
                .collect::<Vec<_>>(),
            ["/combatparser"]
        );
        assert!(manifest.capabilities.contains(&"ui.draw".to_string()));
        assert!(manifest.capabilities.contains(&"chat.print".to_string()));
    }

    #[test]
    fn compatibility_metadata_does_not_override_player_selection() {
        let root = tempfile::tempdir().unwrap();
        let manifest = write_addon(root.path(), "fps", &["different-build"]);
        let packages = discover_addons(root.path()).unwrap();
        assert_eq!(
            select_addon_manifests(
                &packages,
                &[AddonPreference {
                    id: "fps".into(),
                    enabled: true
                }]
            ),
            vec![std::fs::canonicalize(manifest).unwrap()]
        );
    }

    #[test]
    fn duplicate_installed_ids_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        write_addon_at(root.path(), "first", "fps", &["2012.09.19.0001"]);
        write_addon_at(root.path(), "second", "fps", &["2012.09.19.0001"]);
        assert!(matches!(
            discover_addons(root.path()),
            Err(AddonDiscoveryError::DuplicateId { id, .. }) if id == "fps"
        ));
    }

    #[test]
    fn layered_discovery_returns_shipped_and_user_packages() {
        let shipped = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        let fps = write_addon(shipped.path(), "fps", &["2012.09.19.0001"]);
        let custom = write_addon(user.path(), "custom", &["2012.09.19.0001"]);
        let found = discover_addons_layered(shipped.path(), user.path()).unwrap();
        assert_eq!(
            found
                .iter()
                .map(|package| (package.id.as_str(), package.manifest_path.clone()))
                .collect::<Vec<_>>(),
            [
                ("custom", std::fs::canonicalize(custom).unwrap()),
                ("fps", std::fs::canonicalize(fps).unwrap()),
            ]
        );
    }

    #[test]
    fn layered_discovery_keeps_the_shipped_package_for_a_reused_id() {
        let shipped = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        let fps = write_addon(shipped.path(), "fps", &["2012.09.19.0001"]);
        write_addon(user.path(), "fps", &["2012.09.19.0001"]);
        let found = discover_addons_layered(shipped.path(), user.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].manifest_path, std::fs::canonicalize(fps).unwrap());
    }

    #[test]
    fn layered_discovery_rejects_duplicates_inside_the_user_root() {
        let shipped = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        write_addon(shipped.path(), "fps", &["2012.09.19.0001"]);
        write_addon_at(user.path(), "first", "custom", &["2012.09.19.0001"]);
        write_addon_at(user.path(), "second", "custom", &["2012.09.19.0001"]);
        assert!(matches!(
            discover_addons_layered(shipped.path(), user.path()),
            Err(AddonDiscoveryError::DuplicateId { id, .. }) if id == "custom"
        ));
    }

    #[test]
    fn layered_discovery_of_one_root_matches_single_root_discovery() {
        let root = tempfile::tempdir().unwrap();
        write_addon(root.path(), "fps", &["2012.09.19.0001"]);
        write_addon(root.path(), "wiki", &["2012.09.19.0001"]);
        assert_eq!(
            discover_addons_layered(root.path(), root.path()).unwrap(),
            discover_addons(root.path()).unwrap()
        );
        write_addon_at(root.path(), "copy", "fps", &["2012.09.19.0001"]);
        assert!(matches!(
            discover_addons_layered(root.path(), root.path()),
            Err(AddonDiscoveryError::DuplicateId { id, .. }) if id == "fps"
        ));
    }

    #[test]
    fn malformed_and_missing_entry_manifests_are_omitted() {
        let root = tempfile::tempdir().unwrap();
        let malformed = root.path().join("malformed");
        std::fs::create_dir_all(&malformed).unwrap();
        std::fs::write(malformed.join(ADDON_MANIFEST_FILE_NAME), "not toml = [").unwrap();
        let missing = root.path().join("missing");
        std::fs::create_dir_all(&missing).unwrap();
        std::fs::write(
            missing.join(ADDON_MANIFEST_FILE_NAME),
            "manifest_schema_version = 1\nid = \"missing\"\nkind = \"addon\"\nname = \"Missing\"\nauthor = \"Tester\"\nversion = \"1.0.0\"\ndescription = \"Missing entry.\"\nentry = \"main.lua\"\napi_version = \"0.1\"\nminimum_runtime_version = \"0.1.0\"\nsupported_client_builds = [\"2012.09.19.0001\"]\ncapabilities = []\n",
        )
        .unwrap();
        assert!(discover_addons(root.path()).unwrap().is_empty());
    }
}
