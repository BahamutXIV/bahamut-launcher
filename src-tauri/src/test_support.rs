/// Temporary directory whose ancestors are real directories: macOS places
/// `env::temp_dir()` under `/var`, a symlink the installer's path checks refuse.
/// Windows keeps the plain path because `canonicalize` yields a verbatim prefix.
pub(crate) fn tempdir() -> std::io::Result<tempfile::TempDir> {
    #[cfg(windows)]
    let base = std::env::temp_dir();
    #[cfg(not(windows))]
    let base = std::env::temp_dir().canonicalize()?;
    tempfile::Builder::new().tempdir_in(base)
}
