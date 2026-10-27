use std::path::{Path, PathBuf};

use bahamut_launcher::config::extension_config::ExtensionsConfig;

use crate::commands::extensions::{extension_folder_path, extension_inventory_view_with_overlays};

fn addon_package(id: &str, supported: bool) -> bahamut_launcher::extensions::AddonPackage {
    bahamut_launcher::extensions::AddonPackage {
        manifest_path: PathBuf::from(format!("addons/{id}/addon.toml")),
        id: id.into(),
        name: id.into(),
        author: "Tester".into(),
        version: "1.0.0".into(),
        description: "Test addon.".into(),
        homepage: None,
        supported_client_builds: vec![if supported {
            bahamut_launcher::extensions::SUPPORTED_GAME_VERSION.into()
        } else {
            "different-build".into()
        }],
        capabilities: Vec::new(),
        commands: Vec::new(),
    }
}

fn overlay_package(id: &str, payload: &str) -> bahamut_launcher::extensions::OverlayPackage {
    bahamut_launcher::extensions::OverlayPackage {
        manifest_path: PathBuf::from(format!("plugins/dats/{id}/overlay.toml")),
        root_path: PathBuf::from(format!("plugins/dats/{id}")),
        id: id.into(),
        name: id.into(),
        author: "Tester".into(),
        version: "1.0.0".into(),
        description: "Test overlay.".into(),
        homepage: None,
        payload_files: vec![PathBuf::from(payload)],
    }
}

#[test]
fn addon_inventory_uses_persisted_state_order_and_compatibility() {
    let mut config = ExtensionsConfig::default();
    config.set_addon_enabled("zeta", true).unwrap();
    config.set_addon_enabled("alpha", false).unwrap();
    config.set_addon_enabled("legacy", true).unwrap();
    let inventory = extension_inventory_view_with_overlays(
        vec![
            addon_package("alpha", true),
            addon_package("legacy", false),
            addon_package("zeta", true),
            addon_package("fps", true),
        ],
        &config,
        Vec::new(),
        &bahamut_launcher::config::extension_config::DatsConfig::default(),
    );
    assert_eq!(
        inventory
            .addons
            .iter()
            .map(|addon| addon.id.as_str())
            .collect::<Vec<_>>(),
        vec!["fps", "zeta", "alpha", "legacy"]
    );
    assert!(inventory.addons[0].enabled);
    assert!(inventory.addons[1].enabled);
    assert!(!inventory.addons[2].enabled);
    assert_eq!(inventory.addons[3].status, "Enabled");
    assert_eq!(
        inventory.addons[3].compatibility,
        "Client build is not listed by this package"
    );
    assert_eq!(inventory.plugins.len(), 2);
    assert_eq!(inventory.plugins[0].id, "screenshot");
    assert_eq!(inventory.plugins[0].author, "Aeshur");
    assert_eq!(inventory.plugins[0].version, "1.0");
    assert_eq!(inventory.plugins[0].status, "Enabled");
    assert!(inventory.plugins[0].enabled);
    assert_eq!(inventory.plugins[0].commands.len(), 1);
    assert_eq!(inventory.plugins[0].commands[0].usage, "/screenshot");
    assert_eq!(inventory.plugins[1].id, "discord-rpc");
    assert_eq!(inventory.plugins[1].name, "DiscordRPC");
    assert!(inventory.plugins[1].enabled);
}

#[test]
fn overlay_inventory_reports_persisted_order_for_overlapping_packages() {
    let config = ExtensionsConfig::default();
    let dat_config = bahamut_launcher::config::extension_config::DatsConfig {
        packages: vec![
            bahamut_launcher::config::extension_config::ExtensionPreference {
                id: "beta".into(),
                enabled: true,
            },
            bahamut_launcher::config::extension_config::ExtensionPreference {
                id: "alpha".into(),
                enabled: true,
            },
        ],
    };
    let inventory = extension_inventory_view_with_overlays(
        Vec::new(),
        &config,
        vec![
            overlay_package("alpha", "data/shared.DAT"),
            overlay_package("beta", "data/shared.DAT"),
        ],
        &dat_config,
    );
    assert_eq!(
        inventory
            .overlays
            .iter()
            .map(|package| package.id.as_str())
            .collect::<Vec<_>>(),
        vec!["beta", "alpha"]
    );
    assert!(inventory.overlays.iter().all(|package| package.enabled));
}

#[test]
fn extension_folder_allowlist_exposes_only_package_roots() {
    let layout = bahamut_launcher::extensions::ExtensionLayout::under_launcher_dir(Path::new(
        "portable-root",
    ));
    assert_eq!(
        extension_folder_path(&layout, "addons").unwrap(),
        Path::new("portable-root/addons")
    );
    assert_eq!(
        extension_folder_path(&layout, "plugins").unwrap(),
        Path::new("portable-root/plugins")
    );
    assert_eq!(
        extension_folder_path(&layout, "logs").unwrap(),
        Path::new("portable-root/logs")
    );
    assert_eq!(
        extension_folder_path(&layout, "screenshots").unwrap(),
        Path::new("portable-root/screenshots")
    );
    assert_eq!(
        extension_folder_path(&layout, "backups").unwrap(),
        bahamut_launcher::config::dirs::backups_dir().unwrap()
    );
    assert_eq!(
        extension_folder_path(&layout, "install").unwrap(),
        Path::new("portable-root")
    );
    assert!(extension_folder_path(&layout, "dats").is_err());
}
