//! Supported-client identity and the pre-launch extension gate.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub const CLIENT_EXECUTABLE_NAME: &str = "ffxivgame.exe";
pub const GAME_VERSION_FILE_NAME: &str = "game.ver";

/// The tuple recorded in `docs/extensions.md` for the supported client.
pub const SUPPORTED_CLIENT_SHA256: &str =
    "9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9";
pub const SUPPORTED_CLIENT_BYTE_LENGTH: u64 = 15_996_808;
pub const SUPPORTED_GAME_VERSION: &str = crate::version::FFXIV_GAME_VERSION;
pub const RUNTIME_STUB_MARKER: &str = "BAHAMUT_STUB_CLIENT_RUNTIME_V1";
pub const RETAIL_IMAGE_BASE: u64 = 0x0040_0000;
pub const RETAIL_SERVER_UTC_RVA: u32 = 0x009A_15E3;
pub const RETAIL_LOBBY_HOST_RVA: u32 = 0x00B9_0110;
pub const SERVER_UTC_PATCH_SIZE: usize = 5;
pub const LOBBY_HOST_PATCH_SIZE: usize = 0x14;
pub const RUNTIME_API_VERSION: u32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIdentity {
    pub executable_sha256: String,
    pub executable_byte_length: u64,
    pub game_version: String,
}

impl ClientIdentity {
    pub fn from_install(game_dir: &Path) -> Result<Self, IdentityReadError> {
        let executable_path = game_dir.join(CLIENT_EXECUTABLE_NAME);
        let bytes = std::fs::read(&executable_path).map_err(|source| IdentityReadError::Io {
            path: executable_path,
            source,
        })?;
        let game_version_path = game_dir.join(GAME_VERSION_FILE_NAME);
        let game_version = std::fs::read_to_string(&game_version_path).map_err(|source| {
            IdentityReadError::Io {
                path: game_version_path,
                source,
            }
        })?;

        Ok(Self {
            executable_sha256: digest_hex(&bytes),
            executable_byte_length: bytes.len() as u64,
            game_version: game_version.trim().to_owned(),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityReadError {
    #[error("could not read client identity file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityMismatch {
    #[error("client executable SHA-256 mismatch: expected {expected}, found {actual}")]
    ExecutableSha256 {
        expected: &'static str,
        actual: String,
    },
    #[error("client executable byte length mismatch: expected {expected}, found {actual}")]
    ExecutableByteLength { expected: u64, actual: u64 },
    #[error("game.ver mismatch: expected {expected}, found {actual}")]
    GameVersion {
        expected: &'static str,
        actual: String,
    },
}

/// Verify every component of the supported-client tuple in contract order.
pub fn verify_supported_client(identity: &ClientIdentity) -> Result<(), IdentityMismatch> {
    if !identity
        .executable_sha256
        .eq_ignore_ascii_case(SUPPORTED_CLIENT_SHA256)
    {
        return Err(IdentityMismatch::ExecutableSha256 {
            expected: SUPPORTED_CLIENT_SHA256,
            actual: identity.executable_sha256.clone(),
        });
    }
    if identity.executable_byte_length != SUPPORTED_CLIENT_BYTE_LENGTH {
        return Err(IdentityMismatch::ExecutableByteLength {
            expected: SUPPORTED_CLIENT_BYTE_LENGTH,
            actual: identity.executable_byte_length,
        });
    }
    if identity.game_version != SUPPORTED_GAME_VERSION {
        return Err(IdentityMismatch::GameVersion {
            expected: SUPPORTED_GAME_VERSION,
            actual: identity.game_version.clone(),
        });
    }
    Ok(())
}

pub fn gate_install(game_dir: &Path) -> Result<ClientIdentity, IdentityGateError> {
    let identity = ClientIdentity::from_install(game_dir)?;
    verify_supported_client(&identity)?;
    Ok(identity)
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityGateError {
    #[error(transparent)]
    Read(#[from] IdentityReadError),
    #[error(transparent)]
    Mismatch(#[from] IdentityMismatch),
}

fn digest_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const CPP_RUNTIME_CONTRACT: &str = include_str!("../../client/api/runtime_contract.h");

    fn cpp_literal(name: &str) -> String {
        let needle = format!(" {name}");
        let line = CPP_RUNTIME_CONTRACT
            .lines()
            .find(|line| line.contains(&needle))
            .unwrap_or_else(|| panic!("missing C++ contract constant {name}"));
        line.split_once('=')
            .unwrap_or_else(|| panic!("malformed C++ contract constant {name}"))
            .1
            .trim()
            .trim_end_matches(';')
            .trim()
            .to_owned()
    }

    fn cpp_string(name: &str) -> String {
        let literal = cpp_literal(name);
        literal
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or_else(|| panic!("C++ contract constant {name} is not a string"))
            .to_owned()
    }

    fn cpp_number(name: &str) -> u64 {
        let literal = cpp_literal(name);
        let digits = literal.trim_end_matches(['u', 'U', 'l', 'L']);
        if let Some(hex) = digits.strip_prefix("0x") {
            u64::from_str_radix(hex, 16)
                .unwrap_or_else(|_| panic!("invalid hexadecimal C++ contract constant {name}"))
        } else {
            digits
                .parse()
                .unwrap_or_else(|_| panic!("invalid decimal C++ contract constant {name}"))
        }
    }

    fn supported_identity() -> ClientIdentity {
        ClientIdentity {
            executable_sha256: SUPPORTED_CLIENT_SHA256.to_owned(),
            executable_byte_length: SUPPORTED_CLIENT_BYTE_LENGTH,
            game_version: SUPPORTED_GAME_VERSION.to_owned(),
        }
    }

    #[test]
    fn matching_identity_is_verified() {
        assert!(verify_supported_client(&supported_identity()).is_ok());
    }

    #[test]
    fn executable_hash_mismatch_is_rejected() {
        let mut identity = supported_identity();
        identity.executable_sha256.replace_range(..1, "0");
        assert!(matches!(
            verify_supported_client(&identity),
            Err(IdentityMismatch::ExecutableSha256 { .. })
        ));
    }

    #[test]
    fn executable_length_mismatch_is_rejected() {
        let mut identity = supported_identity();
        identity.executable_byte_length += 1;
        assert!(matches!(
            verify_supported_client(&identity),
            Err(IdentityMismatch::ExecutableByteLength { .. })
        ));
    }

    #[test]
    fn game_version_mismatch_is_rejected() {
        let mut identity = supported_identity();
        identity.game_version.push('x');
        assert!(matches!(
            verify_supported_client(&identity),
            Err(IdentityMismatch::GameVersion { .. })
        ));
    }

    #[test]
    fn identity_reads_hash_length_and_trimmed_game_version() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = b"client";
        std::fs::write(temp.path().join(CLIENT_EXECUTABLE_NAME), bytes).unwrap();
        std::fs::write(
            temp.path().join(GAME_VERSION_FILE_NAME),
            b"2012.09.19.0001\r\n",
        )
        .unwrap();

        let identity = ClientIdentity::from_install(temp.path()).unwrap();
        assert_eq!(identity.executable_byte_length, bytes.len() as u64);
        assert_eq!(identity.game_version, SUPPORTED_GAME_VERSION);
        assert_eq!(identity.executable_sha256.len(), 64);
    }

    #[test]
    fn runtime_contract_constants_match_across_languages() {
        assert_eq!(cpp_string("kClientSha256"), SUPPORTED_CLIENT_SHA256);
        assert_eq!(
            cpp_number("kClientByteLength"),
            SUPPORTED_CLIENT_BYTE_LENGTH
        );
        assert_eq!(cpp_string("kGameVersion"), SUPPORTED_GAME_VERSION);
        assert_eq!(cpp_string("kStubMarker"), RUNTIME_STUB_MARKER);
        assert_eq!(cpp_number("kRetailImageBase"), RETAIL_IMAGE_BASE);
        assert_eq!(
            cpp_number("kRetailServerUtcRva"),
            u64::from(RETAIL_SERVER_UTC_RVA)
        );
        assert_eq!(
            cpp_number("kRetailLobbyHostRva"),
            u64::from(RETAIL_LOBBY_HOST_RVA)
        );
        assert_eq!(
            cpp_number("kServerUtcPatchSize"),
            SERVER_UTC_PATCH_SIZE as u64
        );
        assert_eq!(
            cpp_number("kLobbyHostPatchSize"),
            LOBBY_HOST_PATCH_SIZE as u64
        );
        assert_eq!(
            cpp_number("kRuntimeApiVersion"),
            u64::from(RUNTIME_API_VERSION)
        );
    }
}
