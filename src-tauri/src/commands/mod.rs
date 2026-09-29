pub(crate) mod auth;
pub(crate) mod extensions;
pub(crate) mod home;
pub(crate) mod install;
pub(crate) mod launcher_updates;
pub(crate) mod repair;
pub(crate) mod settings;
pub(crate) mod support;
pub(crate) mod window;

pub(crate) use auth::{game_status, launch_game, login, logout, register, validate_session};
pub(crate) use extensions::{
    get_camera_zoom_selection, get_extension_inventory, get_object_distance_selection,
    open_extension_folder, reorder_dat_package, set_addon_enabled, set_camera_zoom_selection,
    set_dat_package_enabled, set_discord_rpc_enabled, set_object_distance_selection,
    set_screenshot_enabled,
};
pub(crate) use home::{get_home_status, launcher_version, list_news};
pub(crate) use install::{
    cancel_install, detect_game_install_command, install_game, install_quote, install_status,
    pause_install, pick_directory, pick_install_dir, reset_install, resume_install,
};
pub(crate) use launcher_updates::{
    apply_launcher_update, check_launcher_update, get_launcher_update_status,
    launcher_update_restart_available,
};
pub(crate) use repair::{
    cancel_game_repair, game_repair_status, pause_game_repair, resume_game_repair,
    start_game_repair,
};
pub(crate) use settings::{
    config_tool_supported, get_borderless_monitors, get_game_settings, get_launcher_behavior,
    get_server_settings, launch_config_tool, save_server_profile, set_borderless_monitor,
    set_close_on_game_start, set_game_settings, set_native_resolution_override,
    set_selected_server,
};
pub(crate) use support::{
    create_backup, get_launcher_log, open_external, open_launcher_log, restore_backup,
};
pub(crate) use window::{control_window, fit_window_to_work_area};
