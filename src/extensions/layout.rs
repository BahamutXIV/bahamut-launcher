//! Portable extension directories rooted beside the launcher executable.

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionLayout {
    pub root: PathBuf,
    pub addons: PathBuf,
    pub plugins: PathBuf,
    pub screenshot_plugin_path: PathBuf,
    pub discord_rpc_plugin_path: PathBuf,
    pub dats: PathBuf,
    pub scripts: PathBuf,
    pub logs: PathBuf,
    pub addon_settings: PathBuf,
    pub launcher_logs: PathBuf,
    pub chat_logs: PathBuf,
    pub screenshots: PathBuf,
}

impl ExtensionLayout {
    pub fn under_launcher_dir(launcher_dir: &Path) -> Self {
        let root = launcher_dir.to_path_buf();
        let config = root.join(CONFIG_DIR_NAME);
        let plugins = root.join(PLUGINS_DIR_NAME);
        let logs = root.join(LOGS_DIR_NAME);
        Self {
            addons: root.join(ADDONS_DIR_NAME),
            plugins: plugins.clone(),
            screenshot_plugin_path: plugins.join(SCREENSHOT_PLUGIN_FILE_NAME),
            discord_rpc_plugin_path: plugins.join(DISCORD_RPC_PLUGIN_FILE_NAME),
            dats: plugins.join(DATS_PLUGIN_DIR_NAME),
            scripts: root.join(SCRIPTS_DIR_NAME),
            addon_settings: config.join("addons"),
            launcher_logs: logs.join("launcher"),
            chat_logs: logs.join("chat"),
            screenshots: root.join(SCREENSHOTS_DIR_NAME),
            logs,
            root,
        }
    }

    pub fn create(&self) -> std::io::Result<()> {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_stable_under_one_portable_root() {
        let layout = ExtensionLayout::under_launcher_dir(Path::new("mock-root"));
        assert_eq!(layout.root, Path::new("mock-root"));
        assert_eq!(layout.addons, layout.root.join("addons"));
        assert_eq!(layout.plugins, layout.root.join("plugins"));
        assert_eq!(
            layout.screenshot_plugin_path,
            layout.root.join("plugins/screenshot.dll")
        );
        assert_eq!(
            layout.discord_rpc_plugin_path,
            layout.root.join("plugins/discord-rpc.dll")
        );
        assert_eq!(layout.dats, layout.root.join("plugins/dats"));
        assert_eq!(layout.addon_settings, layout.root.join("config/addons"));
        assert_eq!(layout.launcher_logs, layout.root.join("logs/launcher"));
        assert_eq!(layout.chat_logs, layout.root.join("logs/chat"));
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
}
