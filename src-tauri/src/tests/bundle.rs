use std::path::{Path, PathBuf};

use crate::commands::extensions::extension_folder_path;

/// Build `<parent>/Bahamut Launcher.app` with an `Info.plist` and return its executable path.
fn write_test_bundle(parent: &Path) -> PathBuf {
    let contents = parent.join("Bahamut Launcher.app/Contents");
    std::fs::create_dir_all(contents.join("MacOS")).unwrap();
    std::fs::create_dir_all(contents.join("Resources")).unwrap();
    std::fs::write(contents.join("Info.plist"), "<plist/>\n").unwrap();
    let exe = contents.join("MacOS/bahamut-launcher");
    std::fs::write(&exe, b"").unwrap();
    exe
}

#[test]
fn bundled_extension_folders_open_only_state_root_directories() {
    let fixture = crate::test_support::tempdir().unwrap();
    let exe = write_test_bundle(fixture.path());
    let state = fixture.path().join("home/.bahamut-launcher");
    let roots = bahamut_launcher::config::dirs::resolve_roots(&exe, Some(state.clone())).unwrap();
    let layout = bahamut_launcher::extensions::ExtensionLayout::new(&roots.install, &roots.state);
    for (target, expected) in [
        ("addons", state.join("addons")),
        ("plugins", state.join("plugins")),
        ("logs", state.join("logs")),
        ("screenshots", state.join("screenshots")),
        ("install", state.clone()),
    ] {
        assert_eq!(
            extension_folder_path(&layout, target).unwrap(),
            expected,
            "{target}"
        );
    }
    assert!(!roots.install.starts_with(&state));
    assert!(extension_folder_path(&layout, "dats").is_err());
}

#[test]
fn startup_update_recovery_succeeds_on_a_new_bundled_state_root() {
    let fixture = crate::test_support::tempdir().unwrap();
    let exe = write_test_bundle(fixture.path());
    let bundle = fixture.path().join("Bahamut Launcher.app");
    let state = fixture.path().join("home/.bahamut-launcher");
    let roots = bahamut_launcher::config::dirs::resolve_roots(&exe, Some(state.clone())).unwrap();
    let bundle_entries = || {
        let mut entries = Vec::new();
        let mut pending = vec![bundle.clone()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path.clone());
                }
                entries.push(path);
            }
        }
        entries.sort();
        entries
    };
    let bundle_before = bundle_entries();

    assert!(
        crate::launcher_updates::recover_failed_launcher_update(&roots.state, false).is_err(),
        "a state root that does not exist yet is rejected"
    );
    assert_eq!(
        crate::request_pending_update_recovery_for(&roots, false),
        Ok(false),
        "startup recovery creates the state root before probing it"
    );
    assert_eq!(
        crate::request_pending_update_recovery_for(&roots, true),
        Ok(false)
    );
    assert!(state.join("config/launcher-update.lock").is_file());
    assert!(state.join("config/launcher-update-helper.lock").is_file());
    assert_eq!(bundle_entries(), bundle_before);
}
