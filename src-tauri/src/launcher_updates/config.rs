//! Launcher update configuration file and endpoint validation.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bahamut_launcher::atomic_fs::is_link_or_reparse;
use bahamut_launcher::release;
use reqwest::Url;
use serde::Deserialize;

use super::fs_io::{ensure_existing_directory, read_regular_file, resolve_local_path};
use super::receipts::Receipt;
use super::verify::{validate_version, verify_launcher_release};

const CONFIG_FILE: &str = "launcher-updates.json";

const CONFIG_SCHEMA: u32 = 1;

const MAX_CONFIG_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LauncherUpdateConfig {
    schema_version: u32,
    pub(super) metadata_url: String,
    pub(super) signature_url: String,
    pub(super) artifact_root_url: String,
    trusted_public_key_path: String,
    bootstrap: Option<BootstrapConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BootstrapConfig {
    version: String,
    metadata_path: String,
    signature_path: String,
}

impl LauncherUpdateConfig {
    pub(super) fn load(root: &Path) -> Result<Self, String> {
        ensure_existing_directory(root, "launcher directory")?;
        let config_dir = root.join("config");
        match fs::symlink_metadata(&config_dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err("Launcher updates are blocked: provision config/launcher-updates.json with explicit stable HTTPS endpoints, a trustedPublicKeyPath, and a signed bootstrap release (metadata, signature, and version).".into());
            }
            Err(error) => {
                return Err(format!(
                    "Could not inspect launcher config directory: {error}"
                ));
            }
            Ok(details) if details.is_dir() && !is_link_or_reparse(&details) => {}
            Ok(_) => return Err("Launcher config directory is not a plain directory.".into()),
        }
        let path = root.join("config").join(CONFIG_FILE);
        let bytes = read_regular_file(&path, MAX_CONFIG_BYTES, "launcher update configuration")
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    "Launcher updates are blocked: provision config/launcher-updates.json with explicit stable HTTPS endpoints, a trustedPublicKeyPath, and a signed bootstrap release (metadata, signature, and version).".to_owned()
                } else {
                    format!("Could not read config/{CONFIG_FILE}: {error}")
                }
            })?;
        let config: Self = serde_json::from_slice(&bytes)
            .map_err(|_| format!("config/{CONFIG_FILE} is not valid launcher update JSON."))?;
        config.validate(root)?;
        Ok(config)
    }

    fn validate(&self, root: &Path) -> Result<(), String> {
        if self.schema_version != CONFIG_SCHEMA {
            return Err(format!("Unsupported config/{CONFIG_FILE} schema version."));
        }
        validate_endpoint(&self.metadata_url, "metadata URL")?;
        validate_endpoint(&self.signature_url, "signature URL")?;
        validate_endpoint(&self.artifact_root_url, "artifact root URL")?;
        let key_path = resolve_local_path(root, &self.trusted_public_key_path)?;
        let key = read_regular_file(
            &key_path,
            release::ED25519_PUBLIC_KEY_BYTES,
            "trusted launcher public key",
        )
        .map_err(|error| format!("Could not read the configured trusted public key: {error}"))?;
        if key.len() != release::ED25519_PUBLIC_KEY_BYTES {
            return Err("The configured launcher public key must be exactly 32 bytes.".into());
        }
        let bootstrap = self.bootstrap.as_ref().ok_or_else(|| {
            "Launcher updates are blocked: config/launcher-updates.json must explicitly provision a signed bootstrap metadata file, signature file, and version.".to_owned()
        })?;
        validate_version(&bootstrap.version)?;
        resolve_local_path(root, &bootstrap.metadata_path)?;
        resolve_local_path(root, &bootstrap.signature_path)?;
        Ok(())
    }

    pub(super) fn key_path(&self, root: &Path) -> Result<PathBuf, String> {
        resolve_local_path(root, &self.trusted_public_key_path)
    }

    pub(super) fn public_key(&self, root: &Path) -> Result<Vec<u8>, String> {
        release::read_public_key_file(&self.key_path(root)?)
            .map_err(|error| format!("Could not read the configured trusted public key: {error}"))
    }

    pub(super) fn bootstrap_receipt(&self, root: &Path, key: &[u8]) -> Result<Receipt, String> {
        let bootstrap = self.bootstrap.as_ref().ok_or_else(|| {
            "Launcher updates are blocked because no signed bootstrap release is configured."
                .to_owned()
        })?;
        let metadata_path = resolve_local_path(root, &bootstrap.metadata_path)?;
        let signature_path = resolve_local_path(root, &bootstrap.signature_path)?;
        let metadata = release::read_metadata_file(&metadata_path).map_err(|error| {
            format!("Could not read the configured bootstrap metadata: {error}")
        })?;
        let signature = release::read_signature_file(&signature_path).map_err(|error| {
            format!("Could not read the configured bootstrap signature: {error}")
        })?;
        let verified = verify_launcher_release(&metadata, &signature, key)?;
        if verified.metadata.version != bootstrap.version {
            return Err(
                "The configured bootstrap version does not match its signed metadata.".into(),
            );
        }
        Ok(Receipt {
            verified,
            metadata,
            signature,
        })
    }
}

fn validate_endpoint(value: &str, label: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| format!("Configured launcher {label} is invalid."))?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(format!(
            "Configured launcher {label} must not contain credentials or a fragment."
        ));
    }
    match url.scheme() {
        "https" if url.host_str().is_some() => Ok(url),
        "http" if is_loopback_url(&url) => Ok(url),
        _ => Err(format!("Configured launcher {label} must use HTTPS.")),
    }
}

fn is_loopback_url(url: &Url) -> bool {
    match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback()),
        None => false,
    }
}
