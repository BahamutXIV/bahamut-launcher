use super::frontend_source;

#[test]
fn frontend_gamepad_poll_reads_only_the_active_controller() {
    let frontend = frontend_source();
    assert!(frontend.contains(
        "readControllerActions(activeGamepad, controllerStates.get(activeGamepad.index))"
    ));
    assert!(!frontend.contains("for (const gamepad of gamepads.filter(Boolean))"));
}

#[test]
fn settings_frontend_matches_flat_backend_contract() {
    let frontend = frontend_source();
    assert!(frontend.contains("invoke('get_game_settings')"));
    assert!(frontend.contains("invoke('set_game_settings', { settings:next })"));
    assert!(frontend.contains("invoke('get_launcher_behavior')"));
    assert!(frontend.contains("invoke('set_close_on_game_start', { closeOnGameStart })"));
    assert!(
        frontend.contains("invoke('set_native_resolution_override', { nativeResolutionOverride })")
    );
    assert!(frontend.contains("invoke(action === 'create' ? 'create_backup' : 'restore_backup'"));
    assert!(frontend.contains("User Settings and Macros"));
    assert!(frontend.contains("data-backup-target=\"extensions\""));
    assert!(frontend.contains("Open Backup Folder"));
    assert!(frontend.contains("Open Install Folder"));
    assert!(frontend.contains("id=\"settings-confirmation-dialog\""));
    assert!(frontend.contains("data-settings-tab=\"general\""));
    assert!(frontend.contains("data-settings-tab=\"graphics\""));
    assert!(frontend.contains("data-settings-tab=\"misc\""));
    assert!(!frontend.contains("maintenance"));
    assert!(!frontend.contains("data-settings-tab=\"controls\""));
    assert!(!frontend.contains("id=\"screenshot-hotkey\""));
    assert!(frontend.contains("Close Launcher on Game Start"));
    assert!(frontend.contains("Native Resolution Override"));
    assert!(!frontend.contains(">Language<"));
    assert!(frontend.contains("data-action=\"open-profiles\""));
    assert!(frontend.contains("data-screen=\"profiles\""));
    assert!(frontend.contains("data-help-action=\"open\""));
    assert!(frontend.contains("Incorrect username or password."));
    assert!(!frontend.contains("The username or password was not accepted."));
    assert!(!frontend.contains("Game settings saved."));
    assert!(!frontend.contains("data-game-settings-tab"));
    assert!(frontend.contains("data-settings-action=\"open-config\""));
    assert!(frontend.contains("config_tool_supported"));
    assert!(frontend.contains("launch_config_tool"));
    assert!(!frontend.contains("data-extension-folder=\"logs\""));
    assert!(!frontend.contains("get_extension_runtime_status"));
    assert!(!frontend.contains("run_extension_diagnostics"));
    assert!(!frontend.contains("Overlay packages"));
    assert!(!frontend.contains("Enabled packages are applied in first-hit order."));
    assert!(!frontend.contains("Open Overlay Folder"));
    assert!(!frontend.contains("data-extension-folder=\"dats\""));
    assert!(!frontend.contains("Overlay scale"));
    assert!(!frontend.contains("Update channel"));
    assert!(!frontend.contains("data-extension-action"));
    assert!(!frontend.contains("value=\"Ctrl+Shift+O\""));
    assert!(!frontend.contains("Use the packaged runtime on the next game launch"));
    assert!(!frontend.contains("Extensions were quarantined"));
    assert!(!frontend.contains("Steam Deck support is unavailable."));
    assert!(frontend.contains("Steam Deck not detected."));
    assert!(frontend.contains("Apply Steam Deck Defaults"));
    assert!(!frontend.contains("The current controller remains active while connected"));
    assert!(!frontend.contains("first connected controller with recent input"));
    assert!(!frontend.contains("id=\"shell-status\""));
    assert!(!frontend.contains(" title=\""));
}
