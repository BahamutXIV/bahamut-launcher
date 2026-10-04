//! Build the encrypted command-line argument that the FFXIV 1.23b client
//! receives after the executable path.
//!
//! Envelope:
//!
//! ```text
//! sqex0002<URL-safe base64( Blowfish-LE-ECB encrypted plaintext )>!////
//! ```
//!
//! Plaintext (NUL-terminated, with the literal leading space and the
//! spaces around each `=` preserved exactly as the original launcher
//! emitted them):
//!
//! ```text
//!  T =<tick> /LANG =en-us /REGION =2 /SERVER_UTC =1356916742 /SESSION_ID =<session_id>\0
//! ```
//!
//! Blowfish key is the lowercase eight-character hex of `tick & !0xFFFF`,
//! used as an 8-byte ASCII key. Only complete 8-byte blocks are
//! encrypted; trailing bytes pass through unchanged. The encrypted buffer
//! is base64-encoded with `+` -> `-` and `/` -> `_`.
//!
//! See `docs/handshake.md` for the implemented contract and its retail limits.

use base64::Engine as _;
use blowfish::Blowfish;
use byteorder::LE;
use cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};

/// The 56-character Bahamut session-id contract from `docs/handshake.md`.
pub const SESSION_ID_LEN: usize = 56;

/// The fixed `SERVER_UTC` value baked into every command-line plaintext.
///
/// Note: the encoded launch argument uses the decimal value `1356916742`,
/// while the separate PE `SERVER_UTC` immediate-load patch writes
/// the bytes `B8 12 E8 E0 50`, which decode to the
/// little-endian `mov eax, 0x50E0E812`. These are kept as two separate
/// facts per `docs/handshake.md` and must not be collapsed
/// without a client-binary recheck.
pub const SERVER_UTC_PLAINTEXT: u32 = 1_356_916_742;

const ENVELOPE_PREFIX: &str = "sqex0002";
const ENVELOPE_SUFFIX: &str = "!////";
const BLOWFISH_BLOCK: usize = 8;

/// Launch-argument errors; only session-id length is checked here, after [`crate::login::dev_token`] validation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LaunchArgsError {
    #[error("session id must be exactly {expected} characters (got {actual})")]
    SessionIdLength { expected: usize, actual: usize },
}

/// A built launch argument and the legacy plaintext used to produce it.
#[derive(Debug, Clone)]
pub struct LaunchArguments {
    pub encoded_argument: String,
    pub tick_count: u32,
    /// NUL-terminated legacy plaintext fed into Blowfish.
    /// Test-inspection only; the launch path uses encoded_argument.
    pub plaintext: Vec<u8>,
}

/// Build the encrypted command-line argument; `tick_count` must match the client's `GetTickCount` clock.
pub fn build_launch_argument(
    session_id: &str,
    tick_count: u32,
) -> Result<LaunchArguments, LaunchArgsError> {
    if session_id.len() != SESSION_ID_LEN {
        return Err(LaunchArgsError::SessionIdLength {
            expected: SESSION_ID_LEN,
            actual: session_id.len(),
        });
    }

    let plaintext = format_plaintext(session_id, tick_count);

    let mut ciphertext = plaintext.clone();
    let key = derive_key(tick_count);
    encrypt_in_place(&mut ciphertext, key.as_bytes());

    let encoded = base64::engine::general_purpose::URL_SAFE.encode(&ciphertext);
    let encoded_argument = format!("{ENVELOPE_PREFIX}{encoded}{ENVELOPE_SUFFIX}");

    Ok(LaunchArguments {
        encoded_argument,
        tick_count,
        plaintext,
    })
}

/// Derive the lowercase 8-byte ASCII Blowfish key from `tick & !0xFFFF`.
pub fn derive_key(tick: u32) -> String {
    format!("{:08x}", tick & !0xFFFFu32)
}

fn format_plaintext(session_id: &str, tick: u32) -> Vec<u8> {
    let s = format!(
        " T ={tick} /LANG =en-us /REGION =2 /SERVER_UTC ={server_utc} /SESSION_ID ={session_id}",
        server_utc = SERVER_UTC_PLAINTEXT,
    );
    let mut bytes = s.into_bytes();
    bytes.push(0u8);
    bytes
}

fn encrypt_in_place(buffer: &mut [u8], key: &[u8]) {
    let cipher = Blowfish::<LE>::new_from_slice(key)
        .expect("an 8-byte hex key is within the Blowfish 4..=56 byte key range");
    // Encrypt only complete 8-byte blocks; leave any trailing partial block unchanged.
    for block in buffer.chunks_exact_mut(BLOWFISH_BLOCK) {
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SESSION_ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef01234567";

    const FIXED_TICK: u32 = 0x1234_5678;

    #[test]
    fn valid_session_id_is_accepted() {
        let args = build_launch_argument(VALID_SESSION_ID, FIXED_TICK).expect("valid input");
        assert_eq!(args.tick_count, FIXED_TICK);
    }

    #[test]
    fn short_session_id_is_rejected() {
        let short = &VALID_SESSION_ID[..SESSION_ID_LEN - 1];
        let err = build_launch_argument(short, FIXED_TICK).unwrap_err();
        assert_eq!(
            err,
            LaunchArgsError::SessionIdLength {
                expected: SESSION_ID_LEN,
                actual: SESSION_ID_LEN - 1,
            }
        );
    }

    #[test]
    fn long_session_id_is_rejected() {
        let long = format!("{VALID_SESSION_ID}x");
        let err = build_launch_argument(&long, FIXED_TICK).unwrap_err();
        assert_eq!(
            err,
            LaunchArgsError::SessionIdLength {
                expected: SESSION_ID_LEN,
                actual: SESSION_ID_LEN + 1,
            }
        );
    }

    #[test]
    fn empty_session_id_is_rejected() {
        let err = build_launch_argument("", FIXED_TICK).unwrap_err();
        assert_eq!(
            err,
            LaunchArgsError::SessionIdLength {
                expected: SESSION_ID_LEN,
                actual: 0,
            }
        );
    }

    #[test]
    fn argument_has_legacy_prefix_and_suffix() {
        let args = build_launch_argument(VALID_SESSION_ID, FIXED_TICK).unwrap();
        assert!(
            args.encoded_argument.starts_with(ENVELOPE_PREFIX),
            "missing sqex0002 prefix: {}",
            args.encoded_argument
        );
        assert!(
            args.encoded_argument.ends_with(ENVELOPE_SUFFIX),
            "missing !//// suffix: {}",
            args.encoded_argument
        );
    }

    #[test]
    fn encoded_body_is_url_safe() {
        let args = build_launch_argument(VALID_SESSION_ID, FIXED_TICK).unwrap();
        let body = args
            .encoded_argument
            .strip_prefix(ENVELOPE_PREFIX)
            .and_then(|s| s.strip_suffix(ENVELOPE_SUFFIX))
            .expect("envelope wrapping verified above");
        assert!(
            !body.contains('+') && !body.contains('/'),
            "body contains non-url-safe base64 characters: {body}",
        );
    }

    #[test]
    fn output_is_deterministic_for_fixed_tick() {
        let a = build_launch_argument(VALID_SESSION_ID, FIXED_TICK).unwrap();
        let b = build_launch_argument(VALID_SESSION_ID, FIXED_TICK).unwrap();
        assert_eq!(a.encoded_argument, b.encoded_argument);
        assert_eq!(a.plaintext, b.plaintext);
    }

    #[test]
    fn plaintext_matches_legacy_envelope() {
        let args = build_launch_argument(VALID_SESSION_ID, FIXED_TICK).unwrap();
        let last = args.plaintext.last().copied();
        assert_eq!(last, Some(0u8), "plaintext must be NUL-terminated");
        let without_nul = &args.plaintext[..args.plaintext.len() - 1];
        let s = std::str::from_utf8(without_nul).expect("plaintext is ASCII");
        let expected = format!(
            " T ={tick} /LANG =en-us /REGION =2 /SERVER_UTC =1356916742 /SESSION_ID ={sid}",
            tick = FIXED_TICK,
            sid = VALID_SESSION_ID,
        );
        assert_eq!(s, expected);
    }

    #[test]
    fn key_masks_low_16_bits() {
        assert_eq!(derive_key(0x1234_5678), "12340000");
        assert_eq!(derive_key(0x1234_FFFF), "12340000");
        assert_eq!(derive_key(0xDEAD_BEEF), "dead0000");
        assert_eq!(derive_key(0), "00000000");
    }

    #[test]
    fn ticks_sharing_a_key_window_produce_distinct_plaintexts() {
        let a = build_launch_argument(VALID_SESSION_ID, 0x1234_0000).unwrap();
        let b = build_launch_argument(VALID_SESSION_ID, 0x1234_FFFF).unwrap();
        assert_eq!(derive_key(a.tick_count), derive_key(b.tick_count));
        assert_ne!(a.encoded_argument, b.encoded_argument);
    }
}
