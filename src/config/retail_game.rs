//! Lossless codec and atomic updater for the retail `config.sys` file.
//!
//! The file is a fixed-size array of little-endian `u32` words.  Only the
//! documented words are interpreted or changed; all other bytes are carried
//! through verbatim so client-specific state is not discarded.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::launcher_ini;
use crate::config::preferences::{
    DisplayMode, GameSettings, Multisampling, SUPPORTED_RESOLUTIONS, ShadowDetail,
    TextureFiltering, TextureQuality,
};

pub const CONFIG_SYS_SIZE: usize = 684;
pub const CONFIG_SYS_STAMP: u32 = 0x2012_0419;
pub const BACKUP_SUFFIX: &str = ".bak";

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, thiserror::Error)]
pub enum RetailGameError {
    #[error("config.sys has {actual} bytes; expected {expected}")]
    InvalidSize { actual: usize, expected: usize },
    #[error("config.sys has stamp 0x{actual:08x}; expected 0x{expected:08x}")]
    InvalidStamp { actual: u32, expected: u32 },
    #[error("config.sys word {word} has unsupported value {value}")]
    InvalidValue { word: usize, value: u32 },
    #[error("config.sys resolution {width}x{height} is unsupported")]
    UnsupportedResolution { width: u32, height: u32 },
    #[error("could not read or atomically replace config.sys: {0}")]
    Io(#[from] io::Error),
    #[cfg(target_os = "windows")]
    #[error("Windows MoveFileExW failed: {0}")]
    WindowsMove(#[from] windows::core::Error),
    #[error("could not persist imported game settings: {0}")]
    LauncherConfig(#[from] launcher_ini::LauncherConfigError),
}

/// Unmapped words of a newly created `config.sys`
const NEW_IMAGE_UNMAPPED_WORDS: &[(usize, u32)] = &[
    (1, 0),     // UPnP port mapping: automatic
    (2, 55296), // UPnP port
    (3, 0),     // devices
    (10, 1),
    (17, 0),
    (21, 0), // use Windows system font: off
    (22, 2), // font quality
    (23, 1), // font thickness
];
const NEW_IMAGE_FONT_OFFSET: usize = 96;
const NEW_IMAGE_FONT: &str = "MS UI Gothic";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrepareOutcome {
    Imported,
    Applied,
    Created,
}

pub fn backup_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let mut backup = path.as_os_str().to_os_string();
    backup.push(BACKUP_SUFFIX);
    PathBuf::from(backup)
}

/// Decode an in-memory retail `config.sys` image after structural validation.
pub fn decode(bytes: &[u8]) -> Result<GameSettings, RetailGameError> {
    decode_image(bytes, false)
}

fn decode_image(
    bytes: &[u8],
    allow_unsupported_resolution: bool,
) -> Result<GameSettings, RetailGameError> {
    validate_size_and_stamp(bytes)?;

    let word = |index: usize| read_word(bytes, index);
    let width = word(5);
    let height = word(6);
    if !allow_unsupported_resolution {
        validate_resolution(width, height)?;
    }

    Ok(GameSettings {
        initialized: true,
        display_mode: decode_display_mode(word(4))?,
        width,
        height,
        graphics: crate::config::preferences::GraphicsSettings {
            general_quality: decode_quality(word(7), 7, 10)?,
            background_quality: decode_quality(word(8), 8, 5)?,
            multisampling: decode_multisampling(word(9))?,
            shadow_detail: decode_shadow_detail(word(11))?,
            ambient_occlusion: decode_bool(word(12), 12)?,
            depth_of_field: decode_bool(word(13), 13)?,
            cutscene_effects: !decode_bool(word(14), 14)?,
            texture_quality: decode_texture_quality(word(15))?,
            texture_filtering: decode_texture_filtering(word(16))?,
            hardware_mouse: decode_bool(word(20), 20)?,
        },
        audio: crate::config::preferences::AudioSettings {
            enabled: !decode_bool(word(18), 18)?,
            play_in_background: decode_bool(word(19), 19)?,
        },
    })
}

pub fn import_from_path(path: impl AsRef<Path>) -> Result<GameSettings, RetailGameError> {
    decode(&fs::read(path)?)
}

/// Prepare the retail configuration using an optional launch-only monitor
/// resolution while keeping the launcher's saved game resolution untouched.
pub fn prepare_at_paths_with_native_resolution(
    config: &mut launcher_ini::LauncherConfig,
    launcher_path: &Path,
    retail_path: &Path,
    native_resolution: Option<(u32, u32)>,
) -> Result<PrepareOutcome, RetailGameError> {
    // Only an absent file is created; any other read failure stops launch.
    let Some(original) = read_existing(retail_path)? else {
        let fallback = config.preferences.game.clone();
        let mut adopted = effective_settings(&fallback, native_resolution);
        adopted.initialized = true;
        create_at_path_for_preparation(retail_path, &adopted, native_resolution)?;
        if !config.preferences.game.initialized {
            let mut persisted = fallback;
            persisted.initialized = true;
            let previous = std::mem::replace(&mut config.preferences.game, persisted);
            if let Err(error) = config.save_to(launcher_path) {
                config.preferences.game = previous;
                let _ = fs::remove_file(retail_path);
                return Err(error.into());
            }
        }
        tracing::info!(
            path = %retail_path.display(),
            "created config.sys from the launcher game settings"
        );
        return Ok(PrepareOutcome::Created);
    };

    if config.preferences.game.initialized {
        let settings = effective_settings(&config.preferences.game, native_resolution);
        apply_image_for_preparation(retail_path, &original, &settings, native_resolution)?;
        return Ok(PrepareOutcome::Applied);
    }

    let imported = decode(&original)?;
    let previous = std::mem::replace(&mut config.preferences.game, imported);
    if let Err(error) = config.save_to(launcher_path) {
        config.preferences.game = previous;
        return Err(error.into());
    }
    let settings = effective_settings(&config.preferences.game, native_resolution);
    apply_image_for_preparation(retail_path, &original, &settings, native_resolution)?;
    Ok(PrepareOutcome::Imported)
}

fn effective_settings(
    settings: &GameSettings,
    native_resolution: Option<(u32, u32)>,
) -> GameSettings {
    let Some((width, height)) = native_resolution else {
        return settings.clone();
    };
    let mut effective = settings.clone();
    effective.width = width;
    effective.height = height;
    effective
}

fn read_existing(path: &Path) -> Result<Option<Vec<u8>>, RetailGameError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Apply mapped settings while preserving every other byte in `config.sys`.
///
/// The replacement is staged and flushed in the same directory.  On Windows,
/// `ReplaceFileW` performs the replacement and creates the `.bak` sidecar as a
/// single OS operation.  Other platforms stage the backup first and then use
/// the platform's same-directory atomic `rename` primitive.
fn apply_image_for_preparation(
    path: &Path,
    original: &[u8],
    settings: &GameSettings,
    native_resolution: Option<(u32, u32)>,
) -> Result<(), RetailGameError> {
    let _ = decode_image(original, true)?;
    validate_settings_with_native_resolution(settings, native_resolution)?;

    let mut updated = original.to_vec();
    encode_mapped_words(&mut updated, settings);
    if updated == original {
        return Ok(());
    }

    let temporary = temporary_path(path);
    let mut temporary_file = match create_temporary(&temporary, path) {
        Ok(file) => file,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error.into());
        }
    };
    if let Err(error) = temporary_file.write_all(&updated).and_then(|_| {
        temporary_file.flush()?;
        temporary_file.sync_all()
    }) {
        drop(temporary_file);
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    drop(temporary_file);

    let result = replace_with_backup(path, &temporary, &backup_path(path), original);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Create a missing `config.sys` from `settings` over the documented
/// retail defaults for every unmapped word.
///
/// The image is staged and flushed in the same directory and then linked
/// into place.  An existing file is never replaced and no `.bak` is written.
pub fn create_at_path(
    path: impl AsRef<Path>,
    settings: &GameSettings,
) -> Result<(), RetailGameError> {
    create_at_path_for_preparation(path, settings, None)
}

fn create_at_path_for_preparation(
    path: impl AsRef<Path>,
    settings: &GameSettings,
    native_resolution: Option<(u32, u32)>,
) -> Result<(), RetailGameError> {
    let path = path.as_ref();
    validate_settings_with_native_resolution(settings, native_resolution)?;
    let image = new_image(settings);

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(path);
    let mut temporary_file = match create_temporary(&temporary, path) {
        Ok(file) => file,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error.into());
        }
    };
    if let Err(error) = temporary_file.write_all(&image).and_then(|_| {
        temporary_file.flush()?;
        temporary_file.sync_all()
    }) {
        drop(temporary_file);
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    drop(temporary_file);

    // A same-directory hard link publishes the complete staged image while
    // atomically refusing to replace a destination created by another process.
    match fs::hard_link(&temporary, path) {
        Ok(()) => {
            if let Err(error) = fs::remove_file(&temporary) {
                tracing::warn!(
                    path = %temporary.display(),
                    error = %error,
                    "could not remove linked config.sys staging file"
                );
            }
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error.into())
        }
    }
}

fn new_image(settings: &GameSettings) -> Vec<u8> {
    let mut bytes = vec![0_u8; CONFIG_SYS_SIZE];
    write_word(&mut bytes, 0, CONFIG_SYS_STAMP);
    for &(word, value) in NEW_IMAGE_UNMAPPED_WORDS {
        write_word(&mut bytes, word, value);
    }
    for (index, unit) in NEW_IMAGE_FONT.encode_utf16().enumerate() {
        let offset = NEW_IMAGE_FONT_OFFSET + index * 2;
        bytes[offset..offset + 2].copy_from_slice(&unit.to_le_bytes());
    }
    encode_mapped_words(&mut bytes, settings);
    bytes
}

fn validate_size_and_stamp(bytes: &[u8]) -> Result<(), RetailGameError> {
    if bytes.len() != CONFIG_SYS_SIZE {
        return Err(RetailGameError::InvalidSize {
            actual: bytes.len(),
            expected: CONFIG_SYS_SIZE,
        });
    }
    let stamp = read_word(bytes, 0);
    if stamp != CONFIG_SYS_STAMP {
        return Err(RetailGameError::InvalidStamp {
            actual: stamp,
            expected: CONFIG_SYS_STAMP,
        });
    }
    Ok(())
}

fn validate_resolution(width: u32, height: u32) -> Result<(), RetailGameError> {
    if SUPPORTED_RESOLUTIONS.contains(&(width, height)) {
        Ok(())
    } else {
        Err(RetailGameError::UnsupportedResolution { width, height })
    }
}

fn decode_display_mode(value: u32) -> Result<DisplayMode, RetailGameError> {
    match value {
        0 => Ok(DisplayMode::Windowed),
        1 => Ok(DisplayMode::FullScreen),
        value => Err(invalid_value(4, value)),
    }
}

fn decode_quality(value: u32, word: usize, max_visible: u32) -> Result<u8, RetailGameError> {
    match value {
        value if value < max_visible => Ok((value + 1) as u8),
        value => Err(invalid_value(word, value)),
    }
}

fn decode_multisampling(value: u32) -> Result<Multisampling, RetailGameError> {
    match value {
        0 => Ok(Multisampling::None),
        1 => Ok(Multisampling::X2),
        3 => Ok(Multisampling::X4),
        7 => Ok(Multisampling::X8),
        value => Err(invalid_value(9, value)),
    }
}

fn decode_shadow_detail(value: u32) -> Result<ShadowDetail, RetailGameError> {
    match value {
        0 => Ok(ShadowDetail::Lowest),
        1 => Ok(ShadowDetail::Low),
        2 => Ok(ShadowDetail::Standard),
        3 => Ok(ShadowDetail::High),
        4 => Ok(ShadowDetail::Highest),
        value => Err(invalid_value(11, value)),
    }
}

fn decode_bool(value: u32, word: usize) -> Result<bool, RetailGameError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        value => Err(invalid_value(word, value)),
    }
}

fn decode_texture_quality(value: u32) -> Result<TextureQuality, RetailGameError> {
    match value {
        0 => Ok(TextureQuality::High),
        1 => Ok(TextureQuality::Standard),
        2 => Ok(TextureQuality::Low),
        value => Err(invalid_value(15, value)),
    }
}

fn decode_texture_filtering(value: u32) -> Result<TextureFiltering, RetailGameError> {
    match value {
        0 => Ok(TextureFiltering::Highest),
        1 => Ok(TextureFiltering::High),
        2 => Ok(TextureFiltering::Standard),
        3 => Ok(TextureFiltering::Low),
        value => Err(invalid_value(16, value)),
    }
}

fn invalid_value(word: usize, value: u32) -> RetailGameError {
    RetailGameError::InvalidValue { word, value }
}

fn validate_settings_with_native_resolution(
    settings: &GameSettings,
    native_resolution: Option<(u32, u32)>,
) -> Result<(), RetailGameError> {
    if !SUPPORTED_RESOLUTIONS.contains(&(settings.width, settings.height))
        && native_resolution != Some((settings.width, settings.height))
    {
        return Err(RetailGameError::UnsupportedResolution {
            width: settings.width,
            height: settings.height,
        });
    }
    if !(1..=10).contains(&(settings.graphics.general_quality as u32)) {
        return Err(invalid_value(
            7,
            settings.graphics.general_quality.saturating_sub(1) as u32,
        ));
    }
    if !(1..=5).contains(&(settings.graphics.background_quality as u32)) {
        return Err(invalid_value(
            8,
            settings.graphics.background_quality.saturating_sub(1) as u32,
        ));
    }
    Ok(())
}

fn encode_mapped_words(bytes: &mut [u8], settings: &GameSettings) {
    write_word(
        bytes,
        4,
        match settings.display_mode {
            DisplayMode::Windowed | DisplayMode::Borderless => 0,
            DisplayMode::FullScreen => 1,
        },
    );
    write_word(bytes, 5, settings.width);
    write_word(bytes, 6, settings.height);
    write_word(bytes, 7, settings.graphics.general_quality as u32 - 1);
    write_word(bytes, 8, settings.graphics.background_quality as u32 - 1);
    write_word(
        bytes,
        9,
        match settings.graphics.multisampling {
            Multisampling::None => 0,
            Multisampling::X2 => 1,
            Multisampling::X4 => 3,
            Multisampling::X8 => 7,
        },
    );
    write_word(
        bytes,
        11,
        match settings.graphics.shadow_detail {
            ShadowDetail::Lowest => 0,
            ShadowDetail::Low => 1,
            ShadowDetail::Standard => 2,
            ShadowDetail::High => 3,
            ShadowDetail::Highest => 4,
        },
    );
    write_word(bytes, 12, settings.graphics.ambient_occlusion as u32);
    write_word(bytes, 13, settings.graphics.depth_of_field as u32);
    write_word(bytes, 14, (!settings.graphics.cutscene_effects) as u32);
    write_word(
        bytes,
        15,
        match settings.graphics.texture_quality {
            TextureQuality::High => 0,
            TextureQuality::Standard => 1,
            TextureQuality::Low => 2,
        },
    );
    write_word(
        bytes,
        16,
        match settings.graphics.texture_filtering {
            TextureFiltering::Highest => 0,
            TextureFiltering::High => 1,
            TextureFiltering::Standard => 2,
            TextureFiltering::Low => 3,
        },
    );
    write_word(bytes, 18, (!settings.audio.enabled) as u32);
    write_word(bytes, 19, settings.audio.play_in_background as u32);
    write_word(bytes, 20, settings.graphics.hardware_mouse as u32);
}

fn read_word(bytes: &[u8], index: usize) -> u32 {
    let offset = index * 4;
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("validated word offset"),
    )
}

fn write_word(bytes: &mut [u8], index: usize, value: u32) {
    let offset = index * 4;
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn temporary_path(path: &Path) -> PathBuf {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos() as u64);
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(format!(
        ".tmp-{}-{}",
        std::process::id(),
        timestamp ^ counter
    ));
    PathBuf::from(temporary)
}

fn create_temporary(temporary: &Path, original: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary)?;
    if let Ok(metadata) = fs::metadata(original) {
        file.set_permissions(metadata.permissions())?;
    }
    Ok(file)
}

fn stage_backup(backup: &Path, original: &[u8]) -> Result<(PathBuf, bool), RetailGameError> {
    let staged = temporary_path(backup);
    let previous = temporary_path(backup);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)?;
    if let Err(error) = file
        .write_all(original)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(&staged);
        return Err(error.into());
    }
    drop(file);

    let had_previous = backup.exists();
    if had_previous && let Err(error) = fs::rename(backup, &previous) {
        let _ = fs::remove_file(&staged);
        return Err(error.into());
    }
    if let Err(error) = fs::rename(&staged, backup) {
        let rollback = if had_previous {
            fs::rename(&previous, backup)
        } else {
            Ok(())
        };
        let _ = fs::remove_file(&staged);
        return match rollback {
            Ok(()) => Err(error.into()),
            Err(rollback) => Err(combined_rollback_error(error, rollback)),
        };
    }
    Ok((previous, had_previous))
}

fn rollback_backup(backup: &Path, previous: &Path, had_previous: bool) -> Result<(), io::Error> {
    match fs::remove_file(backup) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if had_previous {
        fs::rename(previous, backup)?;
    }
    Ok(())
}

fn combined_rollback_error(
    operation: impl std::fmt::Display,
    rollback: impl std::fmt::Display,
) -> RetailGameError {
    RetailGameError::Io(io::Error::other(format!(
        "atomic replacement failed ({operation}); restoring the previous backup also failed ({rollback})"
    )))
}

#[cfg(not(target_os = "windows"))]
fn replace_with_backup(
    path: &Path,
    temporary: &Path,
    backup: &Path,
    original: &[u8],
) -> Result<(), RetailGameError> {
    let (previous_backup, had_backup) = stage_backup(backup, original)?;
    if let Err(error) = fs::rename(temporary, path) {
        if let Err(rollback) = rollback_backup(backup, &previous_backup, had_backup) {
            return Err(combined_rollback_error(error, rollback));
        }
        return Err(error.into());
    }
    if had_backup {
        let _ = fs::remove_file(previous_backup);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn replace_with_backup(
    path: &Path,
    temporary: &Path,
    backup: &Path,
    original: &[u8],
) -> Result<(), RetailGameError> {
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let (previous_backup, had_backup) = stage_backup(backup, original)?;
    let path_wide = wide_path(path);
    let temporary_wide = wide_path(temporary);
    let result = unsafe {
        MoveFileExW(
            windows::core::PCWSTR(temporary_wide.as_ptr()),
            windows::core::PCWSTR(path_wide.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if let Err(error) = result {
        if let Err(rollback) = rollback_backup(backup, &previous_backup, had_backup) {
            return Err(combined_rollback_error(error, rollback));
        }
        return Err(error.into());
    }
    if had_backup {
        let _ = fs::remove_file(previous_backup);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> Vec<u8> {
        let mut bytes = vec![0xA5; CONFIG_SYS_SIZE];
        write_word(&mut bytes, 0, CONFIG_SYS_STAMP);
        write_word(&mut bytes, 4, 1);
        write_word(&mut bytes, 5, 1920);
        write_word(&mut bytes, 6, 1080);
        write_word(&mut bytes, 7, 4);
        write_word(&mut bytes, 8, 3);
        write_word(&mut bytes, 9, 7);
        write_word(&mut bytes, 11, 4);
        write_word(&mut bytes, 12, 1);
        write_word(&mut bytes, 13, 0);
        write_word(&mut bytes, 14, 0);
        write_word(&mut bytes, 15, 0);
        write_word(&mut bytes, 16, 3);
        write_word(&mut bytes, 18, 0);
        write_word(&mut bytes, 19, 1);
        write_word(&mut bytes, 20, 0);
        bytes
    }

    fn decoded() -> GameSettings {
        decode(&image()).unwrap()
    }

    #[test]
    fn decodes_mapped_words_and_inversions() {
        let settings = decoded();
        assert_eq!(settings.display_mode, DisplayMode::FullScreen);
        assert_eq!((settings.width, settings.height), (1920, 1080));
        assert_eq!(settings.graphics.general_quality, 5);
        assert_eq!(settings.graphics.background_quality, 4);
        assert_eq!(settings.graphics.multisampling, Multisampling::X8);
        assert_eq!(settings.graphics.shadow_detail, ShadowDetail::Highest);
        assert!(settings.graphics.ambient_occlusion);
        assert!(!settings.graphics.depth_of_field);
        assert!(settings.graphics.cutscene_effects);
        assert_eq!(settings.graphics.texture_quality, TextureQuality::High);
        assert_eq!(settings.graphics.texture_filtering, TextureFiltering::Low);
        assert!(settings.audio.enabled);
        assert!(settings.audio.play_in_background);
        assert!(!settings.graphics.hardware_mouse);
    }

    #[test]
    fn retail_windowed_value_decodes_as_windowed() {
        let mut bytes = image();
        write_word(&mut bytes, 4, 0);
        assert_eq!(decode(&bytes).unwrap().display_mode, DisplayMode::Windowed);
    }

    #[test]
    fn borderless_encodes_as_the_retail_windowed_value() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.sys");
        let original = image();
        fs::write(&path, &original).unwrap();

        let mut settings = decoded();
        settings.display_mode = DisplayMode::Borderless;
        let mut config = launcher_ini::LauncherConfig::defaults();
        config.preferences.game = settings;
        prepare_at_paths_with_native_resolution(
            &mut config,
            &directory.path().join("bahamut.ini"),
            &path,
            None,
        )
        .unwrap();

        assert_eq!(read_word(&fs::read(&path).unwrap(), 4), 0);
    }

    #[test]
    fn rejects_size_stamp_resolution_and_mapped_values() {
        let mut bytes = image();
        bytes.pop();
        assert!(matches!(
            decode(&bytes),
            Err(RetailGameError::InvalidSize { .. })
        ));

        let mut bytes = image();
        write_word(&mut bytes, 0, 0);
        assert!(matches!(
            decode(&bytes),
            Err(RetailGameError::InvalidStamp { .. })
        ));

        let mut bytes = image();
        write_word(&mut bytes, 5, 1);
        assert!(matches!(
            decode(&bytes),
            Err(RetailGameError::UnsupportedResolution { .. })
        ));

        for (word, value) in [
            (4, 2),
            (7, 10),
            (9, 2),
            (11, 5),
            (12, 2),
            (13, 2),
            (14, 2),
            (15, 3),
            (16, 4),
            (18, 2),
            (19, 2),
            (20, 2),
        ] {
            let mut bytes = image();
            write_word(&mut bytes, word, value);
            assert!(
                matches!(decode(&bytes), Err(RetailGameError::InvalidValue { word: actual, .. }) if actual == word)
            );
        }
    }

    #[test]
    fn apply_preserves_unmapped_bytes_and_writes_backup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.sys");
        let original = image();
        fs::write(&path, &original).unwrap();

        let mut settings = decoded();
        settings.display_mode = DisplayMode::Windowed;
        settings.graphics.hardware_mouse = true;
        let mut config = launcher_ini::LauncherConfig::defaults();
        config.preferences.game = settings;
        prepare_at_paths_with_native_resolution(
            &mut config,
            &directory.path().join("bahamut.ini"),
            &path,
            None,
        )
        .unwrap();

        let updated = fs::read(&path).unwrap();
        let backup = fs::read(backup_path(&path)).unwrap();
        assert_eq!(backup, original);
        for index in 0..CONFIG_SYS_SIZE / 4 {
            if [4, 20].contains(&index) {
                continue;
            }
            assert_eq!(
                &updated[index * 4..index * 4 + 4],
                &original[index * 4..index * 4 + 4]
            );
        }
        assert_eq!(read_word(&updated, 4), 0);
        assert_eq!(read_word(&updated, 20), 1);
    }

    #[test]
    fn apply_encodes_every_mapped_setting() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.sys");
        let original = image();
        fs::write(&path, &original).unwrap();

        let mut settings = decoded();
        settings.display_mode = DisplayMode::Windowed;
        settings.width = 1280;
        settings.height = 720;
        settings.graphics.multisampling = Multisampling::X4;
        settings.graphics.general_quality = 10;
        settings.graphics.background_quality = 5;
        settings.graphics.shadow_detail = ShadowDetail::Low;
        settings.graphics.ambient_occlusion = false;
        settings.graphics.depth_of_field = true;
        settings.graphics.cutscene_effects = false;
        settings.graphics.hardware_mouse = true;
        settings.graphics.texture_quality = TextureQuality::Standard;
        settings.graphics.texture_filtering = TextureFiltering::High;
        settings.audio.enabled = false;
        settings.audio.play_in_background = false;

        let mut config = launcher_ini::LauncherConfig::defaults();
        config.preferences.game = settings.clone();
        prepare_at_paths_with_native_resolution(
            &mut config,
            &directory.path().join("bahamut.ini"),
            &path,
            None,
        )
        .unwrap();
        assert_eq!(decode(&fs::read(&path).unwrap()).unwrap(), settings);
        assert_eq!(fs::read(backup_path(&path)).unwrap(), original);
    }

    #[test]
    fn no_op_does_not_create_or_change_backup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.sys");
        let original = image();
        fs::write(&path, &original).unwrap();
        let backup = backup_path(&path);
        fs::write(&backup, b"existing backup").unwrap();
        let backup_before = fs::read(&backup).unwrap();
        let mut config = launcher_ini::LauncherConfig::defaults();
        config.preferences.game = decoded();
        prepare_at_paths_with_native_resolution(
            &mut config,
            &directory.path().join("bahamut.ini"),
            &path,
            None,
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read(&backup).unwrap(), backup_before);
    }

    #[test]
    fn invalid_input_leaves_config_and_backup_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.sys");
        let original = image();
        fs::write(&path, &original).unwrap();
        let backup = backup_path(&path);
        fs::write(&backup, b"existing backup").unwrap();
        let before = fs::read(&backup).unwrap();

        let mut bad = decoded();
        bad.width = 1;
        let mut config = launcher_ini::LauncherConfig::defaults();
        config.preferences.game = bad;
        assert!(
            prepare_at_paths_with_native_resolution(
                &mut config,
                &directory.path().join("bahamut.ini"),
                &path,
                None,
            )
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read(&backup).unwrap(), before);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn staged_write_failure_leaves_config_and_backup_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.sys");
        let original = image();
        fs::write(&path, &original).unwrap();
        let backup = backup_path(&path);
        fs::write(&backup, b"existing backup").unwrap();
        let before = fs::read(&backup).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();

        let mut settings = decoded();
        settings.display_mode = DisplayMode::Windowed;
        let mut config = launcher_ini::LauncherConfig::defaults();
        config.preferences.game = settings;
        assert!(
            prepare_at_paths_with_native_resolution(
                &mut config,
                &directory.path().join("bahamut.ini"),
                &path,
                None,
            )
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read(&backup).unwrap(), before);
    }

    #[test]
    fn replacement_failure_restores_the_previous_backup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("destination-directory");
        fs::create_dir(&path).unwrap();
        let temporary = directory.path().join("staged-config.sys");
        fs::write(&temporary, b"replacement").unwrap();
        let backup = directory.path().join("config.sys.bak");
        fs::write(&backup, b"previous backup").unwrap();

        assert!(replace_with_backup(&path, &temporary, &backup, b"original").is_err());
        assert!(path.is_dir());
        assert_eq!(fs::read(&backup).unwrap(), b"previous backup");
    }

    #[test]
    fn first_prepare_imports_without_rewriting_retail_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let launcher_path = directory.path().join("bahamut.ini");
        let retail_path = directory.path().join("config.sys");
        let original = image();
        fs::write(&retail_path, &original).unwrap();
        let mut config = launcher_ini::LauncherConfig::defaults();

        assert_eq!(
            prepare_at_paths_with_native_resolution(
                &mut config,
                &launcher_path,
                &retail_path,
                None,
            )
            .unwrap(),
            PrepareOutcome::Imported
        );
        assert!(config.preferences.game.initialized);
        assert_eq!(fs::read(&retail_path).unwrap(), original);
        assert!(!backup_path(&retail_path).exists());
        assert_eq!(
            launcher_ini::LauncherConfig::from_ini_str(
                &fs::read_to_string(&launcher_path).unwrap(),
            )
            .unwrap()
            .preferences
            .game,
            config.preferences.game
        );
    }

    #[test]
    fn failed_first_import_save_restores_memory_and_retail() {
        let directory = tempfile::tempdir().unwrap();
        let retail_path = directory.path().join("config.sys");
        let original = image();
        fs::write(&retail_path, &original).unwrap();
        let mut config = launcher_ini::LauncherConfig::defaults();
        let before = config.preferences.game.clone();

        assert!(
            prepare_at_paths_with_native_resolution(
                &mut config,
                directory.path(),
                &retail_path,
                None,
            )
            .is_err()
        );
        assert_eq!(config.preferences.game, before);
        assert_eq!(fs::read(&retail_path).unwrap(), original);
        assert!(!backup_path(&retail_path).exists());
    }

    #[test]
    fn missing_retail_file_is_created_from_pending_ini_values() {
        let directory = tempfile::tempdir().unwrap();
        let launcher_path = directory.path().join("bahamut.ini");
        let retail_path = directory.path().join("config.sys");
        let mut config = launcher_ini::LauncherConfig::defaults();
        config.preferences.game.width = 1920;
        config.preferences.game.height = 1080;
        config.preferences.game.graphics.multisampling = Multisampling::X4;
        assert!(!config.preferences.game.initialized);

        assert_eq!(
            prepare_at_paths_with_native_resolution(
                &mut config,
                &launcher_path,
                &retail_path,
                None,
            )
            .unwrap(),
            PrepareOutcome::Created
        );
        assert!(config.preferences.game.initialized);
        let created = fs::read(&retail_path).unwrap();
        assert_eq!(created.len(), CONFIG_SYS_SIZE);
        assert_eq!(decode(&created).unwrap(), config.preferences.game);
        assert!(!backup_path(&retail_path).exists());
        assert_eq!(
            launcher_ini::LauncherConfig::from_ini_str(
                &fs::read_to_string(&launcher_path).unwrap()
            )
            .unwrap()
            .preferences
            .game,
            config.preferences.game
        );
    }

    #[test]
    fn missing_retail_file_is_created_for_initialized_settings() {
        let directory = tempfile::tempdir().unwrap();
        let launcher_path = directory.path().join("bahamut.ini");
        let retail_path = directory
            .path()
            .join("My Games/FINAL FANTASY XIV/config.sys");
        let mut config = launcher_ini::LauncherConfig::defaults();
        config.preferences.game = decoded();

        assert_eq!(
            prepare_at_paths_with_native_resolution(
                &mut config,
                &launcher_path,
                &retail_path,
                None,
            )
            .unwrap(),
            PrepareOutcome::Created
        );
        assert_eq!(
            decode(&fs::read(&retail_path).unwrap()).unwrap(),
            config.preferences.game
        );
        assert!(!backup_path(&retail_path).exists());
        assert!(!launcher_path.exists());
    }

    #[test]
    fn native_resolution_override_changes_only_launch_time_retail_settings() {
        let directory = tempfile::tempdir().unwrap();
        let launcher_path = directory.path().join("bahamut.ini");
        let retail_path = directory.path().join("config.sys");
        let original = image();
        fs::write(&retail_path, &original).unwrap();
        let mut config = launcher_ini::LauncherConfig::defaults();
        config.preferences.game = decoded();
        let fallback = (
            config.preferences.game.width,
            config.preferences.game.height,
        );

        assert_eq!(
            prepare_at_paths_with_native_resolution(
                &mut config,
                &launcher_path,
                &retail_path,
                Some((5120, 1440)),
            )
            .unwrap(),
            PrepareOutcome::Applied
        );
        assert_eq!(
            (
                config.preferences.game.width,
                config.preferences.game.height
            ),
            fallback
        );
        let effective = decode_image(&fs::read(&retail_path).unwrap(), true).unwrap();
        assert_eq!((effective.width, effective.height), (5120, 1440));

        prepare_at_paths_with_native_resolution(&mut config, &launcher_path, &retail_path, None)
            .unwrap();
        let restored = decode(&fs::read(&retail_path).unwrap()).unwrap();
        assert_eq!((restored.width, restored.height), fallback);

        let directory = tempfile::tempdir().unwrap();
        let launcher_path = directory.path().join("bahamut.ini");
        let retail_path = directory.path().join("config.sys");
        let mut config = launcher_ini::LauncherConfig::defaults();
        assert_eq!(
            prepare_at_paths_with_native_resolution(
                &mut config,
                &launcher_path,
                &retail_path,
                Some((1920, 1080)),
            )
            .unwrap(),
            PrepareOutcome::Created
        );
        assert!(config.preferences.game.initialized);
        assert_eq!(
            (
                config.preferences.game.width,
                config.preferences.game.height
            ),
            (1280, 720)
        );
        let created = decode(&fs::read(&retail_path).unwrap()).unwrap();
        assert_eq!((created.width, created.height), (1920, 1080));
    }

    #[test]
    fn created_image_carries_the_documented_unmapped_defaults() {
        let image = new_image(&GameSettings::default());
        assert_eq!(image.len(), CONFIG_SYS_SIZE);
        assert_eq!(read_word(&image, 0), CONFIG_SYS_STAMP);
        for &(word, value) in NEW_IMAGE_UNMAPPED_WORDS {
            assert_eq!(read_word(&image, word), value, "word {word}");
        }
        let font: Vec<u8> = NEW_IMAGE_FONT
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(&image[96..96 + font.len()], &font[..]);
        assert!(image[96 + font.len()..160].iter().all(|&byte| byte == 0));
        assert!(image[160..].iter().all(|&byte| byte == 0));
        assert_eq!(
            decode(&image).unwrap(),
            GameSettings {
                initialized: true,
                ..GameSettings::default()
            }
        );
    }

    #[test]
    fn create_never_replaces_an_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let retail_path = directory.path().join("config.sys");
        fs::write(&retail_path, b"not a config").unwrap();

        let error = create_at_path(&retail_path, &GameSettings::default()).unwrap_err();
        assert!(matches!(
            error,
            RetailGameError::Io(ref io) if io.kind() == io::ErrorKind::AlreadyExists
        ));
        assert_eq!(fs::read(&retail_path).unwrap(), b"not a config");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn unreadable_retail_path_is_not_treated_as_missing() {
        let directory = tempfile::tempdir().unwrap();
        let launcher_path = directory.path().join("bahamut.ini");
        let retail_path = directory.path().join("config.sys");
        fs::create_dir(&retail_path).unwrap();

        for initialized in [false, true] {
            let mut config = launcher_ini::LauncherConfig::defaults();
            config.preferences.game.initialized = initialized;
            assert!(
                prepare_at_paths_with_native_resolution(
                    &mut config,
                    &launcher_path,
                    &retail_path,
                    None,
                )
                .is_err()
            );
            assert_eq!(config.preferences.game.initialized, initialized);
        }
        assert!(retail_path.is_dir());
        assert!(!launcher_path.exists());
    }

    #[test]
    fn failed_save_after_creation_removes_the_created_file() {
        let directory = tempfile::tempdir().unwrap();
        let retail_path = directory.path().join("config.sys");
        let mut config = launcher_ini::LauncherConfig::defaults();
        let before = config.preferences.game.clone();

        assert!(
            prepare_at_paths_with_native_resolution(
                &mut config,
                directory.path(),
                &retail_path,
                None,
            )
            .is_err()
        );
        assert_eq!(config.preferences.game, before);
        assert!(!retail_path.exists());
        assert!(!backup_path(&retail_path).exists());
    }

    #[test]
    fn later_prepare_applies_ini_values_and_preserves_unmapped_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let launcher_path = directory.path().join("bahamut.ini");
        let retail_path = directory.path().join("config.sys");
        let original = image();
        fs::write(&retail_path, &original).unwrap();
        let mut config = launcher_ini::LauncherConfig::defaults();
        config.preferences.game = decoded();
        config.preferences.game.display_mode = DisplayMode::Windowed;

        assert_eq!(
            prepare_at_paths_with_native_resolution(
                &mut config,
                &launcher_path,
                &retail_path,
                None,
            )
            .unwrap(),
            PrepareOutcome::Applied
        );
        let updated = fs::read(&retail_path).unwrap();
        assert_eq!(read_word(&updated, 4), 0);
        for index in 0..CONFIG_SYS_SIZE / 4 {
            if index == 4 {
                continue;
            }
            assert_eq!(
                &updated[index * 4..index * 4 + 4],
                &original[index * 4..index * 4 + 4]
            );
        }
    }
}
