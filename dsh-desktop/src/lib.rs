//! Minimal Tauri desktop client for DeepSeek Harness.
//!
//! The app is a thin shell: it hosts the harness's own web application in a
//! webview instead of reimplementing any product UI. On startup it makes sure
//! the harness web server is reachable (spawning `pnpm dsh web` when it is
//! not), then navigates the webview to `http://127.0.0.1:3080/`. The spawned
//! server process is reaped when the app exits.
//!
//! The shell also owns link routing: a URL that would navigate the webview
//! away from the app or open a second window — an external link clicked in a
//! chat message, a `target="_blank"` anchor, a `mailto:` — is handed to the
//! system default browser instead, so the client never leaves the harness UI
//! (see [`is_client_url`] and the webview handlers in [`run`]).

mod commands;
mod server;

use std::time::Duration;

use tauri::webview::{NewWindowFeatures, NewWindowResponse};
use tauri::Listener;
use url::Url;

/// Whether `url` belongs to this client's own surface: the bundled loading
/// page (the Tauri custom protocol) or the harness web app origin. Every
/// other URL is external.
///
/// The custom protocol that serves the embedded assets has a different
/// identity per platform: `tauri://localhost` on macOS and Linux, but the
/// `http(s)://tauri.localhost` workaround on Windows and Android — see
/// `Manager::tauri_protocol_url` in the tauri crate. Treating the Windows
/// form as external was a regression: the loading page would be handed to
/// the system browser at startup.
fn is_client_url(url: &Url, app: &Url) -> bool {
    url.scheme() == "tauri"
        || url.host_str() == Some("tauri.localhost")
        || (cfg!(dev) && url.host_str() == Some("localhost"))
        || (url.scheme() == app.scheme()
            && url.host_str() == app.host_str()
            && url.port_or_known_default() == app.port_or_known_default())
}

/// Hands `url` to the system default browser. Runs on a detached thread so
/// the webview callbacks never block on the opener process.
fn open_externally(url: &Url) {
    let _ = open::that_detached(url.as_str());
}

pub fn run() {
    tauri::Builder::default()
        .manage(server::ServerState::default())
        .invoke_handler(tauri::generate_handler![commands::set_repo_dir])
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
            let app_url = server::web_url();
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("loading.html".into()),
            )
            .title("DeepSeek Harness")
            .inner_size(1280.0, 820.0)
            .min_inner_size(640.0, 480.0)
            // A top-level navigation away from the app (for example clicking
            // an external link that is not handled by the harness UI) opens
            // in the system default browser instead of taking the webview
            // with it. In-app navigations keep working: the loading page and
            // the harness app origin always pass through.
            .on_navigation(move |url| {
                if is_client_url(url, &app_url) {
                    true
                } else {
                    open_externally(url);
                    false
                }
            })
            // window.open / target="_blank" (for example a link opened with
            // a modifier key) never creates a second webview window; the URL
            // goes to the system default browser.
            .on_new_window(move |url, _features: NewWindowFeatures| {
                open_externally(&url);
                NewWindowResponse::Deny
            })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_urls_are_internal_but_external_links_are_not() {
        let app = Url::parse("http://127.0.0.1:3080/").expect("static URL");
        // The bundled loading page uses the tauri:// scheme on macOS/Linux…
        assert!(is_client_url(
            &Url::parse("tauri://localhost/loading.html").expect("static URL"),
            &app
        ));
        // …and the http://tauri.localhost workaround on Windows/Android. A
        // regression here opened the loading page in the system browser at
        // startup on Windows.
        assert!(is_client_url(
            &Url::parse("http://tauri.localhost/loading.html").expect("static URL"),
            &app
        ));
        // The harness web app origin, including deep routes.
        assert!(is_client_url(&Url::parse("http://127.0.0.1:3080/").unwrap(), &app));
        assert!(is_client_url(
            &Url::parse("http://127.0.0.1:3080/some/route?x=1").unwrap(),
            &app
        ));
        // Anything else is external: other sites, other ports, mailto.
        assert!(!is_client_url(&Url::parse("https://example.com/").unwrap(), &app));
        assert!(!is_client_url(&Url::parse("http://127.0.0.1:9999/").unwrap(), &app));
        assert!(!is_client_url(&Url::parse("mailto:hi@example.com").unwrap(), &app));
    }
}
