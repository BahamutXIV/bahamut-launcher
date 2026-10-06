//! JSON types for Bahamut HTTP auth, locked to `docs/auth.md`.
//! `created_at` and `expires_at` are RFC 3339 UTC strings on the wire.

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// `POST /api/v1/accounts` request; `password` is zeroized on drop, but the serialized reqwest body is outside this crate's control.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegisterRequest {
    pub username: String,
    pub password: Zeroizing<String>,
}

/// `POST /api/v1/accounts` success body (`201 Created`); it has no session token, so registration must be followed by login (see `docs/auth.md`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegisterResponse {
    pub username: String,
    pub created_at: String,
}

/// `POST /api/v1/sessions` request; password handling matches [`RegisterRequest`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoginRequest {
    pub username: String,
    pub password: Zeroizing<String>,
}

/// `POST /api/v1/sessions` success body (`200 OK`); `session_id` is 56 lowercase hex characters per `docs/handshake.md`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoginResponse {
    pub session_id: String,
    pub username: String,
    pub expires_at: String,
}

/// Non-2xx auth response body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorEnvelope {
    pub error: ApiError,
}

/// Error payload; unknown `code` values remain raw for forward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

impl ApiError {
    /// Map codes from `docs/auth.md` to variants; unknown codes return `None`.
    pub fn known_code(&self) -> Option<ErrorCode> {
        ErrorCode::parse(&self.code)
    }
}

/// Return whether a login response token matches the auth contract's 56 lowercase hex characters.
pub fn is_contract_session_id(value: &str) -> bool {
    value.len() == crate::launcher::launch_args::SESSION_ID_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Error codes locked in `docs/auth.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    ValidationError,
    UsernameTaken,
    InvalidCredentials,
    RateLimited,
    ServerError,
}

impl ErrorCode {
    /// Return the wire string for this code.
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::ValidationError => "validation_error",
            ErrorCode::UsernameTaken => "username_taken",
            ErrorCode::InvalidCredentials => "invalid_credentials",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::ServerError => "server_error",
        }
    }

    /// Parse a wire string, returning `None` for unknown codes.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "validation_error" => ErrorCode::ValidationError,
            "username_taken" => ErrorCode::UsernameTaken,
            "invalid_credentials" => ErrorCode::InvalidCredentials,
            "rate_limited" => ErrorCode::RateLimited,
            "server_error" => ErrorCode::ServerError,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_request_round_trips() {
        let req = RegisterRequest {
            username: "asaved-as-entered".into(),
            password: Zeroizing::new("hunter22-passphrase".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: RegisterRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn register_request_decodes_contract_example() {
        let json = r#"{"username":"alice","password":"secretpw"}"#;
        let req: RegisterRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.username, "alice");
        assert_eq!(req.password.as_str(), "secretpw");
    }

    #[test]
    fn register_response_decodes_wire_fields() {
        let json = r#"{
            "username": "asaved-as-entered",
            "created_at": "created-at-fixture"
        }"#;
        let resp: RegisterResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.username, "asaved-as-entered");
        assert_eq!(resp.created_at, "created-at-fixture");
    }

    #[test]
    fn register_response_has_no_session_field() {
        let json = serde_json::to_value(RegisterResponse {
            username: "alice".into(),
            created_at: "created-at-fixture".into(),
        })
        .unwrap();
        let obj = json.as_object().unwrap();
        assert!(obj.contains_key("username"));
        assert!(obj.contains_key("created_at"));
        assert!(
            !obj.contains_key("session_id"),
            "registration response must not carry a session token, \
             see docs/auth.md security rationale"
        );
    }

    #[test]
    fn login_request_round_trips() {
        let req = LoginRequest {
            username: "alice".into(),
            password: Zeroizing::new("pw".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: LoginRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn login_response_decodes_wire_fields() {
        let json = r#"{
            "session_id": "0123456789abcdef0123456789abcdef0123456789abcdef01234567",
            "username": "asaved-as-entered",
            "expires_at": "expires-at-fixture"
        }"#;
        let resp: LoginResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.session_id.len(), 56);
        assert!(resp.session_id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(resp.expires_at, "expires-at-fixture");
    }

    #[test]
    fn contract_session_id_rejects_wrong_shape() {
        let valid = "0123456789abcdef0123456789abcdef0123456789abcdef01234567";
        assert!(is_contract_session_id(valid));
        assert!(!is_contract_session_id(&valid.to_uppercase()));
        assert!(!is_contract_session_id(&valid[..valid.len() - 1]));
        assert!(!is_contract_session_id(&format!(
            "{}g",
            &valid[..valid.len() - 1]
        )));
    }

    #[test]
    fn error_envelope_decodes_contract_example() {
        let json = r#"{
            "error": {
                "code": "username_taken",
                "message": "Username is already registered."
            }
        }"#;
        let env: ErrorEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(env.error.code, "username_taken");
        assert_eq!(env.error.known_code(), Some(ErrorCode::UsernameTaken));
        assert_eq!(env.error.message, "Username is already registered.");
    }

    #[test]
    fn error_code_as_str_matches_contract_table() {
        assert_eq!(ErrorCode::ValidationError.as_str(), "validation_error");
        assert_eq!(ErrorCode::UsernameTaken.as_str(), "username_taken");
        assert_eq!(
            ErrorCode::InvalidCredentials.as_str(),
            "invalid_credentials"
        );
        assert_eq!(ErrorCode::RateLimited.as_str(), "rate_limited");
        assert_eq!(ErrorCode::ServerError.as_str(), "server_error");
    }

    #[test]
    fn error_code_parse_round_trips_every_variant() {
        for code in [
            ErrorCode::ValidationError,
            ErrorCode::UsernameTaken,
            ErrorCode::InvalidCredentials,
            ErrorCode::RateLimited,
            ErrorCode::ServerError,
        ] {
            assert_eq!(ErrorCode::parse(code.as_str()), Some(code));
        }
    }

    #[test]
    fn error_code_parse_rejects_unknown_strings() {
        assert!(ErrorCode::parse("future_code").is_none());
        assert!(ErrorCode::parse("").is_none());
        assert!(ErrorCode::parse("ValidationError").is_none());
    }

    #[test]
    fn api_error_with_unknown_code_decodes_gracefully() {
        let json = r#"{"code":"future_code","message":"some new server thing"}"#;
        let err: ApiError = serde_json::from_str(json).unwrap();
        assert_eq!(err.code, "future_code");
        assert_eq!(err.known_code(), None);
    }

    #[test]
    fn requests_ignore_unknown_fields() {
        let json = r#"{
            "username": "alice",
            "password": "pw",
            "future_field": "should not break parse"
        }"#;
        let req: RegisterRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.username, "alice");
        assert_eq!(req.password.as_str(), "pw");
    }
}
