use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum WindowControlAction {
    StartDragging,
    Minimize,
    Close,
}

pub(crate) trait WindowControlTarget {
    fn start_dragging_window(&self) -> tauri::Result<()>;
    fn minimize_window(&self) -> tauri::Result<()>;
    fn close_window(&self) -> tauri::Result<()>;
}

impl WindowControlTarget for tauri::WebviewWindow {
    fn start_dragging_window(&self) -> tauri::Result<()> {
        self.start_dragging()
    }

    fn minimize_window(&self) -> tauri::Result<()> {
        self.minimize()
    }

    fn close_window(&self) -> tauri::Result<()> {
        self.close()
    }
}

pub(crate) fn apply_window_control(
    window: &impl WindowControlTarget,
    action: WindowControlAction,
) -> Result<(), String> {
    match action {
        WindowControlAction::StartDragging => window.start_dragging_window(),
        WindowControlAction::Minimize => window.minimize_window(),
        WindowControlAction::Close => window.close_window(),
    }
    .map_err(|err| err.to_string())
}

#[tauri::command]
pub(crate) fn control_window(
    window: tauri::WebviewWindow,
    action: WindowControlAction,
) -> Result<(), String> {
    apply_window_control(&window, action)
}

pub(crate) fn window_size_for_work_area(
    available_width: f64,
    available_height: f64,
) -> Result<tauri::LogicalSize<f64>, String> {
    if !available_width.is_finite()
        || !available_height.is_finite()
        || available_width <= 0.0
        || available_height <= 0.0
    {
        return Err("window work area must have positive finite dimensions".to_string());
    }

    Ok(tauri::LogicalSize::new(
        available_width.min(1280.0),
        available_height.min(800.0),
    ))
}

#[tauri::command]
pub(crate) fn fit_window_to_work_area(
    window: tauri::WebviewWindow,
    available_width: f64,
    available_height: f64,
) -> Result<(), String> {
    let size = window_size_for_work_area(available_width, available_height)?;
    window.set_size(size).map_err(|err| err.to_string())
}
