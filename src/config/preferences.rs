//! Typed launcher and extension settings owned by portable configuration files.

use std::path::PathBuf;
use std::str::FromStr;

/// Image formats emitted by the first-party screenshot module.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScreenshotFormat {
    #[default]
    Png,
    Bmp,
}

impl std::fmt::Display for ScreenshotFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Png => "png",
            Self::Bmp => "bmp",
        })
    }
}

impl FromStr for ScreenshotFormat {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "png" => Ok(Self::Png),
            "bmp" => Ok(Self::Bmp),
            _ => Err(()),
        }
    }
}

/// Keys offered for the built-in Screenshot binding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum ScreenshotHotkey {
    #[serde(rename = "print_screen")]
    #[default]
    PrintScreen,
    #[serde(rename = "insert")]
    Insert,
    #[serde(rename = "f1")]
    F1,
    #[serde(rename = "f2")]
    F2,
    #[serde(rename = "f3")]
    F3,
    #[serde(rename = "f4")]
    F4,
    #[serde(rename = "f5")]
    F5,
    #[serde(rename = "f6")]
    F6,
    #[serde(rename = "f7")]
    F7,
    #[serde(rename = "f8")]
    F8,
    #[serde(rename = "f9")]
    F9,
}

impl ScreenshotHotkey {
    const ALL: [Self; 11] = [
        Self::PrintScreen,
        Self::Insert,
        Self::F1,
        Self::F2,
        Self::F3,
        Self::F4,
        Self::F5,
        Self::F6,
        Self::F7,
        Self::F8,
        Self::F9,
    ];
}

impl std::fmt::Display for ScreenshotHotkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::PrintScreen => "print_screen",
            Self::Insert => "insert",
            Self::F1 => "f1",
            Self::F2 => "f2",
            Self::F3 => "f3",
            Self::F4 => "f4",
            Self::F5 => "f5",
            Self::F6 => "f6",
            Self::F7 => "f7",
            Self::F8 => "f8",
            Self::F9 => "f9",
        })
    }
}

impl FromStr for ScreenshotHotkey {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|hotkey| hotkey.to_string() == value)
            .ok_or(())
    }
}

/// The display mode values accepted by the portable game settings owner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum DisplayMode {
    #[serde(rename = "windowed")]
    #[default]
    Windowed,
    #[serde(rename = "borderless")]
    Borderless,
    #[serde(rename = "fullscreen")]
    FullScreen,
}

impl std::fmt::Display for DisplayMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Windowed => "windowed",
            Self::Borderless => "borderless",
            Self::FullScreen => "fullscreen",
        })
    }
}

impl FromStr for DisplayMode {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "windowed" => Ok(Self::Windowed),
            "borderless" => Ok(Self::Borderless),
            "fullscreen" => Ok(Self::FullScreen),
            _ => Err(()),
        }
    }
}

/// The multisampling values accepted by the portable game settings owner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum Multisampling {
    #[serde(rename = "none")]
    #[default]
    None,
    #[serde(rename = "2x")]
    X2,
    #[serde(rename = "4x")]
    X4,
    #[serde(rename = "8x")]
    X8,
}

impl std::fmt::Display for Multisampling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "none",
            Self::X2 => "2x",
            Self::X4 => "4x",
            Self::X8 => "8x",
        })
    }
}

impl FromStr for Multisampling {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "none" => Ok(Self::None),
            "2x" => Ok(Self::X2),
            "4x" => Ok(Self::X4),
            "8x" => Ok(Self::X8),
            _ => Err(()),
        }
    }
}

/// The shadow detail values accepted by the portable game settings owner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum ShadowDetail {
    #[serde(rename = "lowest")]
    Lowest,
    #[serde(rename = "low")]
    Low,
    #[serde(rename = "standard")]
    #[default]
    Standard,
    #[serde(rename = "high")]
    High,
    #[serde(rename = "highest")]
    Highest,
}

impl std::fmt::Display for ShadowDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Lowest => "lowest",
            Self::Low => "low",
            Self::Standard => "standard",
            Self::High => "high",
            Self::Highest => "highest",
        })
    }
}

impl FromStr for ShadowDetail {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "lowest" => Ok(Self::Lowest),
            "low" => Ok(Self::Low),
            "standard" => Ok(Self::Standard),
            "high" => Ok(Self::High),
            "highest" => Ok(Self::Highest),
            _ => Err(()),
        }
    }
}

/// The texture quality values accepted by the portable game settings owner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum TextureQuality {
    #[serde(rename = "high")]
    High,
    #[serde(rename = "standard")]
    #[default]
    Standard,
    #[serde(rename = "low")]
    Low,
}

impl std::fmt::Display for TextureQuality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::High => "high",
            Self::Standard => "standard",
            Self::Low => "low",
        })
    }
}

impl FromStr for TextureQuality {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "high" => Ok(Self::High),
            "standard" => Ok(Self::Standard),
            "low" => Ok(Self::Low),
            _ => Err(()),
        }
    }
}

/// Supported FFXIV client resolution pairs.
pub const SUPPORTED_RESOLUTIONS: &[(u32, u32)] = &[
    (1024, 768),
    (1152, 864),
    (1280, 720),
    (1280, 768),
    (1280, 800),
    (1280, 854),
    (1280, 960),
    (1280, 1024),
    (1360, 768),
    (1366, 768),
    (1368, 768),
    (1400, 1050),
    (1440, 900),
    (1440, 1050),
    (1440, 1080),
    (1600, 900),
    (1600, 1024),
    (1600, 1050),
    (1600, 1200),
    (1680, 1050),
    (1920, 1080),
    (1920, 1200),
    (1920, 1440),
    (2048, 1536),
    (2560, 1440),
    (2560, 1600),
    (2560, 2048),
];

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GameSettings {
    /// False means the launcher has not adopted these settings for mutation.
    pub initialized: bool,
    pub display_mode: DisplayMode,
    pub width: u32,
    pub height: u32,
    pub graphics: GraphicsSettings,
    pub audio: AudioSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GraphicsSettings {
    pub multisampling: Multisampling,
    pub general_quality: u8,
    pub background_quality: u8,
    pub shadow_detail: ShadowDetail,
    pub ambient_occlusion: bool,
    pub depth_of_field: bool,
    pub cutscene_effects: bool,
    pub hardware_mouse: bool,
    pub texture_quality: TextureQuality,
    pub texture_filtering: TextureFiltering,
}

/// The texture filtering values accepted by the portable game settings owner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum TextureFiltering {
    #[serde(rename = "highest")]
    Highest,
    #[serde(rename = "high")]
    High,
    #[serde(rename = "standard")]
    #[default]
    Standard,
    #[serde(rename = "low")]
    Low,
}

impl std::fmt::Display for TextureFiltering {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Highest => "highest",
            Self::High => "high",
            Self::Standard => "standard",
            Self::Low => "low",
        })
    }
}

impl FromStr for TextureFiltering {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "highest" => Ok(Self::Highest),
            "high" => Ok(Self::High),
            "standard" => Ok(Self::Standard),
            "low" => Ok(Self::Low),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AudioSettings {
    pub enabled: bool,
    pub play_in_background: bool,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Self {
            multisampling: Multisampling::None,
            general_quality: 8,
            background_quality: 5,
            shadow_detail: ShadowDetail::Standard,
            ambient_occlusion: false,
            depth_of_field: false,
            cutscene_effects: true,
            hardware_mouse: true,
            texture_quality: TextureQuality::Standard,
            texture_filtering: TextureFiltering::Standard,
        }
    }
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            play_in_background: false,
        }
    }
}

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            initialized: false,
            display_mode: DisplayMode::Windowed,
            width: 1280,
            height: 720,
            graphics: GraphicsSettings::default(),
            audio: AudioSettings::default(),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GameSettingsError {
    #[error("unsupported resolution {width}x{height}")]
    UnsupportedResolution { width: u32, height: u32 },
    #[error("general quality must be from 1 through 10")]
    GeneralQualityOutOfRange,
    #[error("background quality must be from 1 through 5")]
    BackgroundQualityOutOfRange,
}

impl GameSettings {
    pub fn validate(&self) -> Result<(), GameSettingsError> {
        if !SUPPORTED_RESOLUTIONS.contains(&(self.width, self.height)) {
            return Err(GameSettingsError::UnsupportedResolution {
                width: self.width,
                height: self.height,
            });
        }
        if !(1..=10).contains(&self.graphics.general_quality) {
            return Err(GameSettingsError::GeneralQualityOutOfRange);
        }
        if !(1..=5).contains(&self.graphics.background_quality) {
            return Err(GameSettingsError::BackgroundQualityOutOfRange);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preferences {
    pub launcher: LauncherSection,
    pub developer: DeveloperSection,
    pub game: GameSettings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenshotSettings {
    pub format: ScreenshotFormat,
    pub hide_overlays: bool,
    pub hotkey: ScreenshotHotkey,
}

impl Default for ScreenshotSettings {
    fn default() -> Self {
        Self {
            format: ScreenshotFormat::Png,
            hide_overlays: true,
            hotkey: ScreenshotHotkey::PrintScreen,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherSection {
    pub close_on_game_start: bool,
    pub native_resolution_override: bool,
    pub game_location: Option<PathBuf>,
    /// Override only the delivery host, never the shipped content identities.
    pub content_root: Option<String>,
    /// `None` uses `XIVLegacy_Downloads` under the user's Documents folder.
    pub download_cache_dir: Option<PathBuf>,
    /// Optional Windows monitor device-interface identity for borderless mode.
    pub borderless_monitor: Option<String>,
}

pub const MAX_BORDERLESS_MONITOR_ID_BYTES: usize = 1024;

pub fn valid_borderless_monitor_id(id: &str) -> bool {
    !id.trim().is_empty()
        && id.len() <= MAX_BORDERLESS_MONITOR_ID_BYTES
        && !id.chars().any(char::is_control)
}

impl Default for LauncherSection {
    fn default() -> Self {
        Self {
            close_on_game_start: true,
            native_resolution_override: false,
            game_location: None,
            content_root: None,
            download_cache_dir: None,
            borderless_monitor: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeveloperSection {
    /// Widens Linux Wine `WINEDEBUG`; macOS ignores it and uses its own default.
    pub enable_verbose_wine_debug: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_defaults_match_the_portable_contract() {
        let game = GameSettings::default();
        assert!(!game.initialized);
        assert_eq!(game.display_mode, DisplayMode::Windowed);
        assert_eq!((game.width, game.height), (1280, 720));
        assert_eq!(game.graphics.multisampling, Multisampling::None);
        assert_eq!(game.graphics.general_quality, 8);
        assert_eq!(game.graphics.background_quality, 5);
        assert_eq!(game.graphics.shadow_detail, ShadowDetail::Standard);
        assert!(!game.graphics.ambient_occlusion);
        assert!(!game.graphics.depth_of_field);
        assert!(game.graphics.cutscene_effects);
        assert!(game.graphics.hardware_mouse);
        assert_eq!(game.graphics.texture_quality, TextureQuality::Standard);
        assert_eq!(game.graphics.texture_filtering, TextureFiltering::Standard);
        assert!(game.audio.enabled);
        assert!(!game.audio.play_in_background);
        game.validate().unwrap();
    }

    #[test]
    fn game_enum_values_use_stable_lowercase_tokens() {
        assert_eq!(DisplayMode::Borderless.to_string(), "borderless");
        assert_eq!(DisplayMode::FullScreen.to_string(), "fullscreen");
        assert_eq!(Multisampling::X2.to_string(), "2x");
        assert_eq!(ShadowDetail::Highest.to_string(), "highest");
        assert_eq!(TextureQuality::Low.to_string(), "low");
        assert_eq!(TextureFiltering::Highest.to_string(), "highest");
        assert!("FULLSCREEN".parse::<DisplayMode>().is_err());
        assert!("4X".parse::<Multisampling>().is_err());
    }

    #[test]
    fn screenshot_defaults_match_the_capture_contract() {
        let screenshot = ScreenshotSettings::default();
        assert_eq!(screenshot.format, ScreenshotFormat::Png);
        assert!(screenshot.hide_overlays);
        assert_eq!(screenshot.hotkey, ScreenshotHotkey::PrintScreen);
        assert_eq!(ScreenshotFormat::Bmp.to_string(), "bmp");
        assert!("jpg".parse::<ScreenshotFormat>().is_err());
        assert!("f10".parse::<ScreenshotHotkey>().is_err());
    }

    #[test]
    fn game_resolution_must_be_a_supported_pair() {
        let mut game = GameSettings {
            width: 1280,
            height: 768,
            ..GameSettings::default()
        };
        game.validate().unwrap();
        game.height = 769;
        assert!(matches!(
            game.validate(),
            Err(GameSettingsError::UnsupportedResolution {
                width: 1280,
                height: 769
            })
        ));
    }
}
