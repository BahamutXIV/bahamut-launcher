//! Portable extension selection and extension-owned settings.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::atomic_write::write_atomic;
use crate::config::dirs;
use crate::config::preferences::ScreenshotSettings;

const PLUGINS_SECTION: &str = "plugins";
const PLUGIN_SECTION_PREFIX: &str = "plugin.";
const ADDONS_SECTION: &str = "addons";
const ADDON_SECTION_PREFIX: &str = "addon.";
const DATS_SECTION: &str = "dats";
const DAT_SECTION_PREFIX: &str = "dat.";
const GRAPHICS_SECTION: &str = "graphics";

pub const SCREENSHOT_PLUGIN_ID: &str = "screenshot";
pub const DISCORD_RPC_PLUGIN_ID: &str = "discord-rpc";
pub const OBJECT_DISTANCE_PLUGIN_ID: &str = "object-distance";
pub const CAMERA_ZOOM_PLUGIN_ID: &str = "camera-zoom";
pub const OFFICIAL_DAT_OVERLAY_PACKAGE_ID: &str = "bahamut-dats-overlay";

type IniSections = BTreeMap<String, BTreeMap<String, String>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionPreference {
    pub id: String,
    pub enabled: bool,
}

pub type AddonPreference = ExtensionPreference;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionsConfig {
    pub plugins: Vec<ExtensionPreference>,
    pub addons: Vec<AddonPreference>,
    pub object_distance_percent: u16,
    pub camera_zoom_limit: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatsConfig {
    pub packages: Vec<ExtensionPreference>,
}

impl Default for DatsConfig {
    fn default() -> Self {
        Self {
            packages: vec![ExtensionPreference {
                id: OFFICIAL_DAT_OVERLAY_PACKAGE_ID.into(),
                enabled: true,
            }],
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExtensionConfigError {
    #[error("extension configuration is malformed at line {line}: {message}")]
    Ini { line: usize, message: String },
    #[error("extension configuration is missing [{section}] {key}")]
    Missing { section: String, key: &'static str },
    #[error("extension configuration [{section}] {key} is invalid: {message}")]
    Invalid {
        section: String,
        key: &'static str,
        message: String,
    },
    #[error("extension configuration contains unsupported section [{0}]")]
    UnknownSection(String),
    #[error("extension configuration [{section}] contains unsupported key {key}")]
    UnknownKey { section: String, key: String },
    #[error("extension configuration contains duplicate {kind} id {id:?}")]
    DuplicateId { kind: &'static str, id: String },
    #[error("extension configuration names unsupported plugin {0:?}")]
    UnsupportedPlugin(String),
    #[error("could not read or write extension configuration {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not resolve extension configuration: {0}")]
    Directory(#[from] dirs::ConfigDirError),
}

impl Default for ExtensionsConfig {
    fn default() -> Self {
        Self {
            plugins: vec![
                ExtensionPreference {
                    id: SCREENSHOT_PLUGIN_ID.into(),
                    enabled: true,
                },
                ExtensionPreference {
                    id: DISCORD_RPC_PLUGIN_ID.into(),
                    enabled: true,
                },
                ExtensionPreference {
                    id: OBJECT_DISTANCE_PLUGIN_ID.into(),
                    enabled: true,
                },
                ExtensionPreference {
                    id: CAMERA_ZOOM_PLUGIN_ID.into(),
                    enabled: true,
                },
            ],
            addons: [
                ("chatlogs", true),
                ("combatparser", false),
                ("distance", true),
                ("fps", true),
                ("packetlogger", false),
                ("pos", true),
                ("targethp", true),
                ("wiki", true),
                ("zonename", true),
            ]
            .into_iter()
            .map(|(id, enabled)| ExtensionPreference {
                id: id.into(),
                enabled,
            })
            .collect(),
            object_distance_percent: 200,
            camera_zoom_limit: 15,
        }
    }
}

impl ExtensionsConfig {
    pub fn load() -> Result<Self, ExtensionConfigError> {
        load_or_create(
            &dirs::extensions_config_path()?,
            Self::default,
            Self::from_ini_str,
            Self::to_ini_string,
        )
    }

    pub fn save(&self) -> Result<(), ExtensionConfigError> {
        self.save_to(&dirs::extensions_config_path()?)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), ExtensionConfigError> {
        self.validate()?;
        write_round_tripped(path, self.to_ini_string(), Self::from_ini_str, self)
    }

    pub fn from_ini_str(text: &str) -> Result<Self, ExtensionConfigError> {
        let sections = parse_ini(text)?;
        validate_known_sections(
            &sections,
            &[PLUGINS_SECTION, ADDONS_SECTION, GRAPHICS_SECTION],
            &[PLUGIN_SECTION_PREFIX, ADDON_SECTION_PREFIX],
        )?;
        let defaults = Self::default();
        let (object_distance_percent, camera_zoom_limit) =
            if let Some(graphics) = sections.get(GRAPHICS_SECTION) {
                reject_unknown_keys(
                    GRAPHICS_SECTION,
                    graphics,
                    &["object_distance_percent", "camera_zoom_limit"],
                )?;
                (
                    parse_graphics_number(graphics, "object_distance_percent")?,
                    parse_graphics_number(graphics, "camera_zoom_limit")?,
                )
            } else {
                (defaults.object_distance_percent, defaults.camera_zoom_limit)
            };
        let config = Self {
            plugins: parse_preferences(
                &sections,
                PLUGINS_SECTION,
                PLUGIN_SECTION_PREFIX,
                "plugin",
            )?,
            addons: parse_preferences(&sections, ADDONS_SECTION, ADDON_SECTION_PREFIX, "addon")?,
            object_distance_percent,
            camera_zoom_limit,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn to_ini_string(&self) -> String {
        let mut output = format!(
            "; BahamutXIV Launcher extension selection.\n\
             ; Numbered rows define persistent enablement and launch order.\n\n\
             [graphics]\nobject_distance_percent = {}\ncamera_zoom_limit = {}\n\n[plugins]\n\n",
            self.object_distance_percent, self.camera_zoom_limit,
        );
        push_preferences(&mut output, "plugin", &self.plugins);
        output.push_str("[addons]\n\n");
        push_preferences(&mut output, "addon", &self.addons);
        output
    }

    pub fn plugin_enabled(&self, id: &str) -> bool {
        self.plugins
            .iter()
            .find(|plugin| plugin.id == id)
            .is_some_and(|plugin| plugin.enabled)
    }

    pub fn object_distance_selection(&self) -> Option<u16> {
        self.plugin_enabled(OBJECT_DISTANCE_PLUGIN_ID)
            .then_some(self.object_distance_percent)
    }

    pub fn set_object_distance_selection(
        &mut self,
        percent: Option<u16>,
    ) -> Result<(), ExtensionConfigError> {
        if let Some(value) = percent {
            validate_object_distance_percent(value)?;
            self.object_distance_percent = value;
        }
        self.set_plugin_enabled(OBJECT_DISTANCE_PLUGIN_ID, percent.is_some())
    }

    pub fn camera_zoom_selection(&self) -> Option<u8> {
        self.plugin_enabled(CAMERA_ZOOM_PLUGIN_ID)
            .then_some(self.camera_zoom_limit)
    }

    pub fn set_camera_zoom_selection(
        &mut self,
        limit: Option<u8>,
    ) -> Result<(), ExtensionConfigError> {
        if let Some(value) = limit {
            validate_camera_zoom_limit(value)?;
            self.camera_zoom_limit = value;
        }
        self.set_plugin_enabled(CAMERA_ZOOM_PLUGIN_ID, limit.is_some())
    }

    pub fn set_object_distance_percent(
        &mut self,
        percent: u16,
    ) -> Result<(), ExtensionConfigError> {
        validate_object_distance_percent(percent)?;
        self.object_distance_percent = percent;
        Ok(())
    }

    pub fn set_camera_zoom_limit(&mut self, limit: u8) -> Result<(), ExtensionConfigError> {
        validate_camera_zoom_limit(limit)?;
        self.camera_zoom_limit = limit;
        Ok(())
    }

    pub fn set_plugin_enabled(
        &mut self,
        id: &str,
        enabled: bool,
    ) -> Result<(), ExtensionConfigError> {
        if id != SCREENSHOT_PLUGIN_ID
            && id != DISCORD_RPC_PLUGIN_ID
            && id != OBJECT_DISTANCE_PLUGIN_ID
            && id != CAMERA_ZOOM_PLUGIN_ID
        {
            return Err(ExtensionConfigError::UnsupportedPlugin(id.into()));
        }
        set_enabled(&mut self.plugins, id, enabled)?;
        self.validate()
    }

    pub fn set_addon_enabled(
        &mut self,
        id: &str,
        enabled: bool,
    ) -> Result<(), ExtensionConfigError> {
        set_enabled(&mut self.addons, id, enabled)?;
        self.validate()
    }

    fn validate(&self) -> Result<(), ExtensionConfigError> {
        validate_object_distance_percent(self.object_distance_percent)?;
        validate_camera_zoom_limit(self.camera_zoom_limit)?;
        validate_preferences("plugin", &self.plugins)?;
        if let Some(plugin) = self.plugins.iter().find(|plugin| {
            plugin.id != SCREENSHOT_PLUGIN_ID
                && plugin.id != DISCORD_RPC_PLUGIN_ID
                && plugin.id != OBJECT_DISTANCE_PLUGIN_ID
                && plugin.id != CAMERA_ZOOM_PLUGIN_ID
        }) {
            return Err(ExtensionConfigError::UnsupportedPlugin(plugin.id.clone()));
        }
        validate_preferences("addon", &self.addons)
    }
}

impl DatsConfig {
    pub fn load() -> Result<Self, ExtensionConfigError> {
        load_or_create(
            &dirs::dats_config_path()?,
            Self::default,
            Self::from_ini_str,
            Self::to_ini_string,
        )
    }

    pub fn from_ini_str(text: &str) -> Result<Self, ExtensionConfigError> {
        let sections = parse_ini(text)?;
        validate_known_sections(&sections, &[DATS_SECTION], &[DAT_SECTION_PREFIX])?;
        let mut packages =
            parse_preferences(&sections, DATS_SECTION, DAT_SECTION_PREFIX, "DAT package")?;
        validate_preferences("DAT package", &packages)?;
        packages = with_official_overlay_first(packages);
        let config = Self { packages };
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self) -> Result<(), ExtensionConfigError> {
        self.save_to(&dirs::dats_config_path()?)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), ExtensionConfigError> {
        let config = Self {
            packages: with_official_overlay_first(self.packages.clone()),
        };
        config.validate()?;
        write_round_tripped(path, config.to_ini_string(), Self::from_ini_str, &config)
    }

    pub fn to_ini_string(&self) -> String {
        let mut output = String::from(
            "; BahamutXIV Launcher DAT overlay package selection.\n\
             ; Numbered rows define first-hit priority; Dats-Overlay itself is always enabled.\n\n\
             [dats]\n\n",
        );
        push_preferences(
            &mut output,
            "dat",
            &with_official_overlay_first(self.packages.clone()),
        );
        output
    }

    /// Set persistent enablement without changing the package's row position.
    pub fn set_package_enabled(
        &mut self,
        id: &str,
        enabled: bool,
    ) -> Result<(), ExtensionConfigError> {
        if id == OFFICIAL_DAT_OVERLAY_PACKAGE_ID {
            return Err(ExtensionConfigError::Invalid {
                section: DATS_SECTION.into(),
                key: "enabled",
                message: "the official DAT overlay is always enabled".into(),
            });
        }
        set_enabled(&mut self.packages, id, enabled)?;
        self.validate()
    }

    /// Move a package among installed rows while retaining stale rows.
    pub fn reorder_package_for_installed(
        &mut self,
        id: &str,
        position: usize,
        installed_ids: &[String],
    ) -> Result<(), ExtensionConfigError> {
        validate_package_id(id, "DAT package")?;
        if id == OFFICIAL_DAT_OVERLAY_PACKAGE_ID {
            return Err(ExtensionConfigError::Invalid {
                section: DATS_SECTION.into(),
                key: "position",
                message: "the official DAT overlay is fixed in the first position".into(),
            });
        }
        let mut installed = Vec::new();
        for installed_id in installed_ids {
            validate_package_id(installed_id, "DAT package")?;
            if installed.iter().any(|candidate| candidate == installed_id) {
                return Err(ExtensionConfigError::DuplicateId {
                    kind: "DAT package",
                    id: installed_id.clone(),
                });
            }
            installed.push(installed_id.clone());
        }
        if !installed.iter().any(|installed_id| installed_id == id) {
            return Err(ExtensionConfigError::Invalid {
                section: DATS_SECTION.into(),
                key: "id",
                message: format!("DAT package {id:?} is not installed"),
            });
        }
        if position >= installed.len() {
            return Err(ExtensionConfigError::Invalid {
                section: DATS_SECTION.into(),
                key: "position",
                message: format!("DAT package position must be below {}", installed.len()),
            });
        }
        if installed
            .iter()
            .any(|installed_id| installed_id == OFFICIAL_DAT_OVERLAY_PACKAGE_ID)
            && position == 0
        {
            return Err(ExtensionConfigError::Invalid {
                section: DATS_SECTION.into(),
                key: "position",
                message: "the official DAT overlay is fixed in the first position".into(),
            });
        }

        let mut ordered = self
            .packages
            .iter()
            .filter(|package| installed.iter().any(|id| id == &package.id))
            .map(|package| package.id.clone())
            .collect::<Vec<_>>();
        for installed_id in installed_ids {
            if !ordered.iter().any(|id| id == installed_id) {
                ordered.push(installed_id.clone());
            }
        }
        let current = ordered
            .iter()
            .position(|candidate| candidate == id)
            .expect("installed package was checked above");
        let package_id = ordered.remove(current);
        ordered.insert(position, package_id);

        let existing_enablement = self
            .packages
            .iter()
            .map(|package| (package.id.as_str(), package.enabled))
            .collect::<BTreeMap<_, _>>();
        let mut rows = self.packages.clone();
        let mut ordered_index = 0;
        for row in &mut rows {
            if installed.iter().any(|installed_id| installed_id == &row.id) {
                let id = &ordered[ordered_index];
                row.enabled = existing_enablement
                    .get(id.as_str())
                    .copied()
                    .unwrap_or(false);
                row.id = id.clone();
                ordered_index += 1;
            }
        }
        while ordered_index < ordered.len() {
            let id = &ordered[ordered_index];
            rows.push(ExtensionPreference {
                id: id.clone(),
                enabled: existing_enablement
                    .get(id.as_str())
                    .copied()
                    .unwrap_or(false),
            });
            ordered_index += 1;
        }
        self.packages = rows;
        self.validate()
    }

    fn validate(&self) -> Result<(), ExtensionConfigError> {
        validate_preferences("DAT package", &self.packages)
    }
}

fn with_official_overlay_first(packages: Vec<ExtensionPreference>) -> Vec<ExtensionPreference> {
    let mut ordered = Vec::with_capacity(packages.len() + 1);
    ordered.push(ExtensionPreference {
        id: OFFICIAL_DAT_OVERLAY_PACKAGE_ID.into(),
        enabled: true,
    });
    ordered.extend(
        packages
            .into_iter()
            .filter(|package| package.id != OFFICIAL_DAT_OVERLAY_PACKAGE_ID),
    );
    ordered
}

pub fn load_screenshot_settings() -> Result<ScreenshotSettings, ExtensionConfigError> {
    load_or_create(
        &dirs::screenshot_settings_path()?,
        ScreenshotSettings::default,
        screenshot_settings_from_ini_str,
        screenshot_settings_to_ini_string,
    )
}

pub(crate) fn screenshot_settings_from_ini_str(
    text: &str,
) -> Result<ScreenshotSettings, ExtensionConfigError> {
    let sections = parse_ini(text)?;
    validate_known_sections(&sections, &["screenshot"], &[])?;
    let screenshot = section(&sections, "screenshot")?;
    reject_unknown_keys(
        "screenshot",
        screenshot,
        &["format", "hide_overlays", "hotkey"],
    )?;
    Ok(ScreenshotSettings {
        format: parse_enum(screenshot, "screenshot", "format", "expected png or bmp")?,
        hide_overlays: parse_bool(screenshot, "screenshot", "hide_overlays")?,
        hotkey: parse_enum(
            screenshot,
            "screenshot",
            "hotkey",
            "expected print_screen, insert, or f1 through f9",
        )?,
    })
}

fn screenshot_settings_to_ini_string(settings: &ScreenshotSettings) -> String {
    format!(
        "; BahamutXIV Screenshot plugin settings.\n\n\
         [screenshot]\nformat = {}\nhide_overlays = {}\nhotkey = {}\n",
        settings.format, settings.hide_overlays, settings.hotkey
    )
}

fn load_or_create<T>(
    path: &Path,
    default: impl FnOnce() -> T,
    parse: impl Fn(&str) -> Result<T, ExtensionConfigError>,
    serialize: impl Fn(&T) -> String,
) -> Result<T, ExtensionConfigError>
where
    T: PartialEq,
{
    if path.exists() {
        let text = fs::read_to_string(path).map_err(|source| ExtensionConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        return parse(&text);
    }
    let value = default();
    write_round_tripped(path, serialize(&value), parse, &value)?;
    Ok(value)
}

fn write_round_tripped<T: PartialEq>(
    path: &Path,
    body: String,
    parse: impl Fn(&str) -> Result<T, ExtensionConfigError>,
    expected: &T,
) -> Result<(), ExtensionConfigError> {
    if parse(&body)? != *expected {
        return Err(ExtensionConfigError::Invalid {
            section: "configuration".into(),
            key: "configuration",
            message: "serialized configuration did not round-trip".into(),
        });
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ExtensionConfigError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    write_atomic(path, body.as_bytes()).map_err(|source| ExtensionConfigError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn parse_preferences(
    sections: &IniSections,
    owner_section: &'static str,
    prefix: &'static str,
    kind: &'static str,
) -> Result<Vec<ExtensionPreference>, ExtensionConfigError> {
    reject_unknown_keys(owner_section, section(sections, owner_section)?, &[])?;
    let mut numbered = BTreeMap::new();
    for (name, values) in sections {
        let Some(number) = name.strip_prefix(prefix) else {
            continue;
        };
        let index = number
            .parse::<usize>()
            .map_err(|_| ExtensionConfigError::Invalid {
                section: name.clone(),
                key: "section",
                message: format!("{kind} sections must be numbered from 1"),
            })?;
        if index == 0 || numbered.insert(index, (name, values)).is_some() {
            return Err(ExtensionConfigError::Invalid {
                section: name.clone(),
                key: "section",
                message: format!("{kind} sections must use unique positive numbers"),
            });
        }
    }
    let mut preferences = Vec::with_capacity(numbered.len());
    for (expected, (index, (name, values))) in numbered.into_iter().enumerate() {
        if index != expected + 1 {
            return Err(ExtensionConfigError::Invalid {
                section: name.clone(),
                key: "section",
                message: format!("{kind} sections must be contiguous from {prefix}1"),
            });
        }
        reject_unknown_keys(name, values, &["id", "enabled"])?;
        preferences.push(ExtensionPreference {
            id: required(values, name, "id")?.into(),
            enabled: parse_bool(values, name, "enabled")?,
        });
    }
    Ok(preferences)
}

fn push_preferences(output: &mut String, prefix: &str, preferences: &[ExtensionPreference]) {
    for (index, preference) in preferences.iter().enumerate() {
        output.push_str(&format!(
            "[{prefix}.{}]\nid = {}\nenabled = {}\n\n",
            index + 1,
            preference.id,
            preference.enabled
        ));
    }
}

fn set_enabled(
    preferences: &mut Vec<ExtensionPreference>,
    id: &str,
    enabled: bool,
) -> Result<(), ExtensionConfigError> {
    validate_package_id(id, "extension")?;
    if let Some(preference) = preferences.iter_mut().find(|item| item.id == id) {
        preference.enabled = enabled;
    } else {
        preferences.push(ExtensionPreference {
            id: id.into(),
            enabled,
        });
    }
    Ok(())
}

fn validate_preferences(
    kind: &'static str,
    preferences: &[ExtensionPreference],
) -> Result<(), ExtensionConfigError> {
    let mut ids = HashSet::new();
    for preference in preferences {
        validate_package_id(&preference.id, kind)?;
        if !ids.insert(preference.id.as_str()) {
            return Err(ExtensionConfigError::DuplicateId {
                kind,
                id: preference.id.clone(),
            });
        }
    }
    Ok(())
}

fn validate_package_id(id: &str, section: &str) -> Result<(), ExtensionConfigError> {
    if !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Ok(());
    }
    Err(ExtensionConfigError::Invalid {
        section: section.into(),
        key: "id",
        message: "expected 1-64 lowercase ASCII letters, digits, or hyphens".into(),
    })
}

fn parse_ini(text: &str) -> Result<IniSections, ExtensionConfigError> {
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
                return Err(ExtensionConfigError::Ini {
                    line: line_number,
                    message: "invalid section header".into(),
                });
            }
            let name = line[1..line.len() - 1].trim();
            if name.is_empty() || sections.contains_key(name) {
                return Err(ExtensionConfigError::Ini {
                    line: line_number,
                    message: format!("empty or duplicate section {name:?}"),
                });
            }
            sections.insert(name.into(), BTreeMap::new());
            current = Some(name.into());
            continue;
        }
        let section_name = current.as_ref().ok_or_else(|| ExtensionConfigError::Ini {
            line: line_number,
            message: "key appears before any section".into(),
        })?;
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| ExtensionConfigError::Ini {
                line: line_number,
                message: "expected key = value".into(),
            })?;
        let key = key.trim();
        if key.is_empty() {
            return Err(ExtensionConfigError::Ini {
                line: line_number,
                message: "empty key".into(),
            });
        }
        let values = sections
            .get_mut(section_name)
            .expect("current section exists");
        if values.insert(key.into(), value.trim().into()).is_some() {
            return Err(ExtensionConfigError::Ini {
                line: line_number,
                message: format!("duplicate key {key:?}"),
            });
        }
    }
    Ok(sections)
}

fn validate_known_sections(
    sections: &IniSections,
    fixed: &[&str],
    prefixes: &[&str],
) -> Result<(), ExtensionConfigError> {
    if let Some(name) = sections.keys().find(|name| {
        !fixed.contains(&name.as_str()) && !prefixes.iter().any(|prefix| name.starts_with(prefix))
    }) {
        return Err(ExtensionConfigError::UnknownSection(name.clone()));
    }
    Ok(())
}

fn section<'a>(
    sections: &'a IniSections,
    name: &'static str,
) -> Result<&'a BTreeMap<String, String>, ExtensionConfigError> {
    sections
        .get(name)
        .ok_or_else(|| ExtensionConfigError::Missing {
            section: name.into(),
            key: "section",
        })
}

fn reject_unknown_keys(
    section: &str,
    values: &BTreeMap<String, String>,
    known: &[&str],
) -> Result<(), ExtensionConfigError> {
    if let Some(key) = values.keys().find(|key| !known.contains(&key.as_str())) {
        return Err(ExtensionConfigError::UnknownKey {
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
) -> Result<&'a str, ExtensionConfigError> {
    values
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ExtensionConfigError::Missing {
            section: section.into(),
            key,
        })
}

fn parse_bool(
    values: &BTreeMap<String, String>,
    section: &str,
    key: &'static str,
) -> Result<bool, ExtensionConfigError> {
    required(values, section, key)?
        .parse::<bool>()
        .map_err(|_| ExtensionConfigError::Invalid {
            section: section.into(),
            key,
            message: "expected true or false".into(),
        })
}

fn validate_object_distance_percent(percent: u16) -> Result<(), ExtensionConfigError> {
    if !(125..=200).contains(&percent) || !percent.is_multiple_of(25) {
        return Err(ExtensionConfigError::Invalid {
            section: GRAPHICS_SECTION.into(),
            key: "object_distance_percent",
            message: "expected 125, 150, 175, or 200".into(),
        });
    }
    Ok(())
}

fn validate_camera_zoom_limit(limit: u8) -> Result<(), ExtensionConfigError> {
    if !(11..=15).contains(&limit) {
        return Err(ExtensionConfigError::Invalid {
            section: GRAPHICS_SECTION.into(),
            key: "camera_zoom_limit",
            message: "expected 11 through 15".into(),
        });
    }
    Ok(())
}

fn parse_graphics_number<T>(
    values: &BTreeMap<String, String>,
    key: &'static str,
) -> Result<T, ExtensionConfigError>
where
    T: std::str::FromStr,
{
    required(values, GRAPHICS_SECTION, key)?
        .parse::<T>()
        .map_err(|_| ExtensionConfigError::Invalid {
            section: GRAPHICS_SECTION.into(),
            key,
            message: "expected a whole number".into(),
        })
}

fn parse_enum<T>(
    values: &BTreeMap<String, String>,
    section: &str,
    key: &'static str,
    message: &'static str,
) -> Result<T, ExtensionConfigError>
where
    T: std::str::FromStr<Err = ()>,
{
    required(values, section, key)?
        .parse::<T>()
        .map_err(|_| ExtensionConfigError::Invalid {
            section: section.into(),
            key,
            message: message.into(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::preferences::{ScreenshotFormat, ScreenshotHotkey};

    #[test]
    fn extension_defaults_and_order_round_trip() {
        let mut config = ExtensionsConfig::default();
        assert!(config.plugins.iter().all(|plugin| plugin.enabled));
        assert_eq!(
            config
                .addons
                .iter()
                .map(|addon| (addon.id.as_str(), addon.enabled))
                .collect::<Vec<_>>(),
            [
                ("chatlogs", true),
                ("combatparser", false),
                ("distance", true),
                ("fps", true),
                ("packetlogger", false),
                ("pos", true),
                ("targethp", true),
                ("wiki", true),
                ("zonename", true),
            ]
        );
        config.set_addon_enabled("zeta", true).unwrap();
        config.set_addon_enabled("alpha", false).unwrap();
        config
            .set_plugin_enabled(SCREENSHOT_PLUGIN_ID, false)
            .unwrap();
        config
            .set_plugin_enabled(CAMERA_ZOOM_PLUGIN_ID, true)
            .unwrap();
        let reparsed = ExtensionsConfig::from_ini_str(&config.to_ini_string()).unwrap();
        assert_eq!(reparsed, config);
        assert_eq!(reparsed.addons[9].id, "zeta");
        assert!(reparsed.addons[9].enabled);
        assert_eq!(reparsed.addons[10].id, "alpha");
        assert!(!reparsed.addons[10].enabled);
        assert!(!reparsed.plugin_enabled(SCREENSHOT_PLUGIN_ID));
        assert!(reparsed.plugin_enabled(CAMERA_ZOOM_PLUGIN_ID));
    }

    #[test]
    fn graphics_values_round_trip_and_legacy_selection_keeps_defaults() {
        let mut config = ExtensionsConfig::default();
        config.set_object_distance_percent(150).unwrap();
        config.set_camera_zoom_limit(12).unwrap();
        assert_eq!(
            ExtensionsConfig::from_ini_str(&config.to_ini_string()).unwrap(),
            config
        );

        let legacy = ExtensionsConfig::default().to_ini_string().replace(
            "[graphics]\nobject_distance_percent = 200\ncamera_zoom_limit = 15\n\n",
            "",
        );
        assert_eq!(
            ExtensionsConfig::from_ini_str(&legacy).unwrap(),
            ExtensionsConfig::default()
        );

        for invalid in [124, 130, 225] {
            assert!(config.set_object_distance_percent(invalid).is_err());
            assert_eq!(config.object_distance_percent, 150);
        }
        for invalid in [10, 16] {
            assert!(config.set_camera_zoom_limit(invalid).is_err());
            assert_eq!(config.camera_zoom_limit, 12);
        }
    }

    #[test]
    fn graphics_choice_updates_enablement_and_value_together() {
        let mut config = ExtensionsConfig::default();
        assert_eq!(config.object_distance_selection(), Some(200));
        assert_eq!(config.camera_zoom_selection(), Some(15));
        config.set_object_distance_selection(Some(150)).unwrap();
        config.set_camera_zoom_selection(Some(12)).unwrap();
        assert_eq!(config.object_distance_selection(), Some(150));
        assert_eq!(config.camera_zoom_selection(), Some(12));
        assert!(config.set_object_distance_selection(Some(130)).is_err());
        assert!(config.set_camera_zoom_selection(Some(16)).is_err());
        assert_eq!(config.object_distance_selection(), Some(150));
        assert_eq!(config.camera_zoom_selection(), Some(12));
        config.set_object_distance_selection(None).unwrap();
        config.set_camera_zoom_selection(None).unwrap();
        assert_eq!(config.object_distance_selection(), None);
        assert_eq!(config.camera_zoom_selection(), None);
        assert_eq!(config.object_distance_percent, 150);
        assert_eq!(config.camera_zoom_limit, 12);
        assert_eq!(
            ExtensionsConfig::from_ini_str(&config.to_ini_string()).unwrap(),
            config
        );
    }

    #[test]
    fn extension_rows_reject_invalid_ids_duplicates_and_gaps() {
        let base = ExtensionsConfig::default().to_ini_string();
        for invalid in [
            base.replace("id = fps", "id = FPS"),
            base.replace("id = screenshot", "id = unsupported-plugin"),
            base.replace("[addon.1]", "[addon.2]"),
            base.replace(
                "[addons]",
                "[addon.2]\nid = fps\nenabled = false\n\n[addons]",
            ),
        ] {
            assert!(ExtensionsConfig::from_ini_str(&invalid).is_err());
        }
    }

    #[test]
    fn dat_packages_preserve_first_hit_order() {
        let config = DatsConfig {
            packages: vec![
                ExtensionPreference {
                    id: OFFICIAL_DAT_OVERLAY_PACKAGE_ID.into(),
                    enabled: true,
                },
                ExtensionPreference {
                    id: "high-priority".into(),
                    enabled: true,
                },
                ExtensionPreference {
                    id: "fallback".into(),
                    enabled: false,
                },
            ],
        };
        assert_eq!(
            DatsConfig::from_ini_str(&config.to_ini_string()).unwrap(),
            config
        );
    }

    #[test]
    fn official_dat_overlay_is_normalized_enabled_and_first() {
        let mut config = DatsConfig::from_ini_str(
            "[dats]\n\n[dat.1]\nid = custom\nenabled = true\n\n[dat.2]\nid = bahamut-dats-overlay\nenabled = false\n",
        )
        .unwrap();
        assert_eq!(
            config.packages,
            vec![
                ExtensionPreference {
                    id: OFFICIAL_DAT_OVERLAY_PACKAGE_ID.into(),
                    enabled: true,
                },
                ExtensionPreference {
                    id: "custom".into(),
                    enabled: true,
                },
            ]
        );
        assert!(
            config
                .set_package_enabled(OFFICIAL_DAT_OVERLAY_PACKAGE_ID, false)
                .is_err()
        );
        assert!(
            config
                .reorder_package_for_installed(
                    OFFICIAL_DAT_OVERLAY_PACKAGE_ID,
                    1,
                    &[OFFICIAL_DAT_OVERLAY_PACKAGE_ID.into(), "custom".into()],
                )
                .is_err()
        );
        assert!(
            config
                .reorder_package_for_installed(
                    "custom",
                    0,
                    &[OFFICIAL_DAT_OVERLAY_PACKAGE_ID.into(), "custom".into()],
                )
                .is_err()
        );
    }

    #[test]
    fn dat_enablement_preserves_rows_and_reorder_is_explicit() {
        let mut config = DatsConfig {
            packages: vec![
                ExtensionPreference {
                    id: OFFICIAL_DAT_OVERLAY_PACKAGE_ID.into(),
                    enabled: true,
                },
                ExtensionPreference {
                    id: "first".into(),
                    enabled: true,
                },
                ExtensionPreference {
                    id: "second".into(),
                    enabled: true,
                },
            ],
        };
        config.set_package_enabled("second", false).unwrap();
        config
            .reorder_package_for_installed(
                "second",
                1,
                &[
                    OFFICIAL_DAT_OVERLAY_PACKAGE_ID.into(),
                    String::from("first"),
                    String::from("second"),
                ],
            )
            .unwrap();
        assert_eq!(
            config.packages,
            vec![
                ExtensionPreference {
                    id: OFFICIAL_DAT_OVERLAY_PACKAGE_ID.into(),
                    enabled: true,
                },
                ExtensionPreference {
                    id: "second".into(),
                    enabled: false,
                },
                ExtensionPreference {
                    id: "first".into(),
                    enabled: true,
                },
            ]
        );
        assert!(
            config
                .reorder_package_for_installed("missing", 0, &[String::from("first")])
                .is_err()
        );
        assert!(
            config
                .reorder_package_for_installed(
                    "first",
                    2,
                    &[String::from("first"), String::from("second")]
                )
                .is_err()
        );
    }

    #[test]
    fn dat_reorder_ignores_stale_rows_and_adds_new_package_rows() {
        let mut config = DatsConfig {
            packages: vec![
                ExtensionPreference {
                    id: "stale".into(),
                    enabled: true,
                },
                ExtensionPreference {
                    id: "alpha".into(),
                    enabled: true,
                },
            ],
        };
        config
            .reorder_package_for_installed("new", 0, &[String::from("alpha"), String::from("new")])
            .unwrap();
        assert_eq!(
            config.packages,
            vec![
                ExtensionPreference {
                    id: "stale".into(),
                    enabled: true,
                },
                ExtensionPreference {
                    id: "new".into(),
                    enabled: false,
                },
                ExtensionPreference {
                    id: "alpha".into(),
                    enabled: true,
                },
            ]
        );
    }

    #[test]
    fn screenshot_settings_round_trip_and_reject_invalid_values() {
        let settings = ScreenshotSettings {
            format: ScreenshotFormat::Bmp,
            hide_overlays: false,
            hotkey: ScreenshotHotkey::Insert,
        };
        let text = screenshot_settings_to_ini_string(&settings);
        assert_eq!(screenshot_settings_from_ini_str(&text).unwrap(), settings);
        assert!(screenshot_settings_from_ini_str(&text.replace("bmp", "jpg")).is_err());
        assert!(
            screenshot_settings_from_ini_str(&text.replace("hotkey = insert", "hotkey = f10"))
                .is_err()
        );
    }

    #[test]
    fn tracked_starters_match_defaults() {
        assert_eq!(
            ExtensionsConfig::from_ini_str(include_str!("../../configs/extensions.ini")).unwrap(),
            ExtensionsConfig::default()
        );
        assert_eq!(
            DatsConfig::from_ini_str(include_str!("../../configs/dats.ini")).unwrap(),
            DatsConfig::default()
        );
        assert_eq!(
            screenshot_settings_from_ini_str(include_str!(
                "../../configs/plugins/screenshot/settings.ini"
            ))
            .unwrap(),
            ScreenshotSettings::default()
        );
    }

    #[test]
    fn missing_files_are_created_from_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("extensions.ini");
        let loaded = load_or_create(
            &path,
            ExtensionsConfig::default,
            ExtensionsConfig::from_ini_str,
            ExtensionsConfig::to_ini_string,
        )
        .unwrap();
        assert_eq!(loaded, ExtensionsConfig::default());
        assert_eq!(
            ExtensionsConfig::from_ini_str(&fs::read_to_string(path).unwrap()).unwrap(),
            loaded
        );
    }

    #[cfg(windows)]
    #[test]
    fn locked_replacements_preserve_each_settings_file() {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        fn check(save: impl Fn(&Path) -> Result<(), ExtensionConfigError>) {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("settings.ini");
            save(&path).unwrap();
            OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(b"; previous file\n")
                .unwrap();
            let original = fs::read(&path).unwrap();
            // Allow writes so the old truncating save would succeed; deny replacement.
            let locked = OpenOptions::new()
                .read(true)
                .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
                .open(&path)
                .unwrap();

            assert!(matches!(save(&path), Err(ExtensionConfigError::Io { .. })));
            assert_eq!(fs::read(&path).unwrap(), original);
            assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
            drop(locked);
            save(&path).unwrap();
            assert!(
                !fs::read_to_string(&path)
                    .unwrap()
                    .contains("; previous file")
            );
        }

        check(|path| ExtensionsConfig::default().save_to(path));
        check(|path| DatsConfig::default().save_to(path));
    }
}
