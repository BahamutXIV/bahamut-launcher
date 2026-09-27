//! Best-effort Linux DXVK provisioning for the 32-bit D3D9 client; failures fall back to wined3d.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::runtime_archive::{LINUX_DXVK_3_0, RuntimeArchivePin, download_verified};
use super::wine::WineRuntime;
use crate::config::dirs;

/// Pinned DXVK release; its exact archive identity is in `runtime_archive`.
const DXVK_VERSION: &str = "3.0";

/// 32-bit DLLs installed into the prefix's `syswow64`.
const DXVK_DLLS: &[&str] = &["d3d9.dll", "dxgi.dll"];
const CACHE_VERIFICATION_MARKER: &str = ".bahamut-sha256";
static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);

/// Prefer native DXVK and fall back to builtin wined3d when initialization fails.
const DXVK_OVERRIDES: &str = "d3d9=n,b;dxgi=n,b";

/// Install DXVK when possible and return its override fragment, or `None` for wined3d.
pub fn ensure_dxvk(runtime: &WineRuntime) -> Option<String> {
    if !vulkan_available() {
        tracing::warn!(
            "no Vulkan ICD found under /usr/share/vulkan or /etc/vulkan - DXVK unavailable, using wined3d"
        );
        return None;
    }
    let cache_root = match dirs::data_dir() {
        Ok(dir) => dir.join("runtime"),
        Err(e) => {
            tracing::warn!("cannot resolve DXVK cache dir ({e}); using wined3d");
            return None;
        }
    };
    match install_dxvk(runtime, &cache_root) {
        Ok(()) => {
            tracing::info!("DXVK {DXVK_VERSION} active (Direct3D 9 -> Vulkan)");
            Some(DXVK_OVERRIDES.to_string())
        }
        Err(e) => {
            tracing::warn!("DXVK setup failed ({e}); falling back to wined3d");
            None
        }
    }
}

fn vulkan_available() -> bool {
    ["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d"]
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .filter_map(|e| e.ok())
        .any(|e| e.path().extension().is_some_and(|ext| ext == "json"))
}

/// Install the pinned 32-bit DLLs into `syswow64`, using a `.dxvk-version` marker.
fn install_dxvk(runtime: &WineRuntime, cache_root: &Path) -> Result<(), String> {
    install_dxvk_with_pin(runtime, cache_root, LINUX_DXVK_3_0)
}

fn install_dxvk_with_pin(
    runtime: &WineRuntime,
    cache_root: &Path,
    pin: RuntimeArchivePin,
) -> Result<(), String> {
    let syswow64 = runtime.prefix.join("drive_c/windows/syswow64");
    if !syswow64.is_dir() {
        return Err(format!(
            "prefix syswow64 missing at {} (prefix not initialised?)",
            syswow64.display()
        ));
    }
    let marker = runtime.prefix.join(".dxvk-version");
    let installed_marker = format!("{DXVK_VERSION}\nsha256:{}", pin.sha256);
    if std::fs::read_to_string(&marker).ok().as_deref() == Some(installed_marker.as_str())
        && DXVK_DLLS.iter().all(|dll| syswow64.join(dll).is_file())
    {
        return Ok(());
    }

    let dxvk_dir = ensure_downloaded(cache_root, pin)?;
    for dll in DXVK_DLLS {
        let src = dxvk_dir.join("x32").join(dll);
        let dst = syswow64.join(dll);
        std::fs::copy(&src, &dst)
            .map_err(|e| format!("copying {} -> {}: {e}", src.display(), dst.display()))?;
    }
    std::fs::write(&marker, installed_marker)
        .map_err(|e| format!("writing DXVK marker {}: {e}", marker.display()))?;
    tracing::info!("installed DXVK {DXVK_VERSION} into {}", syswow64.display());
    Ok(())
}

/// Return a cache directory only when a verified archive produced both DLLs.
fn ensure_downloaded(cache_root: &Path, pin: RuntimeArchivePin) -> Result<PathBuf, String> {
    fs::create_dir_all(cache_root)
        .map_err(|e| format!("creating DXVK cache dir {}: {e}", cache_root.display()))?;

    let dxvk_dir = cache_root.join(format!("dxvk-{DXVK_VERSION}-{}", pin.sha256));
    if cache_is_verified(&dxvk_dir, pin) {
        return Ok(dxvk_dir);
    }

    let staging = create_staging_dir(cache_root)?;
    let result = (|| {
        let archive = staging.join("dxvk.tar.gz");
        tracing::info!("downloading DXVK {DXVK_VERSION} ({})", pin.url);
        download_verified(pin, &archive).map_err(|e| format!("verifying {}: {e}", pin.name))?;
        extract_tar_gz(&archive, &staging)?;

        let staged_runtime = staging.join(format!("dxvk-{DXVK_VERSION}"));
        for dll in DXVK_DLLS {
            let path = staged_runtime.join("x32").join(dll);
            if !path.is_file() {
                return Err(format!(
                    "DXVK archive did not contain the expected {}",
                    path.display()
                ));
            }
        }
        fs::write(staged_runtime.join(CACHE_VERIFICATION_MARKER), pin.sha256).map_err(|e| {
            format!(
                "writing DXVK cache verification marker in {}: {e}",
                staged_runtime.display()
            )
        })?;

        if dxvk_dir.exists() {
            fs::remove_dir_all(&dxvk_dir).map_err(|e| {
                format!("removing unverified DXVK cache {}: {e}", dxvk_dir.display())
            })?;
        }
        fs::rename(&staged_runtime, &dxvk_dir).map_err(|e| {
            format!(
                "publishing verified DXVK cache {} -> {}: {e}",
                staged_runtime.display(),
                dxvk_dir.display()
            )
        })?;
        Ok(dxvk_dir.clone())
    })();
    let _ = fs::remove_dir_all(&staging);
    result
}

fn cache_is_verified(dxvk_dir: &Path, pin: RuntimeArchivePin) -> bool {
    fs::read_to_string(dxvk_dir.join(CACHE_VERIFICATION_MARKER))
        .ok()
        .is_some_and(|marker| marker == pin.sha256)
        && DXVK_DLLS
            .iter()
            .all(|dll| dxvk_dir.join("x32").join(dll).is_file())
}

fn create_staging_dir(cache_root: &Path) -> Result<PathBuf, String> {
    for _ in 0..128 {
        let id = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
        let path = cache_root.join(format!(".dxvk-stage-{}-{id}", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "creating DXVK staging dir {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Err(format!(
        "could not allocate a unique DXVK staging dir under {}",
        cache_root.display()
    ))
}

fn extract_tar_gz(archive: &Path, dst: &Path) -> Result<(), String> {
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(dst)
        .status()
        .map_err(|e| format!("running `tar -xzf` for the DXVK archive: {e}"))?;
    if !status.success() {
        return Err(format!(
            "`tar -xzf {} -C {}` failed with {status}",
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

    fn test_pin(name: &str, url: &str, size: u64, sha256: &str) -> RuntimeArchivePin {
        RuntimeArchivePin {
            name: Box::leak(name.to_owned().into_boxed_str()),
            url: Box::leak(url.to_owned().into_boxed_str()),
            size,
            sha256: Box::leak(sha256.to_owned().into_boxed_str()),
        }
    }

    fn runtime(prefix: PathBuf) -> WineRuntime {
        WineRuntime {
            wine_bin: PathBuf::from("/usr/bin/wine"),
            prefix,
        }
    }

    fn dxvk_archive() -> Vec<u8> {
        let temp = tempfile::tempdir().unwrap();
        let x32 = temp.path().join(format!("dxvk-{DXVK_VERSION}/x32"));
        fs::create_dir_all(&x32).unwrap();
        fs::write(x32.join("d3d9.dll"), b"verified d3d9").unwrap();
        fs::write(x32.join("dxgi.dll"), b"verified dxgi").unwrap();
        let archive = temp.path().join("dxvk.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(temp.path())
            .arg(format!("dxvk-{DXVK_VERSION}"))
            .status()
            .unwrap();
        assert!(status.success(), "creating DXVK test archive: {status:?}");
        fs::read(archive).unwrap()
    }

    fn set_up_prefix(root: &Path) -> PathBuf {
        let prefix = root.join("prefix");
        let syswow64 = prefix.join("drive_c/windows/syswow64");
        fs::create_dir_all(&syswow64).unwrap();
        fs::write(syswow64.join("d3d9.dll"), b"old d3d9").unwrap();
        fs::write(syswow64.join("dxgi.dll"), b"old dxgi").unwrap();
        fs::write(prefix.join(".dxvk-version"), DXVK_VERSION).unwrap();
        prefix
    }

    #[test]
    fn bad_digest_cannot_use_legacy_cache_or_change_prefix_or_marker() {
        let server = MockServer::start();
        let body = b"tampered DXVK archive";
        let mock = server.mock(|when, then| {
            when.method(GET).path("/dxvk");
            then.status(200).body(body.as_slice());
        });
        let temp = tempfile::tempdir().unwrap();
        let cache_root = temp.path().join("runtime");
        let legacy_x32 = cache_root.join(format!("dxvk-{DXVK_VERSION}/x32"));
        fs::create_dir_all(&legacy_x32).unwrap();
        fs::write(legacy_x32.join("d3d9.dll"), b"unverified cache d3d9").unwrap();
        fs::write(legacy_x32.join("dxgi.dll"), b"unverified cache dxgi").unwrap();
        let rejected_sha256 = "0000000000000000000000000000000000000000000000000000000000000000";
        let unverified_x32 = cache_root
            .join(format!("dxvk-{DXVK_VERSION}-{rejected_sha256}"))
            .join("x32");
        fs::create_dir_all(&unverified_x32).unwrap();
        fs::write(
            unverified_x32.join("d3d9.dll"),
            b"unverified pinned-path cache",
        )
        .unwrap();
        let prefix = set_up_prefix(temp.path());
        let pin = test_pin(
            "test DXVK",
            &server.url("/dxvk"),
            body.len() as u64,
            rejected_sha256,
        );

        let error = install_dxvk_with_pin(&runtime(prefix.clone()), &cache_root, pin).unwrap_err();

        mock.assert();
        assert!(error.contains("SHA-256 mismatch"), "got {error:?}");
        assert_eq!(
            fs::read(legacy_x32.join("d3d9.dll")).unwrap(),
            b"unverified cache d3d9"
        );
        assert_eq!(
            fs::read(legacy_x32.join("dxgi.dll")).unwrap(),
            b"unverified cache dxgi"
        );
        assert_eq!(
            fs::read(unverified_x32.join("d3d9.dll")).unwrap(),
            b"unverified pinned-path cache"
        );
        let syswow64 = prefix.join("drive_c/windows/syswow64");
        assert_eq!(fs::read(syswow64.join("d3d9.dll")).unwrap(), b"old d3d9");
        assert_eq!(fs::read(syswow64.join("dxgi.dll")).unwrap(), b"old dxgi");
        assert_eq!(
            fs::read_to_string(prefix.join(".dxvk-version")).unwrap(),
            DXVK_VERSION
        );
        assert_eq!(fs::read_dir(&cache_root).unwrap().count(), 2);
    }

    #[test]
    fn verified_install_records_archive_identity_and_reuses_only_verified_cache() {
        let server = MockServer::start();
        let archive = dxvk_archive();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/dxvk");
            then.status(200).body(archive.as_slice());
        });
        let temp = tempfile::tempdir().unwrap();
        let cache_root = temp.path().join("runtime");
        let prefix = set_up_prefix(temp.path());
        let sha256 = format!("{:x}", Sha256::digest(&archive));
        let pin = test_pin(
            "test DXVK",
            &server.url("/dxvk"),
            archive.len() as u64,
            &sha256,
        );

        install_dxvk_with_pin(&runtime(prefix.clone()), &cache_root, pin).unwrap();

        let syswow64 = prefix.join("drive_c/windows/syswow64");
        assert_eq!(
            fs::read(syswow64.join("d3d9.dll")).unwrap(),
            b"verified d3d9"
        );
        assert_eq!(
            fs::read(syswow64.join("dxgi.dll")).unwrap(),
            b"verified dxgi"
        );
        assert_eq!(
            fs::read_to_string(prefix.join(".dxvk-version")).unwrap(),
            format!("{DXVK_VERSION}\nsha256:{sha256}")
        );

        fs::remove_file(prefix.join(".dxvk-version")).unwrap();
        fs::remove_file(syswow64.join("d3d9.dll")).unwrap();
        fs::remove_file(syswow64.join("dxgi.dll")).unwrap();
        install_dxvk_with_pin(&runtime(prefix.clone()), &cache_root, pin).unwrap();

        mock.assert_hits(1);
        assert_eq!(
            fs::read(syswow64.join("d3d9.dll")).unwrap(),
            b"verified d3d9"
        );
        assert_eq!(
            fs::read(syswow64.join("dxgi.dll")).unwrap(),
            b"verified dxgi"
        );
    }
}
