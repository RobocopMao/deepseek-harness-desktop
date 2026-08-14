//! Minimal Tauri desktop client for DeepSeek Harness.
//!
//! The app is a thin shell: it hosts the harness's own web application in a
//! webview instead of reimplementing any product UI. On startup it makes sure
//! the harness web server is reachable (spawning `pnpm dsh web` when it is
//! not), then navigates the webview to `http://127.0.0.1:3080/`. The spawned
//! server process is reaped when the app exits.

mod server;

use std::time::Duration;

use tauri::Listener;

pub fn run() {
    tauri::Builder::default()
        .manage(server::ServerState::default())
        .setup(|app| {
            let handle = app.handle().clone();
            // The loading page signals that its event listeners are
            // registered; the boot sequence waits for it (or a short grace
            // period) so an early failure event is never lost to a page that
            // is still loading.
            let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
            handle.listen("loading-ready", move |_| {
                let _ = ready_tx.send(());
            });
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("loading.html".into()),
            )
            .title("DeepSeek Harness Desktop")
            .inner_size(1280.0, 820.0)
            .min_inner_size(640.0, 480.0)
            .build()?;
            std::thread::spawn(move || {
                let _ = ready_rx.recv_timeout(Duration::from_millis(1500));
                server::boot(handle);
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                server::kill_child(app_handle);
            }
        });
}
