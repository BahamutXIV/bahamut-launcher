use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::launcher_ini::LauncherConfig;
use crate::config::preferences::GameSettings;
use crate::config::{dirs, retail_game};
use zip::write::SimpleFileOptions;

use super::catalog::{BACKUP_KEEP, latest_backup};
use super::create::create_backup_in;
use super::paths::read_sorted_dir;
use super::restore::restore_latest_backup_in;
use super::transaction::remove_path;
use super::{BackupError, BackupLayout, BackupTarget};

const FIXTURE_LABEL: &str = "fixture";

fn fixture_layout(temp: &tempfile::TempDir) -> BackupLayout {
    let launcher_root = temp.path().join("launcher");
    BackupLayout {
        backups_root: launcher_root.join("backups"),
        user_settings_root: temp.path().join("documents/FINAL FANTASY XIV"),
        launcher_config_path: launcher_root.join("config/bahamut.ini"),
        launcher_root,
    }
}

fn zip_names(path: &Path) -> Vec<String> {
    let file = File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut names = (0..archive.len())
        .map(|index| archive.by_index(index).unwrap().name().to_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn write_valid_retail_config(path: &Path, width: u32, height: u32) {
    let settings = GameSettings {
        initialized: true,
        width,
        height,
        ..GameSettings::default()
    };
    retail_game::create_at_path(path, &settings).unwrap();
}

#[cfg(windows)]
fn create_junction(link: &Path, target: &Path) {
    use std::process::Command;

    let output = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("mklink should start");
    assert!(
        output.status.success(),
        "mklink failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn user_backup_includes_only_config_and_character_data() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    fs::create_dir_all(layout.user_settings_root.join("user/00000001")).unwrap();
    fs::create_dir_all(layout.user_settings_root.join("user/00000001/log")).unwrap();
    fs::create_dir_all(layout.user_settings_root.join("screenshots")).unwrap();
    fs::write(layout.user_settings_root.join("config.sys"), b"config").unwrap();
    fs::write(layout.user_settings_root.join("config.pad"), b"pad").unwrap();
    fs::write(
        layout.user_settings_root.join("user/00000001/macros.dat"),
        b"macros",
    )
    .unwrap();
    fs::write(
        layout.user_settings_root.join("user/00000001/game"),
        b"game",
    )
    .unwrap();
    fs::write(
        layout.user_settings_root.join("user/00000001/log/chat.log"),
        b"chat",
    )
    .unwrap();
    fs::write(
        layout.user_settings_root.join("screenshots/private.png"),
        b"image",
    )
    .unwrap();

    let archive_path =
        create_backup_in(BackupTarget::UserSettings, &layout, FIXTURE_LABEL).unwrap();

    assert_eq!(
        zip_names(&archive_path),
        ["config.pad", "config.sys", "user/00000001/game"]
    );
}

#[test]
fn extension_backup_excludes_payloads_logs_and_launcher_config() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    for (relative, contents) in [
        ("config/extensions.ini", "[plugins]\n\n[addons]\n"),
        ("config/dats.ini", "[dats]\n"),
        ("config/addons/fps/settings.ini", "show = true\n"),
        (
            "config/plugins/screenshot/settings.ini",
            "[screenshot]\nformat = png\nhide_overlays = true\nhotkey = print_screen\n",
        ),
        ("scripts/default.txt", "/addon load fps\n"),
        ("config/bahamut.ini", "secret = no\n"),
        ("addons/fps/addon.toml", "id = 'fps'\n"),
        ("plugins/dats/world/overlay.toml", "id = 'world'\n"),
        ("plugins/screenshot.dll", "binary\n"),
        ("logs/chat/example.log", "log\n"),
    ] {
        let path = layout.launcher_root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    let archive_path = create_backup_in(BackupTarget::Extensions, &layout, FIXTURE_LABEL).unwrap();

    assert_eq!(
        zip_names(&archive_path),
        [
            "addons/fps/addon.toml",
            "config/addons/fps/settings.ini",
            "config/dats.ini",
            "config/extensions.ini",
            "config/plugins/screenshot/settings.ini",
            "plugins/dats/world/overlay.toml",
            "scripts/default.txt",
        ]
    );
}

#[test]
fn backup_collision_order_survives_retention_and_double_digit_suffixes() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    fs::create_dir_all(&layout.user_settings_root).unwrap();
    let source = layout.user_settings_root.join("config.pad");
    let timestamp = "20000103-173524-123";
    let mut created = Vec::new();
    for sequence in 1..=12 {
        fs::write(&source, format!("backup {sequence}")).unwrap();
        let path = create_backup_in(BackupTarget::UserSettings, &layout, timestamp).unwrap();
        assert_eq!(
            latest_backup(BackupTarget::UserSettings, &layout.backups_root).unwrap(),
            Some(path.clone())
        );
        created.push(path);
    }
    for (index, path) in created.iter().enumerate() {
        assert_eq!(path.exists(), index >= 12 - BACKUP_KEEP);
    }
    fs::write(&source, b"changed after backup").unwrap();
    restore_latest_backup_in(BackupTarget::UserSettings, &layout).unwrap();
    assert_eq!(fs::read(&source).unwrap(), b"backup 12");
    fs::write(&source, b"next millisecond").unwrap();
    let later =
        create_backup_in(BackupTarget::UserSettings, &layout, "20000103-173524-124").unwrap();
    assert_eq!(
        latest_backup(BackupTarget::UserSettings, &layout.backups_root).unwrap(),
        Some(later)
    );
}

#[test]
fn concurrent_backups_have_distinct_complete_archives() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    fs::create_dir_all(&layout.user_settings_root).unwrap();
    fs::write(layout.user_settings_root.join("config.pad"), b"pad").unwrap();
    let timestamp = "20000103-173524-123";
    let paths = std::thread::scope(|scope| {
        let first =
            scope.spawn(|| create_backup_in(BackupTarget::UserSettings, &layout, timestamp));
        let second =
            scope.spawn(|| create_backup_in(BackupTarget::UserSettings, &layout, timestamp));
        [
            first.join().unwrap().unwrap(),
            second.join().unwrap().unwrap(),
        ]
    });
    assert_ne!(paths[0], paths[1]);
    for path in &paths {
        assert_eq!(zip_names(path), ["config.pad"]);
    }
    assert_eq!(fs::read_dir(&layout.backups_root).unwrap().count(), 2);
}

#[test]
fn backup_does_not_truncate_an_existing_partial_file() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    fs::create_dir_all(&layout.user_settings_root).unwrap();
    fs::write(layout.user_settings_root.join("config.pad"), b"pad").unwrap();
    fs::create_dir_all(&layout.backups_root).unwrap();
    let partial = layout.backups_root.join("UserSettings_fixture.zip.partial");
    fs::write(&partial, b"another operation").unwrap();
    let archive = create_backup_in(BackupTarget::UserSettings, &layout, FIXTURE_LABEL).unwrap();
    assert_eq!(zip_names(&archive), ["config.pad"]);
    assert_eq!(fs::read(partial).unwrap(), b"another operation");
}

#[cfg(unix)]
#[test]
fn backup_preserves_partial_links_and_rejects_a_linked_output_root() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    fs::create_dir_all(&layout.user_settings_root).unwrap();
    fs::write(layout.user_settings_root.join("config.pad"), b"pad").unwrap();
    fs::create_dir_all(&layout.backups_root).unwrap();
    let sentinel = temp.path().join("sentinel");
    fs::write(&sentinel, b"keep").unwrap();
    let partial = layout.backups_root.join("UserSettings_fixture.zip.partial");
    std::os::unix::fs::symlink(&sentinel, &partial).unwrap();
    create_backup_in(BackupTarget::UserSettings, &layout, FIXTURE_LABEL).unwrap();
    assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
    assert!(
        fs::symlink_metadata(&partial)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let other = tempfile::tempdir().unwrap();
    let linked_layout = fixture_layout(&other);
    fs::create_dir_all(&linked_layout.user_settings_root).unwrap();
    fs::write(linked_layout.user_settings_root.join("config.pad"), b"pad").unwrap();
    fs::create_dir_all(&linked_layout.launcher_root).unwrap();
    std::os::unix::fs::symlink(temp.path(), &linked_layout.backups_root).unwrap();
    assert!(create_backup_in(BackupTarget::UserSettings, &linked_layout, FIXTURE_LABEL).is_err());
    assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
}

#[test]
fn backup_retention_keeps_newest_five_per_target() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    fs::create_dir_all(&layout.user_settings_root).unwrap();
    fs::write(layout.user_settings_root.join("config.pad"), b"pad").unwrap();
    for index in 0..7 {
        create_backup_in(
            BackupTarget::UserSettings,
            &layout,
            &format!("fixture-{index}"),
        )
        .unwrap();
    }
    let backups = read_sorted_dir(&layout.backups_root)
        .unwrap()
        .into_iter()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("UserSettings_")
        })
        .count();
    assert_eq!(backups, BACKUP_KEEP);
}

#[test]
fn user_restore_preserves_screenshots_and_reconciles_launcher_settings() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    fs::create_dir_all(layout.user_settings_root.join("user/00000001/log")).unwrap();
    write_valid_retail_config(&layout.user_settings_root.join("config.sys"), 1280, 720);
    fs::write(
        layout.user_settings_root.join("user/00000001/game"),
        b"saved",
    )
    .unwrap();
    fs::write(
        layout.user_settings_root.join("user/00000001/log/chat.log"),
        b"keep",
    )
    .unwrap();
    fs::create_dir_all(layout.user_settings_root.join("screenshots")).unwrap();
    fs::write(
        layout.user_settings_root.join("screenshots/keep.png"),
        b"image",
    )
    .unwrap();
    LauncherConfig::defaults()
        .save_to(&layout.launcher_config_path)
        .unwrap();
    create_backup_in(BackupTarget::UserSettings, &layout, FIXTURE_LABEL).unwrap();

    fs::write(layout.user_settings_root.join("user/00000001/game"), b"new").unwrap();
    fs::write(
        layout.user_settings_root.join("user/00000001/new.cmb"),
        b"new",
    )
    .unwrap();
    remove_path(&layout.user_settings_root.join("config.sys")).unwrap();
    write_valid_retail_config(&layout.user_settings_root.join("config.sys"), 1920, 1080);

    restore_latest_backup_in(BackupTarget::UserSettings, &layout).unwrap();

    assert_eq!(
        fs::read(layout.user_settings_root.join("user/00000001/game")).unwrap(),
        b"saved"
    );
    assert!(
        layout
            .user_settings_root
            .join("user/00000001/new.cmb")
            .exists()
    );
    assert_eq!(
        fs::read(layout.user_settings_root.join("user/00000001/log/chat.log")).unwrap(),
        b"keep"
    );
    assert!(
        layout
            .user_settings_root
            .join("screenshots/keep.png")
            .is_file()
    );
    let config =
        LauncherConfig::from_ini_str(&fs::read_to_string(&layout.launcher_config_path).unwrap())
            .unwrap();
    assert_eq!(
        (
            config.preferences.game.width,
            config.preferences.game.height
        ),
        (1280, 720)
    );
    assert!(config.preferences.game.initialized);
}

#[test]
fn user_restore_without_config_sys_preserves_launcher_settings() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    fs::create_dir_all(&layout.user_settings_root).unwrap();
    fs::write(layout.user_settings_root.join("config.pad"), b"saved").unwrap();

    let mut launcher_config = LauncherConfig::defaults();
    launcher_config.preferences.game.initialized = true;
    launcher_config.preferences.game.width = 1600;
    launcher_config.preferences.game.height = 900;
    launcher_config
        .save_to(&layout.launcher_config_path)
        .unwrap();
    create_backup_in(BackupTarget::UserSettings, &layout, FIXTURE_LABEL).unwrap();

    fs::write(layout.user_settings_root.join("config.pad"), b"current").unwrap();
    restore_latest_backup_in(BackupTarget::UserSettings, &layout).unwrap();

    assert_eq!(
        fs::read(layout.user_settings_root.join("config.pad")).unwrap(),
        b"saved"
    );
    let restored_launcher =
        LauncherConfig::from_ini_str(&fs::read_to_string(&layout.launcher_config_path).unwrap())
            .unwrap();
    assert_eq!(
        (
            restored_launcher.preferences.game.width,
            restored_launcher.preferences.game.height
        ),
        (1600, 900)
    );
}

#[test]
fn extension_restore_replaces_owned_units_and_preserves_runtime_files() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    for (relative, contents) in [
        ("addons/fps/addon.toml", "saved addon\n"),
        ("config/extensions.ini", "[plugins]\n\n[addons]\n"),
        ("config/addons/fps/settings.ini", "saved settings\n"),
        ("plugins/dats/world/overlay.toml", "saved dat\n"),
        ("scripts/default.txt", "/addon load fps\n"),
        ("plugins/screenshot.dll", "runtime dll\n"),
        ("logs/chat/example.log", "runtime log\n"),
    ] {
        let path = layout.launcher_root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    create_backup_in(BackupTarget::Extensions, &layout, FIXTURE_LABEL).unwrap();

    fs::write(
        layout.launcher_root.join("addons/fps/addon.toml"),
        "current addon\n",
    )
    .unwrap();
    fs::create_dir_all(layout.launcher_root.join("addons/new")).unwrap();
    fs::write(
        layout.launcher_root.join("addons/new/addon.toml"),
        "not in snapshot\n",
    )
    .unwrap();
    fs::write(
        layout.launcher_root.join("plugins/screenshot.dll"),
        "new runtime dll\n",
    )
    .unwrap();
    fs::write(
        layout.launcher_root.join("logs/chat/example.log"),
        "new runtime log\n",
    )
    .unwrap();

    restore_latest_backup_in(BackupTarget::Extensions, &layout).unwrap();

    assert_eq!(
        fs::read_to_string(layout.launcher_root.join("addons/fps/addon.toml")).unwrap(),
        "saved addon\n"
    );
    assert!(!layout.launcher_root.join("addons/new/addon.toml").exists());
    assert_eq!(
        fs::read_to_string(layout.launcher_root.join("plugins/screenshot.dll")).unwrap(),
        "new runtime dll\n"
    );
    assert_eq!(
        fs::read_to_string(layout.launcher_root.join("logs/chat/example.log")).unwrap(),
        "new runtime log\n"
    );
}

#[cfg(windows)]
#[test]
fn extension_restore_rejects_junction_before_replacing_live_units() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    let external_root = temp.path().join("external");
    let external_config = external_root.join("config");
    fs::create_dir_all(layout.launcher_root.join("addons/fps")).unwrap();
    fs::create_dir_all(&layout.backups_root).unwrap();
    fs::create_dir_all(&external_config).unwrap();
    fs::write(external_config.join("extensions.ini"), b"external\n").unwrap();
    fs::write(external_config.join("external-only.txt"), b"keep\n").unwrap();
    fs::write(
        layout.launcher_root.join("addons/fps/addon.toml"),
        b"live addon\n",
    )
    .unwrap();
    create_junction(&layout.launcher_root.join("config"), &external_config);

    let archive_path = layout.backups_root.join("Extensions_fixture.zip");
    let file = File::create(&archive_path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    archive
        .start_file("addons/fps/addon.toml", SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"restored addon\n").unwrap();
    archive
        .start_file("config/extensions.ini", SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"[plugins]\n\n[addons]\n").unwrap();
    archive.finish().unwrap();

    let restore_result = restore_latest_backup_in(BackupTarget::Extensions, &layout);
    fs::remove_dir(layout.launcher_root.join("config")).unwrap();

    assert!(matches!(
        restore_result,
        Err(BackupError::UnsafeDestination(_))
    ));
    assert_eq!(
        fs::read(layout.launcher_root.join("addons/fps/addon.toml")).unwrap(),
        b"live addon\n"
    );
    assert_eq!(
        fs::read(external_config.join("extensions.ini")).unwrap(),
        b"external\n"
    );
    assert_eq!(
        fs::read(external_config.join("external-only.txt")).unwrap(),
        b"keep\n"
    );
}

#[cfg(windows)]
#[test]
fn user_restore_rejects_launcher_config_junction_before_replacing_live_units() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    let external_root = temp.path().join("external");
    let external_config = external_root.join("config");
    fs::create_dir_all(layout.user_settings_root.join("user/00000001")).unwrap();
    fs::create_dir_all(&layout.backups_root).unwrap();
    fs::create_dir_all(&external_config).unwrap();
    create_junction(&layout.launcher_root.join("config"), &external_config);
    fs::write(
        &layout.launcher_config_path,
        LauncherConfig::defaults().to_ini_string(),
    )
    .unwrap();
    fs::write(external_config.join("external-only.txt"), b"keep\n").unwrap();
    let external_launcher_config = fs::read(&layout.launcher_config_path).unwrap();
    write_valid_retail_config(&layout.user_settings_root.join("config.sys"), 1280, 720);
    fs::write(
        layout.user_settings_root.join("user/00000001/game"),
        b"saved\n",
    )
    .unwrap();
    create_backup_in(BackupTarget::UserSettings, &layout, FIXTURE_LABEL).unwrap();
    fs::write(
        layout.user_settings_root.join("user/00000001/game"),
        b"current\n",
    )
    .unwrap();

    let restore_result = restore_latest_backup_in(BackupTarget::UserSettings, &layout);
    fs::remove_dir(layout.launcher_root.join("config")).unwrap();

    assert!(matches!(
        restore_result,
        Err(BackupError::UnsafeDestination(_))
    ));
    assert_eq!(
        fs::read(layout.user_settings_root.join("user/00000001/game")).unwrap(),
        b"current\n"
    );
    assert_eq!(
        fs::read(external_config.join("bahamut.ini")).unwrap(),
        external_launcher_config
    );
    assert_eq!(
        fs::read(external_config.join("external-only.txt")).unwrap(),
        b"keep\n"
    );
}

#[test]
fn invalid_extension_backup_leaves_live_files_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    fs::create_dir_all(layout.launcher_root.join("config")).unwrap();
    fs::create_dir_all(&layout.backups_root).unwrap();
    fs::write(
        layout.launcher_root.join("config/extensions.ini"),
        "[plugins]\n\n[addons]\n",
    )
    .unwrap();
    let archive_path = layout.backups_root.join("Extensions_fixture.zip");
    let file = File::create(&archive_path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    archive
        .start_file("config/extensions.ini", SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"[unknown]\nvalue = true\n").unwrap();
    archive.finish().unwrap();

    assert!(restore_latest_backup_in(BackupTarget::Extensions, &layout).is_err());
    assert_eq!(
        fs::read_to_string(layout.launcher_root.join("config/extensions.ini")).unwrap(),
        "[plugins]\n\n[addons]\n"
    );
}

#[test]
fn extension_file_unit_cannot_be_replaced_by_a_directory() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    fs::create_dir_all(layout.launcher_root.join("config")).unwrap();
    fs::create_dir_all(&layout.backups_root).unwrap();
    fs::write(
        layout.launcher_root.join("config/extensions.ini"),
        "[plugins]\n\n[addons]\n",
    )
    .unwrap();
    let archive_path = layout.backups_root.join("Extensions_fixture.zip");
    let file = File::create(&archive_path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    archive
        .start_file("config/extensions.ini/child", SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"unexpected").unwrap();
    archive.finish().unwrap();

    assert!(matches!(
        restore_latest_backup_in(BackupTarget::Extensions, &layout),
        Err(BackupError::UnsafeEntry(_))
    ));
    assert_eq!(
        fs::read_to_string(layout.launcher_root.join("config/extensions.ini")).unwrap(),
        "[plugins]\n\n[addons]\n"
    );
}

#[test]
fn screenshot_settings_file_unit_cannot_be_replaced_by_a_directory() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    let live_settings = layout
        .launcher_root
        .join("config/plugins/screenshot/settings.ini");
    fs::create_dir_all(live_settings.parent().unwrap()).unwrap();
    fs::write(&live_settings, "[screenshot]\nformat = png\n").unwrap();
    fs::create_dir_all(&layout.backups_root).unwrap();
    let archive_path = layout.backups_root.join("Extensions_fixture.zip");
    let file = File::create(&archive_path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    archive
        .start_file(
            "config/plugins/screenshot/settings.ini/child",
            SimpleFileOptions::default(),
        )
        .unwrap();
    archive.write_all(b"unexpected").unwrap();
    archive.finish().unwrap();

    assert!(matches!(
        restore_latest_backup_in(BackupTarget::Extensions, &layout),
        Err(BackupError::UnsafeEntry(_))
    ));
    assert_eq!(
        fs::read_to_string(live_settings).unwrap(),
        "[screenshot]\nformat = png\n"
    );
}

#[test]
fn extension_restore_rejects_live_settings_directory_before_replacing_it() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    let live_settings = layout
        .launcher_root
        .join("config/plugins/screenshot/settings.ini");
    fs::create_dir_all(live_settings.join("nested")).unwrap();
    fs::write(live_settings.join("nested/current.ini"), b"keep\n").unwrap();
    fs::create_dir_all(&layout.backups_root).unwrap();
    let archive_path = layout.backups_root.join("Extensions_fixture.zip");
    let file = File::create(&archive_path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    archive
        .start_file(
            "config/plugins/screenshot/settings.ini",
            SimpleFileOptions::default(),
        )
        .unwrap();
    archive
        .write_all(b"[screenshot]\nformat = png\nhide_overlays = true\nhotkey = print_screen\n")
        .unwrap();
    archive.finish().unwrap();

    assert!(matches!(
        restore_latest_backup_in(BackupTarget::Extensions, &layout),
        Err(BackupError::UnexpectedPathType(path)) if path == live_settings
    ));
    assert_eq!(
        fs::read(live_settings.join("nested/current.ini")).unwrap(),
        b"keep\n"
    );
}

#[test]
fn extension_restore_checks_omitted_file_paths_in_replaced_units() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    let live_settings = layout
        .launcher_root
        .join("config/plugins/screenshot/settings.ini");
    fs::create_dir_all(live_settings.join("nested")).unwrap();
    fs::write(live_settings.join("nested/current.ini"), b"keep\n").unwrap();
    let live_addon = layout.launcher_root.join("addons/fixture.txt");
    fs::create_dir_all(live_addon.parent().unwrap()).unwrap();
    fs::write(&live_addon, b"live addon\n").unwrap();
    fs::create_dir_all(&layout.backups_root).unwrap();
    let archive_path = layout.backups_root.join("Extensions_fixture.zip");
    let mut archive = zip::ZipWriter::new(File::create(&archive_path).unwrap());
    for entry in ["addons/fixture.txt", "config/plugins/discord/fixture.txt"] {
        archive
            .start_file(entry, SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"replacement\n").unwrap();
    }
    archive.finish().unwrap();

    assert!(matches!(
        restore_latest_backup_in(BackupTarget::Extensions, &layout),
        Err(BackupError::UnexpectedPathType(path)) if path == live_settings
    ));
    assert_eq!(fs::read(&live_addon).unwrap(), b"live addon\n");
    assert_eq!(
        fs::read(live_settings.join("nested/current.ini")).unwrap(),
        b"keep\n"
    );

    // An unrelated replacement must not inspect paths it will not replace.
    let mut archive = zip::ZipWriter::new(File::create(&archive_path).unwrap());
    archive
        .start_file("addons/fixture.txt", SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"replacement\n").unwrap();
    archive.finish().unwrap();
    restore_latest_backup_in(BackupTarget::Extensions, &layout).unwrap();
    assert_eq!(fs::read(live_addon).unwrap(), b"replacement\n");
    assert_eq!(
        fs::read(live_settings.join("nested/current.ini")).unwrap(),
        b"keep\n"
    );
}

#[test]
fn extension_directory_unit_cannot_be_replaced_by_a_file() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    fs::create_dir_all(layout.launcher_root.join("addons/fps")).unwrap();
    fs::create_dir_all(&layout.backups_root).unwrap();
    fs::write(
        layout.launcher_root.join("addons/fps/addon.toml"),
        "live addon\n",
    )
    .unwrap();
    let archive_path = layout.backups_root.join("Extensions_fixture.zip");
    let file = File::create(&archive_path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    archive
        .start_file("addons", SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"unexpected").unwrap();
    archive.finish().unwrap();

    assert!(matches!(
        restore_latest_backup_in(BackupTarget::Extensions, &layout),
        Err(BackupError::UnsafeEntry(_))
    ));
    assert_eq!(
        fs::read_to_string(layout.launcher_root.join("addons/fps/addon.toml")).unwrap(),
        "live addon\n"
    );
}

#[test]
fn extension_backup_rejects_explicit_directory_entries() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    fs::create_dir_all(layout.launcher_root.join("addons/fps")).unwrap();
    fs::create_dir_all(&layout.backups_root).unwrap();
    fs::write(
        layout.launcher_root.join("addons/fps/addon.toml"),
        "live addon\n",
    )
    .unwrap();
    let archive_path = layout.backups_root.join("Extensions_fixture.zip");
    let file = File::create(&archive_path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    archive
        .add_directory("addons/empty/", SimpleFileOptions::default())
        .unwrap();
    archive.finish().unwrap();

    assert!(matches!(
        restore_latest_backup_in(BackupTarget::Extensions, &layout),
        Err(BackupError::UnsafeEntry(_))
    ));
    assert_eq!(
        fs::read_to_string(layout.launcher_root.join("addons/fps/addon.toml")).unwrap(),
        "live addon\n"
    );
}

#[test]
fn traversal_entry_is_rejected_before_live_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let layout = fixture_layout(&temp);
    fs::create_dir_all(layout.user_settings_root.join("user")).unwrap();
    fs::write(layout.user_settings_root.join("user/live.dat"), b"live").unwrap();
    fs::create_dir_all(&layout.backups_root).unwrap();
    let archive_path = layout.backups_root.join("UserSettings_fixture.zip");
    let file = File::create(&archive_path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    archive
        .start_file("../outside.txt", SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"unsafe").unwrap();
    archive.finish().unwrap();

    assert!(matches!(
        restore_latest_backup_in(BackupTarget::UserSettings, &layout),
        Err(BackupError::UnsafeEntry(_))
    ));
    assert_eq!(
        fs::read(layout.user_settings_root.join("user/live.dat")).unwrap(),
        b"live"
    );
    assert!(!temp.path().join("outside.txt").exists());
}

fn tree(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path.clone());
            }
            paths.push(path.strip_prefix(root).unwrap().to_path_buf());
        }
    }
    paths.sort();
    paths
}

#[cfg(unix)]
fn set_read_only(path: &Path, read_only: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mode = if read_only { 0o555 } else { 0o755 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn split_layout_backup_and_restore_write_only_under_the_state_root() {
    let temp = tempfile::tempdir().unwrap();
    let exe = dirs::write_test_bundle(temp.path());
    let bundle = temp.path().join("Bahamut Launcher.app");
    let home = temp.path().join("home");
    let state = home.join(".bahamut-launcher");
    let roots = dirs::resolve_roots(&exe, Some(state.clone())).unwrap();
    assert_eq!(roots.state, state);
    let layout = BackupLayout {
        backups_root: roots.state.join(dirs::PORTABLE_BACKUPS_DIR_NAME),
        user_settings_root: temp.path().join("documents/FINAL FANTASY XIV"),
        launcher_config_path: roots.state.join("config/bahamut.ini"),
        launcher_root: roots.state.clone(),
    };
    assert_eq!(
        layout.transaction_parent(BackupTarget::Extensions),
        roots.state
    );
    for (relative, contents) in [
        ("addons/custom/addon.toml", "user addon\n"),
        ("config/extensions.ini", "[plugins]\n\n[addons]\n"),
        ("scripts/default.txt", "/addon load fps\n"),
    ] {
        let path = roots.state.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    let shipped = roots.install.join("addons/fps/addon.toml");
    fs::create_dir_all(shipped.parent().unwrap()).unwrap();
    fs::write(&shipped, "shipped addon\n").unwrap();
    let bundle_before = tree(&bundle);

    // A restore transaction beside the state root fails against a read-only parent.
    #[cfg(unix)]
    set_read_only(&home, true);
    let archive = create_backup_in(BackupTarget::Extensions, &layout, FIXTURE_LABEL);
    fs::write(roots.state.join("addons/custom/addon.toml"), "changed\n").unwrap();
    let restored = restore_latest_backup_in(BackupTarget::Extensions, &layout);
    #[cfg(unix)]
    set_read_only(&home, false);

    let archive = archive.unwrap();
    assert_eq!(restored.unwrap(), archive);
    assert!(archive.starts_with(roots.state.join("backups")));
    assert_eq!(
        zip_names(&archive),
        [
            "addons/custom/addon.toml",
            "config/extensions.ini",
            "scripts/default.txt",
        ]
    );
    assert_eq!(
        fs::read_to_string(roots.state.join("addons/custom/addon.toml")).unwrap(),
        "user addon\n"
    );
    assert_eq!(tree(&bundle), bundle_before);
    assert_eq!(
        tree(&home).first(),
        Some(&PathBuf::from(".bahamut-launcher"))
    );
    assert!(
        tree(&home)
            .iter()
            .all(|path| path.starts_with(".bahamut-launcher")),
        "{:?}",
        tree(&home)
    );
    assert!(
        !tree(&roots.state)
            .iter()
            .any(|path| path.to_string_lossy().contains(".bahamut-restore-")),
        "restore work must be removed after commit"
    );
}
