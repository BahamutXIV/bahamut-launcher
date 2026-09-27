//! Hash-pinned runtime archives shared by the Wine platform backends.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

#[derive(Clone, Copy)]
pub(super) struct RuntimeArchivePin {
    pub(super) name: &'static str,
    pub(super) url: &'static str,
    pub(super) size: u64,
    pub(super) sha256: &'static str,
}

// GitHub Releases API `digest` and `size` for asset #376627659:
// https://api.github.com/repos/Sikarugir-App/Template/releases/assets/376627659
#[cfg(any(target_os = "macos", test))]
pub(super) const MACOS_WRAPPER_1_0_11: RuntimeArchivePin = RuntimeArchivePin {
    name: "Sikarugir wrapper 1.0.11",
    url: "https://github.com/Sikarugir-App/Wrapper/releases/download/v1.0/Template-1.0.11.tar.xz",
    size: 84_533_420,
    sha256: "9fa15479e7ff6abd99c1d07be285fb95f41fc6991586502427152b1f7d6ccb8a",
};

// GitHub Releases API `digest` and `size` for asset #387912714:
// https://api.github.com/repos/Sikarugir-App/Engines/releases/assets/387912714
#[cfg(any(target_os = "macos", test))]
pub(super) const MACOS_ENGINE_WS12_23_7_1_4: RuntimeArchivePin = RuntimeArchivePin {
    name: "Sikarugir Wine engine WS12WineCX23.7.1_4",
    url: "https://github.com/Sikarugir-App/Engines/releases/download/v1.0/WS12WineCX23.7.1_4.tar.xz",
    size: 173_886_736,
    sha256: "f1519042639f37ef20240d5f5a90911568b48bec4816ee1c6e1c44a33c69b64c",
};

// GitHub Releases API `digest` and `size` for asset #486981946:
// https://api.github.com/repos/doitsujin/dxvk/releases/assets/486981946
#[cfg(any(target_os = "linux", test))]
pub(super) const LINUX_DXVK_3_0: RuntimeArchivePin = RuntimeArchivePin {
    name: "DXVK 3.0",
    url: "https://github.com/doitsujin/dxvk/releases/download/v3.0/dxvk-3.0.tar.gz",
    size: 17_866_587,
    sha256: "7dd3243fe1b260a0e9b0b9e49d672ae32e3398bee18c97e7e8569d0ef0eca92d",
};

/// Stream an archive to a new staging file, rejecting excess bytes and any
/// size or SHA-256 mismatch before callers can extract or install it.
pub(super) fn download_verified(pin: RuntimeArchivePin, destination: &Path) -> Result<(), String> {
    let mut response = reqwest::blocking::get(pin.url)
        .and_then(|response| response.error_for_status())
        .map_err(|error| format!("GET {}: {error}", pin.url))?;

    if let Some(content_length) = response.content_length()
        && content_length != pin.size
    {
        return Err(format!(
            "{} has unexpected Content-Length {content_length}, expected {}",
            pin.name, pin.size
        ));
    }

    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| format!("creating {}: {error}", destination.display()))?;

    let result = copy_and_verify(&mut response, &mut output, pin, destination);

    drop(output);
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result
}

fn copy_and_verify<R: Read, W: Write>(
    reader: &mut R,
    output: &mut W,
    pin: RuntimeArchivePin,
    destination: &Path,
) -> Result<(), String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;

    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("reading {}: {error}", pin.name))?;
        if read == 0 {
            break;
        }

        let chunk_size = read as u64;
        if total.saturating_add(chunk_size) > pin.size {
            return Err(format!(
                "{} exceeded its {}-byte size limit",
                pin.name, pin.size
            ));
        }

        output
            .write_all(&buffer[..read])
            .map_err(|error| format!("writing {}: {error}", destination.display()))?;
        hasher.update(&buffer[..read]);
        total += chunk_size;
    }

    if total != pin.size {
        return Err(format!(
            "{} has {total} bytes, expected {}",
            pin.name, pin.size
        ));
    }

    let actual_sha256 = format!("{:x}", hasher.finalize());
    if actual_sha256 != pin.sha256 {
        return Err(format!(
            "{} SHA-256 mismatch: expected {}, got {actual_sha256}",
            pin.name, pin.sha256
        ));
    }

    output
        .flush()
        .map_err(|error| format!("flushing {}: {error}", destination.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use std::io::Cursor;

    fn test_pin(size: u64, sha256: &'static str) -> RuntimeArchivePin {
        RuntimeArchivePin {
            name: "test archive",
            url: "http://example.invalid/archive",
            size,
            sha256,
        }
    }

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    #[test]
    fn published_runtime_pins_have_sha256_and_bounded_sizes() {
        for pin in [
            MACOS_WRAPPER_1_0_11,
            MACOS_ENGINE_WS12_23_7_1_4,
            LINUX_DXVK_3_0,
        ] {
            assert_eq!(pin.sha256.len(), 64, "{}", pin.name);
            assert!(pin.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert!(pin.size > 0, "{}", pin.name);
        }
    }

    #[test]
    fn copy_and_verify_accepts_exact_bytes() {
        let bytes = b"exact archive bytes";
        let pin = test_pin(
            bytes.len() as u64,
            Box::leak(sha256(bytes).into_boxed_str()),
        );
        let mut reader = Cursor::new(bytes);
        let mut output = Vec::new();

        copy_and_verify(&mut reader, &mut output, pin, Path::new("test archive")).unwrap();

        assert_eq!(output, bytes);
    }

    #[test]
    fn copy_and_verify_rejects_short_input() {
        let bytes = b"short archive";
        let pin = test_pin(
            (bytes.len() + 1) as u64,
            Box::leak(sha256(bytes).into_boxed_str()),
        );
        let mut reader = Cursor::new(bytes);
        let mut output = Vec::new();

        let error =
            copy_and_verify(&mut reader, &mut output, pin, Path::new("test archive")).unwrap_err();

        assert!(error.contains("expected"), "got {error:?}");
        assert_eq!(output, bytes);
        assert!(output.len() as u64 <= pin.size);
    }

    #[test]
    fn copy_and_verify_rejects_overlong_input_without_exceeding_limit() {
        let bytes = b"too many archive bytes";
        let limit = (bytes.len() - 1) as u64;
        let pin = test_pin(limit, Box::leak(sha256(bytes).into_boxed_str()));
        let mut reader = Cursor::new(bytes);
        let mut output = Vec::new();

        let error =
            copy_and_verify(&mut reader, &mut output, pin, Path::new("test archive")).unwrap_err();

        assert!(error.contains("size limit"), "got {error:?}");
        assert!(output.len() as u64 <= limit);
    }

    #[test]
    fn bad_digest_removes_the_staged_archive_without_touching_live_files() {
        let server = MockServer::start();
        let body = b"bad digest payload";
        let mock = server.mock(|when, then| {
            when.method(GET).path("/archive");
            then.status(200).body(body.as_slice());
        });
        let pin = RuntimeArchivePin {
            name: "test archive",
            url: Box::leak(server.url("/archive").into_boxed_str()),
            size: body.len() as u64,
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        };
        let temp = tempfile::tempdir().unwrap();
        let staged_archive = temp.path().join("archive.tar.gz");
        let live_component = temp.path().join("live-component");
        fs::write(&live_component, b"previous installation").unwrap();

        let error = download_verified(pin, &staged_archive).unwrap_err();

        mock.assert();
        assert!(error.contains("SHA-256 mismatch"), "got {error:?}");
        assert!(!staged_archive.exists());
        assert_eq!(fs::read(live_component).unwrap(), b"previous installation");
    }

    #[test]
    fn rejects_oversized_responses_before_writing_past_the_pin() {
        let server = MockServer::start();
        let body = b"too many bytes";
        let mock = server.mock(|when, then| {
            when.method(GET).path("/archive");
            then.status(200).body(body.as_slice());
        });
        let pin = RuntimeArchivePin {
            name: "test archive",
            url: Box::leak(server.url("/archive").into_boxed_str()),
            size: (body.len() - 1) as u64,
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        };
        let temp = tempfile::tempdir().unwrap();
        let staged_archive = temp.path().join("archive.tar.gz");

        let error = download_verified(pin, &staged_archive).unwrap_err();

        mock.assert();
        assert!(error.contains("Content-Length"), "got {error:?}");
        assert!(!staged_archive.exists());
    }
}
