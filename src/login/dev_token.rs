//! Non-retail developer-token bypass for offline launch-argument testing.
//! It must not become the default user-facing login flow; production auth is in `src/auth`.

use crate::launcher::launch_args::SESSION_ID_LEN;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DevTokenError {
    #[error("session token is empty")]
    Empty,
    #[error("session token must be exactly {expected} characters (got {actual})")]
    Length { expected: usize, actual: usize },
    #[error("session token must be hex (0-9, a-f, A-F)")]
    NotHex,
}

/// Trim and validate a session token as exactly [`SESSION_ID_LEN`] ASCII hex characters for the launch argument builder.
pub fn parse_dev_token(input: &str) -> Result<String, DevTokenError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(DevTokenError::Empty);
    }
    if trimmed.len() != SESSION_ID_LEN {
        return Err(DevTokenError::Length {
            expected: SESSION_ID_LEN,
            actual: trimmed.len(),
        });
    }
    if !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(DevTokenError::NotHex);
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn valid_token_is_accepted() {
        assert_eq!(parse_dev_token(VALID).unwrap(), VALID);
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        let padded = format!("  \n  {VALID}  \t\n");
        assert_eq!(parse_dev_token(&padded).unwrap(), VALID);
    }

    #[test]
    fn empty_input_is_rejected() {
        assert_eq!(parse_dev_token("").unwrap_err(), DevTokenError::Empty);
        assert_eq!(
            parse_dev_token("   \n\t").unwrap_err(),
            DevTokenError::Empty
        );
    }

    #[test]
    fn wrong_length_is_rejected() {
        let short = &VALID[..SESSION_ID_LEN - 1];
        assert_eq!(
            parse_dev_token(short).unwrap_err(),
            DevTokenError::Length {
                expected: SESSION_ID_LEN,
                actual: SESSION_ID_LEN - 1,
            },
        );
        let long = format!("{VALID}x");
        assert_eq!(
            parse_dev_token(&long).unwrap_err(),
            DevTokenError::Length {
                expected: SESSION_ID_LEN,
                actual: SESSION_ID_LEN + 1,
            },
        );
    }

    #[test]
    fn non_hex_chars_are_rejected() {
        let mut bad = String::from(VALID);
        bad.replace_range(0..1, "z");
        assert_eq!(bad.len(), SESSION_ID_LEN);
        assert_eq!(parse_dev_token(&bad).unwrap_err(), DevTokenError::NotHex);
    }

    #[test]
    fn uppercase_hex_is_accepted() {
        let upper = VALID.to_uppercase();
        assert_eq!(parse_dev_token(&upper).unwrap(), upper);
    }
}
