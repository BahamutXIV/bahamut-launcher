//! Extension directories: shipped packages and native plugins live under the install root, and
//! writable extension state lives under the state root.

use std::io;
use std::path::{Path, PathBuf};

pub const ADDONS_DIR_NAME: &str = "addons";
pub const PLUGINS_DIR_NAME: &str = "plugins";
pub const DATS_PLUGIN_DIR_NAME: &str = "dats";
pub const CONFIG_DIR_NAME: &str = "config";
pub const SCRIPTS_DIR_NAME: &str = "scripts";
pub const LOGS_DIR_NAME: &str = "logs";
pub const SCREENSHOTS_DIR_NAME: &str = "screenshots";
pub const SCREENSHOT_PLUGIN_FILE_NAME: &str = "screenshot.dll";
pub const DISCORD_RPC_PLUGIN_FILE_NAME: &str = "discord-rpc.dll";
pub const STARTUP_SCRIPT_FILE_NAME: &str = "default.txt";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionLayout {
    pub install_root: PathBuf,
    pub state_root: PathBuf,
    /// Addon packages shipped with the launcher.
    pub shipped_addons: PathBuf,
    /// Addon packages the player installs.
    pub addons: PathBuf,
    /// Writable plugin folder that holds the player's DAT packages.
    pub plugins: PathBuf,
    pub screenshot_plugin_path: PathBuf,
    pub discord_rpc_plugin_path: PathBuf,
    /// DAT packages shipped with the launcher, including the official overlay.
    pub shipped_dats: PathBuf,
    /// DAT packages the player installs.
    pub dats: PathBuf,
    /// Folder holding the shipped startup script seed.
    pub shipped_scripts: PathBuf,
    pub scripts: PathBuf,
    pub logs: PathBuf,
    pub addon_settings: PathBuf,
    pub launcher_logs: PathBuf,
    pub chat_logs: PathBuf,
    pub screenshots: PathBuf,
}

impl ExtensionLayout {
    pub fn new(install_root: &Path, state_root: &Path) -> Self {
        let install_plugins = install_root.join(PLUGINS_DIR_NAME);
        let plugins = state_root.join(PLUGINS_DIR_NAME);
        let config = state_root.join(CONFIG_DIR_NAME);
        let logs = state_root.join(LOGS_DIR_NAME);
        Self {
            shipped_addons: install_root.join(ADDONS_DIR_NAME),
            addons: state_root.join(ADDONS_DIR_NAME),
            screenshot_plugin_path: install_plugins.join(SCREENSHOT_PLUGIN_FILE_NAME),
            discord_rpc_plugin_path: install_plugins.join(DISCORD_RPC_PLUGIN_FILE_NAME),
            shipped_dats: install_plugins.join(DATS_PLUGIN_DIR_NAME),
            dats: plugins.join(DATS_PLUGIN_DIR_NAME),
            plugins,
            shipped_scripts: install_root.join(SCRIPTS_DIR_NAME),
            scripts: state_root.join(SCRIPTS_DIR_NAME),
            addon_settings: config.join("addons"),
            launcher_logs: logs.join("launcher"),
            chat_logs: logs.join("chat"),
            screenshots: state_root.join(SCREENSHOTS_DIR_NAME),
            logs,
            install_root: install_root.to_path_buf(),
            state_root: state_root.to_path_buf(),
        }
    }

    pub fn under_launcher_dir(launcher_dir: &Path) -> Self {
        Self::new(launcher_dir, launcher_dir)
    }

    /// Create the writable extension directories, all of which sit under the state root.
    pub fn create(&self) -> io::Result<()> {
        for directory in [
            &self.addons,
            &self.plugins,
            &self.dats,
            &self.scripts,
            &self.addon_settings,
            &self.launcher_logs,
            &self.chat_logs,
            &self.screenshots,
        ] {
            std::fs::create_dir_all(directory)?;
        }
        Ok(())
    }

    pub fn startup_script_path(&self) -> PathBuf {
        self.scripts.join(STARTUP_SCRIPT_FILE_NAME)
    }

    /// Copy the shipped startup script into the state root when the state copy is missing;
    /// return whether it was written.
    pub fn seed_startup_script(&self) -> io::Result<bool> {
        if same_directory(&self.shipped_scripts, &self.scripts)
            || self.startup_script_path().exists()
        {
            return Ok(false);
        }
        let seed = match std::fs::read(self.shipped_scripts.join(STARTUP_SCRIPT_FILE_NAME)) {
            Ok(seed) => seed,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        crate::config::dirs::bootstrap_file_if_missing(
            &self.scripts,
            STARTUP_SCRIPT_FILE_NAME,
            seed,
        )
    }
}

/// Whether two package roots name the same directory.
pub(super) fn same_directory(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (std::fs::canonicalize(left), std::fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::dirs::{resolve_roots, write_test_bundle};
    use crate::extensions::{
        ADDON_MANIFEST_FILE_NAME, OVERLAY_MANIFEST_FILE_NAME, addons::discover_addons_layered,
        overlay_packages::discover_overlay_packages_layered,
    };

    fn tree(root: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path.clone());
                }
                paths.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
        paths.sort();
        paths
    }

    #[test]
    fn layout_is_stable_under_one_portable_root() {
        let layout = ExtensionLayout::under_launcher_dir(Path::new("mock-root"));
        let root = Path::new("mock-root");
        assert_eq!(layout.install_root, root);
        assert_eq!(layout.state_root, root);
        assert_eq!(layout.addons, root.join("addons"));
        assert_eq!(layout.shipped_addons, layout.addons);
        assert_eq!(layout.plugins, root.join("plugins"));
        assert_eq!(
            layout.screenshot_plugin_path,
            root.join("plugins/screenshot.dll")
        );
        assert_eq!(
            layout.discord_rpc_plugin_path,
            root.join("plugins/discord-rpc.dll")
        );
        assert_eq!(layout.dats, root.join("plugins/dats"));
        assert_eq!(layout.shipped_dats, layout.dats);
        assert_eq!(layout.shipped_scripts, layout.scripts);
        assert_eq!(layout.addon_settings, root.join("config/addons"));
        assert_eq!(layout.launcher_logs, root.join("logs/launcher"));
        assert_eq!(layout.chat_logs, root.join("logs/chat"));
    }

    #[test]
    fn create_materializes_every_managed_directory() {
        let temp = tempfile::tempdir().unwrap();
        let layout = ExtensionLayout::under_launcher_dir(temp.path());
        layout.create().unwrap();
        for directory in [
            layout.addons,
            layout.plugins,
            layout.dats,
            layout.scripts,
            layout.addon_settings,
            layout.launcher_logs,
            layout.chat_logs,
            layout.screenshots,
        ] {
            assert!(directory.is_dir(), "missing {}", directory.display());
        }
    }

    #[test]
    fn split_layout_reads_shipped_content_from_install_and_writes_state() {
        let install = Path::new("install");
        let state = Path::new("state");
        let layout = ExtensionLayout::new(install, state);
        assert_eq!(layout.install_root, install);
        assert_eq!(layout.state_root, state);
        assert_eq!(layout.shipped_addons, install.join("addons"));
        assert_eq!(layout.shipped_dats, install.join("plugins/dats"));
        assert_eq!(layout.shipped_scripts, install.join("scripts"));
        assert_eq!(
            layout.screenshot_plugin_path,
            install.join("plugins/screenshot.dll")
        );
        assert_eq!(
            layout.discord_rpc_plugin_path,
            install.join("plugins/discord-rpc.dll")
        );
        for writable in [
            &layout.addons,
            &layout.plugins,
            &layout.dats,
            &layout.scripts,
            &layout.logs,
            &layout.addon_settings,
            &layout.launcher_logs,
            &layout.chat_logs,
            &layout.screenshots,
        ] {
            assert!(writable.starts_with(state), "{}", writable.display());
        }
        assert_eq!(layout.addons, state.join("addons"));
        assert_eq!(layout.dats, state.join("plugins/dats"));
        assert_eq!(layout.addon_settings, state.join("config/addons"));
        assert_eq!(
            layout.startup_script_path(),
            state.join("scripts/default.txt")
        );
    }

    #[test]
    fn split_create_leaves_the_install_root_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let install = temp.path().join("install");
        std::fs::create_dir(&install).unwrap();
        let state = temp.path().join("state");
        let layout = ExtensionLayout::new(&install, &state);
        layout.create().unwrap();
        assert!(tree(&install).is_empty());
        for directory in [&layout.addons, &layout.dats, &layout.chat_logs] {
            assert!(directory.is_dir(), "missing {}", directory.display());
        }
    }

    #[test]
    fn startup_script_is_seeded_once_from_the_install_root() {
        let temp = tempfile::tempdir().unwrap();
        let install = temp.path().join("install");
        let state = temp.path().join("state");
        let layout = ExtensionLayout::new(&install, &state);
        assert!(!layout.seed_startup_script().unwrap(), "no seed is shipped");

        std::fs::create_dir_all(&layout.shipped_scripts).unwrap();
        std::fs::write(
            layout.shipped_scripts.join(STARTUP_SCRIPT_FILE_NAME),
            "/addon load fps\n",
        )
        .unwrap();
        assert!(layout.seed_startup_script().unwrap());
        assert_eq!(
            std::fs::read_to_string(layout.startup_script_path()).unwrap(),
            "/addon load fps\n"
        );

        std::fs::write(layout.startup_script_path(), "# edited\n").unwrap();
        assert!(!layout.seed_startup_script().unwrap());
        assert_eq!(
            std::fs::read_to_string(layout.startup_script_path()).unwrap(),
            "# edited\n"
        );

        let portable = ExtensionLayout::under_launcher_dir(&install);
        std::fs::remove_file(portable.startup_script_path()).unwrap();
        assert!(!portable.seed_startup_script().unwrap());
        assert!(!portable.startup_script_path().exists());
    }

    #[test]
    fn bundled_roots_layer_shipped_and_user_extensions() {
        let temp = tempfile::tempdir().unwrap();
        let exe = write_test_bundle(temp.path());
        let state = temp.path().join("home/.bahamut-launcher");
        let roots = resolve_roots(&exe, Some(state.clone())).unwrap();
        let layout = ExtensionLayout::new(&roots.install, &roots.state);
        assert_eq!(layout.state_root, state);

        let shipped_addon = layout.shipped_addons.join("fps");
        std::fs::create_dir_all(&shipped_addon).unwrap();
        std::fs::write(shipped_addon.join("main.lua"), "function draw() end\n").unwrap();
        std::fs::write(
            shipped_addon.join(ADDON_MANIFEST_FILE_NAME),
            "manifest_schema_version = 1\nid = \"fps\"\nkind = \"addon\"\nname = \"fps\"\nauthor = \"Tester\"\nversion = \"1.0.0\"\ndescription = \"Test addon.\"\nentry = \"main.lua\"\napi_version = \"0.1\"\nminimum_runtime_version = \"0.1.0\"\nsupported_client_builds = [\"2012.09.19.0001\"]\ncapabilities = []\n",
        )
        .unwrap();
        let official = layout.shipped_dats.join("bahamut-dats-overlay");
        std::fs::create_dir_all(&official).unwrap();
        std::fs::write(
            official.join(OVERLAY_MANIFEST_FILE_NAME),
            include_str!("../../plugins/dats/bahamut-dats-overlay/overlay.toml"),
        )
        .unwrap();
        std::fs::create_dir_all(&layout.shipped_scripts).unwrap();
        std::fs::write(
            layout.shipped_scripts.join(STARTUP_SCRIPT_FILE_NAME),
            "# seed\n",
        )
        .unwrap();
        let bundle_before = tree(&temp.path().join("Bahamut Launcher.app"));

        layout.create().unwrap();
        assert!(layout.seed_startup_script().unwrap());

        assert_eq!(
            tree(&temp.path().join("Bahamut Launcher.app")),
            bundle_before
        );
        assert!(layout.addons.is_dir() && layout.addons.starts_with(&state));
        assert_eq!(
            std::fs::read_to_string(state.join("scripts/default.txt")).unwrap(),
            "# seed\n"
        );
        let addons = discover_addons_layered(&layout.shipped_addons, &layout.addons).unwrap();
        assert_eq!(addons.len(), 1);
        assert!(
            addons[0]
                .manifest_path
                .starts_with(std::fs::canonicalize(&roots.install).unwrap())
        );
        let overlays =
            discover_overlay_packages_layered(&layout.shipped_dats, &layout.dats).unwrap();
        assert_eq!(
            overlays
                .iter()
                .map(|package| package.id.as_str())
                .collect::<Vec<_>>(),
            ["bahamut-dats-overlay"]
        );
    }
}
