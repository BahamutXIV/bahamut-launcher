use bahamut_launcher::config::dirs;
use bahamut_launcher::config::launcher_ini::LauncherConfig;
use bahamut_launcher::launcher::launch_args;
use bahamut_launcher::launcher::pe_patch;
use bahamut_launcher::logging;

fn main() {
    logging::init();

    tracing::info!(
        version = bahamut_launcher::version::LAUNCHER_VERSION,
        "bahamut-launcher scaffold starting; see docs/handshake.md"
    );
    tracing::info!(
        session_id_len = launch_args::SESSION_ID_LEN,
        server_utc = launch_args::SERVER_UTC_PLAINTEXT,
        "launch_args loaded"
    );

    let config_path = dirs::launcher_config_path().ok();
    if let Some(path) = config_path.as_ref() {
        tracing::info!(configuration = %path.display(), "launcher configuration path resolved");
    }

    let config = match LauncherConfig::load() {
        Ok(config) => config,
        Err(err) => {
            tracing::warn!(error = %err, "launcher configuration load failed");
            LauncherConfig::defaults()
        }
    };
    tracing::info!(
        game_location = ?config.preferences.launcher.game_location,
        selected_server = %config.selected_server,
        "launcher configuration loaded"
    );

    match config
        .servers
        .iter()
        .find(|profile| profile.display_name == config.selected_server)
    {
        Some(profile) => tracing::info!(
            display_name = %profile.display_name,
            host = %profile.host,
            api_base = %profile.api_base(),
            "server profile loaded"
        ),
        None => tracing::warn!("selected server profile is missing from bahamut.ini",),
    }

    tracing::info!(
        encryption_time_rva = format_args!("0x{:08X}", pe_patch::ENCRYPTION_TIME_PATCH_RVA),
        lobby_host_slot_bytes = pe_patch::LOBBY_HOST_NAME_SLOT_SIZE,
        "pe_patch planner loaded"
    );

    match bahamut_launcher::platform::detect_game_install() {
        Some(path) => tracing::info!(install = %path.display(), "install detection succeeded"),
        None => tracing::warn!(
            "install detection found nothing; set game_location in bahamut.ini \
             or use the Tauri UI override field",
        ),
    }
}
