use std::path::Path;

use bahamut_launcher::config::dirs;
use bahamut_launcher::diagnostics::{free_disk_bytes, system_diagnostics};
use bahamut_launcher::extensions::layout::ExtensionLayout;
use chrono::{DateTime, SecondsFormat, Utc};
use tauri::Manager;

use crate::shell_config::{resolve_download_cache_dir, resolve_game_dir};

pub(crate) fn log_support_diagnostics(log_path: Option<&Path>) {
    tracing::info!("--- SUPPORT DIAGNOSTICS ---");
    tracing::info!(
        "Launcher version = {}",
        bahamut_launcher::version::LAUNCHER_VERSION
    );

    let roots = match dirs::launcher_roots() {
        Ok(roots) => roots,
        Err(error) => {
            tracing::warn!(%error, "Install root = unavailable");
            return;
        }
    };
    let layout = ExtensionLayout::new(&roots.install, &roots.state);
    let config_root = dirs::portable_config_dir().ok();
    let game_root = resolve_game_dir();
    let download_cache = resolve_download_cache_dir().ok();

    log_path_value("Install root", Some(&roots.install));
    log_path_value("State root", Some(&roots.state));
    log_path_value("Game root", game_root.as_deref());
    log_path_value("Download cache", download_cache.as_deref());
    log_path_value("Config root", config_root.as_deref());
    log_path_value("Logs root", Some(&layout.logs));
    log_path_value("Shipped addons root", Some(&layout.shipped_addons));
    log_path_value("Addons root", Some(&layout.addons));
    log_path_value("Shipped DAT packages root", Some(&layout.shipped_dats));
    log_path_value("DAT packages root", Some(&layout.dats));
    log_path_value("Plugins root", Some(&layout.plugins));
    log_path_value("Screenshots root", Some(&layout.screenshots));
    log_path_value("Launcher log", log_path);

    let system = system_diagnostics();
    tracing::info!(
        "Platform = {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    tracing::info!("OS = {}", system.os);
    tracing::info!(
        "CPU = {} | Logical cores: {}",
        system.cpu,
        system.logical_cores
    );
    match (system.memory_total_bytes, system.memory_available_bytes) {
        (Some(total), Some(available)) => tracing::info!(
            "RAM = Total: {} | Available: {}",
            format_bytes(total),
            format_bytes(available)
        ),
        _ => tracing::info!("RAM = unavailable"),
    }
    if system.gpus.is_empty() {
        tracing::info!("GPU = unavailable");
    } else {
        tracing::info!("GPU = {}", system.gpus.join(" | "));
    }

    log_free_disk("install root", &roots.install);
    log_free_disk("state root", &roots.state);
    if let Some(download_cache) = download_cache.as_deref() {
        log_free_disk("download cache", download_cache);
    }

    if let Ok(executable) = std::env::current_exe() {
        log_artifact("Launcher executable", &executable);
    } else {
        tracing::warn!("Launcher executable = unavailable");
    }
    let (loader, module) = crate::extensions::packaged_client_paths_under(&roots.install);
    log_artifact("Client loader", &loader);
    log_artifact("Client module", &module);
    log_artifact("Screenshot plugin", &layout.screenshot_plugin_path);
    log_artifact("DiscordRPC plugin", &layout.discord_rpc_plugin_path);
}

pub(crate) fn log_display_diagnostics(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        tracing::info!("Screens = unavailable (main window missing)");
        return;
    };
    match window.available_monitors() {
        Ok(monitors) if monitors.is_empty() => tracing::info!("Screens = none reported"),
        Ok(monitors) => {
            for (index, monitor) in monitors.iter().enumerate() {
                let size = monitor.size();
                let name = monitor.name().map(String::as_str).unwrap_or("unknown");
                tracing::info!(
                    "Screen {} = {} {}x{} @ {:.2}x",
                    index + 1,
                    name,
                    size.width,
                    size.height,
                    monitor.scale_factor()
                );
            }
        }
        Err(error) => tracing::info!(%error, "Screens = unavailable"),
    }
}

fn log_path_value(label: &str, path: Option<&Path>) {
    match path {
        Some(path) => tracing::info!("{label} = {}", path.display()),
        None => tracing::info!("{label} = unavailable"),
    }
}

fn log_free_disk(label: &str, path: &Path) {
    match free_disk_bytes(path) {
        Some(bytes) => tracing::info!("Free disk ({label}) = {}", format_bytes(bytes)),
        None => tracing::info!("Free disk ({label}) = unavailable"),
    }
}

fn log_artifact(label: &str, path: &Path) {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            let modified = metadata
                .modified()
                .ok()
                .map(DateTime::<Utc>::from)
                .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Millis, true))
                .unwrap_or_else(|| "unavailable".to_string());
            tracing::info!(
                "{label} = present, {}, modified {modified}",
                format_bytes(metadata.len())
            );
        }
        Ok(_) => tracing::warn!("{label} = invalid (not a file)"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!("{label} = missing")
        }
        Err(error) => tracing::warn!(%error, "{label} = unavailable"),
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.2} GB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.2} MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.2} KB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

#[cfg(test)]
mod tests {
    use super::format_bytes;

    #[test]
    fn byte_format_is_stable_for_support_logs() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024), "3.00 MB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.00 GB");
    }
}
