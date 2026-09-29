//! Managed Linux Wine engine: a hash-pinned upstream Wine build unpacked into the launcher's runtime cache.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::runtime_archive::{RuntimeArchivePin, download_verified};

/// Pinned engine release; its exact archive identity is in `runtime_archive`.
pub(super) const ENGINE_VERSION: &str = "11.18";

const ENGINE_DIR_PREFIX: &str = "wine-";
const CACHE_VERIFICATION_MARKER: &str = ".bahamut-sha256";

/// Serializes installs across launcher instances; never deleted, since a waiter may hold it open.
const ENGINE_LOCK_FILE: &str = ".wine-engine.lock";
const STAGING_PREFIX: &str = ".wine-stage-";
const ENGINE_EXECUTABLES: &[&str] = &["bin/wine", "bin/wineserver"];

/// Present only in a build that runs 32-bit Windows programs, which the client is.
const ENGINE_I386_NTDLL: &str = "lib/wine/i386-windows/ntdll.dll";

static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);

/// Return the engine directory, installing the pinned engine when no verified copy exists.
#[cfg(target_os = "linux")]
pub(super) fn ensure_engine(cache_root: &Path) -> Result<PathBuf, String> {
    ensure_engine_with_pin(
        cache_root,
        super::runtime_archive::LINUX_WINE_11_18_WOW64,
        &engine_archive_top_dir(),
    )
}

/// Top-level directory inside the pinned engine archive.
fn engine_archive_top_dir() -> String {
    format!("{ENGINE_DIR_PREFIX}{ENGINE_VERSION}-amd64-wow64")
}

/// The engine's `wine` loader.
pub(super) fn wine_binary(engine_dir: &Path) -> PathBuf {
    engine_dir.join("bin/wine")
}

pub(super) fn ensure_engine_with_pin(
    cache_root: &Path,
    pin: RuntimeArchivePin,
    archive_top_dir: &str,
) -> Result<PathBuf, String> {
    ensure_engine_observed(cache_root, pin, archive_top_dir, || {})
}

/// `on_contended` runs once another holder of the install lock has been observed, before waiting.
fn ensure_engine_observed(
    cache_root: &Path,
    pin: RuntimeArchivePin,
    archive_top_dir: &str,
    on_contended: impl FnOnce(),
) -> Result<PathBuf, String> {
    fs::create_dir_all(cache_root).map_err(|e| {
        format!(
            "creating the Wine engine cache {}: {e}",
            cache_root.display()
        )
    })?;

    let engine_dir = cache_root.join(format!(
        "{ENGINE_DIR_PREFIX}{ENGINE_VERSION}-{}",
        pin.sha256
    ));
    if engine_is_verified(&engine_dir, pin) {
        return Ok(engine_dir);
    }
    if let Some(tool) = missing_unpack_tool(tar_version().as_deref(), xz_present()) {
        return Err(format!(
            "unpacking the Wine engine needs `{tool}`, which is not installed"
        ));
    }

    let lock = lock_cache(cache_root, on_contended);
    if lock.is_some() {
        if engine_is_verified(&engine_dir, pin) {
            return Ok(engine_dir);
        }
        // Under the lock no other install is running, so every staging directory is abandoned.
        remove_abandoned_staging(cache_root);
    }

    // Staging shares the cache volume so the final rename is atomic.
    let staging = create_staging_dir(cache_root)?;
    let result = install_into(&staging, &engine_dir, pin, archive_top_dir);
    let _ = fs::remove_dir_all(&staging);
    result?;

    tracing::info!(engine = %engine_dir.display(), "installed Wine {ENGINE_VERSION}");
    remove_superseded_engines(cache_root, &engine_dir);
    drop(lock);
    Ok(engine_dir)
}

/// Take the exclusive install lock, waiting for another instance; `None` installs unlocked.
fn lock_cache(cache_root: &Path, on_contended: impl FnOnce()) -> Option<File> {
    let path = cache_root.join(ENGINE_LOCK_FILE);
    let file = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) => {
            tracing::warn!(
                "opening {}: {e}; installing without the lock",
                path.display()
            );
            return None;
        }
    };
    match file.try_lock() {
        Ok(()) => return Some(file),
        Err(TryLockError::WouldBlock) => {}
        Err(TryLockError::Error(e)) => {
            tracing::warn!(
                "locking {}: {e}; installing without the lock",
                path.display()
            );
            return None;
        }
    }
    tracing::info!("another launcher instance is installing the Wine engine; waiting for it");
    on_contended();
    match file.lock() {
        Ok(()) => Some(file),
        Err(e) => {
            tracing::warn!(
                "locking {}: {e}; installing without the lock",
                path.display()
            );
            None
        }
    }
}

fn remove_abandoned_staging(cache_root: &Path) {
    let Ok(entries) = fs::read_dir(cache_root) else {
        return;
    };
    for entry in entries.flatten() {
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(STAGING_PREFIX));
        if !is_staging || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let path = entry.path();
        match fs::remove_dir_all(&path) {
            Ok(()) => tracing::info!("removed abandoned Wine engine staging {}", path.display()),
            Err(e) => tracing::warn!(
                "could not remove abandoned Wine engine staging {}: {e}",
                path.display()
            ),
        }
    }
}

/// The tool `tar -xJf` lacks: bsdtar decodes xz itself, GNU tar runs the `xz` program.
fn missing_unpack_tool(tar_version: Option<&str>, xz_present: bool) -> Option<&'static str> {
    match tar_version {
        None => Some("tar"),
        Some(version) if version.contains("bsdtar") => None,
        Some(_) if xz_present => None,
        Some(_) => Some("xz"),
    }
}

fn tar_version() -> Option<String> {
    let out = Command::new("tar").arg("--version").output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn xz_present() -> bool {
    Command::new("xz")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

fn install_into(
    staging: &Path,
    engine_dir: &Path,
    pin: RuntimeArchivePin,
    archive_top_dir: &str,
) -> Result<(), String> {
    let archive = staging.join("wine.tar.xz");
    tracing::info!(
        "downloading Wine {ENGINE_VERSION} ({} bytes from {})",
        pin.size,
        pin.url
    );
    download_verified(pin, &archive).map_err(|e| format!("verifying {}: {e}", pin.name))?;
    extract_tar_xz(&archive, staging)?;
    let _ = fs::remove_file(&archive);

    let staged_engine = staging.join(archive_top_dir);
    for relative in ENGINE_EXECUTABLES {
        let path = staged_engine.join(relative);
        if !is_executable_file(&path) {
            return Err(format!(
                "{} did not contain an executable {}",
                pin.name,
                path.display()
            ));
        }
    }
    let ntdll = staged_engine.join(ENGINE_I386_NTDLL);
    if !ntdll.is_file() {
        return Err(format!(
            "{} did not contain {} (no 32-bit Windows support)",
            pin.name,
            ntdll.display()
        ));
    }
    fs::write(staged_engine.join(CACHE_VERIFICATION_MARKER), pin.sha256).map_err(|e| {
        format!(
            "writing the Wine engine verification marker in {}: {e}",
            staged_engine.display()
        )
    })?;

    remove_path(engine_dir).map_err(|e| {
        format!(
            "removing the unverified Wine engine {}: {e}",
            engine_dir.display()
        )
    })?;
    fs::rename(&staged_engine, engine_dir).map_err(|e| {
        format!(
            "publishing the Wine engine {} -> {}: {e}",
            staged_engine.display(),
            engine_dir.display()
        )
    })
}

fn engine_is_verified(engine_dir: &Path, pin: RuntimeArchivePin) -> bool {
    fs::read_to_string(engine_dir.join(CACHE_VERIFICATION_MARKER))
        .ok()
        .is_some_and(|marker| marker == pin.sha256)
        && ENGINE_EXECUTABLES
            .iter()
            .all(|relative| is_executable_file(&engine_dir.join(relative)))
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Only directories this launcher marked are removed; anything else under `wine-*` is the user's.
fn remove_superseded_engines(cache_root: &Path, current: &Path) {
    let entries = match fs::read_dir(cache_root) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(
                "listing {} for superseded Wine engines: {e}",
                cache_root.display()
            );
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_engine_name = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(ENGINE_DIR_PREFIX));
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if path == current
            || !is_engine_name
            || !is_dir
            || !path.join(CACHE_VERIFICATION_MARKER).is_file()
        {
            continue;
        }
        match fs::remove_dir_all(&path) {
            Ok(()) => tracing::info!("removed superseded Wine engine {}", path.display()),
            Err(e) => tracing::warn!(
                "could not remove superseded Wine engine {}: {e}",
                path.display()
            ),
        }
    }
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn create_staging_dir(cache_root: &Path) -> Result<PathBuf, String> {
    for _ in 0..128 {
        let id = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
        let path = cache_root.join(format!("{STAGING_PREFIX}{}-{id}", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "creating the Wine engine staging dir {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Err(format!(
        "could not allocate a unique Wine engine staging dir under {}",
        cache_root.display()
    ))
}

fn extract_tar_xz(archive: &Path, dst: &Path) -> Result<(), String> {
    let status = Command::new("tar")
        .arg("-xJf")
        .arg(archive)
        .arg("-C")
        .arg(dst)
        .status()
        .map_err(|e| {
            format!(
                "running `tar -xJf` for the Wine engine archive (are tar and xz installed?): {e}"
            )
        })?;
    if !status.success() {
        return Err(format!(
            "`tar -xJf {} -C {}` failed with {status} (are tar and xz installed?)",
            archive.display(),
            dst.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use sha2::{Digest, Sha256};

    const TOP_DIR: &str = "wine-test-amd64-wow64";

    fn test_pin(url: &str, archive: &[u8], sha256: &str) -> RuntimeArchivePin {
        RuntimeArchivePin {
            name: "test Wine engine",
            url: Box::leak(url.to_owned().into_boxed_str()),
            size: archive.len() as u64,
            sha256: Box::leak(sha256.to_owned().into_boxed_str()),
        }
    }

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    /// Build an xz engine archive; `omit` names relative paths to leave out.
    fn engine_archive(omit: &[&str]) -> Vec<u8> {
        let temp = tempfile::tempdir().unwrap();
        let top = temp.path().join(TOP_DIR);
        let bin = top.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(top.join("lib/wine/i386-windows")).unwrap();
        for relative in ENGINE_EXECUTABLES {
            if omit.contains(relative) {
                continue;
            }
            let path = top.join(relative);
            fs::write(&path, b"#!/bin/sh\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::os::unix::fs::symlink("wine", bin.join("wineboot")).unwrap();
        if !omit.contains(&ENGINE_I386_NTDLL) {
            fs::write(top.join(ENGINE_I386_NTDLL), b"ntdll").unwrap();
        }
        let archive = temp.path().join("wine.tar.xz");
        let status = Command::new("tar")
            .arg("-cJf")
            .arg(&archive)
            .arg("-C")
            .arg(temp.path())
            .arg(TOP_DIR)
            .status()
            .unwrap();
        assert!(status.success(), "creating Wine test archive: {status:?}");
        fs::read(archive).unwrap()
    }

    fn serve<'a>(server: &'a MockServer, archive: &[u8]) -> httpmock::Mock<'a> {
        let body = archive.to_vec();
        server.mock(move |when, then| {
            when.method(GET).path("/wine");
            then.status(200).body(body);
        })
    }

    fn entry_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn verified_install_publishes_the_engine_and_reuses_it() {
        let server = MockServer::start();
        let archive = engine_archive(&[]);
        let mock = serve(&server, &archive);
        let temp = tempfile::tempdir().unwrap();
        let cache_root = temp.path().join("runtime");
        let digest = sha256(&archive);
        let pin = test_pin(&server.url("/wine"), &archive, &digest);

        let engine = ensure_engine_with_pin(&cache_root, pin, TOP_DIR).unwrap();

        assert_eq!(
            engine,
            cache_root.join(format!("wine-{ENGINE_VERSION}-{digest}"))
        );
        assert_eq!(
            fs::read_to_string(engine.join(CACHE_VERIFICATION_MARKER)).unwrap(),
            digest
        );
        assert!(is_executable_file(&wine_binary(&engine)));
        assert_eq!(
            fs::read_link(engine.join("bin/wineboot")).unwrap(),
            Path::new("wine")
        );
        assert_eq!(
            entry_names(&cache_root),
            vec![
                ENGINE_LOCK_FILE.to_string(),
                format!("wine-{ENGINE_VERSION}-{digest}")
            ]
        );

        let again = ensure_engine_with_pin(&cache_root, pin, TOP_DIR).unwrap();

        assert_eq!(again, engine);
        mock.assert_hits(1);
    }

    #[test]
    fn bad_digest_leaves_no_engine_and_no_staging() {
        let server = MockServer::start();
        let archive = engine_archive(&[]);
        let mock = serve(&server, &archive);
        let temp = tempfile::tempdir().unwrap();
        let cache_root = temp.path().join("runtime");
        let pin = test_pin(
            &server.url("/wine"),
            &archive,
            "0000000000000000000000000000000000000000000000000000000000000000",
        );

        let error = ensure_engine_with_pin(&cache_root, pin, TOP_DIR).unwrap_err();

        mock.assert();
        assert!(error.contains("SHA-256 mismatch"), "got {error:?}");
        assert_eq!(entry_names(&cache_root), vec![ENGINE_LOCK_FILE]);
    }

    #[test]
    fn archive_missing_a_required_file_is_rejected() {
        for missing in ["bin/wineserver", ENGINE_I386_NTDLL] {
            let server = MockServer::start();
            let archive = engine_archive(&[missing]);
            let mock = serve(&server, &archive);
            let temp = tempfile::tempdir().unwrap();
            let cache_root = temp.path().join("runtime");
            let pin = test_pin(&server.url("/wine"), &archive, &sha256(&archive));

            let error = ensure_engine_with_pin(&cache_root, pin, TOP_DIR).unwrap_err();

            mock.assert();
            assert!(error.contains(missing), "{missing}: got {error:?}");
            assert_eq!(
                entry_names(&cache_root),
                vec![ENGINE_LOCK_FILE],
                "{missing}"
            );
        }
    }

    #[test]
    fn unverified_copy_at_the_final_path_is_replaced() {
        let server = MockServer::start();
        let archive = engine_archive(&[]);
        let mock = serve(&server, &archive);
        let temp = tempfile::tempdir().unwrap();
        let cache_root = temp.path().join("runtime");
        let digest = sha256(&archive);
        let partial = cache_root.join(format!("wine-{ENGINE_VERSION}-{digest}"));
        fs::create_dir_all(partial.join("bin")).unwrap();
        fs::write(partial.join("bin/wine"), b"partial").unwrap();
        let pin = test_pin(&server.url("/wine"), &archive, &digest);

        let engine = ensure_engine_with_pin(&cache_root, pin, TOP_DIR).unwrap();

        mock.assert();
        assert_eq!(engine, partial);
        assert_eq!(fs::read(wine_binary(&engine)).unwrap(), b"#!/bin/sh\n");
        assert!(engine.join(CACHE_VERIFICATION_MARKER).is_file());
    }

    #[test]
    fn install_removes_marked_superseded_engines_only() {
        let server = MockServer::start();
        let archive = engine_archive(&[]);
        let _mock = serve(&server, &archive);
        let temp = tempfile::tempdir().unwrap();
        let cache_root = temp.path().join("runtime");
        let superseded = cache_root.join("wine-11.17-aaaa");
        fs::create_dir_all(superseded.join("bin")).unwrap();
        fs::write(superseded.join(CACHE_VERIFICATION_MARKER), "aaaa").unwrap();
        let unmarked = cache_root.join("wine-custom");
        fs::create_dir_all(unmarked.join("bin")).unwrap();
        let dxvk = cache_root.join("dxvk-3.0-bbbb");
        fs::create_dir_all(&dxvk).unwrap();
        fs::write(dxvk.join(CACHE_VERIFICATION_MARKER), "bbbb").unwrap();
        let digest = sha256(&archive);
        let pin = test_pin(&server.url("/wine"), &archive, &digest);

        ensure_engine_with_pin(&cache_root, pin, TOP_DIR).unwrap();

        assert_eq!(
            entry_names(&cache_root),
            vec![
                ENGINE_LOCK_FILE.to_string(),
                "dxvk-3.0-bbbb".to_string(),
                format!("wine-{ENGINE_VERSION}-{digest}"),
                "wine-custom".to_string(),
            ]
        );
    }

    /// Lay out a complete engine by hand, as another instance would have published it.
    fn publish_engine(engine_dir: &Path, marker: &str) {
        for relative in ENGINE_EXECUTABLES {
            let path = engine_dir.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"published by hand").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(engine_dir.join(CACHE_VERIFICATION_MARKER), marker).unwrap();
    }

    #[test]
    fn engine_marked_with_another_digest_is_replaced() {
        let server = MockServer::start();
        let archive = engine_archive(&[]);
        let mock = serve(&server, &archive);
        let temp = tempfile::tempdir().unwrap();
        let cache_root = temp.path().join("runtime");
        let digest = sha256(&archive);
        let engine_dir = cache_root.join(format!("wine-{ENGINE_VERSION}-{digest}"));
        publish_engine(
            &engine_dir,
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        );
        let pin = test_pin(&server.url("/wine"), &archive, &digest);

        let engine = ensure_engine_with_pin(&cache_root, pin, TOP_DIR).unwrap();

        mock.assert();
        assert_eq!(engine, engine_dir);
        assert_eq!(
            fs::read_to_string(engine.join(CACHE_VERIFICATION_MARKER)).unwrap(),
            digest
        );
        assert_eq!(fs::read(wine_binary(&engine)).unwrap(), b"#!/bin/sh\n");
    }

    #[test]
    fn install_removes_abandoned_staging_directories() {
        let server = MockServer::start();
        let archive = engine_archive(&[]);
        let _mock = serve(&server, &archive);
        let temp = tempfile::tempdir().unwrap();
        let cache_root = temp.path().join("runtime");
        let abandoned = cache_root.join(format!("{STAGING_PREFIX}999999-0"));
        fs::create_dir_all(abandoned.join(TOP_DIR)).unwrap();
        fs::write(abandoned.join("wine.tar.xz"), b"partial download").unwrap();
        let digest = sha256(&archive);
        let pin = test_pin(&server.url("/wine"), &archive, &digest);

        ensure_engine_with_pin(&cache_root, pin, TOP_DIR).unwrap();

        assert_eq!(
            entry_names(&cache_root),
            vec![
                ENGINE_LOCK_FILE.to_string(),
                format!("wine-{ENGINE_VERSION}-{digest}")
            ]
        );
    }

    #[test]
    fn install_waits_for_the_lock_holder_and_reuses_its_engine() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/wine");
            then.status(404);
        });
        let temp = tempfile::tempdir().unwrap();
        let cache_root = temp.path().join("runtime");
        fs::create_dir_all(&cache_root).unwrap();
        let digest = "abababababababababababababababababababababababababababababababab";
        let pin = RuntimeArchivePin {
            name: "test Wine engine",
            url: Box::leak(server.url("/wine").into_boxed_str()),
            size: 1,
            sha256: digest,
        };
        let engine_dir = cache_root.join(format!("wine-{ENGINE_VERSION}-{digest}"));
        let holder = File::create(cache_root.join(ENGINE_LOCK_FILE)).unwrap();
        holder.lock().unwrap();
        let (contended_tx, contended_rx) = std::sync::mpsc::channel();

        let waiter = {
            let cache_root = cache_root.clone();
            std::thread::spawn(move || {
                ensure_engine_observed(&cache_root, pin, TOP_DIR, move || {
                    contended_tx.send(()).unwrap();
                })
            })
        };
        // The waiter signals only after it found the lock held; a failure here means it never waited.
        contended_rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the waiter never observed the held lock");
        publish_engine(&engine_dir, digest);
        holder.unlock().unwrap();

        assert_eq!(waiter.join().unwrap().unwrap(), engine_dir);
        mock.assert_hits(0);
    }

    #[test]
    #[cfg(target_os = "linux")]
    #[ignore = "downloads the pinned Wine engine from its release host"]
    fn pinned_engine_downloads_and_unpacks() {
        let temp = tempfile::tempdir().unwrap();
        let cache_root = temp.path().join("runtime");

        let engine = ensure_engine(&cache_root).unwrap();

        assert!(is_executable_file(&wine_binary(&engine)));
        assert!(engine.join(ENGINE_I386_NTDLL).is_file());
        assert_eq!(
            fs::read_link(engine.join("bin/wineboot")).unwrap(),
            Path::new("wine")
        );
    }

    #[test]
    fn archive_top_dir_matches_the_pinned_asset() {
        let top = engine_archive_top_dir();
        assert!(
            super::super::runtime_archive::LINUX_WINE_11_18_WOW64
                .url
                .ends_with(&format!("/{ENGINE_VERSION}/{top}.tar.xz")),
            "{top}"
        );
    }

    #[test]
    fn unpack_tool_check_names_the_missing_tool() {
        let gnu = "tar (GNU tar) 1.35";
        let bsd = "bsdtar 3.5.3 - libarchive 3.7.4 zlib/1.2.12 liblzma/5.4.3";
        assert_eq!(missing_unpack_tool(None, true), Some("tar"));
        assert_eq!(missing_unpack_tool(Some(bsd), false), None);
        assert_eq!(missing_unpack_tool(Some(gnu), false), Some("xz"));
        assert_eq!(missing_unpack_tool(Some(gnu), true), None);
    }
}
