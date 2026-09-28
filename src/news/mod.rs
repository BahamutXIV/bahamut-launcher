//! File-based launcher news feed at `<state-root>/config/news.toml`.
//! Missing files bootstrap the embedded default; an existing file is authoritative and is not fetched over HTTP.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Embedded fallback written on first run when no news file exists.
const DEFAULT_NEWS: &str = include_str!("default_news.toml");

const NEWS_FILE: &str = "news.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewsItem {
    /// Free-form ASCII date rendered verbatim.
    pub date: String,
    pub title: String,
    /// Plain-text body; the UI does not render markdown.
    pub body: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct NewsFile {
    items: Vec<NewsItem>,
}

/// Loader errors keep I/O and TOML failures distinct.
#[derive(Debug, thiserror::Error)]
pub enum NewsError {
    #[error("could not resolve the launcher state directory: {0}")]
    NoStateDir(io::Error),
    #[error("news file io error: {0}")]
    Io(#[from] io::Error),
    #[error("news file is malformed: {0}")]
    Toml(Box<toml::de::Error>),
}

impl From<toml::de::Error> for NewsError {
    fn from(value: toml::de::Error) -> Self {
        NewsError::Toml(Box::new(value))
    }
}

/// Resolve the `<state-root>/config/news.toml` path.
pub fn news_path() -> Result<PathBuf, NewsError> {
    let dir = crate::config::dirs::state_config_dir().map_err(NewsError::NoStateDir)?;
    Ok(dir.join(NEWS_FILE))
}

/// Parse news TOML; an empty string returns an empty list.
pub fn from_toml_str(toml_text: &str) -> Result<Vec<NewsItem>, NewsError> {
    let file: NewsFile = toml::from_str(toml_text)?;
    Ok(file.items)
}

/// Load `path`, using the embedded default only when it is missing; malformed content returns [`NewsError::Toml`].
pub fn load_in(path: &Path) -> Result<Vec<NewsItem>, NewsError> {
    match fs::read_to_string(path) {
        Ok(text) => from_toml_str(&text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => from_toml_str(DEFAULT_NEWS),
        Err(err) => Err(err.into()),
    }
}

/// Load the default path, falling back to embedded content on any failure.
pub fn list() -> Vec<NewsItem> {
    match news_path() {
        Ok(path) => match load_in(&path) {
            Ok(items) => items,
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "news file unreadable; using embedded default",
                );
                from_toml_str(DEFAULT_NEWS).unwrap_or_default()
            }
        },
        Err(err) => {
            tracing::warn!(error = %err, "could not resolve news path; using embedded default");
            from_toml_str(DEFAULT_NEWS).unwrap_or_default()
        }
    }
}

/// Write `dir/news.toml` only when absent; return whether it was written.
pub fn bootstrap_default_if_missing_in(dir: &Path) -> io::Result<bool> {
    crate::config::dirs::bootstrap_file_if_missing(dir, NEWS_FILE, DEFAULT_NEWS)
}

/// Bootstrap `news.toml` under the state root's config directory.
pub fn bootstrap_default_if_missing() -> io::Result<bool> {
    let path = news_path().map_err(|e| match e {
        NewsError::NoStateDir(io_err) => io_err,
        other => io::Error::other(other.to_string()),
    })?;
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("news_path returned a path without a parent directory"))?;
    bootstrap_default_if_missing_in(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_default_parses_to_at_least_one_item() {
        let items = from_toml_str(DEFAULT_NEWS).unwrap();
        assert!(
            !items.is_empty(),
            "embedded default should produce at least one news card",
        );
        assert!(items.iter().all(|i| !i.title.is_empty()));
    }

    #[test]
    fn empty_input_yields_empty_list() {
        let items = from_toml_str("").unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn unknown_field_is_rejected() {
        let toml = r#"
            [[items]]
            date = "May 1"
            title = "x"
            body = "y"
            mystery = "rejected"
        "#;
        let err = from_toml_str(toml).unwrap_err();
        assert!(matches!(err, NewsError::Toml(_)));
    }

    #[test]
    fn load_in_returns_default_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("news.toml");
        assert!(!path.exists());
        let items = load_in(&path).unwrap();
        let embedded = from_toml_str(DEFAULT_NEWS).unwrap();
        assert_eq!(items, embedded);
    }

    #[test]
    fn load_in_parses_on_disk_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("news.toml");
        fs::write(
            &path,
            r#"
                [[items]]
                date = "Today"
                title = "Local override"
                body = "Hand-edited entry."
            "#,
        )
        .unwrap();
        let items = load_in(&path).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Local override");
    }

    #[test]
    fn bootstrap_writes_default_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("config");
        let wrote = bootstrap_default_if_missing_in(&inner).unwrap();
        assert!(wrote);
        let path = inner.join("news.toml");
        assert!(path.exists());
        let items = load_in(&path).unwrap();
        let embedded = from_toml_str(DEFAULT_NEWS).unwrap();
        assert_eq!(items, embedded);
    }

    #[test]
    fn bootstrap_is_idempotent_when_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path();
        fs::write(
            inner.join("news.toml"),
            r#"
                [[items]]
                date = "X"
                title = "User content"
                body = "y"
            "#,
        )
        .unwrap();
        let wrote = bootstrap_default_if_missing_in(inner).unwrap();
        assert!(!wrote);
        let items = load_in(&inner.join("news.toml")).unwrap();
        assert_eq!(items[0].title, "User content");
    }
}
