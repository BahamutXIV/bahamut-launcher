use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use bahamut_launcher::digest::sha256_hex;
use bahamut_launcher::release::{
    self, Channel, FileOwnership, Product, ReleaseInventory, ReleaseMetadata, Target,
};
use bahamut_launcher::update_helper::{OFFICIAL_OVERLAY_MANIFEST, PENDING_FILE};

use super::apply::apply_launcher_update_with;
use super::fetch::{MAX_METADATA_BYTES, cached_launcher_artifact, fetch_bounded};
use super::fs_io::{checked_root, write_json_new, write_json_replace};
#[cfg(windows)]
use super::helper::process_matches_executable;
use super::helper::{create_sibling_stage, stage_trusted_helper_from_artifact};
use super::pending::{HelperOperation, PendingUpdate, read_pending};
use super::receipts::{ReceiptPointer, load_receipt, persist_receipt, read_pointer};
use super::status::{
    LauncherUpdateResult, LauncherUpdateState, check_launcher_update, get_launcher_update_status,
};
use super::verify::{launcher_scope, verify_launcher_release};
use super::{ARTIFACT_CACHE, OFFER_FILE, STATE_FILE, UPDATE_HELPER_FILE};
use bahamut_launcher::release::ArtifactFormat;
use ring::signature::{Ed25519KeyPair, KeyPair};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::thread;

#[test]
fn pending_update_requires_an_explicit_operation() {
    let root = crate::test_support::tempdir().unwrap();
    let pending = PendingUpdate {
        schema_version: 1,
        target: root.path().to_path_buf(),
        stage: root.path().join("stage"),
        transaction: root.path().join("stage/transaction"),
        operation: HelperOperation::Repair,
        helper_pid: None,
        previous_metadata_sha256: "a".repeat(64),
        new_metadata_sha256: "a".repeat(64),
    };
    let path = root.path().join(PENDING_FILE);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_json_new(&path, &pending).unwrap();
    assert_eq!(
        read_pending(root.path()).unwrap().unwrap().operation,
        HelperOperation::Repair
    );

    let mut incomplete = serde_json::to_value(&pending).unwrap();
    incomplete.as_object_mut().unwrap().remove("operation");
    let bytes = serde_json::to_vec(&incomplete).unwrap();
    fs::write(&path, &bytes).unwrap();
    assert_eq!(
        read_pending(root.path()).unwrap_err(),
        "Launcher update receipt JSON is invalid."
    );
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn update_states_serialize_to_their_ipc_strings() {
    for (state, wire) in [
        (LauncherUpdateState::Blocked, "blocked"),
        (LauncherUpdateState::Ready, "ready"),
        (LauncherUpdateState::UpdateAvailable, "update_available"),
        (LauncherUpdateState::UpdateDownloaded, "update_downloaded"),
        (LauncherUpdateState::Current, "current"),
        (LauncherUpdateState::Handoff, "handoff"),
    ] {
        assert_eq!(serde_json::to_value(state).unwrap(), wire);
    }
    let result = LauncherUpdateResult {
        state: LauncherUpdateState::Handoff,
        message: "m".into(),
    };
    assert_eq!(
        serde_json::to_string(&result).unwrap(),
        r#"{"state":"handoff","message":"m"}"#
    );
}

#[test]
fn missing_config_is_blocked_without_network() {
    let root = crate::test_support::tempdir().unwrap();
    let status = get_launcher_update_status(root.path());
    assert_eq!(status.state, LauncherUpdateState::Blocked);
    assert!(status.message.contains("launcher-updates.json"));
    assert!(
        check_launcher_update(root.path())
            .unwrap_err()
            .contains("launcher-updates.json")
    );
}

#[test]
fn missing_bootstrap_blocks_before_configured_loopback_endpoints_are_read() {
    let root = crate::test_support::tempdir().unwrap();
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(root.path().join("config/key.bin"), [0; 32]).unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let (url, server) = counted_fixture_server(Arc::clone(&requests));
    let config = serde_json::json!({
        "schemaVersion":1,"metadataUrl":url,"signatureUrl":url,"artifactRootUrl":url,
        "trustedPublicKeyPath":"config/key.bin"
    });
    fs::write(
        root.path().join("config/launcher-updates.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    assert!(
        check_launcher_update(root.path())
            .unwrap_err()
            .contains("bootstrap")
    );
    assert_eq!(requests.load(Ordering::SeqCst), 0);
    server.join().unwrap();
}

#[test]
fn redirects_and_html_metadata_are_rejected() {
    for (response, expected) in [
        (
            "HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP 302",
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 13\r\nConnection: close\r\n\r\n<html></html>",
            "HTML",
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let response = response.to_owned();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            stream.write_all(response.as_bytes()).unwrap();
        });
        let error = fetch_bounded(
            &format!("http://{address}/metadata"),
            MAX_METADATA_BYTES,
            "metadata",
        )
        .unwrap_err();
        assert!(error.contains(expected), "{error}");
        server.join().unwrap();
    }
}

#[test]
fn signed_downgrade_is_rejected_before_offer_is_persisted() {
    let root = crate::test_support::tempdir().unwrap();
    let fixture = SignedFixture::new();
    fixture.prepare_root(root.path(), "1.0.0");
    let lower = fixture.release("0.9.0");
    let (url, server) = fixture.serve(&[lower.metadata.clone(), lower.signature.clone()]);
    fixture.write_config(root.path(), &url, Some("1.0.0"));
    let error = check_launcher_update(root.path()).unwrap_err();
    assert!(error.contains("Older remote release offers") || error.contains("downgrade rejected"));
    assert!(!root.path().join(OFFER_FILE).exists());
    server.join().unwrap();
}

#[test]
fn new_signed_offer_rejects_legacy_seed_overlay_manifest() {
    let root = crate::test_support::tempdir().unwrap();
    let fixture = SignedFixture::new();
    fixture.prepare_root(root.path(), "1.0.0");
    let next = fixture.legacy_release("1.1.0");
    let (url, server) = fixture.serve(&[next.metadata.clone(), next.signature.clone()]);
    fixture.write_config(root.path(), &url, Some("1.0.0"));

    let error = check_launcher_update(root.path()).unwrap_err();

    assert!(error.contains("manage every official DAT overlay file"));
    assert!(!root.path().join(OFFER_FILE).exists());
    assert_eq!(
        release::accepted_state_summary(&root.path().join(STATE_FILE), launcher_scope()).unwrap(),
        ("1.0.0".into(), Some("1.0.0".into()))
    );
    server.join().unwrap();
}

#[test]
fn new_signed_offer_requires_the_official_overlay_manifest() {
    let root = crate::test_support::tempdir().unwrap();
    let fixture = SignedFixture::new();
    fixture.prepare_root(root.path(), "1.0.0");
    let mut next = fixture.release("1.1.0");
    let mut metadata: ReleaseMetadata = serde_json::from_slice(&next.metadata).unwrap();
    let ReleaseInventory::Launcher { files } = &mut metadata.inventory else {
        panic!("fixture must use a launcher inventory");
    };
    files.retain(|file| file.path != OFFICIAL_OVERLAY_MANIFEST);
    next.metadata = serde_json::to_vec(&metadata).unwrap();
    next.signature = fixture.key.sign(&next.metadata).as_ref().to_vec();
    let (url, server) = fixture.serve(&[next.metadata.clone(), next.signature.clone()]);
    fixture.write_config(root.path(), &url, Some("1.0.0"));

    let error = check_launcher_update(root.path()).unwrap_err();

    assert!(error.contains("manage the official DAT overlay manifest"));
    assert!(!root.path().join(OFFER_FILE).exists());
    server.join().unwrap();
}

#[test]
fn apply_rejects_legacy_seed_overlay_manifest_on_a_new_offer() {
    let root = crate::test_support::tempdir().unwrap();
    let fixture = SignedFixture::new();
    fixture.prepare_root(root.path(), "1.0.0");
    let next = fixture.legacy_release("1.1.0");
    let offer = verify_launcher_release(
        &next.metadata,
        &next.signature,
        fixture.key.public_key().as_ref(),
    )
    .unwrap();
    persist_receipt(
        root.path(),
        &next.metadata,
        &next.signature,
        &offer.metadata_sha256,
    )
    .unwrap();
    write_json_replace(
        &root.path().join(OFFER_FILE),
        &ReceiptPointer {
            schema_version: 1,
            metadata_sha256: offer.metadata_sha256.clone(),
        },
    )
    .unwrap();
    let (url, server) = fixture.serve(&[next.metadata.clone(), next.signature.clone()]);
    fixture.write_config(root.path(), &url, Some("1.0.0"));

    let error = apply_launcher_update_with(root.path(), |_, _, _, _, _, _| {
        panic!("invalid launcher offer must not be handed to the helper")
    })
    .unwrap_err();

    assert!(error.contains("manage every official DAT overlay file"));
    assert!(cached_launcher_artifact(root.path(), &offer.metadata).is_none());
    assert_eq!(
        release::accepted_state_summary(&root.path().join(STATE_FILE), launcher_scope()).unwrap(),
        ("1.0.0".into(), Some("1.0.0".into()))
    );
    server.join().unwrap();
}

#[test]
fn metadata_check_is_download_free_and_apply_downloads_before_handoff() {
    let root = crate::test_support::tempdir().unwrap();
    let fixture = SignedFixture::new();
    fixture.prepare_root(root.path(), "1.0.0");
    let next = fixture.release("1.1.0");
    let (url, server) = fixture.serve(&[
        next.metadata.clone(),
        next.signature.clone(),
        next.metadata.clone(),
        next.signature.clone(),
        next.artifact.clone(),
    ]);
    fixture.write_config(root.path(), &url, Some("1.0.0"));

    let status = check_launcher_update(root.path()).unwrap();
    assert_eq!(status.state, LauncherUpdateState::UpdateAvailable);
    assert_eq!(status.installed_version.as_deref(), Some("1.0.0"));
    assert_eq!(status.offered_version.as_deref(), Some("1.1.0"));
    let pointer = read_pointer(&root.path().join(OFFER_FILE), "test offer")
        .unwrap()
        .unwrap();
    let key = fixture.key.public_key();
    let receipt = load_receipt(root.path(), key.as_ref(), &pointer.metadata_sha256).unwrap();
    let artifact_path = root.path().join("candidate.zip");
    fs::write(&artifact_path, &next.artifact).unwrap();
    release::verify_artifact_file(&receipt.verified.metadata, &artifact_path).unwrap();
    assert_eq!(
        release::accepted_state_summary(&root.path().join(STATE_FILE), launcher_scope()).unwrap(),
        ("1.1.0".into(), Some("1.0.0".into()))
    );
    assert!(!root.path().join(ARTIFACT_CACHE).exists());

    let expected_artifact = next.artifact.clone();
    let result = apply_launcher_update_with(
        root.path(),
        |target, _, previous, replacement, artifact, operation| {
            assert_eq!(operation, HelperOperation::Update);
            assert_eq!(previous.verified.metadata.version, "1.0.0");
            assert_eq!(replacement.verified.metadata.version, "1.1.0");
            assert_eq!(
                cached_launcher_artifact(target, &replacement.verified.metadata).as_deref(),
                Some(artifact)
            );
            assert_eq!(fs::read(artifact).unwrap(), expected_artifact);
            Ok(LauncherUpdateResult {
                state: LauncherUpdateState::Handoff,
                message: "Launcher update helper started.".into(),
            })
        },
    )
    .unwrap();
    assert_eq!(result.state, LauncherUpdateState::Handoff);

    let pointer = read_pointer(&root.path().join(OFFER_FILE), "test offer")
        .unwrap()
        .unwrap();
    let receipt = load_receipt(
        root.path(),
        fixture.key.public_key().as_ref(),
        &pointer.metadata_sha256,
    )
    .unwrap();
    let cached = cached_launcher_artifact(root.path(), &receipt.verified.metadata).unwrap();
    assert_eq!(fs::read(&cached).unwrap(), next.artifact);
    server.join().unwrap();

    fs::write(&cached, b"corrupted cached artifact").unwrap();
    assert_eq!(
        get_launcher_update_status(root.path()).state,
        LauncherUpdateState::UpdateAvailable
    );
}

#[test]
fn tampered_signature_is_rejected_and_stage_is_a_sibling() {
    let root = crate::test_support::tempdir().unwrap();
    let fixture = SignedFixture::new();
    fixture.prepare_root(root.path(), "1.0.0");
    let mut offer = fixture.release("1.1.0");
    offer.signature[0] ^= 0x80;
    let (url, server) = fixture.serve(&[offer.metadata.clone(), offer.signature.clone()]);
    fixture.write_config(root.path(), &url, Some("1.0.0"));
    assert!(
        check_launcher_update(root.path())
            .unwrap_err()
            .contains("signature is invalid")
    );
    assert!(!root.path().join(OFFER_FILE).exists());
    server.join().unwrap();

    let install = root.path().join("install");
    fs::create_dir(&install).unwrap();
    let canonical = checked_root(&install).unwrap();
    let stage = create_sibling_stage(&canonical).unwrap();
    assert_eq!(stage.parent(), canonical.parent());
    assert!(!stage.starts_with(&canonical));
}

#[test]
fn staged_helper_matches_the_signed_launcher_inventory() {
    let fixture = SignedFixture::new();
    let release = fixture.release("1.0.0");
    let verified = verify_launcher_release(
        &release.metadata,
        &release.signature,
        fixture.key.public_key().as_ref(),
    )
    .unwrap();
    let temp = crate::test_support::tempdir().unwrap();
    let target = temp.path().join("install");
    let stage = temp.path().join("stage");
    fs::create_dir(&target).unwrap();
    fs::create_dir(&stage).unwrap();
    let artifact = temp.path().join("launcher.zip");
    fs::write(&artifact, release.artifact).unwrap();

    let helper = stage_trusted_helper_from_artifact(&artifact, &stage, &verified.metadata).unwrap();

    assert_eq!(helper, stage.join(UPDATE_HELPER_FILE));
    assert_eq!(fs::read(helper).unwrap(), b"fixture trusted update helper");
    assert!(!stage.starts_with(&target));
}

#[cfg(windows)]
#[test]
fn helper_process_identity_rejects_a_reused_pid_for_another_executable() {
    let current = std::env::current_exe().unwrap();
    assert!(process_matches_executable(std::process::id(), &current).unwrap());
    assert!(
        !process_matches_executable(
            std::process::id(),
            &current.with_file_name("bahamut-update-helper.exe")
        )
        .unwrap()
    );
}

struct FixtureRelease {
    metadata: Vec<u8>,
    signature: Vec<u8>,
    artifact: Vec<u8>,
}
struct SignedFixture {
    key: Ed25519KeyPair,
}

impl SignedFixture {
    fn new() -> Self {
        Self {
            key: Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap(),
        }
    }

    fn release(&self, version: &str) -> FixtureRelease {
        self.release_with_overlay_ownership(version, FileOwnership::Managed)
    }

    fn legacy_release(&self, version: &str) -> FixtureRelease {
        self.release_with_overlay_ownership(version, FileOwnership::Seed)
    }

    fn release_with_overlay_ownership(
        &self,
        version: &str,
        overlay_ownership: FileOwnership,
    ) -> FixtureRelease {
        let temp = crate::test_support::tempdir().unwrap();
        let archive_path = temp.path().join("launcher.zip");
        let exe = b"fixture launcher executable";
        let helper = b"fixture trusted update helper";
        let overlay_manifest = b"fixture official overlay manifest";
        let file = File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file(
                "bahamut-launcher.exe",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(exe).unwrap();
        archive
            .start_file(UPDATE_HELPER_FILE, zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(helper).unwrap();
        archive
            .start_file(
                OFFICIAL_OVERLAY_MANIFEST,
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(overlay_manifest).unwrap();
        archive.finish().unwrap();
        let artifact = fs::read(&archive_path).unwrap();
        let artifact_hash = sha256_hex(&artifact);
        let metadata = ReleaseMetadata {
            schema_version: 1,
            product: Product::Launcher,
            channel: Channel::Stable,
            target: Target::WindowsX86_64,
            version: version.into(),
            artifact: release::ArtifactIdentity {
                object_key: format!("launcher/{version}/windows-x86_64/{artifact_hash}.zip"),
                format: ArtifactFormat::Zip,
                length: artifact.len() as u64,
                sha256: artifact_hash,
            },
            inventory: ReleaseInventory::Launcher {
                files: vec![
                    release::InventoryFile {
                        path: "bahamut-launcher.exe".into(),
                        length: exe.len() as u64,
                        sha256: sha256_hex(exe),
                        ownership: FileOwnership::Managed,
                    },
                    release::InventoryFile {
                        path: UPDATE_HELPER_FILE.into(),
                        length: helper.len() as u64,
                        sha256: sha256_hex(helper),
                        ownership: FileOwnership::Managed,
                    },
                    release::InventoryFile {
                        path: OFFICIAL_OVERLAY_MANIFEST.into(),
                        length: overlay_manifest.len() as u64,
                        sha256: sha256_hex(overlay_manifest),
                        ownership: overlay_ownership,
                    },
                ],
            },
        };
        let bytes = serde_json::to_vec(&metadata).unwrap();
        let signature = self.key.sign(&bytes).as_ref().to_vec();
        FixtureRelease {
            metadata: bytes,
            signature,
            artifact,
        }
    }

    fn prepare_root(&self, root: &Path, version: &str) {
        fs::create_dir(root.join("config")).unwrap();
        fs::write(root.join("config/key.bin"), self.key.public_key().as_ref()).unwrap();
        let bootstrap = self.legacy_release(version);
        fs::write(root.join("config/bootstrap.json"), bootstrap.metadata).unwrap();
        fs::write(root.join("config/bootstrap.sig"), bootstrap.signature).unwrap();
    }

    fn write_config(&self, root: &Path, base_url: &str, bootstrap_version: Option<&str>) {
        let config = if let Some(version) = bootstrap_version {
            serde_json::json!({"schemaVersion":1,"metadataUrl":format!("{base_url}/metadata"),"signatureUrl":format!("{base_url}/signature"),"artifactRootUrl":base_url,"trustedPublicKeyPath":"config/key.bin","bootstrap":{"version":version,"metadataPath":"config/bootstrap.json","signaturePath":"config/bootstrap.sig"}})
        } else {
            serde_json::json!({"schemaVersion":1,"metadataUrl":format!("{base_url}/metadata"),"signatureUrl":format!("{base_url}/signature"),"artifactRootUrl":base_url,"trustedPublicKeyPath":"config/key.bin"})
        };
        fs::write(
            root.join("config/launcher-updates.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
    }

    fn serve(&self, responses: &[Vec<u8>]) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let responses = responses.to_vec();
        let server = thread::spawn(move || {
            for body in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request);
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        (format!("http://{address}"), server)
    }
}

fn counted_fixture_server(counter: Arc<AtomicUsize>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for _ in 0..40 {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let response =
                        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = stream.write_all(response);
                    return;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(_) => return,
            }
        }
    });
    (format!("http://{address}"), server)
}
