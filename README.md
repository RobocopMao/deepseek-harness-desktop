# rust/ — Desktop client

English | [中文](README.zh.md)

A minimal [Tauri](https://tauri.app) desktop client for DeepSeek Harness. It is a thin shell, not a reimplementation: it hosts the harness's own web application in a webview and manages the harness web server process.

## What it does

On startup the client makes sure the harness web server is reachable, then hands the webview over to it:

1. Probes the web app URL (`http://127.0.0.1:3080/` by default).
2. When nothing is listening, launches the harness server (`pnpm dsh web --port <target port>`, i.e. `dsh --profile web`) and waits until it accepts connections.
3. Navigates the webview to the URL. All product UI comes from the harness itself — the browser window and the server lifecycle are all this client owns.
4. On exit, reaps the server process it spawned. A server you started yourself is left untouched.

The harness needs a build first (`pnpm run build`) and a `DEEPSEEK_API_KEY` in the environment, exactly as when running `pnpm dsh web` from a terminal.

## Run

```sh
# from a DeepSeek Harness checkout (after pnpm install + pnpm run build)
cd rust
cargo run -p dsh-desktop
```

Launch from a terminal so the harness server inherits `PATH` (node/pnpm) and `DEEPSEEK_API_KEY`.

## Package a DMG

The release build is a plain cargo build plus the Tauri bundler; the app icon
set lives in `dsh-desktop/icons/` and is regenerated from `app-icon.png`
(1024×1024) with the Tauri CLI:

```sh
cd rust/dsh-desktop
npx --yes @tauri-apps/cli@^2 icon app-icon.png   # regenerate the icon set
npx --yes @tauri-apps/cli@^2 build --bundles dmg # release build + DMG
```

The DMG lands at `rust/target/release/bundle/dmg/`. The bundle is
unsigned: on other machines macOS Gatekeeper asks to verify the developer —
right-click the app and choose Open, or sign with a Developer ID and
notarize for friction-free distribution.

## Update after pulling new code

After `git pull`, rebuild the harness and repackage the client:

```sh
# 1. harness: the web server and web UI the client hosts
pnpm install          # only needed when dependencies changed
pnpm run build

# 2. desktop client DMG
cd rust/dsh-desktop
npx --yes @tauri-apps/cli@^2 build --bundles dmg
```

Install the fresh DMG from `rust/target/release/bundle/dmg/`. To bump the
package version, edit `version` in `rust/dsh-desktop/tauri.conf.json` first.
If cargo stalls updating the crates.io index, use the mirror in the
troubleshooting section below.

## Troubleshooting: `cargo run` hangs on "Updating crates.io index"

The first build downloads the crates.io index into `~/.cargo`; on flaky or
throttled networks the fetch can stall. When the repository's own build cache
is present, skip the network entirely:

```sh
cd rust
CARGO_HOME="$PWD/.cargo-home" cargo run -p dsh-desktop
```

For a permanent fix, point cargo at a mirror in `~/.cargo/config.toml`, for
example:

```toml
[source.crates-io]
replace-with = "rsproxy-sparse"

[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"
```

## Configuration

Environment variables, read at startup:

| Variable | Default | Meaning |
|---|---|---|
| `DSH_WEB_URL` | `http://127.0.0.1:3080` | The harness web app URL to attach to. |
| `DSH_WEB_COMMAND` | `pnpm dsh web --port <url port>` | Whitespace-split argv used to launch the server when nothing is listening. The default injects the target URL's port so the server listens where the client probes; a custom command is used verbatim. |
| `DSH_REPO_DIR` | auto-detected | Working directory for the launched server. When unset, the checkout is found by walking up from the working directory and the executable, then by scanning common home locations. |
| `DSH_WEB_START_TIMEOUT_SECS` | `180` | How long to wait for the server to become reachable. |

When the server is already running (for example `pnpm dsh web` in another terminal), the client attaches without launching anything.

## Portability

The DMG is a thin shell, not a standalone app: it hosts the harness web app
and, when needed, launches `pnpm dsh web` from a DeepSeek Harness checkout on
the same machine. On another Mac you therefore need, once:

```sh
git clone https://github.com/deepseek-ai/deepseek-harness
cd deepseek-harness
pnpm install
```

plus `node`/`pnpm` (Homebrew, nvm, fnm, or volta — the client's PATH
auto-detection covers these) and, for real agent turns, `DEEPSEEK_API_KEY` in
the environment. The client is version-agnostic about the harness: it renders
whatever web app the checkout serves, and it can attach to a server you
started yourself (`pnpm dsh web`) without needing the checkout at all.

The current DMG is built for Apple Silicon (`aarch64`); Intel Macs need an
`x86_64-apple-darwin` build.

## Layout

```
rust/
  dsh-desktop/   the Tauri app (Rust core + loading page)
```

The loading page under `dsh-desktop/ui/` is the only frontend asset this client owns; it shows progress and startup errors until the harness web app loads.

## Known Limitations and Deferred Work

- Packaging (`tauri build`) requires the Tauri CLI, fetched on demand with `npx`; `cargo build`/`cargo run` remains the development workflow.
- `DSH_WEB_COMMAND` is split on whitespace; quoted arguments are not supported.
- If the server exits while the app is running, the webview shows the connection error; the client does not restart the server.
