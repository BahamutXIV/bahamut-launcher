//! Bounded signed-release fetches and cached artifact lookup.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bahamut_launcher::atomic_fs::is_link_or_reparse;
use bahamut_launcher::content::http::{self, CheckpointAction, ObjectSpec};
use bahamut_launcher::release::{self, ReleaseMetadata};
use reqwest::blocking::Client;
use reqwest::redirect::Policy;

use super::ARTIFACT_CACHE;
use super::config::LauncherUpdateConfig;
use super::receipts::Receipt;

pub(super) const MAX_METADATA_BYTES: usize = release::MAX_METADATA_BYTES;

const MAX_SIGNATURE_BYTES: usize = release::ED25519_SIGNATURE_BYTES;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

pub(super) fn download_launcher_artifact(
    target: &Path,
    config: &LauncherUpdateConfig,
    offer: &Receipt,
) -> Result<PathBuf, String> {
    let artifact = http::download_object(
        &config.artifact_root_url,
        &ObjectSpec {
            object_key: offer.verified.metadata.artifact.object_key.clone(),
            length: offer.verified.metadata.artifact.length,
            sha256: offer.verified.metadata.artifact.sha256.clone(),
        },
        &target.join(ARTIFACT_CACHE),
        |_| CheckpointAction::Continue,
        |_, _| {},
    )
    .map_err(|error| format!("Could not download the signed launcher artifact: {error}"))?;
    release::verify_artifact_file(&offer.verified.metadata, &artifact)
        .map_err(|error| format!("Launcher artifact verification failed: {error}"))?;
    Ok(artifact)
}

pub(super) fn cached_launcher_artifact(root: &Path, metadata: &ReleaseMetadata) -> Option<PathBuf> {
    let path = root.join(ARTIFACT_CACHE).join(format!(
        "{}-{}.object",
        metadata.artifact.sha256, metadata.artifact.length
    ));
    let details = fs::symlink_metadata(&path).ok()?;
    if !details.is_file() || is_link_or_reparse(&details) {
        return None;
    }
    release::verify_artifact_file(metadata, &path).ok()?;
    Some(path)
}

pub(super) fn fetch_signed_release(
    config: &LauncherUpdateConfig,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let metadata = fetch_bounded(&config.metadata_url, MAX_METADATA_BYTES, "signed metadata")?;
    let signature = fetch_bounded(&config.signature_url, MAX_SIGNATURE_BYTES, "signature")?;
    if signature.len() != MAX_SIGNATURE_BYTES {
        return Err("Launcher release signature must be exactly 64 bytes.".into());
    }
    Ok((metadata, signature))
}

pub(super) fn fetch_bounded(url: &str, maximum: usize, label: &str) -> Result<Vec<u8>, String> {
    let client = Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(Policy::none())
        .no_proxy()
        .build()
        .map_err(|error| format!("Could not configure launcher update transport: {error}"))?;
    let mut response = client
        .get(url)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .map_err(|error| format!("Could not fetch launcher {label}: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Launcher {label} endpoint returned HTTP {}.",
            response.status().as_u16()
        ));
    }
    if response
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Err(format!(
            "Launcher {label} endpoint used an unsupported content encoding."
        ));
    }
    if response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/html"))
    {
        return Err(format!("Launcher {label} endpoint returned HTML."));
    }
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(format!("Launcher {label} exceeds its size limit."));
    }
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read launcher {label}: {error}"))?;
    if bytes.len() > maximum {
        return Err(format!("Launcher {label} exceeds its size limit."));
    }
    if looks_like_html(&bytes) {
        return Err(format!("Launcher {label} endpoint returned HTML."));
    }
    Ok(bytes)
}

fn looks_like_html(bytes: &[u8]) -> bool {
    let prefix = &bytes[..bytes.len().min(128)];
    let text = String::from_utf8_lossy(prefix)
        .trim_start()
        .to_ascii_lowercase();
    text.starts_with("<!doctype html") || text.starts_with("<html")
}
