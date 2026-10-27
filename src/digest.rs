//! SHA-256 helpers for lowercase hexadecimal content identities.

use sha2::{Digest, Sha256};
use std::io::{self, Read};

const READ_CHUNK: usize = 64 * 1024;

/// Lowercase hexadecimal SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Stream `input` to its end and return its length and lowercase hexadecimal
/// SHA-256.
pub fn sha256_reader(input: &mut impl Read) -> io::Result<(u64, String)> {
    let mut digest = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = vec![0_u8; READ_CHUNK];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        length = length
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "length overflows"))?;
        digest.update(&buffer[..read]);
    }
    Ok((length, format!("{:x}", digest.finalize())))
}

/// Whether `value` is a SHA-256 digest in lowercase hexadecimal.
pub fn is_lower_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn digests_bytes_and_streams_identically() {
        assert_eq!(sha256_hex(b"abc"), ABC);
        let bytes = vec![7_u8; READ_CHUNK * 2 + 3];
        assert_eq!(
            sha256_reader(&mut bytes.as_slice()).unwrap(),
            (bytes.len() as u64, sha256_hex(&bytes))
        );
    }

    #[test]
    fn accepts_only_lowercase_sha256_hex() {
        assert!(is_lower_sha256_hex(ABC));
        assert!(!is_lower_sha256_hex(&ABC.to_ascii_uppercase()));
        assert!(!is_lower_sha256_hex(&ABC[1..]));
        assert!(!is_lower_sha256_hex(&ABC.replace('a', "g")));
    }
}
