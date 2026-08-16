//! Tauri commands for the loading page.

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

use crate::server;

/// Sets the DeepSeek Harness checkout path, persists it to the client
/// config, and restarts the boot sequence (reloading the loading page).
/// Reachable from the startup error screen for layouts no heuristic can
/// auto-detect, such as the client and the checkout on different drives.
#[tauri::command]
pub fn set_repo_dir(app: AppHandle, path: String) -> Result<(), String> {
    let dir = PathBuf::from(path);
    if !server::is_repo_root(&dir) {
        return Err(format!(
            "{} is not a DeepSeek Harness checkout (no pnpm-workspace.yaml)",
            dir.display()
        ));
    }
    server::write_repo_config(&server::config_path(), &dir)?;
    let handle = app.clone();
    std::thread::spawn(move || server::boot(handle));
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.eval("window.location.href = 'loading.html';");
    }
    Ok(())
}
