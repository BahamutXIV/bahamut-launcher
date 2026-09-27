//! HTTPS client for the Bahamut auth surface; see `docs/auth.md`.
//! Plain HTTP is allowed only for `127.0.0.1`, `localhost`, and `::1`.
//! One request per call; retry and backoff belong above this layer.

use std::time::Duration;

use reqwest::Response;
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER, USER_AGENT};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::auth::types::{
    ApiError, ErrorCode, ErrorEnvelope, LoginRequest, LoginResponse, RegisterRequest,
    RegisterResponse,
};

const ACCOUNTS_PATH: &str = "accounts";
const SESSIONS_PATH: &str = "sessions";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const USER_AGENT_VALUE: &str = concat!("bahamut-launcher/", env!("BAHAMUT_GIT_DESCRIBE"));
const BODY_EXCERPT_LIMIT: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum AuthClientError {
    #[error("the saved session belongs to a different authentication endpoint")]
    SessionEndpointMismatch,
    /// No HTTP status was received because of a network, DNS, TLS, or timeout failure.
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),

    /// Contract error response per `docs/auth.md`; `known` maps recognized codes, unknown codes stay raw, and retry data comes only from HTTP 429's `Retry-After`.
    #[error("server returned {status}: {code} ({message})")]
    Api {
        status: u16,
        code: String,
        message: String,
        known: Option<ErrorCode>,
        retry_after_seconds: Option<u32>,
    },

    /// Completed HTTP exchange whose body is neither the success type nor [`ErrorEnvelope`].
    #[error("malformed response body: {0}")]
    Malformed(String),

    #[error("invalid api base url {url:?}: {reason}")]
    BadUrl { url: String, reason: String },

    /// Plain HTTP rejected for non-loopback hosts.
    #[error("plain HTTP is only allowed for loopback hosts, got {0:?}")]
    InsecureNonLoopback(String),
}

/// HTTPS auth client; `api_base` is normalized with a trailing slash for `/accounts` and `/sessions` resolution.
#[derive(Debug, Clone)]
pub struct AuthClient {
    http: reqwest::Client,
    api_base: url::Url,
}

/// Result of validating a retained session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionValidation {
    /// Session exists and has not expired (HTTP 200).
    Valid,
    /// Backend rejected the session (HTTP 401).
    Invalid,
    /// Validation was inconclusive; retain the session rather than force logout.
    Unknown,
}

impl AuthClient {
    /// Only a matching issuing endpoint may receive a retained credential.
    pub fn for_session(
        api_base: &str,
        issuing_endpoint: Option<&str>,
    ) -> Result<Self, AuthClientError> {
        let mut expected = issuing_endpoint
            .and_then(|value| url::Url::parse(value).ok())
            .ok_or(AuthClientError::SessionEndpointMismatch)?;
        ensure_trailing_slash(&mut expected);
        let mut current =
            url::Url::parse(api_base).map_err(|_| AuthClientError::SessionEndpointMismatch)?;
        ensure_trailing_slash(&mut current);
        if expected != current {
            return Err(AuthClientError::SessionEndpointMismatch);
        }
        Self::new(api_base)
    }

    pub fn endpoint(&self) -> &str {
        self.api_base.as_str()
    }

    pub fn new(api_base: &str) -> Result<Self, AuthClientError> {
        let mut url = url::Url::parse(api_base).map_err(|e| AuthClientError::BadUrl {
            url: api_base.to_owned(),
            reason: e.to_string(),
        })?;
        validate_scheme(&url)?;
        ensure_trailing_slash(&mut url);

        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()?;

        Ok(Self {
            http,
            api_base: url,
        })
    }

    /// POST `/accounts`; 2xx decodes the response, while non-2xx returns a contract or malformed-body error.
    pub async fn register(
        &self,
        req: &RegisterRequest,
    ) -> Result<RegisterResponse, AuthClientError> {
        self.post(ACCOUNTS_PATH, req).await
    }

    /// POST `/sessions`; follows the same success and error rules as [`AuthClient::register`].
    pub async fn login(&self, req: &LoginRequest) -> Result<LoginResponse, AuthClientError> {
        self.post(SESSIONS_PATH, req).await
    }

    /// GET `/sessions/current`; inconclusive results return [`SessionValidation::Unknown`] so transient failures do not force logout.
    pub async fn validate_session(&self, token: &str) -> SessionValidation {
        let path = format!("{SESSIONS_PATH}/current");
        let url = match self.api_base.join(&path) {
            Ok(url) => url,
            Err(_) => return SessionValidation::Unknown,
        };
        match self.http.get(url).bearer_auth(token).send().await {
            Ok(response) => match response.status() {
                StatusCode::OK => SessionValidation::Valid,
                StatusCode::UNAUTHORIZED => SessionValidation::Invalid,
                _ => SessionValidation::Unknown,
            },
            Err(_) => SessionValidation::Unknown,
        }
    }

    /// DELETE `/sessions/<session_id>`; revocation is best-effort and any 2xx response succeeds.
    pub async fn logout(&self, session_id: &str) -> Result<(), AuthClientError> {
        let path = format!("{SESSIONS_PATH}/{session_id}");
        let url = self
            .api_base
            .join(&path)
            .map_err(|e| AuthClientError::BadUrl {
                url: self.api_base.to_string(),
                reason: format!("could not append {path:?}: {e}"),
            })?;
        let response = self.http.delete(url).send().await?;
        decode_unit_response(response).await
    }

    async fn post<Req, Resp>(&self, path: &str, body: &Req) -> Result<Resp, AuthClientError>
    where
        Req: Serialize,
        Resp: DeserializeOwned,
    {
        let url = self
            .api_base
            .join(path)
            .map_err(|e| AuthClientError::BadUrl {
                url: self.api_base.to_string(),
                reason: format!("could not append {path:?}: {e}"),
            })?;
        let response = self.http.post(url).json(body).send().await?;
        decode_response(response).await
    }
}

fn validate_scheme(url: &url::Url) -> Result<(), AuthClientError> {
    match url.scheme() {
        "https" => Ok(()),
        "http" => {
            let host = url.host_str().unwrap_or("");
            if is_loopback_host(host) {
                Ok(())
            } else {
                Err(AuthClientError::InsecureNonLoopback(host.to_owned()))
            }
        }
        other => Err(AuthClientError::BadUrl {
            url: url.to_string(),
            reason: format!("unsupported scheme {other:?} (expected http or https)"),
        }),
    }
}

fn ensure_trailing_slash(url: &mut url::Url) {
    if !url.path().ends_with('/') {
        let with_slash = format!("{}/", url.path());
        url.set_path(&with_slash);
    }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

async fn decode_response<Resp: DeserializeOwned>(
    response: Response,
) -> Result<Resp, AuthClientError> {
    let status = response.status();
    if status.is_success() {
        let body = response.text().await?;
        return serde_json::from_str::<Resp>(&body).map_err(|e| {
            AuthClientError::Malformed(format!(
                "expected success body to deserialize: {e}; body excerpt: {}",
                truncate(&body, BODY_EXCERPT_LIMIT)
            ))
        });
    }
    Err(decode_error(response).await)
}

async fn decode_unit_response(response: Response) -> Result<(), AuthClientError> {
    if response.status().is_success() {
        return Ok(());
    }
    Err(decode_error(response).await)
}

async fn decode_error(response: Response) -> AuthClientError {
    let status = response.status();
    let retry_after = parse_retry_after(response.headers(), status);
    let body = match response.text().await {
        Ok(body) => body,
        Err(e) => return AuthClientError::Transport(e),
    };
    match serde_json::from_str::<ErrorEnvelope>(&body) {
        Ok(envelope) => {
            let ApiError { code, message } = envelope.error;
            let known = ErrorCode::parse(&code);
            AuthClientError::Api {
                status: status.as_u16(),
                code,
                message,
                known,
                retry_after_seconds: retry_after,
            }
        }
        Err(e) => AuthClientError::Malformed(format!(
            "non-2xx response did not match error envelope: status={}, parse error={e}; \
             body excerpt: {}",
            status.as_u16(),
            truncate(&body, BODY_EXCERPT_LIMIT),
        )),
    }
}

fn parse_retry_after(headers: &HeaderMap, status: StatusCode) -> Option<u32> {
    if status != StatusCode::TOO_MANY_REQUESTS {
        return None;
    }
    headers
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
}

fn truncate(body: &str, max: usize) -> &str {
    if body.len() <= max {
        body
    } else {
        let mut end = max;
        while end > 0 && !body.is_char_boundary(end) {
            end -= 1;
        }
        &body[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn server_api_base(server: &MockServer) -> String {
        format!("{}/api/v1", server.base_url())
    }

    fn sample_register_request() -> RegisterRequest {
        RegisterRequest {
            username: "asaved".into(),
            password: zeroize::Zeroizing::new("passphrase".into()),
        }
    }

    fn sample_login_request() -> LoginRequest {
        LoginRequest {
            username: "asaved".into(),
            password: zeroize::Zeroizing::new("passphrase".into()),
        }
    }

    #[test]
    fn new_rejects_invalid_url() {
        let err = AuthClient::new("not a url").unwrap_err();
        assert!(matches!(err, AuthClientError::BadUrl { .. }), "got {err:?}");
    }

    #[test]
    fn new_rejects_non_loopback_http() {
        let err = AuthClient::new("http://example.com/api/v1").unwrap_err();
        match err {
            AuthClientError::InsecureNonLoopback(host) => assert_eq!(host, "example.com"),
            other => panic!("expected InsecureNonLoopback, got {other:?}"),
        }
    }

    #[test]
    fn new_accepts_loopback_http() {
        AuthClient::new("http://127.0.0.1:8080/api/v1").unwrap();
        AuthClient::new("http://localhost:8080/api/v1").unwrap();
        AuthClient::new("http://[::1]:8080/api/v1").unwrap();
    }

    #[test]
    fn new_rejects_non_loopback_ipv6_http() {
        let error = AuthClient::new("http://[2001:db8::1]:8080/api/v1").unwrap_err();
        assert!(matches!(error, AuthClientError::InsecureNonLoopback(_)));
    }

    #[test]
    fn new_accepts_https_non_loopback() {
        AuthClient::new("https://bahamut.example/api/v1").unwrap();
    }

    #[test]
    fn new_rejects_unsupported_scheme() {
        let err = AuthClient::new("ftp://example.com/api/v1").unwrap_err();
        assert!(matches!(err, AuthClientError::BadUrl { .. }), "got {err:?}");
    }

    #[test]
    fn new_normalises_to_trailing_slash() {
        let client = AuthClient::new("http://127.0.0.1:8080/api/v1").unwrap();
        assert!(
            client.api_base.path().ends_with('/'),
            "api_base path should be normalised to end with /, got {:?}",
            client.api_base.path()
        );
    }

    #[tokio::test]
    async fn register_success_returns_decoded_body() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/accounts");
            then.status(201)
                .header("content-type", "application/json")
                .body(r#"{"username":"asaved","created_at":"created-at-fixture"}"#);
        });
        let client = AuthClient::new(&server_api_base(&server)).unwrap();
        let resp = client.register(&sample_register_request()).await.unwrap();
        mock.assert();
        assert_eq!(resp.username, "asaved");
        assert_eq!(resp.created_at, "created-at-fixture");
    }

    #[tokio::test]
    async fn auth_posts_reject_redirects_without_reaching_receiver() {
        let origin = MockServer::start();
        let receiver = MockServer::start();
        let register_receiver = receiver.mock(|when, then| {
            when.method(POST).path("/redirected/accounts");
            then.status(201)
                .header("content-type", "application/json")
                .body(r#"{"username":"asaved","created_at":"created-at-fixture"}"#);
        });
        let login_receiver = receiver.mock(|when, then| {
            when.method(POST).path("/redirected/sessions");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"session_id":"0123456789abcdef0123456789abcdef0123456789abcdef01234567","username":"asaved","expires_at":"expires-at-fixture"}"#,
                );
        });
        let register_origin = origin.mock(|when, then| {
            when.method(POST).path("/api/v1/accounts");
            then.status(307)
                .header("location", receiver.url("/redirected/accounts"));
        });
        let login_origin = origin.mock(|when, then| {
            when.method(POST).path("/api/v1/sessions");
            then.status(307)
                .header("location", receiver.url("/redirected/sessions"));
        });
        let client = AuthClient::new(&server_api_base(&origin)).unwrap();

        let register_err = client
            .register(&sample_register_request())
            .await
            .unwrap_err();
        let login_err = client.login(&sample_login_request()).await.unwrap_err();

        register_origin.assert();
        login_origin.assert();
        register_receiver.assert_hits(0);
        login_receiver.assert_hits(0);
        assert!(
            matches!(register_err, AuthClientError::Malformed(message) if message.contains("status=307"))
        );
        assert!(
            matches!(login_err, AuthClientError::Malformed(message) if message.contains("status=307"))
        );
    }

    #[tokio::test]
    async fn register_username_taken_surfaces_as_known_code() {
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/accounts");
            then.status(409)
                .header("content-type", "application/json")
                .body(r#"{"error":{"code":"username_taken","message":"already in use"}}"#);
        });
        let client = AuthClient::new(&server_api_base(&server)).unwrap();
        let err = client
            .register(&sample_register_request())
            .await
            .unwrap_err();
        match err {
            AuthClientError::Api {
                status,
                known,
                retry_after_seconds,
                ..
            } => {
                assert_eq!(status, 409);
                assert_eq!(known, Some(ErrorCode::UsernameTaken));
                assert_eq!(retry_after_seconds, None);
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn register_validation_error_passes_through_message() {
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/accounts");
            then.status(400)
                .header("content-type", "application/json")
                .body(r#"{"error":{"code":"validation_error","message":"password too short"}}"#);
        });
        let client = AuthClient::new(&server_api_base(&server)).unwrap();
        let err = client
            .register(&sample_register_request())
            .await
            .unwrap_err();
        match err {
            AuthClientError::Api {
                status,
                known,
                message,
                ..
            } => {
                assert_eq!(status, 400);
                assert_eq!(known, Some(ErrorCode::ValidationError));
                assert!(
                    message.contains("password"),
                    "message should pass through verbatim, got {message:?}"
                );
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn login_success_returns_session_id_and_expiry() {
        let server = MockServer::start();
        let session = "0123456789abcdef0123456789abcdef0123456789abcdef01234567";
        let body = format!(
            "{{\"session_id\":\"{session}\",\"username\":\"asaved\",\
             \"expires_at\":\"expires-at-fixture\"}}"
        );
        let _mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/sessions");
            then.status(200)
                .header("content-type", "application/json")
                .body(body);
        });
        let client = AuthClient::new(&server_api_base(&server)).unwrap();
        let resp = client.login(&sample_login_request()).await.unwrap();
        assert_eq!(resp.session_id.len(), 56);
        assert!(resp.session_id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(resp.username, "asaved");
        assert_eq!(resp.expires_at, "expires-at-fixture");
    }

    #[tokio::test]
    async fn login_invalid_credentials_surfaces_as_known_code() {
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/sessions");
            then.status(401)
                .header("content-type", "application/json")
                .body(r#"{"error":{"code":"invalid_credentials","message":"bad username or password"}}"#);
        });
        let client = AuthClient::new(&server_api_base(&server)).unwrap();
        let err = client.login(&sample_login_request()).await.unwrap_err();
        match err {
            AuthClientError::Api { status, known, .. } => {
                assert_eq!(status, 401);
                assert_eq!(known, Some(ErrorCode::InvalidCredentials));
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn login_rate_limited_carries_retry_after_seconds() {
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/sessions");
            then.status(429)
                .header("content-type", "application/json")
                .header("retry-after", "30")
                .body(r#"{"error":{"code":"rate_limited","message":"slow down"}}"#);
        });
        let client = AuthClient::new(&server_api_base(&server)).unwrap();
        let err = client.login(&sample_login_request()).await.unwrap_err();
        match err {
            AuthClientError::Api {
                status,
                known,
                retry_after_seconds,
                ..
            } => {
                assert_eq!(status, 429);
                assert_eq!(known, Some(ErrorCode::RateLimited));
                assert_eq!(retry_after_seconds, Some(30));
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn malformed_success_body_surfaces_as_malformed() {
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/sessions");
            then.status(200)
                .header("content-type", "application/json")
                .body("not json at all");
        });
        let client = AuthClient::new(&server_api_base(&server)).unwrap();
        let err = client.login(&sample_login_request()).await.unwrap_err();
        assert!(matches!(err, AuthClientError::Malformed(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn unknown_error_code_preserves_raw_string() {
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/accounts");
            then.status(400)
                .header("content-type", "application/json")
                .body(r#"{"error":{"code":"future_thing","message":"a code from later"}}"#);
        });
        let client = AuthClient::new(&server_api_base(&server)).unwrap();
        let err = client
            .register(&sample_register_request())
            .await
            .unwrap_err();
        match err {
            AuthClientError::Api {
                status,
                code,
                known,
                ..
            } => {
                assert_eq!(status, 400);
                assert_eq!(code, "future_thing");
                assert_eq!(known, None);
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_2xx_with_non_envelope_body_is_malformed() {
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/sessions");
            then.status(500).body("not even close to json");
        });
        let client = AuthClient::new(&server_api_base(&server)).unwrap();
        let err = client.login(&sample_login_request()).await.unwrap_err();
        assert!(matches!(err, AuthClientError::Malformed(_)), "got {err:?}");
    }

    const SAMPLE_SESSION_ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn retained_endpoint_normalizes_host_port_and_path() {
        let client = AuthClient::for_session(
            "https://EXAMPLE.test:443/api/v1",
            Some("https://example.test/api/v1/"),
        )
        .unwrap();
        assert_eq!(client.endpoint(), "https://example.test/api/v1/");
        for endpoint in [
            "https://other.test/api/v1/",
            "https://example.test:444/api/v1/",
            "http://example.test/api/v1/",
            "https://example.test/other/",
        ] {
            assert!(matches!(
                AuthClient::for_session(endpoint, Some(client.endpoint())),
                Err(AuthClientError::SessionEndpointMismatch)
            ));
        }
        assert!(matches!(
            AuthClient::for_session(client.endpoint(), None),
            Err(AuthClientError::SessionEndpointMismatch)
        ));
    }

    #[tokio::test]
    async fn retained_token_never_reaches_a_replacement_endpoint() {
        let issuing = MockServer::start();
        let replacement = MockServer::start();
        let login = issuing.mock(|when, then| {
            when.method(POST)
                .path("/api/v1/sessions")
                .body(r#"{"username":"asaved","password":"passphrase"}"#);
            then.status(200)
                .header("content-type", "application/json")
                .body(format!(
                    r#"{{"session_id":"{SAMPLE_SESSION_ID}","username":"asaved","expires_at":"expires-at-fixture"}}"#
                ));
        });
        let requests = replacement.mock(|when, then| {
            when.any_request();
            then.status(200);
        });
        let issuer = AuthClient::new(&server_api_base(&issuing)).unwrap();
        let response = issuer.login(&sample_login_request()).await.unwrap();
        let issued_endpoint = issuer.endpoint().to_owned();
        assert_eq!(response.session_id, SAMPLE_SESSION_ID);
        login.assert_hits(1);
        for revoke in [false, true] {
            let result =
                AuthClient::for_session(&server_api_base(&replacement), Some(&issued_endpoint));
            if let Ok(client) = result {
                if revoke {
                    let _ = client.logout(SAMPLE_SESSION_ID).await;
                } else {
                    let _ = client.validate_session(SAMPLE_SESSION_ID).await;
                }
                panic!("replacement endpoint admitted an old credential");
            }
        }
        requests.assert_hits(0);
    }

    #[tokio::test]
    async fn matching_endpoint_can_validate_and_revoke_a_retained_token() {
        let server = MockServer::start();
        let validation = server.mock(|when, then| {
            when.method(GET)
                .path("/api/v1/sessions/current")
                .header("authorization", format!("Bearer {SAMPLE_SESSION_ID}"));
            then.status(200);
        });
        let revocation = server.mock(|when, then| {
            when.method(DELETE)
                .path(format!("/api/v1/sessions/{SAMPLE_SESSION_ID}"));
            then.status(204);
        });
        let issuing = AuthClient::new(&server_api_base(&server)).unwrap();
        let retained =
            AuthClient::for_session(&server_api_base(&server), Some(issuing.endpoint())).unwrap();
        assert_eq!(
            retained.validate_session(SAMPLE_SESSION_ID).await,
            SessionValidation::Valid
        );
        retained.logout(SAMPLE_SESSION_ID).await.unwrap();
        validation.assert_hits(1);
        revocation.assert_hits(1);
    }

    #[tokio::test]
    async fn logout_success_with_empty_204_body_returns_ok() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(DELETE)
                .path(format!("/api/v1/sessions/{SAMPLE_SESSION_ID}"));
            then.status(204);
        });
        let client = AuthClient::new(&server_api_base(&server)).unwrap();
        client.logout(SAMPLE_SESSION_ID).await.unwrap();
        mock.assert();
    }

    #[tokio::test]
    async fn logout_404_surfaces_as_api_error_caller_can_swallow() {
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(DELETE)
                .path(format!("/api/v1/sessions/{SAMPLE_SESSION_ID}"));
            then.status(404)
                .header("content-type", "application/json")
                .body(r#"{"error":{"code":"not_found","message":"endpoint not implemented yet"}}"#);
        });
        let client = AuthClient::new(&server_api_base(&server)).unwrap();
        let err = client.logout(SAMPLE_SESSION_ID).await.unwrap_err();
        match err {
            AuthClientError::Api { status, code, .. } => {
                assert_eq!(status, 404);
                assert_eq!(code, "not_found");
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }
}
