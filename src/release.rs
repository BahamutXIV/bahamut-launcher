//! Offline release authorization and per-scope accepted-version state.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::read::MultiGzDecoder;
use ring::signature::{ED25519, Ed25519KeyPair, UnparsedPublicKey};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use zip::ZipArchive;

use crate::patcher::content::{DeliveryManifest, validate_hash, validate_relative_path};

pub const MAX_METADATA_BYTES: usize = 1024 * 1024;
pub const MAX_DELIVERY_MANIFEST_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_ACCEPTED_STATE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_INVENTORY_FILES: usize = 20_000;
pub const MAX_ACCEPTED_RELEASES: usize = 4096;
pub const ED25519_SEED_BYTES: usize = 32;
pub const ED25519_PUBLIC_KEY_BYTES: usize = 32;
pub const ED25519_SIGNATURE_BYTES: usize = 64;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum ReleaseError {
    #[error("{0}")]
    Invalid(String),
    #[error("JSON encoding or decoding failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("release state I/O failed: {0}")]
    Io(#[from] io::Error),
}

type Result<T> = std::result::Result<T, ReleaseError>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Product {
    Game,
    Launcher,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Channel {
    Stable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum Target {
    #[serde(rename = "platform-independent")]
    PlatformIndependent,
    #[serde(rename = "windows-x86_64")]
    WindowsX86_64,
    #[serde(rename = "linux-x86_64")]
    LinuxX86_64,
    #[serde(rename = "macos-x86_64")]
    MacosX86_64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactFormat {
    Zip,
    TarGz,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOwnership {
    Managed,
    Seed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIdentity {
    pub object_key: String,
    pub format: ArtifactFormat,
    pub length: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryFile {
    pub path: String,
    pub length: u64,
    pub sha256: String,
    pub ownership: FileOwnership,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReleaseInventory {
    GameDelivery {
        manifest_sha256: String,
        target_version: String,
    },
    Launcher {
        files: Vec<InventoryFile>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseMetadata {
    pub schema_version: u32,
    pub product: Product,
    pub channel: Channel,
    pub target: Target,
    pub version: String,
    pub artifact: ArtifactIdentity,
    pub inventory: ReleaseInventory,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReleaseScope {
    pub product: Product,
    pub channel: Channel,
    pub target: Target,
}

impl ReleaseMetadata {
    pub fn scope(&self) -> ReleaseScope {
        ReleaseScope {
            product: self.product,
            channel: self.channel,
            target: self.target,
        }
    }
}

#[derive(Clone, Debug)]
pub struct VerifiedRelease {
    pub metadata: ReleaseMetadata,
    pub metadata_sha256: String,
    pub trusted_key_sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfferDecision {
    Accepted,
    ReuseIdentical,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstalledDecision {
    Recorded,
    AlreadyCurrent,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AcceptedState {
    schema_version: u32,
    scopes: Vec<AcceptedScope>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AcceptedScope {
    product: Product,
    channel: Channel,
    target: Target,
    trusted_key_sha256: String,
    highest_remote: ReleaseIdentity,
    trusted_releases: Vec<ReleaseIdentity>,
    installed: Option<ReleaseIdentity>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ReleaseIdentity {
    version: String,
    metadata_sha256: String,
}

impl ReleaseIdentity {
    fn from_verified(release: &VerifiedRelease) -> Self {
        Self {
            version: release.metadata.version.clone(),
            metadata_sha256: release.metadata_sha256.clone(),
        }
    }
}

pub fn read_metadata_file(path: &Path) -> Result<Vec<u8>> {
    read_bounded_file(path, MAX_METADATA_BYTES, "release metadata")
}

pub fn read_delivery_manifest_file(path: &Path) -> Result<Vec<u8>> {
    read_bounded_file(path, MAX_DELIVERY_MANIFEST_BYTES, "game delivery manifest")
}

pub fn read_signature_file(path: &Path) -> Result<Vec<u8>> {
    read_fixed_file(path, ED25519_SIGNATURE_BYTES, "signature")
}

pub fn read_public_key_file(path: &Path) -> Result<Vec<u8>> {
    read_fixed_file(path, ED25519_PUBLIC_KEY_BYTES, "trusted public key")
}

pub fn read_seed_file(path: &Path) -> Result<Vec<u8>> {
    read_fixed_file(path, ED25519_SEED_BYTES, "signing seed")
}

pub fn write_signature_file(path: &Path, signature: &[u8]) -> Result<()> {
    if signature.len() != ED25519_SIGNATURE_BYTES {
        return Err(invalid("Ed25519 signature must be exactly 64 raw bytes."));
    }
    publish_staged(path, false, |output| output.write_all(signature))?;
    Ok(())
}

pub fn sign_metadata(
    metadata_bytes: &[u8],
    seed: &[u8],
    delivery_manifest_bytes: Option<&[u8]>,
) -> Result<[u8; ED25519_SIGNATURE_BYTES]> {
    if seed.len() != ED25519_SEED_BYTES {
        return Err(invalid(
            "Ed25519 signing seed must be exactly 32 raw bytes.",
        ));
    }
    let _metadata = parse_and_validate_metadata(metadata_bytes, delivery_manifest_bytes)?;
    let key_pair = Ed25519KeyPair::from_seed_unchecked(seed)
        .map_err(|_| invalid("Ed25519 signing seed is invalid."))?;
    let signature = key_pair.sign(metadata_bytes);
    let mut output = [0_u8; ED25519_SIGNATURE_BYTES];
    output.copy_from_slice(signature.as_ref());
    Ok(output)
}

pub fn parse_and_validate_metadata(
    metadata_bytes: &[u8],
    delivery_manifest_bytes: Option<&[u8]>,
) -> Result<ReleaseMetadata> {
    let metadata = parse_metadata(metadata_bytes)?;
    validate_metadata(&metadata, delivery_manifest_bytes)?;
    Ok(metadata)
}

pub fn verify_metadata(
    metadata_bytes: &[u8],
    signature: &[u8],
    public_key: &[u8],
    expected_scope: ReleaseScope,
    delivery_manifest_bytes: Option<&[u8]>,
) -> Result<VerifiedRelease> {
    if metadata_bytes.len() > MAX_METADATA_BYTES {
        return Err(invalid("Release metadata exceeds the 1 MiB limit."));
    }
    if signature.len() != ED25519_SIGNATURE_BYTES {
        return Err(invalid("Ed25519 signature must be exactly 64 raw bytes."));
    }
    if public_key.len() != ED25519_PUBLIC_KEY_BYTES {
        return Err(invalid(
            "Trusted Ed25519 public key must be exactly 32 raw bytes.",
        ));
    }
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(metadata_bytes, signature)
        .map_err(|_| invalid("Release metadata signature is invalid."))?;

    let metadata = parse_and_validate_metadata(metadata_bytes, delivery_manifest_bytes)?;
    if metadata.scope() != expected_scope {
        return Err(invalid(
            "Signed release product, channel, or target does not match the requested scope.",
        ));
    }
    Ok(VerifiedRelease {
        metadata,
        metadata_sha256: sha256_hex(metadata_bytes),
        trusted_key_sha256: sha256_hex(public_key),
    })
}

fn parse_metadata(metadata_bytes: &[u8]) -> Result<ReleaseMetadata> {
    if metadata_bytes.is_empty() || metadata_bytes.len() > MAX_METADATA_BYTES {
        return Err(invalid(
            "Release metadata is empty or exceeds the 1 MiB limit.",
        ));
    }
    serde_json::from_slice(metadata_bytes).map_err(|_| invalid("Release metadata JSON is invalid."))
}

fn validate_metadata(
    metadata: &ReleaseMetadata,
    delivery_manifest_bytes: Option<&[u8]>,
) -> Result<()> {
    if metadata.schema_version != 1 {
        return Err(invalid("Unsupported release metadata schema version."));
    }
    let version = parse_stable_version(&metadata.version)?;
    if metadata.artifact.length == 0 {
        return Err(invalid(
            "Release artifact length must be greater than zero.",
        ));
    }
    validate_relative_path(&metadata.artifact.object_key)
        .map_err(|_| invalid("Release artifact key is unsafe."))?;
    validate_lower_sha256(&metadata.artifact.sha256, "artifact SHA-256")?;
    validate_product_target(metadata.product, metadata.target)?;
    validate_artifact_location(metadata, &version)?;

    match (&metadata.inventory, metadata.product) {
        (
            ReleaseInventory::GameDelivery {
                manifest_sha256,
                target_version,
            },
            Product::Game,
        ) => {
            validate_lower_sha256(manifest_sha256, "delivery manifest SHA-256")?;
            if target_version.is_empty()
                || target_version.len() > 64
                || !target_version.is_ascii()
                || target_version.bytes().any(|byte| byte.is_ascii_control())
            {
                return Err(invalid("Game target version is invalid."));
            }
            let manifest_bytes = delivery_manifest_bytes
                .ok_or_else(|| invalid("Game release requires its delivery manifest."))?;
            if manifest_bytes.len() > MAX_DELIVERY_MANIFEST_BYTES {
                return Err(invalid("Game delivery manifest exceeds the 64 MiB limit."));
            }
            if sha256_hex(manifest_bytes) != *manifest_sha256 {
                return Err(invalid(
                    "Game release delivery manifest identity does not match.",
                ));
            }
            validate_game_manifest(metadata, manifest_bytes, target_version)?;
        }
        (ReleaseInventory::Launcher { files }, Product::Launcher) => {
            if delivery_manifest_bytes.is_some() {
                return Err(invalid(
                    "Only game releases may supply a delivery manifest.",
                ));
            }
            validate_file_inventory(files, false)?;
            for file in files {
                validate_launcher_file(file, metadata.target)?;
            }
        }
        _ => {
            return Err(invalid(
                "Release inventory kind does not match its product.",
            ));
        }
    }
    Ok(())
}

fn validate_product_target(product: Product, target: Target) -> Result<()> {
    let valid = match product {
        Product::Game => target == Target::PlatformIndependent,
        Product::Launcher => matches!(
            target,
            Target::WindowsX86_64 | Target::LinuxX86_64 | Target::MacosX86_64
        ),
    };
    if valid {
        Ok(())
    } else {
        Err(invalid("Release target is not supported for this product."))
    }
}

fn validate_artifact_location(metadata: &ReleaseMetadata, version: &Version) -> Result<()> {
    let digest = metadata.artifact.sha256.as_str();
    let expected_format = match (metadata.product, metadata.target) {
        (Product::Game, Target::PlatformIndependent)
        | (Product::Launcher, Target::WindowsX86_64) => ArtifactFormat::Zip,
        (Product::Launcher, Target::LinuxX86_64 | Target::MacosX86_64) => ArtifactFormat::TarGz,
        _ => return Err(invalid("Release product and target are invalid.")),
    };
    if metadata.artifact.format != expected_format {
        return Err(invalid(
            "Release artifact format does not match its target.",
        ));
    }
    match metadata.product {
        Product::Game => Ok(()),
        Product::Launcher => {
            let extension = match expected_format {
                ArtifactFormat::Zip => "zip",
                ArtifactFormat::TarGz => "tar.gz",
            };
            let expected = format!(
                "launcher/{version}/{}/{digest}.{extension}",
                target_name(metadata.target)
            );
            if metadata.artifact.object_key == expected {
                Ok(())
            } else {
                Err(invalid("Launcher artifact key is not canonical."))
            }
        }
    }
}

fn validate_game_manifest(
    metadata: &ReleaseMetadata,
    manifest_bytes: &[u8],
    target_version: &str,
) -> Result<()> {
    let manifest: DeliveryManifest = serde_json::from_slice(manifest_bytes)
        .map_err(|_| invalid("Game delivery manifest JSON is invalid."))?;
    if !matches!(manifest.schema_version, 1 | 2)
        || (manifest.schema_version == 1 && !manifest.hosted_patches)
    {
        return Err(invalid("Unsupported game delivery manifest schema."));
    }
    let package = manifest
        .base
        .as_ref()
        .ok_or_else(|| invalid("Game delivery manifest has no base package."))?;
    package
        .validate()
        .map_err(|_| invalid("Game delivery manifest inventory is invalid."))?;
    if package.target_version != target_version {
        return Err(invalid(
            "Game release target version does not match its delivery manifest.",
        ));
    }
    if !package.archives.iter().any(|archive| {
        archive.object.object_key == metadata.artifact.object_key
            && archive.object.length == metadata.artifact.length
            && archive.object.sha256 == metadata.artifact.sha256
    }) {
        return Err(invalid(
            "Game release artifact is not an archive in its delivery manifest.",
        ));
    }
    Ok(())
}

fn validate_file_inventory(files: &[InventoryFile], allow_empty: bool) -> Result<()> {
    if (!allow_empty && files.is_empty()) || files.len() > MAX_INVENTORY_FILES {
        return Err(invalid("Release file inventory is empty or too large."));
    }
    let mut names = HashSet::with_capacity(files.len());
    let mut total_bytes = 0_u64;
    for file in files {
        validate_relative_path(&file.path)
            .map_err(|_| invalid("Release inventory contains an unsafe path."))?;
        validate_lower_sha256(&file.sha256, "inventory SHA-256")?;
        if !names.insert(file.path.to_ascii_lowercase()) {
            return Err(invalid(
                "Release inventory contains a case-insensitive path collision.",
            ));
        }
        total_bytes = total_bytes
            .checked_add(file.length)
            .ok_or_else(|| invalid("Release inventory length overflows."))?;
    }
    for name in &names {
        for (index, _) in name.match_indices('/') {
            if names.contains(&name[..index]) {
                return Err(invalid(
                    "Release inventory file is also an ancestor directory.",
                ));
            }
        }
    }
    let _ = total_bytes;
    Ok(())
}

fn validate_launcher_file(file: &InventoryFile, target: Target) -> Result<()> {
    let path = file.path.as_str();
    if path == "scripts/default.txt" {
        return if target == Target::WindowsX86_64 && file.ownership == FileOwnership::Seed {
            Ok(())
        } else {
            Err(invalid("scripts/default.txt is seed-only."))
        };
    }
    let folded_path = path.to_ascii_lowercase();
    if folded_path.starts_with("plugins/dats/bahamut-dats-overlay/") {
        if !path.starts_with("plugins/dats/bahamut-dats-overlay/") {
            return Err(invalid("Official overlay path is not canonical."));
        }
        let legacy_seed = path == "plugins/dats/bahamut-dats-overlay/overlay.toml"
            && file.ownership == FileOwnership::Seed;
        if target == Target::WindowsX86_64
            && (file.ownership == FileOwnership::Managed || legacy_seed)
        {
            return Ok(());
        }
        return Err(invalid(
            "Official overlay files must be owned by the Windows launcher release.",
        ));
    }
    if file.ownership != FileOwnership::Managed {
        return Err(invalid("Launcher seed path is not permitted."));
    }
    if !allowed_launcher_managed_path(path, target) {
        return Err(invalid(
            "Launcher inventory path is protected or outside reviewed package ownership.",
        ));
    }
    Ok(())
}

fn allowed_launcher_managed_path(path: &str, target: Target) -> bool {
    if target == Target::WindowsX86_64 && path.starts_with("plugins/dats/bahamut-dats-overlay/") {
        return true;
    }
    let common = matches!(
        path,
        "LICENSE.md"
            | "README.md"
            | "licenses/MinHook-LICENSE.txt"
            | "licenses/Dear-ImGui-LICENSE.txt"
            | "licenses/Lua-COPYRIGHT.txt"
            | "licenses/Inter-OFL.txt"
            | "licenses/Cinzel-OFL.txt"
            | "licenses/JetBrainsMono-OFL.txt"
            | "licenses/Miniz-LICENSE.txt"
    );
    common
        || match target {
            Target::WindowsX86_64 => matches!(
                path,
                "bahamut-launcher.exe"
                    | "bahamut-update-helper.exe"
                    | "bahamut-loader.exe"
                    | "bahamut.dll"
                    | "plugins/screenshot.dll"
                    | "plugins/discord-rpc.dll"
                    | "prerequisites/vc_redist.x86.exe"
                    | "prerequisites/MicrosoftEdgeWebView2Setup.exe"
                    | "addons/chatlogs/addon.toml"
                    | "addons/chatlogs/chatlogs.lua"
                    | "addons/zonename/addon.toml"
                    | "addons/zonename/zonename.lua"
                    | "addons/packetlogger/addon.toml"
                    | "addons/packetlogger/packetlogger.lua"
                    | "addons/combatparser/addon.toml"
                    | "addons/combatparser/combatparser.lua"
                    | "addons/distance/addon.toml"
                    | "addons/distance/distance.lua"
                    | "addons/fps/addon.toml"
                    | "addons/fps/fps.lua"
                    | "addons/pos/addon.toml"
                    | "addons/pos/pos.lua"
                    | "addons/targethp/addon.toml"
                    | "addons/targethp/targethp.lua"
                    | "addons/wiki/addon.toml"
                    | "addons/wiki/wiki.lua"
            ),
            Target::LinuxX86_64 | Target::MacosX86_64 => path == "bahamut-launcher",
            Target::PlatformIndependent => false,
        }
}

fn parse_stable_version(value: &str) -> Result<Version> {
    let version = Version::parse(value)
        .map_err(|_| invalid("Release version must use MAJOR.MINOR.PATCH."))?;
    if !version.pre.is_empty() || !version.build.is_empty() || version.to_string() != value {
        return Err(invalid(
            "Release version must use stable MAJOR.MINOR.PATCH.",
        ));
    }
    Ok(version)
}

fn validate_lower_sha256(value: &str, label: &str) -> Result<()> {
    validate_hash(value).map_err(|_| invalid(&format!("{label} is invalid.")))?;
    if value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(invalid(&format!("{label} must use lowercase hexadecimal.")));
    }
    Ok(())
}

fn target_name(target: Target) -> &'static str {
    match target {
        Target::PlatformIndependent => "platform-independent",
        Target::WindowsX86_64 => "windows-x86_64",
        Target::LinuxX86_64 => "linux-x86_64",
        Target::MacosX86_64 => "macos-x86_64",
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn invalid(message: &str) -> ReleaseError {
    ReleaseError::Invalid(message.to_owned())
}

fn read_fixed_file(path: &Path, expected: usize, label: &str) -> Result<Vec<u8>> {
    let bytes = read_bounded_file(path, expected, label)?;
    if bytes.len() != expected {
        return Err(invalid(&format!(
            "{label} must be exactly {expected} raw bytes."
        )));
    }
    Ok(bytes)
}

fn read_bounded_file(path: &Path, max: usize, label: &str) -> Result<Vec<u8>> {
    let mut file =
        File::open(path).map_err(|_| invalid(&format!("Could not open {label} file.")))?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid(&format!("Could not read {label} file.")))?;
    if bytes.len() > max {
        return Err(invalid(&format!("{label} file exceeds its size limit.")));
    }
    Ok(bytes)
}

pub fn verify_artifact_file(metadata: &ReleaseMetadata, path: &Path) -> Result<()> {
    let details = fs::symlink_metadata(path)
        .map_err(|_| invalid("Could not inspect release artifact file."))?;
    if !details.is_file() || details.file_type().is_symlink() {
        return Err(invalid("Release artifact is not a regular file."));
    }
    let (length, sha256) = hash_file(path)?;
    if length != metadata.artifact.length || sha256 != metadata.artifact.sha256 {
        return Err(invalid(
            "Release artifact length or SHA-256 does not match.",
        ));
    }
    match &metadata.inventory {
        ReleaseInventory::GameDelivery { .. } => Ok(()),
        ReleaseInventory::Launcher { files } => match metadata.artifact.format {
            ArtifactFormat::Zip => verify_zip_inventory(path, metadata, files),
            ArtifactFormat::TarGz => verify_tar_inventory(path, metadata, files),
        },
    }
}

pub fn bootstrap_accepted_state(
    path: &Path,
    release: &VerifiedRelease,
    starting_version: &str,
) -> Result<()> {
    let starting = parse_stable_version(starting_version)?;
    if starting.to_string() != release.metadata.version {
        return Err(invalid(
            "Trusted starting version must equal the verified release version.",
        ));
    }
    let _state_lock = StateFileLock::acquire(path)?;
    let mut state = match read_state_if_present(path)? {
        Some(state) => state,
        None => AcceptedState {
            schema_version: 1,
            scopes: Vec::new(),
        },
    };
    if state
        .scopes
        .iter()
        .any(|scope| scope.scope() == release.metadata.scope())
    {
        return Err(invalid(
            "Accepted state already has this product, channel, and target.",
        ));
    }
    let identity = ReleaseIdentity::from_verified(release);
    state.scopes.push(AcceptedScope {
        product: release.metadata.product,
        channel: release.metadata.channel,
        target: release.metadata.target,
        trusted_key_sha256: release.trusted_key_sha256.clone(),
        highest_remote: identity.clone(),
        trusted_releases: vec![identity],
        installed: None,
    });
    state.scopes.sort_by_key(AcceptedScope::scope);
    validate_state(&state)?;
    write_state_atomic(path, &state, path.exists())
}

pub fn accept_offer(path: &Path, release: &VerifiedRelease) -> Result<OfferDecision> {
    let _state_lock = StateFileLock::acquire(path)?;
    accept_offer_under_lock(path, release)
}

// The caller holds the adjacent state lock across this read-modify-write transaction.
fn accept_offer_under_lock(path: &Path, release: &VerifiedRelease) -> Result<OfferDecision> {
    let mut state = read_state(path)?;
    let scope = find_scope_mut(&mut state, release.metadata.scope())?;
    verify_state_key(scope, release)?;
    let offered_version = parse_stable_version(&release.metadata.version)?;
    let highest_version = parse_stable_version(&scope.highest_remote.version)?;
    match offered_version.cmp(&highest_version) {
        std::cmp::Ordering::Less => Err(invalid("Older remote release offers are rejected.")),
        std::cmp::Ordering::Equal => {
            if scope.highest_remote.metadata_sha256 == release.metadata_sha256 {
                Ok(OfferDecision::ReuseIdentical)
            } else {
                Err(invalid(
                    "Same-version release metadata changed after acceptance.",
                ))
            }
        }
        std::cmp::Ordering::Greater => {
            if scope.trusted_releases.len() >= MAX_ACCEPTED_RELEASES {
                return Err(invalid("Accepted release history is full."));
            }
            let identity = ReleaseIdentity::from_verified(release);
            scope.trusted_releases.push(identity.clone());
            scope.highest_remote = identity;
            validate_state(&state)?;
            write_state_atomic(path, &state, true)?;
            Ok(OfferDecision::Accepted)
        }
    }
}

pub fn record_installed_release(
    path: &Path,
    release: &VerifiedRelease,
) -> Result<InstalledDecision> {
    let _state_lock = StateFileLock::acquire(path)?;
    let mut state = read_state(path)?;
    let scope = find_scope_mut(&mut state, release.metadata.scope())?;
    verify_state_key(scope, release)?;
    let identity = ReleaseIdentity::from_verified(release);
    if !scope.trusted_releases.contains(&identity) {
        return Err(invalid(
            "Installed release was not previously trusted for this scope.",
        ));
    }
    if scope.installed.as_ref() == Some(&identity) {
        return Ok(InstalledDecision::AlreadyCurrent);
    }
    scope.installed = Some(identity);
    validate_state(&state)?;
    write_state_atomic(path, &state, true)?;
    Ok(InstalledDecision::Recorded)
}

pub fn accepted_state_summary(
    path: &Path,
    scope: ReleaseScope,
) -> Result<(String, Option<String>)> {
    let state = read_state(path)?;
    let scope = find_scope(&state, scope)?;
    Ok((
        scope.highest_remote.version.clone(),
        scope
            .installed
            .as_ref()
            .map(|identity| identity.version.clone()),
    ))
}

impl AcceptedScope {
    fn scope(&self) -> ReleaseScope {
        ReleaseScope {
            product: self.product,
            channel: self.channel,
            target: self.target,
        }
    }
}

fn find_scope_mut(state: &mut AcceptedState, scope: ReleaseScope) -> Result<&mut AcceptedScope> {
    state
        .scopes
        .iter_mut()
        .find(|entry| entry.scope() == scope)
        .ok_or_else(|| {
            invalid("Accepted state has no entry for this product, channel, and target.")
        })
}

fn find_scope(state: &AcceptedState, scope: ReleaseScope) -> Result<&AcceptedScope> {
    state
        .scopes
        .iter()
        .find(|entry| entry.scope() == scope)
        .ok_or_else(|| {
            invalid("Accepted state has no entry for this product, channel, and target.")
        })
}

fn verify_state_key(scope: &AcceptedScope, release: &VerifiedRelease) -> Result<()> {
    if scope.trusted_key_sha256 != release.trusted_key_sha256 {
        return Err(invalid(
            "Trusted public key changed for this release scope.",
        ));
    }
    Ok(())
}

fn validate_state(state: &AcceptedState) -> Result<()> {
    if state.schema_version != 1 || state.scopes.is_empty() || state.scopes.len() > 1024 {
        return Err(invalid(
            "Accepted release state schema or scope count is invalid.",
        ));
    }
    let mut previous_scope = None;
    for scope in &state.scopes {
        validate_product_target(scope.product, scope.target)?;
        validate_lower_sha256(&scope.trusted_key_sha256, "trusted public-key SHA-256")?;
        if scope.trusted_releases.is_empty() || scope.trusted_releases.len() > MAX_ACCEPTED_RELEASES
        {
            return Err(invalid("Accepted release history is empty or too large."));
        }
        if previous_scope.is_some_and(|previous| previous >= scope.scope()) {
            return Err(invalid(
                "Accepted release scopes are duplicated or not in stable order.",
            ));
        }
        previous_scope = Some(scope.scope());
        let mut previous_version: Option<Version> = None;
        for release in &scope.trusted_releases {
            let version = parse_stable_version(&release.version)?;
            validate_lower_sha256(&release.metadata_sha256, "accepted metadata SHA-256")?;
            if previous_version
                .as_ref()
                .is_some_and(|previous| previous >= &version)
            {
                return Err(invalid(
                    "Accepted release history is duplicated or not in ascending order.",
                ));
            }
            previous_version = Some(version);
        }
        if scope.trusted_releases.last() != Some(&scope.highest_remote) {
            return Err(invalid(
                "Accepted state high-water mark does not match its release history.",
            ));
        }
        if let Some(installed) = &scope.installed {
            parse_stable_version(&installed.version)?;
            validate_lower_sha256(&installed.metadata_sha256, "installed metadata SHA-256")?;
            if !scope.trusted_releases.contains(installed) {
                return Err(invalid(
                    "Installed release is absent from trusted release history.",
                ));
            }
        }
    }
    Ok(())
}

fn read_state(path: &Path) -> Result<AcceptedState> {
    let details = fs::symlink_metadata(path)
        .map_err(|_| invalid("Accepted release state does not exist."))?;
    if !details.is_file() || details.file_type().is_symlink() {
        return Err(invalid("Accepted release state is not a regular file."));
    }
    let bytes = read_bounded_file(path, MAX_ACCEPTED_STATE_BYTES, "accepted release state")?;
    let state: AcceptedState = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("Accepted release state JSON is invalid."))?;
    validate_state(&state)?;
    Ok(state)
}

struct StateFileLock {
    _file: File,
}

impl StateFileLock {
    fn acquire(state_path: &Path) -> Result<Self> {
        let lock_path = state_lock_path(state_path)?;
        let parent = parent_directory(&lock_path);
        fs::create_dir_all(parent)?;
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                OpenOptions::new().read(true).write(true).open(&lock_path)?
            }
            Err(error) => return Err(error.into()),
        };
        let details = fs::symlink_metadata(&lock_path)?;
        if !details.is_file() || details.file_type().is_symlink() {
            return Err(invalid("Accepted release lock is not a regular file."));
        }
        file.lock()?;
        Ok(Self { _file: file })
    }
}

impl Drop for StateFileLock {
    fn drop(&mut self) {
        let _ = self._file.unlock();
    }
}

fn state_lock_path(state_path: &Path) -> Result<PathBuf> {
    let name = state_path
        .file_name()
        .ok_or_else(|| invalid("Accepted release state path has no file name."))?;
    let mut lock_name = name.to_os_string();
    lock_name.push(".lock");
    Ok(parent_directory(state_path).join(lock_name))
}

fn read_state_if_present(path: &Path) -> Result<Option<AcceptedState>> {
    match fs::symlink_metadata(path) {
        Ok(details) => {
            if !details.is_file() || details.file_type().is_symlink() {
                return Err(invalid("Accepted release state is not a regular file."));
            }
            read_state(path).map(Some)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn write_state_atomic(path: &Path, state: &AcceptedState, replace: bool) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(state)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_ACCEPTED_STATE_BYTES {
        return Err(invalid("Accepted release state exceeds the 2 MiB limit."));
    }
    publish_staged(path, replace, |output| output.write_all(&bytes))?;
    Ok(())
}

fn publish_staged(
    path: &Path,
    replace: bool,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let parent = parent_directory(path);
    fs::create_dir_all(parent)?;
    let temporary = temporary_path(path)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let written = (|| {
        write(&mut output)?;
        output.flush()?;
        output.sync_all()
    })();
    drop(output);
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let result = if replace {
        replace_file(&temporary, path)
    } else {
        fs::hard_link(&temporary, path).map(|()| {
            let _ = fs::remove_file(&temporary);
        })
    };
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        return result;
    }
    sync_parent(parent)?;
    Ok(())
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn temporary_path(path: &Path) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "state path has no name"))?;
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos() as u64);
    let mut temporary_name = name.to_os_string();
    temporary_name.push(format!(
        ".tmp-{}-{}",
        std::process::id(),
        timestamp ^ counter
    ));
    Ok(parent_directory(path).join(temporary_name))
}

#[cfg(unix)]
fn replace_file(temporary: &Path, path: &Path) -> io::Result<()> {
    fs::rename(temporary, path)
}

#[cfg(windows)]
fn replace_file(temporary: &Path, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let temporary_wide = temporary
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    let path_wide = path
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            windows::core::PCWSTR(temporary_wide.as_ptr()),
            windows::core::PCWSTR(path_wide.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(io::Error::other)
    }
}

#[cfg(not(any(unix, windows)))]
fn replace_file(temporary: &Path, path: &Path) -> io::Result<()> {
    fs::rename(temporary, path)
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> io::Result<()> {
    Ok(())
}

fn hash_file(path: &Path) -> Result<(u64, String)> {
    let mut input = File::open(path).map_err(|_| invalid("Could not read release artifact."))?;
    let mut digest = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|_| invalid("Could not read release artifact."))?;
        if read == 0 {
            break;
        }
        length = length
            .checked_add(read as u64)
            .ok_or_else(|| invalid("Release artifact length overflows."))?;
        digest.update(&buffer[..read]);
    }
    Ok((length, format!("{:x}", digest.finalize())))
}

fn verify_zip_inventory(
    path: &Path,
    metadata: &ReleaseMetadata,
    files: &[InventoryFile],
) -> Result<()> {
    let expected = expected_archive_files(files)?;
    let input = File::open(path).map_err(|_| invalid("Could not open release ZIP."))?;
    let mut archive = ZipArchive::new(input)
        .map_err(|_| invalid("Release artifact is not a valid ZIP archive."))?;
    let mut seen = HashSet::with_capacity(expected.len());
    let mut directories = HashSet::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| invalid("Release ZIP entry is invalid."))?;
        let raw_name = entry.name();
        let is_directory = entry.is_dir() || raw_name.ends_with('/');
        let name = if is_directory {
            raw_name.strip_suffix('/').unwrap_or_default()
        } else {
            raw_name
        };
        if name.is_empty() {
            return Err(invalid("Release ZIP contains an empty path."));
        }
        validate_relative_path(name)
            .map_err(|_| invalid("Release ZIP contains an unsafe path."))?;
        let folded = name.to_ascii_lowercase();
        if is_directory {
            if let Some(mode) = entry.unix_mode() {
                let file_type = mode & 0o170000;
                if file_type != 0 && file_type != 0o040000 {
                    return Err(invalid(
                        "Release ZIP directory entry is not a regular directory.",
                    ));
                }
            }
            if expected.contains_key(&folded) || !directories.insert(folded) {
                return Err(invalid("Release ZIP contains a path collision."));
            }
            validate_archive_directory(metadata, files, name)?;
            continue;
        }
        if let Some(mode) = entry.unix_mode() {
            let file_type = mode & 0o170000;
            if file_type != 0 && file_type != 0o100000 {
                return Err(invalid("Release ZIP contains a non-regular file."));
            }
        }
        let expected_file = expected
            .get(&folded)
            .ok_or_else(|| invalid("Release ZIP has a file outside its inventory."))?;
        if expected_file.path != name || !seen.insert(folded) {
            return Err(invalid(
                "Release ZIP inventory path is duplicated or changed.",
            ));
        }
        if entry.size() != expected_file.length {
            return Err(invalid(
                "Release ZIP file length differs from its inventory.",
            ));
        }
        verify_reader(&mut entry, expected_file)?;
    }
    if seen.len() != expected.len() {
        return Err(invalid("Release ZIP is missing an inventory file."));
    }
    Ok(())
}

fn verify_tar_inventory(
    path: &Path,
    metadata: &ReleaseMetadata,
    files: &[InventoryFile],
) -> Result<()> {
    let expected = expected_archive_files(files)?;
    let input = File::open(path).map_err(|_| invalid("Could not open release TAR.GZ."))?;
    let decoder = MultiGzDecoder::new(input);
    let mut archive = tar::Archive::new(decoder);
    let mut seen = HashSet::with_capacity(expected.len());
    let mut directories = HashSet::new();
    {
        let entries = archive
            .entries()
            .map_err(|_| invalid("Release TAR.GZ directory is invalid."))?;
        for entry in entries {
            let mut entry = entry.map_err(|_| invalid("Release TAR.GZ entry is invalid."))?;
            let raw_path = entry.path_bytes();
            let raw_text = std::str::from_utf8(&raw_path)
                .map_err(|_| invalid("Release TAR.GZ contains a non-UTF-8 path."))?;
            let entry_type = entry.header().entry_type();
            let (name, is_root) = normalize_tar_path(raw_text, entry_type.is_dir());
            if is_root && entry_type.is_dir() {
                continue;
            }
            if is_root {
                return Err(invalid("Release TAR.GZ contains an invalid root entry."));
            }
            validate_relative_path(name)
                .map_err(|_| invalid("Release TAR.GZ contains an unsafe path."))?;
            let folded = name.to_ascii_lowercase();
            if entry_type.is_dir() {
                if expected.contains_key(&folded) || !directories.insert(folded) {
                    return Err(invalid("Release TAR.GZ contains a path collision."));
                }
                validate_archive_directory(metadata, files, name)?;
                continue;
            }
            if !entry_type.is_file() {
                return Err(invalid("Release TAR.GZ contains a non-regular entry."));
            }
            let expected_file = expected
                .get(&folded)
                .ok_or_else(|| invalid("Release TAR.GZ has a file outside its inventory."))?;
            if expected_file.path != name || !seen.insert(folded) {
                return Err(invalid(
                    "Release TAR.GZ inventory path is duplicated or changed.",
                ));
            }
            if entry.size() != expected_file.length {
                return Err(invalid(
                    "Release TAR.GZ file length differs from its inventory.",
                ));
            }
            verify_reader(&mut entry, expected_file)?;
        }
    }
    let mut decoder = archive.into_inner();
    io::copy(&mut decoder, &mut io::sink())
        .map_err(|_| invalid("Release TAR.GZ stream is incomplete or corrupt."))?;
    if seen.len() != expected.len() {
        return Err(invalid("Release TAR.GZ is missing an inventory file."));
    }
    Ok(())
}

fn validate_archive_directory(
    metadata: &ReleaseMetadata,
    files: &[InventoryFile],
    directory: &str,
) -> Result<()> {
    let is_file_ancestor = files.iter().any(|file| {
        file.path
            .strip_prefix(directory)
            .is_some_and(|suffix| suffix.starts_with('/'))
    });
    if is_file_ancestor {
        return Ok(());
    }
    let allowed = match metadata.product {
        Product::Launcher if metadata.target == Target::WindowsX86_64 => matches!(
            directory,
            "addons"
                | "addons/chatlogs"
                | "addons/zonename"
                | "addons/packetlogger"
                | "addons/combatparser"
                | "addons/distance"
                | "addons/fps"
                | "addons/pos"
                | "addons/targethp"
                | "addons/wiki"
                | "plugins"
                | "plugins/dats"
                | "config"
                | "config/addons"
                | "config/plugins"
                | "config/plugins/screenshot"
                | "scripts"
                | "logs"
                | "logs/launcher"
                | "logs/chat"
                | "logs/packets"
                | "screenshots"
                | "licenses"
        ),
        Product::Launcher => directory == "licenses",
        Product::Game => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(invalid(
            "Release archive contains a directory outside package layout.",
        ))
    }
}

fn normalize_tar_path(path: &str, is_directory: bool) -> (&str, bool) {
    if path == "." || path == "./" {
        return ("", true);
    }
    let path = path.strip_prefix("./").unwrap_or(path);
    (
        if is_directory {
            path.strip_suffix('/').unwrap_or(path)
        } else {
            path
        },
        false,
    )
}

fn expected_archive_files(files: &[InventoryFile]) -> Result<BTreeMap<String, &InventoryFile>> {
    let mut expected = BTreeMap::new();
    for file in files {
        let key = file.path.to_ascii_lowercase();
        if expected.insert(key, file).is_some() {
            return Err(invalid("Release inventory contains duplicate files."));
        }
    }
    Ok(expected)
}

fn verify_reader(input: &mut impl Read, expected: &InventoryFile) -> Result<()> {
    let mut digest = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|_| invalid("Release archive file is corrupt."))?;
        if read == 0 {
            break;
        }
        length = length
            .checked_add(read as u64)
            .ok_or_else(|| invalid("Release inventory file length overflows."))?;
        digest.update(&buffer[..read]);
    }
    if length != expected.length || format!("{:x}", digest.finalize()) != expected.sha256 {
        return Err(invalid(
            "Release archive file bytes differ from their inventory identity.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::{Arc, Barrier};

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use ring::signature::KeyPair;
    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::patcher::content::{BaseArchive, BasePackage, InstallFile, PatchTransition};
    use crate::patcher::http::ObjectSpec;
    use crate::version::FFXIV_GAME_VERSION;

    const TEST_SEED: [u8; ED25519_SEED_BYTES] = [0x53; ED25519_SEED_BYTES];

    fn test_keypair() -> Ed25519KeyPair {
        Ed25519KeyPair::from_seed_unchecked(&TEST_SEED).unwrap()
    }

    fn test_public_key() -> Vec<u8> {
        test_keypair().public_key().as_ref().to_vec()
    }

    fn file(path: &str, content: &[u8], ownership: FileOwnership) -> InventoryFile {
        InventoryFile {
            path: path.into(),
            length: content.len() as u64,
            sha256: sha256_hex(content),
            ownership,
        }
    }

    fn launcher_release(
        directory: &Path,
        version: &str,
        target: Target,
        contents: &[u8],
    ) -> (ReleaseMetadata, PathBuf) {
        let launcher_path = if target == Target::WindowsX86_64 {
            "bahamut-launcher.exe"
        } else {
            "bahamut-launcher"
        };
        let mut files = vec![file(launcher_path, contents, FileOwnership::Managed)];
        if target == Target::WindowsX86_64 {
            files.push(file(
                "scripts/default.txt",
                b"/fillmode\n",
                FileOwnership::Seed,
            ));
            files.push(file(
                "plugins/dats/bahamut-dats-overlay/overlay.toml",
                b"[overlay]\n",
                FileOwnership::Managed,
            ));
        }
        let path = directory.join(format!(
            "artifact-{version}.{}",
            match target {
                Target::WindowsX86_64 => "zip",
                _ => "tar.gz",
            }
        ));
        match target {
            Target::WindowsX86_64 => write_zip(
                &path,
                &[
                    (launcher_path, contents),
                    ("scripts/default.txt", b"/fillmode\n"),
                    (
                        "plugins/dats/bahamut-dats-overlay/overlay.toml",
                        b"[overlay]\n",
                    ),
                ],
            ),
            _ => write_tar_gz(&path, &[("./bahamut-launcher", contents)]),
        }
        .unwrap();
        let (length, digest) = hash_file(&path).unwrap();
        (
            ReleaseMetadata {
                schema_version: 1,
                product: Product::Launcher,
                channel: Channel::Stable,
                target,
                version: version.into(),
                artifact: ArtifactIdentity {
                    object_key: format!(
                        "launcher/{version}/{}/{digest}.{}",
                        target_name(target),
                        if target == Target::WindowsX86_64 {
                            "zip"
                        } else {
                            "tar.gz"
                        }
                    ),
                    format: if target == Target::WindowsX86_64 {
                        ArtifactFormat::Zip
                    } else {
                        ArtifactFormat::TarGz
                    },
                    length,
                    sha256: digest,
                },
                inventory: ReleaseInventory::Launcher { files },
            },
            path,
        )
    }

    fn signed_release(
        metadata: &ReleaseMetadata,
        delivery_bytes: Option<&[u8]>,
    ) -> (Vec<u8>, Vec<u8>, VerifiedRelease) {
        let metadata_bytes = serde_json::to_vec(metadata).unwrap();
        let signature = sign_metadata(&metadata_bytes, &TEST_SEED, delivery_bytes).unwrap();
        let verified = verify_metadata(
            &metadata_bytes,
            &signature,
            &test_public_key(),
            metadata.scope(),
            delivery_bytes,
        )
        .unwrap();
        (metadata_bytes, signature.to_vec(), verified)
    }

    fn write_zip(path: &Path, files: &[(&str, &[u8])]) -> io::Result<()> {
        let output = File::create(path)?;
        let mut archive = zip::ZipWriter::new(output);
        for (name, contents) in files {
            archive.start_file(*name, SimpleFileOptions::default())?;
            archive.write_all(contents)?;
        }
        archive.finish()?;
        Ok(())
    }

    fn write_zip_symlink_directory(path: &Path) -> io::Result<()> {
        let output = File::create(path)?;
        let mut archive = zip::ZipWriter::new(output);
        archive.add_symlink("plugins/", "../../outside", SimpleFileOptions::default())?;
        archive.finish()?;
        Ok(())
    }

    fn write_tar_gz(path: &Path, files: &[(&str, &[u8])]) -> io::Result<()> {
        let output = File::create(path)?;
        let encoder = GzEncoder::new(output, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        for name in ["./", "./licenses/"] {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(0o755);
            header.set_cksum();
            archive.append_data(&mut header, name, Cursor::new([]))?;
        }
        for (name, contents) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, name, Cursor::new(contents))?;
        }
        archive.into_inner()?.finish()?;
        Ok(())
    }

    fn game_fixture() -> (ReleaseMetadata, Vec<u8>) {
        let boot = InstallFile {
            path: "ffxivboot.exe".into(),
            length: 1,
            sha256: sha256_hex(b"b"),
        };
        let game = InstallFile {
            path: "ffxivgame.exe".into(),
            length: 1,
            sha256: sha256_hex(b"g"),
        };
        let archive_object = ObjectSpec {
            object_key: "xiv1point0.zip".into(),
            length: b"archive bytes".len() as u64,
            sha256: sha256_hex(b"archive bytes"),
        };
        let manifest = DeliveryManifest {
            schema_version: 2,
            content_root: None,
            hosted_patches: false,
            base: Some(BasePackage {
                baseline_version: FFXIV_GAME_VERSION.into(),
                target_version: FFXIV_GAME_VERSION.into(),
                transition: PatchTransition::None,
                archives: vec![BaseArchive {
                    object: archive_object.clone(),
                    files: vec![boot.clone(), game.clone()],
                    layout: Default::default(),
                    excluded_files: Vec::new(),
                    empty_directories: Vec::new(),
                    apple_metadata_files: 0,
                }],
                final_files: vec![boot, game],
                staging_bytes: 2,
            }),
        };
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let metadata = ReleaseMetadata {
            schema_version: 1,
            product: Product::Game,
            channel: Channel::Stable,
            target: Target::PlatformIndependent,
            version: "1.0.0".into(),
            artifact: ArtifactIdentity {
                object_key: archive_object.object_key,
                format: ArtifactFormat::Zip,
                length: archive_object.length,
                sha256: archive_object.sha256,
            },
            inventory: ReleaseInventory::GameDelivery {
                manifest_sha256: sha256_hex(&manifest_bytes),
                target_version: FFXIV_GAME_VERSION.into(),
            },
        };
        (metadata, manifest_bytes)
    }

    #[test]
    fn signed_launcher_workflow_orders_state_per_scope_and_keeps_rollback_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("accepted.json");
        let public_key = test_public_key();
        let (baseline, baseline_artifact) = launcher_release(
            directory.path(),
            "1.0.0",
            Target::WindowsX86_64,
            b"launcher v1",
        );
        let (baseline_bytes, baseline_signature, baseline_verified) =
            signed_release(&baseline, None);
        verify_artifact_file(&baseline, &baseline_artifact).unwrap();
        assert!(
            verify_metadata(
                &baseline_bytes,
                &baseline_signature,
                &public_key,
                ReleaseScope {
                    product: Product::Launcher,
                    channel: Channel::Stable,
                    target: Target::WindowsX86_64,
                },
                None,
            )
            .is_ok()
        );
        bootstrap_accepted_state(&state_path, &baseline_verified, "1.0.0").unwrap();

        let (newer, newer_artifact) = launcher_release(
            directory.path(),
            "1.2.0",
            Target::WindowsX86_64,
            b"launcher v2",
        );
        let (newer_bytes, newer_signature, newer_verified) = signed_release(&newer, None);
        verify_artifact_file(&newer, &newer_artifact).unwrap();
        assert_eq!(
            accept_offer(&state_path, &newer_verified).unwrap(),
            OfferDecision::Accepted
        );
        assert_eq!(
            accept_offer(&state_path, &newer_verified).unwrap(),
            OfferDecision::ReuseIdentical
        );

        let (changed_same_version, _) = launcher_release(
            directory.path(),
            "1.2.0",
            Target::WindowsX86_64,
            b"changed same version",
        );
        let (_, _, changed_verified) = signed_release(&changed_same_version, None);
        assert!(accept_offer(&state_path, &changed_verified).is_err());
        assert!(accept_offer(&state_path, &baseline_verified).is_err());

        assert_eq!(
            record_installed_release(&state_path, &baseline_verified).unwrap(),
            InstalledDecision::Recorded
        );
        assert_eq!(
            accepted_state_summary(&state_path, baseline.scope()).unwrap(),
            ("1.2.0".into(), Some("1.0.0".into()))
        );

        let (linux_release, linux_artifact) = launcher_release(
            directory.path(),
            "0.1.0",
            Target::LinuxX86_64,
            b"linux launcher",
        );
        let (_, _, linux_verified) = signed_release(&linux_release, None);
        verify_artifact_file(&linux_release, &linux_artifact).unwrap();
        bootstrap_accepted_state(&state_path, &linux_verified, "0.1.0").unwrap();
        let (linux_next, _) = launcher_release(
            directory.path(),
            "0.2.0",
            Target::LinuxX86_64,
            b"linux launcher next",
        );
        let (_, _, linux_next_verified) = signed_release(&linux_next, None);
        assert_eq!(
            accept_offer(&state_path, &linux_next_verified).unwrap(),
            OfferDecision::Accepted
        );
        assert_eq!(sha256_hex(&newer_bytes), newer_verified.metadata_sha256);
        assert_eq!(newer_signature.len(), ED25519_SIGNATURE_BYTES);
    }

    #[test]
    fn game_inventory_pins_delivery_manifest_archive_and_final_files() {
        let (metadata, manifest_bytes) = game_fixture();
        let (_, _, verified) = signed_release(&metadata, Some(&manifest_bytes));
        assert_eq!(verified.metadata.inventory, metadata.inventory);

        let mut wrong_hash = metadata.clone();
        let ReleaseInventory::GameDelivery {
            manifest_sha256, ..
        } = &mut wrong_hash.inventory
        else {
            unreachable!();
        };
        *manifest_sha256 = sha256_hex(b"other manifest");
        assert!(validate_metadata(&wrong_hash, Some(&manifest_bytes)).is_err());

        let mut wrong_archive = metadata.clone();
        wrong_archive.artifact.length += 1;
        assert!(validate_metadata(&wrong_archive, Some(&manifest_bytes)).is_err());

        let ReleaseInventory::GameDelivery { target_version, .. } = &mut wrong_archive.inventory
        else {
            unreachable!();
        };
        *target_version = "different-game-version".into();
        assert!(validate_metadata(&wrong_archive, Some(&manifest_bytes)).is_err());
    }

    #[test]
    fn shipped_game_delivery_manifest_fits_the_bounded_game_metadata_input() {
        let manifest_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("manifests/game-delivery.json");
        let manifest_bytes = fs::read(manifest_path).unwrap();
        assert!(manifest_bytes.len() <= MAX_DELIVERY_MANIFEST_BYTES);
        let manifest: DeliveryManifest = serde_json::from_slice(&manifest_bytes).unwrap();
        let package = manifest.base.as_ref().unwrap();
        let archive = &package.archives[0].object;
        let metadata = ReleaseMetadata {
            schema_version: 1,
            product: Product::Game,
            channel: Channel::Stable,
            target: Target::PlatformIndependent,
            version: "1.0.0".into(),
            artifact: ArtifactIdentity {
                object_key: archive.object_key.clone(),
                format: ArtifactFormat::Zip,
                length: archive.length,
                sha256: archive.sha256.clone(),
            },
            inventory: ReleaseInventory::GameDelivery {
                manifest_sha256: sha256_hex(&manifest_bytes),
                target_version: package.target_version.clone(),
            },
        };
        validate_metadata(&metadata, Some(&manifest_bytes)).unwrap();
    }

    #[test]
    fn launcher_inventory_owns_only_the_official_overlay_package() {
        let directory = tempfile::tempdir().unwrap();
        for (path, ownership) in [
            ("../escape.dll", FileOwnership::Managed),
            ("scripts/default.txt", FileOwnership::Managed),
            (
                "plugins/dats/other-package/overlay.toml",
                FileOwnership::Managed,
            ),
            ("config/bahamut.ini", FileOwnership::Managed),
            ("cache/state.bin", FileOwnership::Managed),
            ("logs/launcher/launcher.log", FileOwnership::Managed),
            ("screenshots/capture.png", FileOwnership::Managed),
            ("state/accepted.json", FileOwnership::Managed),
            ("addons/custom/addon.toml", FileOwnership::Managed),
            ("addons/unknown/addon.toml", FileOwnership::Managed),
            ("plugins/unreviewed.dll", FileOwnership::Managed),
            ("plugins/custom.dll", FileOwnership::Managed),
            (
                "plugins/dats/user-package/overlay.toml",
                FileOwnership::Seed,
            ),
        ] {
            let entries = vec![file(path, b"x", ownership)];
            let (metadata, _) = launcher_release_with_inventory(
                &entries,
                "1.0.0",
                Target::WindowsX86_64,
                directory.path(),
            );
            assert!(validate_metadata(&metadata, None).is_err(), "{path}");
        }

        let official_overlay = file(
            "plugins/dats/bahamut-dats-overlay/data/1C/59/00/CB.DAT",
            b"official payload",
            FileOwnership::Managed,
        );
        let (metadata, _) = launcher_release_with_inventory(
            &[official_overlay],
            "1.0.0",
            Target::WindowsX86_64,
            directory.path(),
        );
        validate_metadata(&metadata, None).unwrap();

        let legacy_seed = file(
            "plugins/dats/bahamut-dats-overlay/overlay.toml",
            b"legacy signed launcher seed",
            FileOwnership::Seed,
        );
        let (metadata, _) = launcher_release_with_inventory(
            &[legacy_seed],
            "1.0.0",
            Target::WindowsX86_64,
            directory.path(),
        );
        validate_metadata(&metadata, None).unwrap();

        let (mut non_windows, _) = launcher_release(
            directory.path(),
            "1.0.0",
            Target::LinuxX86_64,
            b"linux launcher",
        );
        let ReleaseInventory::Launcher { files } = &mut non_windows.inventory else {
            unreachable!();
        };
        files[0].path = "plugins/screenshot.dll".into();
        assert!(validate_metadata(&non_windows, None).is_err());

        let valid_seed = vec![
            file("scripts/default.txt", b"script", FileOwnership::Seed),
            file(
                "plugins/dats/bahamut-dats-overlay/overlay.toml",
                b"managed",
                FileOwnership::Managed,
            ),
        ];
        let (metadata, _) = launcher_release_with_inventory(
            &valid_seed,
            "1.0.0",
            Target::WindowsX86_64,
            directory.path(),
        );
        validate_metadata(&metadata, None).unwrap();

        for target in [Target::LinuxX86_64, Target::MacosX86_64] {
            let seed = vec![file(
                "plugins/dats/bahamut-dats-overlay/overlay.toml",
                b"seed",
                FileOwnership::Seed,
            )];
            let (metadata, _) =
                launcher_release_with_inventory(&seed, "1.0.0", target, directory.path());
            assert!(validate_metadata(&metadata, None).is_err());
        }

        let reviewed_files = vec![
            file("plugins/screenshot.dll", b"plugin", FileOwnership::Managed),
            file("addons/fps/addon.toml", b"[addon]", FileOwnership::Managed),
            file(
                "bahamut-update-helper.exe",
                b"helper",
                FileOwnership::Managed,
            ),
            file(
                "prerequisites/vc_redist.x86.exe",
                b"redistributable",
                FileOwnership::Managed,
            ),
            file(
                "prerequisites/MicrosoftEdgeWebView2Setup.exe",
                b"bootstrapper",
                FileOwnership::Managed,
            ),
        ];
        let (metadata, _) = launcher_release_with_inventory(
            &reviewed_files,
            "1.0.0",
            Target::WindowsX86_64,
            directory.path(),
        );
        validate_metadata(&metadata, None).unwrap();

        assert!(
            validate_file_inventory(
                &[
                    file("addons", b"file", FileOwnership::Managed),
                    file("addons/custom/addon.toml", b"child", FileOwnership::Managed),
                ],
                false,
            )
            .is_err()
        );
        assert!(
            validate_file_inventory(
                &[
                    file("bahamut-launcher.exe", b"one", FileOwnership::Managed),
                    file("BAHAMUT-LAUNCHER.EXE", b"two", FileOwnership::Managed),
                ],
                false,
            )
            .is_err()
        );
    }

    #[test]
    fn zip_directory_symlinks_are_rejected_even_when_the_path_is_allowed() {
        let directory = tempfile::tempdir().unwrap();
        let artifact_path = directory.path().join("directory-symlink.zip");
        write_zip_symlink_directory(&artifact_path).unwrap();
        let (length, digest) = hash_file(&artifact_path).unwrap();
        let metadata = ReleaseMetadata {
            schema_version: 1,
            product: Product::Launcher,
            channel: Channel::Stable,
            target: Target::WindowsX86_64,
            version: "1.0.0".into(),
            artifact: ArtifactIdentity {
                object_key: format!("launcher/1.0.0/windows-x86_64/{digest}.zip"),
                format: ArtifactFormat::Zip,
                length,
                sha256: digest,
            },
            inventory: ReleaseInventory::Launcher {
                files: vec![file(
                    "plugins/dats/bahamut-dats-overlay/overlay.toml",
                    b"manifest",
                    FileOwnership::Managed,
                )],
            },
        };
        assert!(verify_artifact_file(&metadata, &artifact_path).is_err());
    }

    fn launcher_release_with_inventory(
        files: &[InventoryFile],
        version: &str,
        target: Target,
        directory: &Path,
    ) -> (ReleaseMetadata, PathBuf) {
        let format = match target {
            Target::WindowsX86_64 => ArtifactFormat::Zip,
            Target::LinuxX86_64 | Target::MacosX86_64 => ArtifactFormat::TarGz,
            Target::PlatformIndependent => unreachable!("launcher targets are platform-specific"),
        };
        let extension = match format {
            ArtifactFormat::Zip => "zip",
            ArtifactFormat::TarGz => "tar.gz",
        };
        let path = directory.join(format!("inventory-only.{extension}"));
        match format {
            ArtifactFormat::Zip => write_zip(&path, &[]).unwrap(),
            ArtifactFormat::TarGz => write_tar_gz(&path, &[]).unwrap(),
        }
        let (length, digest) = hash_file(&path).unwrap();
        (
            ReleaseMetadata {
                schema_version: 1,
                product: Product::Launcher,
                channel: Channel::Stable,
                target,
                version: version.into(),
                artifact: ArtifactIdentity {
                    object_key: format!(
                        "launcher/{version}/{}/{digest}.{extension}",
                        target_name(target)
                    ),
                    format,
                    length,
                    sha256: digest,
                },
                inventory: ReleaseInventory::Launcher {
                    files: files.to_vec(),
                },
            },
            path,
        )
    }

    #[test]
    fn metadata_schema_rejects_prereleases_unknown_fields_duplicates_and_oversize() {
        let directory = tempfile::tempdir().unwrap();
        let (metadata, _) = launcher_release(
            directory.path(),
            "1.0.0",
            Target::WindowsX86_64,
            b"launcher",
        );
        let mut raw_metadata = serde_json::to_value(&metadata).unwrap();
        raw_metadata["target"] = serde_json::json!("windows-x86_64");
        let raw_metadata = serde_json::to_vec(&raw_metadata).unwrap();
        assert_eq!(
            parse_and_validate_metadata(&raw_metadata, None)
                .unwrap()
                .target,
            Target::WindowsX86_64
        );

        let mut prerelease = metadata.clone();
        prerelease.version = "1.0.0-rc.1".into();
        assert!(validate_metadata(&prerelease, None).is_err());
        for value in ["v1.0.0", "01.0.0", "1.0", "1.0.0+build"] {
            assert!(parse_stable_version(value).is_err(), "{value}");
        }

        let mut extra = serde_json::to_value(metadata).unwrap();
        extra["unknown"] = serde_json::json!(true);
        let extra_bytes = serde_json::to_vec(&extra).unwrap();
        assert!(parse_metadata(&extra_bytes).is_err());
        extra.as_object_mut().unwrap().remove("unknown");
        extra["artifact"]["unknown"] = serde_json::json!(true);
        assert!(parse_metadata(&serde_json::to_vec(&extra).unwrap()).is_err());
        let duplicate = br#"{"schema_version":1,"schema_version":1}"#;
        assert!(parse_metadata(duplicate).is_err());
        let oversized = vec![b' '; MAX_METADATA_BYTES + 1];
        assert!(parse_metadata(&oversized).is_err());
    }

    #[test]
    fn signature_scope_artifact_and_archive_inventory_mismatches_fail() {
        let directory = tempfile::tempdir().unwrap();
        let (metadata, artifact) = launcher_release(
            directory.path(),
            "1.0.0",
            Target::WindowsX86_64,
            b"launcher",
        );
        let (bytes, signature, _) = signed_release(&metadata, None);
        let mut wrong_signature = signature.clone();
        wrong_signature[0] ^= 1;
        assert!(
            verify_metadata(
                &bytes,
                &wrong_signature,
                &test_public_key(),
                metadata.scope(),
                None,
            )
            .is_err()
        );
        let wrong_scope = ReleaseScope {
            product: Product::Launcher,
            channel: Channel::Stable,
            target: Target::LinuxX86_64,
        };
        assert!(
            verify_metadata(&bytes, &signature, &test_public_key(), wrong_scope, None,).is_err()
        );

        let mut tampered_metadata = bytes.clone();
        let version_offset = tampered_metadata
            .windows(b"1.0.0".len())
            .position(|window| window == b"1.0.0")
            .unwrap();
        tampered_metadata[version_offset] = b'2';
        assert!(
            verify_metadata(
                &tampered_metadata,
                &signature,
                &test_public_key(),
                metadata.scope(),
                None,
            )
            .is_err()
        );

        let mut unsupported_channel_value = serde_json::to_value(&metadata).unwrap();
        unsupported_channel_value["channel"] = serde_json::json!("beta");
        let unsupported_channel_bytes = serde_json::to_vec(&unsupported_channel_value).unwrap();
        let unsupported_channel_signature = test_keypair()
            .sign(&unsupported_channel_bytes)
            .as_ref()
            .to_vec();
        assert!(
            verify_metadata(
                &unsupported_channel_bytes,
                &unsupported_channel_signature,
                &test_public_key(),
                metadata.scope(),
                None,
            )
            .is_err()
        );

        assert!(
            verify_metadata(
                &bytes,
                &signature,
                &[0; ED25519_PUBLIC_KEY_BYTES],
                metadata.scope(),
                None,
            )
            .is_err()
        );

        let mut changed = metadata.clone();
        let ReleaseInventory::Launcher { files } = &mut changed.inventory else {
            unreachable!();
        };
        files[0].sha256 = sha256_hex(b"wrong content");
        verify_artifact_file(&changed, &artifact).unwrap_err();
    }

    #[test]
    fn tar_gzip_launcher_inventory_is_verified_without_extracting() {
        let directory = tempfile::tempdir().unwrap();
        let (metadata, artifact) = launcher_release(
            directory.path(),
            "2.0.0",
            Target::LinuxX86_64,
            b"linux launcher",
        );
        validate_metadata(&metadata, None).unwrap();
        verify_artifact_file(&metadata, &artifact).unwrap();
    }

    #[test]
    fn interrupted_state_publication_and_malformed_bootstrap_preserve_prior_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("accepted.json");
        let (metadata, artifact) = launcher_release(
            directory.path(),
            "1.0.0",
            Target::WindowsX86_64,
            b"launcher",
        );
        verify_artifact_file(&metadata, &artifact).unwrap();
        let (_, _, verified) = signed_release(&metadata, None);
        bootstrap_accepted_state(&state_path, &verified, "1.0.0").unwrap();
        let original = fs::read(&state_path).unwrap();
        let failure = publish_staged(&state_path, true, |output| {
            output.write_all(b"partial state")?;
            Err(io::Error::other("injected interrupted publication"))
        });
        assert_eq!(
            failure.unwrap_err().to_string(),
            "injected interrupted publication"
        );
        assert_eq!(fs::read(&state_path).unwrap(), original);
        assert_eq!(read_state(&state_path).unwrap().scopes.len(), 1);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 3);

        fs::write(&state_path, b"{bad state").unwrap();
        assert!(bootstrap_accepted_state(&state_path, &verified, "1.0.0").is_err());
        assert_eq!(fs::read(&state_path).unwrap(), b"{bad state");
    }

    #[test]
    fn starting_version_must_be_explicitly_bound_to_verified_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("accepted.json");
        let (metadata, _) = launcher_release(
            directory.path(),
            "1.0.0",
            Target::WindowsX86_64,
            b"launcher",
        );
        let (_, _, verified) = signed_release(&metadata, None);
        assert!(bootstrap_accepted_state(&state_path, &verified, "0.9.0").is_err());
        bootstrap_accepted_state(&state_path, &verified, "1.0.0").unwrap();
    }

    #[test]
    fn state_is_bound_to_the_explicit_public_key() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("accepted.json");
        let (metadata, _) = launcher_release(
            directory.path(),
            "1.0.0",
            Target::WindowsX86_64,
            b"launcher",
        );
        let (_, _, verified) = signed_release(&metadata, None);
        bootstrap_accepted_state(&state_path, &verified, "1.0.0").unwrap();

        let alternate_seed = [0x11; ED25519_SEED_BYTES];
        let alternate_key = Ed25519KeyPair::from_seed_unchecked(&alternate_seed).unwrap();
        let bytes = serde_json::to_vec(&metadata).unwrap();
        let signature = alternate_key.sign(&bytes);
        let alternate_verified = verify_metadata(
            &bytes,
            signature.as_ref(),
            alternate_key.public_key().as_ref(),
            metadata.scope(),
            None,
        )
        .unwrap();
        assert!(accept_offer(&state_path, &alternate_verified).is_err());
    }

    #[test]
    fn accepted_state_lock_is_persistent_and_exclusive() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("accepted.json");
        let lock_path = state_lock_path(&state_path).unwrap();
        let guard = StateFileLock::acquire(&state_path).unwrap();
        assert!(lock_path.is_file());
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        assert!(contender.try_lock().is_err());
        drop(guard);
        assert!(contender.try_lock().is_ok());
        drop(contender);
        assert!(lock_path.is_file());
    }

    #[test]
    fn stale_lower_offer_cannot_regress_boundary_when_higher_offer_wins() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("accepted.json");
        let (baseline, _) = launcher_release(
            directory.path(),
            "1.0.0",
            Target::WindowsX86_64,
            b"launcher v1",
        );
        let (_, _, baseline_verified) = signed_release(&baseline, None);
        bootstrap_accepted_state(&state_path, &baseline_verified, "1.0.0").unwrap();

        let (lower_offer, _) = launcher_release(
            directory.path(),
            "2.0.0",
            Target::WindowsX86_64,
            b"launcher v2",
        );
        let (_, _, lower_verified) = signed_release(&lower_offer, None);
        let (higher_offer, _) = launcher_release(
            directory.path(),
            "3.0.0",
            Target::WindowsX86_64,
            b"launcher v3",
        );
        let (_, _, higher_verified) = signed_release(&higher_offer, None);

        let _state_lock = StateFileLock::acquire(&state_path).unwrap();
        assert_eq!(
            accept_offer_under_lock(&state_path, &higher_verified).unwrap(),
            OfferDecision::Accepted
        );
        assert!(accept_offer_under_lock(&state_path, &lower_verified).is_err());
        drop(_state_lock);

        let (highest_remote, _) = accepted_state_summary(
            &state_path,
            ReleaseScope {
                product: Product::Launcher,
                channel: Channel::Stable,
                target: Target::WindowsX86_64,
            },
        )
        .unwrap();
        assert_eq!(highest_remote, "3.0.0");
    }

    #[test]
    fn concurrent_accepts_keep_the_highest_remote_version() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("accepted.json");
        let (baseline, _) = launcher_release(
            directory.path(),
            "1.0.0",
            Target::WindowsX86_64,
            b"launcher v1",
        );
        let (_, _, baseline_verified) = signed_release(&baseline, None);
        bootstrap_accepted_state(&state_path, &baseline_verified, "1.0.0").unwrap();

        let (version_two, _) = launcher_release(
            directory.path(),
            "2.0.0",
            Target::WindowsX86_64,
            b"launcher v2",
        );
        let (_, _, version_two_verified) = signed_release(&version_two, None);
        let (version_three, _) = launcher_release(
            directory.path(),
            "3.0.0",
            Target::WindowsX86_64,
            b"launcher v3",
        );
        let (_, _, version_three_verified) = signed_release(&version_three, None);

        let start = Arc::new(Barrier::new(3));
        let spawn_accept = |release: VerifiedRelease| {
            let state_path = state_path.clone();
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                accept_offer(&state_path, &release)
            })
        };
        let version_two_task = spawn_accept(version_two_verified);
        let version_three_task = spawn_accept(version_three_verified);
        start.wait();
        let _ = version_two_task.join().unwrap();
        assert_eq!(
            version_three_task.join().unwrap().unwrap(),
            OfferDecision::Accepted
        );
        let (highest_remote, _) = accepted_state_summary(
            &state_path,
            ReleaseScope {
                product: Product::Launcher,
                channel: Channel::Stable,
                target: Target::WindowsX86_64,
            },
        )
        .unwrap();
        assert_eq!(highest_remote, "3.0.0");
    }
}
