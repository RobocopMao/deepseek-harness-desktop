//! Harness web-server bootstrapping: attach to an already-running server or
//! launch one, wait until it is reachable, then hand the webview over to it.
//!
//! Progress and the launched server's output stream to the loading page as
//! `dsh-boot-status` / `dsh-boot-log` events; failures surface as
//! `dsh-boot-error` with the captured server output tail. The page signals
//! readiness with `loading-ready` before the boot sequence starts, so no
//! early failure event is ever lost.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};
use url::Url;

/// Cap on retained server output lines.
const MAX_LOG_LINES: usize = 100;
/// Default how long to wait for the server to become reachable.
const DEFAULT_START_TIMEOUT: u64 = 180;
/// Extra directories searched for the launch command when it is not on PATH
/// (Finder-launched apps inherit a minimal PATH).
const PROGRAM_FALLBACK_DIRS: [&str; 3] = [
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "~/.local/share/pnpm",
];
/// Directories prepended to the launched server's PATH so a Finder-launched
/// app still finds `node`/`pnpm`: the launched program's own directory, the
/// common package-manager and version-manager locations, then the app's own
/// PATH as a fallback.
const PATH_EXTRA_DIRS: [&str; 5] = [
    "~/.local/share/pnpm",
    "~/.local/share/fnm",
    "~/.volta/bin",
    "~/Library/pnpm",
    "/opt/homebrew/bin",
];

/// The harness web app URL, from `DSH_WEB_URL` or the well-known default.
pub fn web_url() -> Url {
    std::env::var("DSH_WEB_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .and_then(|value| Url::parse(&value).ok())
        .unwrap_or_else(|| Url::parse("http://127.0.0.1:3080").expect("static default URL"))
}

/// The spawned harness server process, when this app launched it.
pub struct ServerState {
    pub child: Mutex<Option<Child>>,
}

impl Default for ServerState {
    fn default() -> Self {
        Self {
            child: Mutex::new(None),
        }
    }
}

/// Whether the harness web server is currently accepting connections.
fn probe(url: &Url) -> bool {
    let Some(host) = url.host_str() else { return false };
    let port = url.port_or_known_default().unwrap_or(80);
    let Ok(mut addresses) = (host, port).to_socket_addrs() else {
        return false;
    };
    let Some(address) = addresses.next() else { return false };
    TcpStream::connect_timeout(&address, Duration::from_millis(400)).is_ok()
}

fn env_or(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.trim().is_empty())
}

/// Shared capture of the launched server's stdout and stderr: keeps a tail
/// for error messages and streams every line to the loading page.
#[derive(Clone)]
struct ServerLog {
    lines: Arc<Mutex<VecDeque<String>>>,
}

impl ServerLog {
    fn new() -> Self {
        Self {
            lines: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    fn push(&self, handle: &AppHandle, line: String) {
        {
            let mut guard = self.lines.lock().unwrap();
            if guard.len() >= MAX_LOG_LINES {
                guard.pop_front();
            }
            guard.push_back(line.clone());
        }
        let _ = handle.emit("dsh-boot-log", line);
    }

    fn tail(&self) -> Vec<String> {
        let guard = self.lines.lock().unwrap();
        guard.iter().cloned().collect()
    }
}

/// Whether `path` is a DeepSeek Harness checkout root.
fn is_repo_root(path: &Path) -> bool {
    path.join("pnpm-workspace.yaml").is_file()
}

/// Walks up from `start` (at most 8 levels); at each level checks the
/// directory itself and a `deepseek-harness` sibling, so a client living
/// next to the checkout (for example `~/projects/deepseek-harness-desktop`)
/// still finds it.
fn walk_up_to_repo(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    for _ in 0..8 {
        if is_repo_root(&current) {
            return Some(current);
        }
        let sibling = current.join("deepseek-harness");
        if is_repo_root(&sibling) {
            return Some(sibling);
        }
        current = current.parent()?.to_path_buf();
    }
    None
}

/// Scans common home locations for the checkout: `~/deepseek-harness` and
/// `~/<projects>/*/deepseek-harness` for each of the usual project
/// directories.
fn scan_home_for_repo(home: &Path) -> Option<PathBuf> {
    for top in ["deepseek-harness", "Projects", "projects", "dev", "code", "src", "workspace", "work"] {
        let base = home.join(top);
        let direct = base.join("deepseek-harness");
        if is_repo_root(&direct) {
            return Some(direct);
        }
        if let Ok(entries) = std::fs::read_dir(&base) {
            for entry in entries.flatten() {
                let nested = entry.path().join("deepseek-harness");
                if is_repo_root(&nested) {
                    return Some(nested);
                }
            }
        }
    }
    None
}

/// The checkout root to launch the server from, tried in order:
/// `DSH_REPO_DIR`, a walk up from the current directory, a walk up from the
/// executable, then a scan of common home locations. Finder-launched apps
/// have `/` as their working directory, so the home scan is what makes them
/// work.
fn resolve_repo_dir() -> Option<PathBuf> {
    if let Some(dir) = env_or("DSH_REPO_DIR") {
        let dir = PathBuf::from(dir);
        if is_repo_root(&dir) {
            return Some(dir);
        }
    }
    if let Some(dir) = std::env::current_dir().ok().and_then(|cwd| walk_up_to_repo(&cwd)) {
        return Some(dir);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent().and_then(|parent| walk_up_to_repo(parent)) {
            return Some(dir);
        }
    }
    std::env::var("HOME").ok().map(PathBuf::from).and_then(|home| scan_home_for_repo(&home))
}

/// Locates `name` on PATH (or among [`PROGRAM_FALLBACK_DIRS`]); a name with a
/// path separator is used as-is.
fn resolve_program(name: &str) -> Option<String> {
    if name.contains('/') {
        return Path::new(name).is_file().then(|| name.to_string());
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = env_or("PATH") {
        candidates.extend(path.split(':').map(PathBuf::from));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    for dir in PROGRAM_FALLBACK_DIRS {
        candidates.push(PathBuf::from(dir.replace('~', &home)));
    }
    for dir in candidates {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

fn spawn_log_reader(stream: impl Read + Send + 'static, handle: AppHandle, log: ServerLog) {
    std::thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let trimmed = line.trim_end().to_string();
            if !trimmed.is_empty() {
                log.push(&handle, trimmed);
            }
        }
    });
}

/// A PATH for the launched server that works even when this app was started
/// from Finder (whose processes inherit a minimal PATH): the launched
/// program's own directory, common node/pnpm and version-manager locations
/// (`~/.nvm/versions/node/*/bin`, fnm), then the app's own PATH as a
/// fallback. Deduplicated, order-preserving.
fn augmented_path(resolved_program: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs: Vec<String> = Vec::new();
    let mut push = |dir: String| {
        if !dir.is_empty() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    };
    if let Some(dir) = Path::new(resolved_program).parent() {
        push(dir.to_string_lossy().into_owned());
    }
    for dir in PATH_EXTRA_DIRS {
        push(dir.replace('~', &home));
    }
    for version_dir in [
        format!("{home}/.nvm/versions/node"),
        format!("{home}/.local/share/fnm/node-versions"),
    ] {
        if let Ok(entries) = std::fs::read_dir(&version_dir) {
            let mut versions: Vec<String> = entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .map(|path| path.join("bin").to_string_lossy().into_owned())
                .collect();
            versions.sort();
            for dir in versions {
                push(dir);
            }
        }
    }
    if let Some(path) = env_or("PATH") {
        for dir in path.split(':') {
            push(dir.to_string());
        }
    }
    dirs.join(":")
}

/// Launches the harness web server on the port of `url`. The command comes
/// from `DSH_WEB_COMMAND` (whitespace-split argv) or defaults to
/// `pnpm dsh web --port <port>`, so the spawned server listens exactly where
/// the app probes; the working directory comes from [`resolve_repo_dir`].
/// Server output is captured and streamed to the loading page.
fn spawn_harness(handle: &AppHandle, log: &ServerLog, url: &Url) -> Result<Child, String> {
    let argv: Vec<String> = match env_or("DSH_WEB_COMMAND") {
        Some(command) => command.split_whitespace().map(str::to_string).collect(),
        None => {
            let port = url.port_or_known_default().unwrap_or(80).to_string();
            vec!["pnpm".into(), "dsh".into(), "web".into(), "--port".into(), port]
        }
    };
    if argv.is_empty() {
        return Err("DSH_WEB_COMMAND must not be empty".into());
    }
    let program = resolve_program(&argv[0]).ok_or_else(|| {
        format!(
            "could not find `{}`.\n\
             Launch the app from a terminal (where PATH includes node and pnpm), \
             or set DSH_WEB_COMMAND to the full command and DSH_REPO_DIR to the \
             harness checkout.",
            argv[0]
        )
    })?;
    let repo_dir = resolve_repo_dir().ok_or_else(|| {
        "could not locate the DeepSeek Harness checkout.\n\
         Launch the app from the checkout directory, or set DSH_REPO_DIR to its path."
            .to_string()
    })?;
    let mut command = Command::new(&program);
    command.args(&argv[1..]);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.env("PATH", augmented_path(&program));
    command.current_dir(&repo_dir);
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to launch `{}`: {error}", argv.join(" ")))?;
    if let Some(stdout) = child.stdout.take() {
        spawn_log_reader(stdout, handle.clone(), log.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_log_reader(stderr, handle.clone(), log.clone());
    }
    Ok(child)
}

/// Boots the app onto the harness web app: ensures the server is reachable
/// (spawning it when needed), streams progress to the loading page, then
/// navigates the main window to it. On failure, emits `dsh-boot-error`.
pub fn boot(handle: AppHandle) {
    let url = web_url();
    let log = ServerLog::new();
    if probe(&url) {
        let _ = handle.emit("dsh-boot-status", format!("{url} is already listening — attaching"));
    } else {
        let repo = resolve_repo_dir()
            .map(|dir| format!(" (repo: {})", dir.display()))
            .unwrap_or_default();
        let _ = handle.emit(
            "dsh-boot-status",
            format!("launching the harness web server for {url}{repo}"),
        );
        match spawn_harness(&handle, &log, &url) {
            Ok(child) => {
                let state = handle.state::<ServerState>();
                *state.child.lock().unwrap() = Some(child);
            }
            Err(message) => {
                let _ = handle.emit("dsh-boot-error", message);
                return;
            }
        }
    }
    let timeout_secs = env_or("DSH_WEB_START_TIMEOUT_SECS")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_START_TIMEOUT);
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let start = Instant::now();
    let mut last_report = Instant::now();
    let failure: Option<String> = loop {
        if probe(&url) {
            break None;
        }
        let exited = {
            let state = handle.state::<ServerState>();
            let mut guard = state.child.lock().unwrap();
            match guard.as_mut() {
                Some(child) => child.try_wait().ok().flatten(),
                None => None,
            }
        };
        if let Some(status) = exited {
            let code = status
                .code()
                .map(|code| format!(" (exit code {code})"))
                .unwrap_or_default();
            let tail = log.tail();
            let suffix = if tail.is_empty() {
                String::new()
            } else {
                format!("\nserver output:\n{}", tail.join("\n"))
            };
            break Some(format!(
                "the harness web server exited{code} before becoming ready{suffix}"
            ));
        }
        if Instant::now() >= deadline {
            break Some(format!("timed out waiting for {url} after {timeout_secs}s"));
        }
        if Instant::now().duration_since(last_report) >= Duration::from_secs(2) {
            let _ = handle.emit(
                "dsh-boot-status",
                format!("waiting for the web server… ({:.0}s)", start.elapsed().as_secs_f32()),
            );
            last_report = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(300));
    };
    match failure {
        None => {
            if let Some(window) = handle.get_webview_window("main") {
                let _ = window.navigate(url);
            }
        }
        Some(message) => {
            let _ = handle.emit("dsh-boot-error", message);
        }
    }
}

/// Reaps the harness server process this app spawned, if any.
pub fn kill_child(handle: &AppHandle) {
    let state = handle.state::<ServerState>();
    let mut guard = state.child.lock().unwrap();
    if let Some(child) = guard.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    *guard = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_detects_a_listening_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind must succeed");
        let address = listener.local_addr().expect("local address");
        let url = Url::parse(&format!("http://127.0.0.1:{}", address.port())).unwrap();
        assert!(probe(&url));
    }

    #[test]
    fn probe_rejects_an_unused_port() {
        let url = Url::parse("http://127.0.0.1:9").unwrap(); // discard port: nothing listens
        assert!(!probe(&url));
    }

    #[test]
    fn web_url_defaults_to_the_harness_web_app() {
        // Env mutation is unsafe in edition 2024; this test is single-threaded
        // for the variable it touches.
        unsafe { std::env::remove_var("DSH_WEB_URL") };
        assert_eq!(web_url().as_str(), "http://127.0.0.1:3080/");
    }

    #[test]
    fn resolve_program_finds_an_absolute_path() {
        let program = resolve_program("/bin/ls").expect("absolute path must resolve");
        assert_eq!(program, "/bin/ls");
    }

    #[test]
    fn resolve_program_rejects_a_missing_name() {
        assert!(resolve_program("definitely-not-a-real-command-xyz").is_none());
    }

    /// Builds a fabricated layout under the crate dir: a fake harness
    /// checkout (`deepseek-harness`) and a standalone client next to it,
    /// mirroring how this repo lives beside the official checkout. Returns
    /// the checkout and the client's src dir.
    fn make_fake_layout(manifest: &Path) -> (PathBuf, PathBuf) {
        let layout = manifest.join(".tmp-layout");
        let checkout = layout.join("deepseek-harness");
        let client_src = layout.join("client-standalone").join("src");
        std::fs::create_dir_all(&checkout).expect("fake checkout must be creatable");
        std::fs::create_dir_all(&client_src).expect("fake client must be creatable");
        std::fs::write(checkout.join("pnpm-workspace.yaml"), "").expect("marker must be writable");
        (checkout, client_src)
    }

    #[test]
    fn repo_detection_uses_env_then_walk_with_sibling_check() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let (checkout, client_src) = make_fake_layout(&manifest);

        assert!(is_repo_root(&checkout));
        assert!(!is_repo_root(&manifest));
        // Walking up from inside the checkout finds the checkout itself.
        assert_eq!(walk_up_to_repo(&checkout.join("packages").join("x")), Some(checkout.clone()));
        // Walking up from a standalone client finds the sibling checkout.
        assert_eq!(walk_up_to_repo(&client_src), Some(checkout.clone()));

        std::env::set_current_dir(&client_src).expect("fake client src must be accessible");
        // A non-root DSH_REPO_DIR is rejected, and the cwd walk finds the
        // sibling checkout.
        unsafe { std::env::set_var("DSH_REPO_DIR", manifest.as_os_str()) };
        assert_eq!(resolve_repo_dir(), Some(checkout.clone()));
        // A valid DSH_REPO_DIR wins directly.
        unsafe { std::env::set_var("DSH_REPO_DIR", checkout.as_os_str()) };
        assert_eq!(resolve_repo_dir(), Some(checkout.clone()));
        unsafe { std::env::remove_var("DSH_REPO_DIR") };

        std::fs::remove_dir_all(manifest.join(".tmp-layout")).expect("fake layout must be removable");
    }

    #[test]
    fn augmented_path_includes_common_locations_and_dedupes() {
        let path = augmented_path("/opt/homebrew/bin/pnpm");
        let dirs: Vec<&str> = path.split(':').collect();
        assert!(dirs.contains(&"/opt/homebrew/bin"));
        assert!(dirs.first().is_some_and(|first| *first == "/opt/homebrew/bin"));
        let mut seen = std::collections::HashSet::new();
        for dir in &dirs {
            assert!(seen.insert(*dir), "duplicate PATH entry: {dir}");
        }
    }

    #[test]
    fn home_scan_finds_a_nested_checkout() {
        // Fake home under the crate: ~/Projects/mine/deepseek-harness.
        let home = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".tmp-home");
        let checkout = home.join("Projects").join("mine").join("deepseek-harness");
        std::fs::create_dir_all(&checkout).expect("fake checkout must be creatable");
        std::fs::write(checkout.join("pnpm-workspace.yaml"), "").expect("marker must be writable");
        let resolved = scan_home_for_repo(&home).expect("nested checkout must be found");
        assert_eq!(resolved, checkout);
        std::fs::remove_dir_all(&home).expect("fake home must be removable");
    }
}
