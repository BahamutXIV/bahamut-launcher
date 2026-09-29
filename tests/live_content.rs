use std::path::PathBuf;

use bahamut_launcher::content::InstallShared;
use bahamut_launcher::content::http::{CheckpointAction, download_object};
use bahamut_launcher::content::installer;
use bahamut_launcher::content::manifest::shipped_manifest;

#[test]
#[ignore = "requires the configured content host and an explicit local cache directory"]
fn reqwest_downloads_the_pinned_full_client() {
    let cache = PathBuf::from(
        std::env::var_os("BAHAMUT_LIVE_CONTENT_CACHE")
            .expect("set BAHAMUT_LIVE_CONTENT_CACHE to an empty local cache directory"),
    );
    let manifest = shipped_manifest().unwrap();
    let archive = &manifest.base.as_ref().unwrap().archives[0].object;
    let root = manifest.content_root.as_deref().unwrap();
    let mut last_reported_gib = 0;
    let path = download_object(
        root,
        archive,
        &cache,
        |_| CheckpointAction::Continue,
        |transferred, _| {
            let gib = transferred / (1024 * 1024 * 1024);
            if gib > last_reported_gib {
                eprintln!("reqwest verified transfer: {gib} GiB");
                last_reported_gib = gib;
            }
        },
    )
    .unwrap();
    assert_eq!(std::fs::metadata(path).unwrap().len(), archive.length);
}

#[test]
#[ignore = "requires a verified cache directory and an explicit fresh destination"]
fn installs_and_verifies_the_pinned_full_client() {
    let cache = PathBuf::from(
        std::env::var_os("BAHAMUT_LIVE_CONTENT_CACHE")
            .expect("set BAHAMUT_LIVE_CONTENT_CACHE to the verified cache directory"),
    );
    let destination = PathBuf::from(
        std::env::var_os("BAHAMUT_LIVE_INSTALL_DESTINATION")
            .expect("set BAHAMUT_LIVE_INSTALL_DESTINATION to a fresh destination"),
    );
    assert!(destination.is_absolute());
    assert!(
        destination
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("m0-acceptance-")
    );
    let manifest = shipped_manifest().unwrap();
    let package = manifest.base.unwrap();
    let quote = installer::quote(&destination, &cache, &package).unwrap();
    eprintln!("install quote: {quote:?}");
    let shared =
        InstallShared::with_totals(package.download_bytes().unwrap(), package.final_files.len());
    installer::install(
        &shared,
        &destination,
        &manifest.content_root.unwrap(),
        &cache,
        &package,
    )
    .unwrap();
    assert!(destination.join("ffxivgame.exe").is_file());
    assert!(destination.join("patch.ver").is_file());
    assert!(!destination.join(".bahamut-install.json").exists());
}
