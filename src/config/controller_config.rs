//! Controller settings kept in `controller.ini`.
//!
//! A separate file keeps launcher builds that reject unknown `bahamut.ini` keys
//! able to read their main configuration after a downgrade; older builds ignore
//! this file entirely.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

use crate::config::atomic_write::write_atomic;
use crate::config::dirs;

const CONTROLLER_SECTION: &str = "controller";
const PAD_SETUP_KEY: &str = "pad_setup";

/// Persisted controller preferences.
///
/// `pad_setup` lets a Linux launch write a matching retail `config.pad`
/// controller selection before Play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ControllerConfig {
    pub pad_setup: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ControllerConfigError {
    #[error("controller INI is malformed at line {line}: {message}")]
    Ini { line: usize, message: String },
    #[error("controller INI [controller] {key} is invalid: {message}")]
    Invalid { key: &'static str, message: String },
    #[error("could not read or write controller configuration: {0}")]
    Io(#[from] io::Error),
    #[error("could not resolve controller configuration: {0}")]
    Directory(#[from] dirs::ConfigDirError),
}

impl ControllerConfig {
    /// Parses `controller.ini` text. Newer launchers may add sections and keys,
    /// so anything this build does not know is ignored.
    pub fn from_ini_str(text: &str) -> Result<Self, ControllerConfigError> {
        let mut in_section = false;
        let mut in_controller = false;
        let mut seen_controller = false;
        let mut seen_keys: HashSet<&str> = HashSet::new();
        let mut pad_setup: Option<&str> = None;
        for (offset, raw) in text.lines().enumerate() {
            let line_number = offset + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                if !line.ends_with(']') || line.len() < 3 {
                    return Err(ControllerConfigError::Ini {
                        line: line_number,
                        message: "invalid section header".into(),
                    });
                }
                let name = line[1..line.len() - 1].trim();
                if name.is_empty() {
                    return Err(ControllerConfigError::Ini {
                        line: line_number,
                        message: "empty section name".into(),
                    });
                }
                in_section = true;
                in_controller = name == CONTROLLER_SECTION;
                if in_controller {
                    if seen_controller {
                        return Err(ControllerConfigError::Ini {
                            line: line_number,
                            message: "duplicate section [controller]".into(),
                        });
                    }
                    seen_controller = true;
                }
                continue;
            }
            if !in_section {
                return Err(ControllerConfigError::Ini {
                    line: line_number,
                    message: "key appears before any section".into(),
                });
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(ControllerConfigError::Ini {
                    line: line_number,
                    message: "expected key = value".into(),
                });
            };
            let key = key.trim();
            if key.is_empty() {
                return Err(ControllerConfigError::Ini {
                    line: line_number,
                    message: "empty key".into(),
                });
            }
            if !in_controller {
                continue;
            }
            if !seen_keys.insert(key) {
                return Err(ControllerConfigError::Ini {
                    line: line_number,
                    message: format!("duplicate key {key:?}"),
                });
            }
            if key == PAD_SETUP_KEY {
                pad_setup = Some(value.trim());
            }
        }
        let pad_setup = match pad_setup {
            None => false,
            Some(value) => value
                .parse::<bool>()
                .map_err(|_| ControllerConfigError::Invalid {
                    key: PAD_SETUP_KEY,
                    message: "expected true or false".into(),
                })?,
        };
        Ok(Self { pad_setup })
    }

    pub fn to_ini_string(&self) -> String {
        format!(
            "; BahamutXIV Launcher controller settings.\n\n\
             [{CONTROLLER_SECTION}]\n{PAD_SETUP_KEY} = {}\n",
            self.pad_setup
        )
    }

    pub fn load() -> Result<Self, ControllerConfigError> {
        Self::load_from(&dirs::controller_config_path()?)
    }

    /// Returns the defaults when the file is missing; never creates it.
    pub fn load_from(path: &Path) -> Result<Self, ControllerConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::from_ini_str(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(&self) -> Result<(), ControllerConfigError> {
        self.save_to(&dirs::controller_config_path()?)
    }

    /// Rewrites the whole file, so keys written by a newer launcher are dropped.
    pub fn save_to(&self, path: &Path) -> Result<(), ControllerConfigError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let body = self.to_ini_string();
        if Self::from_ini_str(&body)? != *self {
            return Err(ControllerConfigError::Invalid {
                key: PAD_SETUP_KEY,
                message: "serialized configuration did not round-trip".into(),
            });
        }
        write_atomic(path, body.as_bytes()).map_err(ControllerConfigError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::launcher_ini::LauncherConfig;

    fn parse(text: &str) -> Result<ControllerConfig, ControllerConfigError> {
        ControllerConfig::from_ini_str(text)
    }

    #[test]
    fn missing_file_loads_default_without_creating_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config").join("controller.ini");
        assert_eq!(
            ControllerConfig::load_from(&path).unwrap(),
            ControllerConfig { pad_setup: false }
        );
        assert!(!path.exists());
        assert!(!path.parent().unwrap().exists());
    }

    #[test]
    fn empty_text_and_missing_key_default_to_off() {
        assert!(!parse("").unwrap().pad_setup);
        assert!(!parse("[controller]\n").unwrap().pad_setup);
    }

    #[test]
    fn serialization_has_the_documented_shape() {
        assert_eq!(
            ControllerConfig { pad_setup: false }.to_ini_string(),
            "; BahamutXIV Launcher controller settings.\n\n[controller]\npad_setup = false\n"
        );
        assert_eq!(
            ControllerConfig { pad_setup: true }.to_ini_string(),
            "; BahamutXIV Launcher controller settings.\n\n[controller]\npad_setup = true\n"
        );
    }

    #[test]
    fn save_and_load_round_trip_both_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("controller.ini");
        for pad_setup in [true, false] {
            let config = ControllerConfig { pad_setup };
            config.save_to(&path).unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), config.to_ini_string());
            assert_eq!(ControllerConfig::load_from(&path).unwrap(), config);
        }
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("state")
            .join("config")
            .join("controller.ini");
        ControllerConfig { pad_setup: true }.save_to(&path).unwrap();
        assert!(ControllerConfig::load_from(&path).unwrap().pad_setup);
    }

    #[test]
    fn unknown_sections_and_keys_are_ignored() {
        let config = parse(
            "; comment\n# comment\n\n[future]\nmode = a\npad_setup = nonsense\n\n\
             [controller]\nnewer_key = 7\npad_setup = true\n[other]\npad_setup = false\n",
        )
        .unwrap();
        assert!(config.pad_setup);
    }

    #[test]
    fn save_drops_keys_written_by_a_newer_launcher() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("controller.ini");
        fs::write(&path, "[controller]\npad_setup = true\nnewer_key = 7\n").unwrap();
        let config = ControllerConfig::load_from(&path).unwrap();
        config.save_to(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), config.to_ini_string());
    }

    #[test]
    fn only_lowercase_true_and_false_are_accepted() {
        for value in ["True", "1", "yes", "", "on"] {
            let error = parse(&format!("[controller]\npad_setup = {value}\n")).unwrap_err();
            assert!(
                matches!(
                    error,
                    ControllerConfigError::Invalid {
                        key: "pad_setup",
                        ..
                    }
                ),
                "{value:?} gave {error:?}"
            );
        }
    }

    #[test]
    fn key_outside_a_section_is_rejected() {
        let error = parse("pad_setup = true\n[controller]\n").unwrap_err();
        assert!(matches!(error, ControllerConfigError::Ini { line: 1, .. }));
    }

    #[test]
    fn malformed_header_and_line_without_equals_are_rejected() {
        assert!(matches!(
            parse("[controller\npad_setup = true\n").unwrap_err(),
            ControllerConfigError::Ini { line: 1, .. }
        ));
        assert!(matches!(
            parse("[controller]\npad_setup\n").unwrap_err(),
            ControllerConfigError::Ini { line: 2, .. }
        ));
    }

    #[test]
    fn duplicate_key_or_section_is_rejected() {
        assert!(matches!(
            parse("[controller]\npad_setup = true\npad_setup = false\n").unwrap_err(),
            ControllerConfigError::Ini { line: 3, .. }
        ));
        assert!(matches!(
            parse("[controller]\n[controller]\n").unwrap_err(),
            ControllerConfigError::Ini { line: 2, .. }
        ));
    }

    #[test]
    fn launcher_ini_defaults_do_not_carry_controller_keys() {
        let text = LauncherConfig::defaults().to_ini_string();
        assert!(!text.contains("pad_setup"));
        assert!(!text.contains("[controller]"));
    }
}
