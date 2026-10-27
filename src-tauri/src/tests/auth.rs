use bahamut_launcher::auth::client::AuthClientError;
use bahamut_launcher::auth::types::ErrorCode;

use crate::commands::auth::translate_client_error;
use crate::presentation::{AUTH_KIND_INVALID_CREDENTIALS, AUTH_KIND_RATE_LIMITED, AuthOp};

#[test]
fn auth_error_mapping_uses_exact_login_kinds_and_retry_after() {
    let invalid = translate_client_error(
        AuthClientError::Api {
            status: 401,
            code: ErrorCode::InvalidCredentials.as_str().into(),
            message: "invalid credentials".into(),
            known: Some(ErrorCode::InvalidCredentials),
            retry_after_seconds: None,
        },
        AuthOp::Login,
    );
    assert_eq!(invalid.kind, AUTH_KIND_INVALID_CREDENTIALS);
    assert!(invalid.retry_after.is_none());

    let limited = translate_client_error(
        AuthClientError::Api {
            status: 429,
            code: ErrorCode::RateLimited.as_str().into(),
            message: "too many attempts".into(),
            known: Some(ErrorCode::RateLimited),
            retry_after_seconds: Some(3),
        },
        AuthOp::Login,
    );
    assert_eq!(limited.kind, AUTH_KIND_RATE_LIMITED);
    assert_eq!(limited.retry_after, Some(3));

    let mismatched = translate_client_error(
        AuthClientError::Api {
            status: 400,
            code: ErrorCode::RateLimited.as_str().into(),
            message: "malformed rate response".into(),
            known: Some(ErrorCode::RateLimited),
            retry_after_seconds: Some(3),
        },
        AuthOp::Login,
    );
    assert_eq!(mismatched.kind, "server");
}

#[test]
fn registration_server_error_uses_stable_create_failure_kind() {
    let error = translate_client_error(
        AuthClientError::Api {
            status: 500,
            code: "server_error".to_string(),
            message: "database detail".to_string(),
            known: Some(ErrorCode::ServerError),
            retry_after_seconds: None,
        },
        AuthOp::Register,
    );

    assert_eq!(error.kind, "create-failed");
    assert!(error.message.is_none());

    let mismatched = translate_client_error(
        AuthClientError::Api {
            status: 503,
            code: "server_error".to_string(),
            message: "unexpected status".to_string(),
            known: Some(ErrorCode::ServerError),
            retry_after_seconds: None,
        },
        AuthOp::Register,
    );

    assert_eq!(mismatched.kind, "server");
}
