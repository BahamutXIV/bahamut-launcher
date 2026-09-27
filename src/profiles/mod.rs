//! Typed server profiles shared by the portable INI owner and runtime routing.

use serde::{Deserialize, Serialize};

/// Embedded server-list fallback for fresh INI files.
pub(crate) const DEFAULT_SERVERS: &str = include_str!("default_servers.toml");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerProfile {
    pub display_name: String,
    pub host: String,
    #[serde(default = "default_auth_port")]
    pub auth_port: u16,
    /// Validated for profile integrity; the current launch patch writes only
    /// the lobby host.
    #[serde(default = "default_lobby_port")]
    pub lobby_port: u16,
    pub use_https: bool,
}

/// Embedded default server-list shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServersFile {
    #[serde(default)]
    pub server: Vec<ServerProfile>,
}

fn default_auth_port() -> u16 {
    8080
}

fn default_lobby_port() -> u16 {
    54994
}

impl ServerProfile {
    /// Build the auth API base for `/accounts` and `/sessions`.
    pub fn api_base(&self) -> String {
        let scheme = if self.use_https { "https" } else { "http" };
        format!("{scheme}://{}:{}/api/v1", self.host, self.auth_port)
    }

    /// Lobby host embedded in the game image; the stock protocol supplies the port.
    pub fn lobby_address(&self) -> &str {
        &self.host
    }

    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.display_name.trim().is_empty() {
            return Err(ProfileError::Validation {
                field: "display_name",
                message: "must not be empty".into(),
            });
        }
        validate_host(&self.host)?;
        if self.auth_port == 0 {
            return Err(ProfileError::Validation {
                field: "auth_port",
                message: "port 0 is not valid; use 1-65535".into(),
            });
        }
        if self.lobby_port == 0 {
            return Err(ProfileError::Validation {
                field: "lobby_port",
                message: "port 0 is not valid; use 1-65535".into(),
            });
        }
        Ok(())
    }
}

fn validate_host(host: &str) -> Result<(), ProfileError> {
    let trimmed = host.trim();
    if trimmed.is_empty() {
        return Err(ProfileError::Validation {
            field: "host",
            message: "must not be empty".into(),
        });
    }
    // A full URL parser normalizes away explicit default ports and empty userinfo.
    url::Host::parse(trimmed).map_err(|_| ProfileError::Validation {
        field: "host",
        message: format!(
            "must be a bare hostname or IP with no port, credentials, path, query, or fragment: {trimmed:?}"
        ),
    })?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("invalid {field}: {message}")]
    Validation {
        field: &'static str,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ServerProfile {
        ServerProfile {
            display_name: "Bahamut Local".into(),
            host: "127.0.0.1".into(),
            auth_port: 8080,
            lobby_port: 54994,
            use_https: false,
        }
    }

    #[test]
    fn embedded_defaults_list_bahamut_then_local() {
        let parsed: ServersFile = toml::from_str(DEFAULT_SERVERS).unwrap();
        assert_eq!(parsed.server.len(), 2);
        assert_eq!(parsed.server[0].display_name, "Bahamut");
        assert_eq!(parsed.server[0].host, "bahamut.stegall.me");
        assert_eq!(parsed.server[1].display_name, "Bahamut Local");
        assert_eq!(parsed.server[1].host, "127.0.0.1");
    }

    #[test]
    fn api_and_lobby_addresses_use_explicit_profile_fields() {
        let profile = sample();
        assert_eq!(profile.api_base(), "http://127.0.0.1:8080/api/v1");
        assert_eq!(profile.lobby_address(), "127.0.0.1");
    }

    #[test]
    fn validates_required_fields_and_ports() {
        let mut profile = sample();
        profile.display_name.clear();
        assert!(matches!(
            profile.validate(),
            Err(ProfileError::Validation {
                field: "display_name",
                ..
            })
        ));
        profile = sample();
        profile.auth_port = 0;
        assert!(matches!(
            profile.validate(),
            Err(ProfileError::Validation {
                field: "auth_port",
                ..
            })
        ));
        profile = sample();
        profile.lobby_port = 0;
        assert!(matches!(
            profile.validate(),
            Err(ProfileError::Validation {
                field: "lobby_port",
                ..
            })
        ));
    }

    #[test]
    fn rejects_non_bare_hosts() {
        for host in [
            "",
            "127.0.0.1:8080",
            "127.0.0.1:80",
            "example.test:80",
            "example.test:",
            "[::1]:80",
            "[::1]:",
            "user@example.test",
            "@example.test",
            "example.test/path",
            "example.test?mode=debug",
            "example.test#fragment",
        ] {
            let mut profile = sample();
            profile.host = host.into();
            assert!(
                matches!(
                    profile.validate(),
                    Err(ProfileError::Validation { field: "host", .. })
                ),
                "accepted non-bare host: {host:?}"
            );
        }
    }

    #[test]
    fn accepts_bare_hosts_and_builds_the_auth_route() {
        for host in ["example.test", "127.0.0.1", "[::1]"] {
            let mut profile = sample();
            profile.host = host.into();
            profile.validate().unwrap();
            let route = url::Url::parse(&profile.api_base()).unwrap();
            assert_eq!(route.host_str(), Some(host));
            assert_eq!(route.port(), Some(profile.auth_port));
            assert_eq!(route.path(), "/api/v1");
        }
    }
}
