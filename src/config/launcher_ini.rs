//! Canonical portable launcher, game, developer, and server settings.
//!
//! Extension selection and extension-owned settings have separate files.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use crate::config::atomic_write::write_atomic;
use crate::config::dirs;
use crate::config::preferences::{
    AudioSettings, GameSettings, GraphicsSettings, Preferences, valid_borderless_monitor_id,
};
use crate::profiles::{self, ServerProfile, ServersFile};

const SERVERS_SECTION: &str = "servers";
const SELECTED_KEY: &str = "selected";
static CONFIG_WRITE: Mutex<()> = Mutex::new(());
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherConfig {
    pub preferences: Preferences,
    pub selected_server: String,
    pub servers: Vec<ServerProfile>,
}

#[derive(Debug, thiserror::Error)]
pub enum LauncherConfigError {
    #[error("launcher INI is malformed at line {line}: {message}")]
    Ini { line: usize, message: String },
    #[error("launcher INI is missing [{section}] {key}")]
    Missing { section: String, key: &'static str },
    #[error("launcher INI [{section}] {key} is invalid: {message}")]
    Invalid {
        section: String,
        key: &'static str,
        message: String,
    },
    #[error("launcher INI contains unsupported section [{0}]")]
    UnknownSection(String),
    #[error("launcher INI [{section}] contains unsupported key {key}")]
    UnknownKey { section: String, key: String },
    #[error("launcher configuration has no server profiles")]
    NoServers,
    #[error("launcher configuration contains duplicate server name {0:?}")]
    DuplicateServer(String),
    #[error("selected server {0:?} is not configured")]
    UnknownSelectedServer(String),
    #[error("server profile {0:?} is not configured")]
    UnknownServer(String),
    #[error("server profile is invalid: {0}")]
    Profile(#[from] profiles::ProfileError),
    #[error("could not read or write launcher configuration: {0}")]
    Io(#[from] io::Error),
    #[error("could not resolve launcher configuration: {0}")]
    Directory(#[from] dirs::ConfigDirError),
}

type IniSections = BTreeMap<String, BTreeMap<String, String>>;

impl LauncherConfig {
    /// Serializes in-process read/modify/write transactions for the launcher INI.
    pub fn write_lock() -> Result<MutexGuard<'static, ()>, LauncherConfigError> {
        CONFIG_WRITE.lock().map_err(|_| {
            LauncherConfigError::Io(io::Error::other(
                "launcher configuration writer is unavailable",
            ))
        })
    }

    pub fn defaults() -> Self {
        let parsed: ServersFile =
            toml::from_str(profiles::DEFAULT_SERVERS).expect("embedded default servers must parse");
        let selected_server = parsed
            .server
            .first()
            .expect("embedded defaults must contain a server")
            .display_name
            .clone();
        Self {
            preferences: Preferences::default(),
            selected_server,
            servers: parsed.server,
        }
    }

    pub fn load() -> Result<Self, LauncherConfigError> {
        let path = dirs::launcher_config_path()?;
        let retail_config = dirs::retail_config_sys_path().ok();
        Self::load_from_paths(&path, retail_config.as_deref())
    }

    fn load_from_paths(
        path: &Path,
        retail_config: Option<&Path>,
    ) -> Result<Self, LauncherConfigError> {
        let creating = !path.exists();
        let mut config = if !creating {
            Self::from_ini_str(&fs::read_to_string(path)?)?
        } else {
            Self::defaults()
        };

        let imported = if !config.preferences.game.initialized {
            retail_config.is_some_and(|retail_path| {
                if !retail_path.is_file() {
                    return false;
                }
                match crate::config::retail_game::import_from_path(retail_path) {
                    Ok(settings) => {
                        config.preferences.game = settings;
                        true
                    }
                    Err(error) => {
                        tracing::warn!(
                            path = %retail_path.display(),
                            error = %error,
                            "retail settings were not imported; keeping seeded defaults"
                        );
                        false
                    }
                }
            })
        } else {
            false
        };

        if creating || imported {
            config.save_to(path)?;
        }
        Ok(config)
    }

    pub fn save(&self) -> Result<(), LauncherConfigError> {
        self.save_to(&dirs::launcher_config_path()?)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), LauncherConfigError> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let body = self.to_ini_string();
        let round_trip = Self::from_ini_str(&body)?;
        if round_trip != *self {
            return Err(LauncherConfigError::Invalid {
                section: "launcher".into(),
                key: "configuration",
                message: "serialized configuration did not round-trip".into(),
            });
        }
        write_atomic(path, body.as_bytes()).map_err(LauncherConfigError::from)
    }

    pub fn select_server(&mut self, display_name: &str) -> Result<(), LauncherConfigError> {
        if !self
            .servers
            .iter()
            .any(|profile| profile.display_name == display_name)
        {
            return Err(LauncherConfigError::UnknownServer(display_name.into()));
        }
        self.selected_server = display_name.into();
        Ok(())
    }

    pub fn replace_server(
        &mut self,
        original_display_name: &str,
        profile: ServerProfile,
    ) -> Result<(), LauncherConfigError> {
        profile.validate()?;
        let index = self
            .servers
            .iter()
            .position(|candidate| candidate.display_name == original_display_name)
            .ok_or_else(|| LauncherConfigError::UnknownServer(original_display_name.into()))?;
        if self
            .servers
            .iter()
            .enumerate()
            .any(|(candidate_index, candidate)| {
                candidate_index != index && candidate.display_name == profile.display_name
            })
        {
            return Err(LauncherConfigError::DuplicateServer(profile.display_name));
        }
        if self.selected_server == original_display_name {
            self.selected_server = profile.display_name.clone();
        }
        self.servers[index] = profile;
        self.validate()
    }

    pub fn from_ini_str(text: &str) -> Result<Self, LauncherConfigError> {
        let sections = parse_ini(text)?;
        validate_known_sections(&sections)?;

        let launcher = section(&sections, "launcher")?;
        reject_unknown_keys(
            "launcher",
            launcher,
            &[
                "close_on_game_start",
                "native_resolution_override",
                "game_location",
                "content_root",
                "patch_download_dir",
                "borderless_monitor",
            ],
        )?;
        let developer = section(&sections, "developer")?;
        reject_unknown_keys("developer", developer, &["enable_verbose_wine_debug"])?;
        let servers_section = section(&sections, SERVERS_SECTION)?;
        reject_unknown_keys(SERVERS_SECTION, servers_section, &[SELECTED_KEY])?;

        let mut preferences = Preferences::default();
        preferences.launcher.close_on_game_start = match launcher.get("close_on_game_start") {
            Some(_) => parse_bool(launcher, "launcher", "close_on_game_start")?,
            None => true,
        };
        preferences.launcher.native_resolution_override =
            match launcher.get("native_resolution_override") {
                Some(_) => parse_bool(launcher, "launcher", "native_resolution_override")?,
                None => false,
            };
        preferences.launcher.game_location = optional_path(launcher, "game_location");
        preferences.launcher.content_root = optional_string(launcher, "content_root");
        preferences.launcher.patch_download_dir = optional_path(launcher, "patch_download_dir");
        preferences.launcher.borderless_monitor = optional_string(launcher, "borderless_monitor");
        preferences.developer.enable_verbose_wine_debug =
            parse_bool(developer, "developer", "enable_verbose_wine_debug")?;
        preferences.game = parse_game_settings(&sections)?;
        let selected_server = required(servers_section, SERVERS_SECTION, SELECTED_KEY)?.to_string();
        let mut numbered = BTreeMap::new();
        for (name, values) in &sections {
            let Some(number) = name.strip_prefix("server.") else {
                continue;
            };
            let index = number
                .parse::<usize>()
                .map_err(|_| LauncherConfigError::Invalid {
                    section: name.clone(),
                    key: "section",
                    message: "server sections must be numbered from 1".into(),
                })?;
            if index == 0 || numbered.insert(index, (name, values)).is_some() {
                return Err(LauncherConfigError::Invalid {
                    section: name.clone(),
                    key: "section",
                    message: "server sections must use unique positive numbers".into(),
                });
            }
        }
        let mut servers = Vec::with_capacity(numbered.len());
        for (expected, (index, (name, values))) in numbered.into_iter().enumerate() {
            if index != expected + 1 {
                return Err(LauncherConfigError::Invalid {
                    section: name.clone(),
                    key: "section",
                    message: "server sections must be contiguous from server.1".into(),
                });
            }
            reject_unknown_keys(
                name,
                values,
                &[
                    "display_name",
                    "host",
                    "auth_port",
                    "lobby_port",
                    "use_https",
                ],
            )?;
            servers.push(ServerProfile {
                display_name: required(values, name, "display_name")?.to_string(),
                host: required(values, name, "host")?.to_string(),
                auth_port: parse_u16(values, name, "auth_port")?,
                lobby_port: parse_u16(values, name, "lobby_port")?,
                use_https: parse_bool(values, name, "use_https")?,
            });
        }
        let config = Self {
            preferences,
            selected_server,
            servers,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn to_ini_string(&self) -> String {
        let p = &self.preferences;
        let mut out = String::from(
            "; BahamutXIV Launcher portable configuration.\n\
             ; Extension selection and extension-owned settings use separate files.\n\n\
             [launcher]\n",
        );
        out.push_str(&format!(
            "close_on_game_start = {}\n",
            p.launcher.close_on_game_start
        ));
        out.push_str(&format!(
            "native_resolution_override = {}\n",
            p.launcher.native_resolution_override
        ));
        push_optional(
            &mut out,
            "game_location",
            p.launcher.game_location.as_deref(),
        );
        push_optional_str(&mut out, "content_root", p.launcher.content_root.as_deref());
        push_optional(
            &mut out,
            "patch_download_dir",
            p.launcher.patch_download_dir.as_deref(),
        );
        push_optional_str(
            &mut out,
            "borderless_monitor",
            p.launcher.borderless_monitor.as_deref(),
        );
        out.push('\n');
        out.push_str(&format!(
            "[game]\ninitialized = {}\ndisplay_mode = {}\nwidth = {}\nheight = {}\n\n",
            p.game.initialized, p.game.display_mode, p.game.width, p.game.height
        ));
        out.push_str(&format!(
            "[game.graphics]\nmultisampling = {}\ngeneral_quality = {}\nbackground_quality = {}\nshadow_detail = {}\nambient_occlusion = {}\ndepth_of_field = {}\ncutscene_effects = {}\nhardware_mouse = {}\ntexture_quality = {}\ntexture_filtering = {}\n\n",
            p.game.graphics.multisampling,
            p.game.graphics.general_quality,
            p.game.graphics.background_quality,
            p.game.graphics.shadow_detail,
            p.game.graphics.ambient_occlusion,
            p.game.graphics.depth_of_field,
            p.game.graphics.cutscene_effects,
            p.game.graphics.hardware_mouse,
            p.game.graphics.texture_quality,
            p.game.graphics.texture_filtering,
        ));
        out.push_str(&format!(
            "[game.audio]\nenabled = {}\nplay_in_background = {}\n\n",
            p.game.audio.enabled, p.game.audio.play_in_background
        ));
        out.push_str(&format!(
            "[developer]\nenable_verbose_wine_debug = {}\n\n",
            p.developer.enable_verbose_wine_debug
        ));
        out.push_str(&format!(
            "[servers]\nselected = {}\n\n",
            self.selected_server
        ));
        for (index, server) in self.servers.iter().enumerate() {
            out.push_str(&format!(
                "[server.{}]\ndisplay_name = {}\nhost = {}\nauth_port = {}\nlobby_port = {}\nuse_https = {}\n\n",
                index + 1,
                server.display_name,
                server.host,
                server.auth_port,
                server.lobby_port,
                server.use_https,
            ));
        }
        out
    }

    pub fn validate(&self) -> Result<(), LauncherConfigError> {
        if self.servers.is_empty() {
            return Err(LauncherConfigError::NoServers);
        }
        validate_game_settings(&self.preferences.game)?;
        let mut names = HashSet::new();
        for server in &self.servers {
            server.validate()?;
            validate_ini_value("display_name", &server.display_name)?;
            validate_ini_value("host", &server.host)?;
            if !names.insert(server.display_name.as_str()) {
                return Err(LauncherConfigError::DuplicateServer(
                    server.display_name.clone(),
                ));
            }
        }
        for (key, value) in [
            ("selected", Some(self.selected_server.as_str())),
            (
                "content_root",
                self.preferences.launcher.content_root.as_deref(),
            ),
        ] {
            if let Some(value) = value {
                validate_ini_value(key, value)?;
            }
        }
        if let Some(id) = &self.preferences.launcher.borderless_monitor
            && !valid_borderless_monitor_id(id)
        {
            return Err(LauncherConfigError::Invalid {
                section: "launcher".into(),
                key: "borderless_monitor",
                message: format!(
                    "expected a nonempty monitor device identity of at most {} bytes without control characters",
                    crate::config::preferences::MAX_BORDERLESS_MONITOR_ID_BYTES
                ),
            });
        }
        for (key, value) in [
            (
                "game_location",
                self.preferences.launcher.game_location.as_deref(),
            ),
            (
                "patch_download_dir",
                self.preferences.launcher.patch_download_dir.as_deref(),
            ),
        ] {
            if let Some(value) = value {
                validate_ini_value(key, &value.to_string_lossy())?;
            }
        }
        if !names.contains(self.selected_server.as_str()) {
            return Err(LauncherConfigError::UnknownSelectedServer(
                self.selected_server.clone(),
            ));
        }
        Ok(())
    }
}

fn validate_ini_value(key: &'static str, value: &str) -> Result<(), LauncherConfigError> {
    if value.contains(['\r', '\n']) {
        return Err(LauncherConfigError::Invalid {
            section: "configuration".into(),
            key,
            message: "values cannot contain line breaks".into(),
        });
    }
    Ok(())
}

fn parse_ini(text: &str) -> Result<IniSections, LauncherConfigError> {
    let mut sections = IniSections::new();
    let mut current: Option<String> = None;
    for (offset, raw) in text.lines().enumerate() {
        let line_number = offset + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if !line.ends_with(']') || line.len() < 3 {
                return Err(LauncherConfigError::Ini {
                    line: line_number,
                    message: "invalid section header".into(),
                });
            }
            let name = line[1..line.len() - 1].trim();
            if name.is_empty() || sections.contains_key(name) {
                return Err(LauncherConfigError::Ini {
                    line: line_number,
                    message: format!("empty or duplicate section {name:?}"),
                });
            }
            sections.insert(name.to_string(), BTreeMap::new());
            current = Some(name.to_string());
            continue;
        }
        let section_name = current.as_ref().ok_or_else(|| LauncherConfigError::Ini {
            line: line_number,
            message: "key appears before any section".into(),
        })?;
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| LauncherConfigError::Ini {
                line: line_number,
                message: "expected key = value".into(),
            })?;
        let key = key.trim();
        if key.is_empty() {
            return Err(LauncherConfigError::Ini {
                line: line_number,
                message: "empty key".into(),
            });
        }
        let values = sections
            .get_mut(section_name)
            .expect("current section exists");
        if values
            .insert(key.to_string(), value.trim().to_string())
            .is_some()
        {
            return Err(LauncherConfigError::Ini {
                line: line_number,
                message: format!("duplicate key {key:?}"),
            });
        }
    }
    Ok(sections)
}

fn validate_known_sections(sections: &IniSections) -> Result<(), LauncherConfigError> {
    for name in sections.keys() {
        if !matches!(
            name.as_str(),
            "launcher" | "game" | "game.graphics" | "game.audio" | "developer" | "servers"
        ) && !name.starts_with("server.")
        {
            return Err(LauncherConfigError::UnknownSection(name.clone()));
        }
    }
    Ok(())
}

fn parse_game_settings(sections: &IniSections) -> Result<GameSettings, LauncherConfigError> {
    let game_sections = ["game", "game.graphics", "game.audio"];
    if let Some(missing) = game_sections
        .iter()
        .find(|name| !sections.contains_key(**name))
    {
        return Err(LauncherConfigError::Missing {
            section: (*missing).into(),
            key: "section",
        });
    }

    let game = section(sections, "game")?;
    reject_unknown_keys(
        "game",
        game,
        &["initialized", "display_mode", "width", "height"],
    )?;
    let graphics = section(sections, "game.graphics")?;
    reject_unknown_keys(
        "game.graphics",
        graphics,
        &[
            "multisampling",
            "general_quality",
            "background_quality",
            "shadow_detail",
            "ambient_occlusion",
            "depth_of_field",
            "cutscene_effects",
            "hardware_mouse",
            "texture_quality",
            "texture_filtering",
        ],
    )?;
    let audio = section(sections, "game.audio")?;
    reject_unknown_keys("game.audio", audio, &["enabled", "play_in_background"])?;

    let settings = GameSettings {
        initialized: parse_bool(game, "game", "initialized")?,
        display_mode: parse_game_enum(
            game,
            "game",
            "display_mode",
            "expected windowed, borderless, or fullscreen",
        )?,
        width: parse_u32(game, "game", "width")?,
        height: parse_u32(game, "game", "height")?,
        graphics: GraphicsSettings {
            multisampling: parse_game_enum(
                graphics,
                "game.graphics",
                "multisampling",
                "expected none, 2x, 4x, or 8x",
            )?,
            general_quality: parse_u8(graphics, "game.graphics", "general_quality")?,
            background_quality: parse_u8(graphics, "game.graphics", "background_quality")?,
            shadow_detail: parse_game_enum(
                graphics,
                "game.graphics",
                "shadow_detail",
                "expected lowest, low, standard, high, or highest",
            )?,
            ambient_occlusion: parse_bool(graphics, "game.graphics", "ambient_occlusion")?,
            depth_of_field: parse_bool(graphics, "game.graphics", "depth_of_field")?,
            cutscene_effects: parse_bool(graphics, "game.graphics", "cutscene_effects")?,
            hardware_mouse: parse_bool(graphics, "game.graphics", "hardware_mouse")?,
            texture_quality: parse_game_enum(
                graphics,
                "game.graphics",
                "texture_quality",
                "expected high, standard, or low",
            )?,
            texture_filtering: parse_game_enum(
                graphics,
                "game.graphics",
                "texture_filtering",
                "expected highest, high, standard, or low",
            )?,
        },
        audio: AudioSettings {
            enabled: parse_bool(audio, "game.audio", "enabled")?,
            play_in_background: parse_bool(audio, "game.audio", "play_in_background")?,
        },
    };
    validate_game_settings(&settings)?;
    Ok(settings)
}

fn validate_game_settings(settings: &GameSettings) -> Result<(), LauncherConfigError> {
    settings.validate().map_err(|error| {
        let (section, key, message) = match error {
            crate::config::preferences::GameSettingsError::UnsupportedResolution {
                width,
                height,
            } => (
                "game",
                "resolution",
                format!("unsupported resolution {width}x{height}"),
            ),
            crate::config::preferences::GameSettingsError::GeneralQualityOutOfRange => (
                "game.graphics",
                "general_quality",
                "expected a value from 1 through 10".into(),
            ),
            crate::config::preferences::GameSettingsError::BackgroundQualityOutOfRange => (
                "game.graphics",
                "background_quality",
                "expected a value from 1 through 5".into(),
            ),
        };
        LauncherConfigError::Invalid {
            section: section.into(),
            key,
            message,
        }
    })
}

fn section<'a>(
    sections: &'a IniSections,
    name: &'static str,
) -> Result<&'a BTreeMap<String, String>, LauncherConfigError> {
    sections
        .get(name)
        .ok_or_else(|| LauncherConfigError::Missing {
            section: name.into(),
            key: "section",
        })
}

fn reject_unknown_keys(
    section: &str,
    values: &BTreeMap<String, String>,
    known: &[&str],
) -> Result<(), LauncherConfigError> {
    if let Some(key) = values.keys().find(|key| !known.contains(&key.as_str())) {
        return Err(LauncherConfigError::UnknownKey {
            section: section.into(),
            key: key.clone(),
        });
    }
    Ok(())
}

fn required<'a>(
    values: &'a BTreeMap<String, String>,
    section: &str,
    key: &'static str,
) -> Result<&'a str, LauncherConfigError> {
    values
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| LauncherConfigError::Missing {
            section: section.into(),
            key,
        })
}

fn optional_string(values: &BTreeMap<String, String>, key: &str) -> Option<String> {
    values.get(key).filter(|value| !value.is_empty()).cloned()
}

fn optional_path(values: &BTreeMap<String, String>, key: &str) -> Option<PathBuf> {
    optional_string(values, key).map(PathBuf::from)
}

fn parse_bool(
    values: &BTreeMap<String, String>,
    section: &str,
    key: &'static str,
) -> Result<bool, LauncherConfigError> {
    required(values, section, key)?
        .parse::<bool>()
        .map_err(|_| LauncherConfigError::Invalid {
            section: section.into(),
            key,
            message: "expected true or false".into(),
        })
}

fn parse_game_enum<T>(
    values: &BTreeMap<String, String>,
    section: &str,
    key: &'static str,
    message: &'static str,
) -> Result<T, LauncherConfigError>
where
    T: std::str::FromStr<Err = ()>,
{
    required(values, section, key)?
        .parse::<T>()
        .map_err(|_| LauncherConfigError::Invalid {
            section: section.into(),
            key,
            message: message.into(),
        })
}

fn parse_u8(
    values: &BTreeMap<String, String>,
    section: &str,
    key: &'static str,
) -> Result<u8, LauncherConfigError> {
    required(values, section, key)?
        .parse::<u8>()
        .map_err(|_| LauncherConfigError::Invalid {
            section: section.into(),
            key,
            message: "expected an unsigned integer".into(),
        })
}

fn parse_u32(
    values: &BTreeMap<String, String>,
    section: &str,
    key: &'static str,
) -> Result<u32, LauncherConfigError> {
    required(values, section, key)?
        .parse::<u32>()
        .map_err(|_| LauncherConfigError::Invalid {
            section: section.into(),
            key,
            message: "expected an unsigned integer".into(),
        })
}

fn parse_u16(
    values: &BTreeMap<String, String>,
    section: &str,
    key: &'static str,
) -> Result<u16, LauncherConfigError> {
    required(values, section, key)?
        .parse::<u16>()
        .map_err(|_| LauncherConfigError::Invalid {
            section: section.into(),
            key,
            message: "expected a port from 1 through 65535".into(),
        })
}

fn push_optional(out: &mut String, key: &str, value: Option<&Path>) {
    push_optional_str(
        out,
        key,
        value.map(|path| path.to_string_lossy()).as_deref(),
    );
}

fn push_optional_str(out: &mut String, key: &str, value: Option<&str>) {
    out.push_str(key);
    out.push_str(" = ");
    if let Some(value) = value {
        out.push_str(value);
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::preferences::{
        DisplayMode, Multisampling, ShadowDetail, TextureFiltering, TextureQuality,
    };

    fn without_sections(text: &str, removed: &[&str]) -> String {
        let mut output = String::new();
        let mut skip = false;
        for line in text.lines() {
            if let Some(name) = line
                .strip_prefix('[')
                .and_then(|line| line.strip_suffix(']'))
            {
                skip = removed.contains(&name);
            }
            if !skip {
                output.push_str(line);
                output.push('\n');
            }
        }
        output
    }

    fn retail_config_image() -> Vec<u8> {
        let mut bytes = vec![0_u8; crate::config::retail_game::CONFIG_SYS_SIZE];
        let mut write = |word: usize, value: u32| {
            let offset = word * 4;
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        };
        write(0, crate::config::retail_game::CONFIG_SYS_STAMP);
        write(4, 1);
        write(5, 1920);
        write(6, 1080);
        write(7, 9);
        write(8, 4);
        write(9, 7);
        write(11, 4);
        write(12, 1);
        write(13, 1);
        write(14, 1);
        write(15, 0);
        write(16, 0);
        write(18, 1);
        write(19, 1);
        write(20, 0);
        bytes
    }

    #[test]
    fn defaults_round_trip_through_ini() {
        let config = LauncherConfig::defaults();
        let reparsed = LauncherConfig::from_ini_str(&config.to_ini_string()).unwrap();
        assert_eq!(reparsed, config);
        assert!(reparsed.preferences.launcher.close_on_game_start);
        assert_eq!(reparsed.servers.len(), 2);
        assert_eq!(reparsed.selected_server, "Bahamut");
    }

    #[test]
    fn missing_close_on_game_start_adopts_enabled_default() {
        let text = LauncherConfig::defaults()
            .to_ini_string()
            .replace("close_on_game_start = true\n", "");
        let parsed = LauncherConfig::from_ini_str(&text).unwrap();
        assert!(parsed.preferences.launcher.close_on_game_start);
        assert!(
            parsed
                .to_ini_string()
                .contains("close_on_game_start = true")
        );
    }

    #[test]
    fn disabled_close_on_game_start_round_trips() {
        let mut config = LauncherConfig::defaults();
        config.preferences.launcher.close_on_game_start = false;
        let reparsed = LauncherConfig::from_ini_str(&config.to_ini_string()).unwrap();
        assert!(!reparsed.preferences.launcher.close_on_game_start);
    }

    #[test]
    fn native_resolution_override_round_trips_and_defaults_off() {
        let config = LauncherConfig::defaults();
        assert!(!config.preferences.launcher.native_resolution_override);
        assert!(
            config
                .to_ini_string()
                .contains("native_resolution_override = false")
        );

        let mut config = config;
        config.preferences.launcher.native_resolution_override = true;
        let text = config.to_ini_string();
        assert!(text.contains("native_resolution_override = true"));
        assert!(
            LauncherConfig::from_ini_str(&text)
                .unwrap()
                .preferences
                .launcher
                .native_resolution_override
        );
    }

    #[test]
    fn borderless_monitor_identity_round_trips_and_defaults_to_nearest_monitor() {
        let mut config = LauncherConfig::defaults();
        assert_eq!(config.preferences.launcher.borderless_monitor, None);
        assert!(config.to_ini_string().contains("borderless_monitor = \n"));

        config.preferences.launcher.borderless_monitor =
            Some(r"\\?\DISPLAY#MONITOR&INSTANCE".into());
        let text = config.to_ini_string();
        assert!(text.contains(r"borderless_monitor = \\?\DISPLAY#MONITOR&INSTANCE"));
        assert_eq!(LauncherConfig::from_ini_str(&text).unwrap(), config);
    }

    #[test]
    fn borderless_monitor_identity_rejects_unbounded_or_control_character_values() {
        let mut config = LauncherConfig::defaults();
        config.preferences.launcher.borderless_monitor = Some("monitor\u{0007}id".into());
        assert!(matches!(
            config.validate(),
            Err(LauncherConfigError::Invalid {
                section,
                key: "borderless_monitor",
                ..
            }) if section == "launcher"
        ));

        config.preferences.launcher.borderless_monitor =
            Some("x".repeat(crate::config::preferences::MAX_BORDERLESS_MONITOR_ID_BYTES + 1));
        assert!(config.validate().is_err());
    }

    #[test]
    fn missing_game_sections_are_rejected() {
        let text = without_sections(
            &LauncherConfig::defaults().to_ini_string(),
            &["game", "game.graphics", "game.audio"],
        );
        assert!(matches!(
            LauncherConfig::from_ini_str(&text),
            Err(LauncherConfigError::Missing { section, key: "section" })
                if section == "game"
        ));
    }

    #[test]
    fn partial_game_sections_are_rejected() {
        let text = without_sections(
            &LauncherConfig::defaults().to_ini_string(),
            &["game.graphics", "game.audio"],
        );
        assert!(matches!(
            LauncherConfig::from_ini_str(&text),
            Err(LauncherConfigError::Missing { section, key: "section" })
                if section == "game.graphics"
        ));
    }

    #[test]
    fn game_sections_reject_unknown_keys_and_invalid_values() {
        let base = LauncherConfig::defaults().to_ini_string();
        let unknown = base.replace("[game.audio]\n", "[game.audio]\nunknown = true\n");
        assert!(matches!(
            LauncherConfig::from_ini_str(&unknown),
            Err(LauncherConfigError::UnknownKey { section, key })
                if section == "game.audio" && key == "unknown"
        ));

        for invalid in [
            base.replace("close_on_game_start = true", "close_on_game_start = maybe"),
            base.replace("display_mode = windowed", "display_mode = unsupported"),
            base.replace("general_quality = 8", "general_quality = 11"),
            base.replace("background_quality = 5", "background_quality = 0"),
            base.replace("height = 720", "height = 769"),
        ] {
            assert!(matches!(
                LauncherConfig::from_ini_str(&invalid),
                Err(LauncherConfigError::Invalid { .. })
            ));
        }
    }

    #[test]
    fn non_default_game_settings_round_trip_canonically() {
        let mut config = LauncherConfig::defaults();
        config.preferences.game = GameSettings {
            initialized: true,
            display_mode: DisplayMode::FullScreen,
            width: 1920,
            height: 1080,
            graphics: GraphicsSettings {
                multisampling: Multisampling::X8,
                general_quality: 1,
                background_quality: 1,
                shadow_detail: ShadowDetail::Lowest,
                ambient_occlusion: true,
                depth_of_field: true,
                cutscene_effects: false,
                hardware_mouse: false,
                texture_quality: TextureQuality::High,
                texture_filtering: TextureFiltering::Highest,
            },
            audio: AudioSettings {
                enabled: false,
                play_in_background: true,
            },
        };
        let text = config.to_ini_string();
        assert!(text.contains("display_mode = fullscreen"));
        assert!(text.contains("multisampling = 8x"));
        assert!(text.contains("texture_filtering = highest"));
        assert_eq!(LauncherConfig::from_ini_str(&text).unwrap(), config);
    }

    #[test]
    fn borderless_display_mode_round_trips_canonically() {
        let mut config = LauncherConfig::defaults();
        config.preferences.game.display_mode = DisplayMode::Borderless;
        let text = config.to_ini_string();
        assert!(text.contains("display_mode = borderless"));
        assert_eq!(LauncherConfig::from_ini_str(&text).unwrap(), config);
    }

    #[test]
    fn tracked_starter_matches_embedded_defaults() {
        let starter = include_str!("../../configs/bahamut.ini");
        assert_eq!(
            LauncherConfig::from_ini_str(starter).unwrap(),
            LauncherConfig::defaults()
        );
    }

    #[test]
    fn save_round_trips_preferences_and_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(dirs::LAUNCHER_CONFIG_FILE);
        let mut config = LauncherConfig::defaults();
        config.preferences.launcher.game_location = Some(PathBuf::from("C:/Games/FFXIV"));
        config.selected_server = "Bahamut Local".into();
        config.save_to(&path).unwrap();
        config.preferences.launcher.close_on_game_start = false;
        config.save_to(&path).unwrap();
        let loaded = LauncherConfig::from_ini_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded, config);
        let mut names = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, vec![dirs::LAUNCHER_CONFIG_FILE.to_string()]);
    }

    #[test]
    fn new_ini_imports_valid_retail_settings_without_rewriting_retail() {
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join(dirs::LAUNCHER_CONFIG_FILE);
        let retail = dir.path().join("config.sys");
        let retail_before = retail_config_image();
        fs::write(&retail, &retail_before).unwrap();

        let loaded = LauncherConfig::load_from_paths(&ini, Some(&retail)).unwrap();

        assert!(ini.exists());
        assert!(loaded.preferences.game.initialized);
        assert_eq!(
            loaded.preferences.game.display_mode,
            DisplayMode::FullScreen
        );
        assert_eq!(
            (
                loaded.preferences.game.width,
                loaded.preferences.game.height
            ),
            (1920, 1080)
        );
        assert_eq!(fs::read(retail).unwrap(), retail_before);
        assert_eq!(
            LauncherConfig::from_ini_str(&fs::read_to_string(ini).unwrap()).unwrap(),
            loaded
        );
    }

    #[test]
    fn missing_or_invalid_retail_keeps_seeded_defaults_pending() {
        for invalid_retail in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let ini = dir.path().join(dirs::LAUNCHER_CONFIG_FILE);
            let retail = dir.path().join("config.sys");
            if invalid_retail {
                fs::write(&retail, b"invalid").unwrap();
            }

            let loaded = LauncherConfig::load_from_paths(&ini, Some(&retail)).unwrap();
            assert_eq!(loaded.preferences.game, GameSettings::default());
            assert!(!loaded.preferences.game.initialized);
            assert!(ini.exists());
        }
    }

    #[test]
    fn rejects_unknown_keys_duplicate_names_and_unknown_selection() {
        let mut text = LauncherConfig::defaults().to_ini_string();
        text = text.replace(
            "close_on_game_start = true",
            "close_on_game_start = true\nmystery = value",
        );
        assert!(matches!(
            LauncherConfig::from_ini_str(&text),
            Err(LauncherConfigError::UnknownKey { .. })
        ));

        let mut duplicate = LauncherConfig::defaults();
        duplicate.servers[1].display_name = duplicate.servers[0].display_name.clone();
        assert!(matches!(
            duplicate.validate(),
            Err(LauncherConfigError::DuplicateServer(_))
        ));

        let mut unknown = LauncherConfig::defaults();
        unknown.selected_server = "Missing".into();
        assert!(matches!(
            unknown.validate(),
            Err(LauncherConfigError::UnknownSelectedServer(_))
        ));
    }

    #[test]
    fn launcher_ini_rejects_retired_extension_sections() {
        let base = LauncherConfig::defaults().to_ini_string();
        for section in ["[screenshot]\nformat = png\n", "[addons]\n"] {
            let text = format!("{base}\n{section}");
            assert!(matches!(
                LauncherConfig::from_ini_str(&text),
                Err(LauncherConfigError::UnknownSection(_))
            ));
        }
    }

    #[test]
    fn selection_and_profile_edits_preserve_one_valid_owner() {
        let mut config = LauncherConfig::defaults();
        config.select_server("Bahamut Local").unwrap();
        assert_eq!(config.selected_server, "Bahamut Local");

        let mut edited = config.servers[1].clone();
        edited.display_name = "Local Development".into();
        edited.auth_port = 9090;
        config.replace_server("Bahamut Local", edited).unwrap();
        assert_eq!(config.selected_server, "Local Development");
        assert_eq!(config.servers[1].auth_port, 9090);
        assert!(matches!(
            config.select_server("Missing"),
            Err(LauncherConfigError::UnknownServer(_))
        ));
    }

    #[test]
    fn existing_malformed_ini_is_reported_without_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join(dirs::LAUNCHER_CONFIG_FILE);
        fs::write(&ini, "[launcher]\nclose_on_game_start = maybe\n").unwrap();
        assert!(LauncherConfig::load_from_paths(&ini, None).is_err());
        assert_eq!(
            fs::read_to_string(ini).unwrap(),
            "[launcher]\nclose_on_game_start = maybe\n"
        );
    }
}
