use std::path::Path;

use crate::presentation::GameInstallInfo;
use crate::shell_config::{resolve_game_dir_with_source, update_launcher_config};
use crate::state::GameIpcState;

#[tauri::command]
pub(crate) fn detect_game_install_command() -> GameInstallInfo {
    let (path, source) = resolve_game_dir_with_source();
    GameInstallInfo {
        detected: path.map(|p| p.to_string_lossy().into_owned()),
        source: source.into(),
    }
}

/// Async so the callback result can be awaited off the runtime; blocking_pick_folder deadlocks here.
#[tauri::command]
pub(crate) async fn pick_install_dir(
    app: tauri::AppHandle,
    state: tauri::State<'_, GameIpcState>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().pick_folder(move |picked| {
        let _ = tx.send(picked);
    });

    let picked = tauri::async_runtime::spawn_blocking(move || rx.recv().ok().flatten())
        .await
        .map_err(|error| format!("Folder selection failed: {error}"))?;
    let Some(picked) = picked else {
        return Ok(None);
    };

    let path = picked.into_path().map_err(|error| error.to_string())?;

    let _reservation = state.begin_restore().ok_or("Wait for game, install, launch, or restore work to finish before selecting another folder.")?;

    persist_game_location(&path)?;

    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Async for the same main-thread dialog constraint as [`pick_install_dir`]; it does not persist `game_location`.
#[tauri::command]
pub(crate) async fn pick_directory(app: tauri::AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;

    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().pick_folder(move |picked| {
        let _ = tx.send(picked);
    });

    let picked = tauri::async_runtime::spawn_blocking(move || rx.recv().ok().flatten())
        .await
        .ok()
        .flatten()?;

    let path = picked.into_path().ok()?;
    Some(path.to_string_lossy().into_owned())
}

/// Persists `game_location` without replacing other preference fields.
pub(crate) fn persist_game_location(path: &Path) -> Result<(), String> {
    update_launcher_config(|config| {
        config.preferences.launcher.game_location = Some(path.to_path_buf());
        Ok(())
    })
    .map(|_| ())
}
