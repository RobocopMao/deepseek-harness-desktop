# deepseek-harness-desktop — 桌面客户端

[English](README.md) | 中文

一个基于 [Tauri](https://tauri.app) 的 DeepSeek Harness 最小桌面客户端。它是薄外壳而不是重实现:在 webview 中承载 harness 自带的 web 应用,并负责管理 harness web 服务器进程。

## 它做什么

启动时,客户端确保 harness web 服务器可达,然后把 webview 交给它:

1. 探测 web 应用地址(默认为 `http://127.0.0.1:3080/`)。
2. 没有服务在监听时,启动 harness 服务器(`pnpm dsh web --no-open --port <目标端口>`,即 `dsh --profile web`),等待其接受连接。`--no-open` 让服务器不再把 URL 交给系统默认浏览器:webview 就是客户端自己的窗口,再弹一个浏览器标签页只会重复。
3. 将 webview 导航到该地址。所有产品 UI 都来自 harness 本身——本客户端只拥有浏览器窗口和服务器生命周期。
4. 退出时回收它启动的服务器进程;你自己启动的服务器保持不动。

和终端里运行 `pnpm dsh web` 一样,harness 需要先构建(`pnpm run build`),环境里需要有 `DEEPSEEK_API_KEY`。

## 运行

```sh
# from a DeepSeek Harness checkout (after pnpm install + pnpm run build)
cargo run -p dsh-desktop
```

请从终端启动,这样 harness 服务器能继承 `PATH`(node/pnpm)和 `DEEPSEEK_API_KEY`。

## 打包 DMG

发布构建就是普通 cargo 构建加上 Tauri 打包器;应用图标集位于 `dsh-desktop/icons/`,由 `app-icon.png`(1024×1024)通过 Tauri CLI 生成:

```sh
cd dsh-desktop
npx --yes @tauri-apps/cli@^2 icon app-icon.png   # regenerate the icon set
npx --yes @tauri-apps/cli@^2 build --bundles dmg # release build + DMG
```

DMG 输出在 `target/release/bundle/dmg/`。该包未签名:在其他机器上,macOS Gatekeeper 会要求验证开发者——右键应用选择"打开",或用 Developer ID 签名并公证,才能无摩擦分发。

## 拉取新代码后更新

`git pull` 之后,依次重建 harness 并重新打包客户端:

```sh
# 1. harness: the web server and web UI the client hosts
pnpm install          # only needed when dependencies changed
pnpm run build

# 2. desktop client DMG
cd dsh-desktop
npx --yes @tauri-apps/cli@^2 build --bundles dmg
```

安装 `target/release/bundle/dmg/` 下的新 DMG。想改版本号,先编辑 `dsh-desktop/tauri.conf.json` 里的 `version`。若 cargo 更新 crates.io 索引卡住,用下方排障章节里的镜像。

## 排障:`cargo run` 卡在 "Updating crates.io index"

首次构建会把 crates.io 索引下载到 `~/.cargo`;网络不稳或被限速时,拉取可能一直挂着。当仓库内已有完整构建缓存时,可以完全跳过网络:

```sh
CARGO_HOME="$PWD/.cargo-home" cargo run -p dsh-desktop
```

想一劳永逸,可在 `~/.cargo/config.toml` 里把 cargo 指向镜像,例如:

```toml
[source.crates-io]
replace-with = "rsproxy-sparse"

[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"
```

## 配置

启动时读取的环境变量:

| 变量 | 默认值 | 含义 |
|---|---|---|
| `DSH_WEB_URL` | `http://127.0.0.1:3080` | 要附着的 harness web 应用地址。 |
| `DSH_WEB_COMMAND` | `pnpm dsh web --no-open --port <url 端口>` | 没有服务监听时,用于启动服务器的按空白拆分的 argv。默认命令会注入目标 URL 的端口让服务器在客户端探测的位置监听,并附加 `--no-open` 让服务器不再弹出系统默认浏览器(webview 就是客户端自己的窗口);自定义命令原样使用。 |
| `DSH_REPO_DIR` | 自动检测 | 启动服务器时的工作目录。未设置时,依次从持久化配置、当前工作目录、可执行文件路径向上查找,再扫描常见家目录位置来定位检出目录。 |
| `DSH_WEB_START_TIMEOUT_SECS` | `180` | 等待服务器可达的秒数。 |

当服务器已在运行(例如在另一个终端里执行了 `pnpm dsh web`),客户端直接附着,不启动任何东西。

## Windows

客户端同样支持 Windows 构建和运行。安装器需要在 Windows 机器上构建(Tauri 不支持从 macOS 交叉编译),需要 Rust MSVC 工具链、WebView2 和 Node:

```sh
npx --yes @tauri-apps/cli@^2 build
```

NSIS 安装器输出在 `target/release/bundle/nsis/`。仓库自带的 `build-installers` 工作流会在每次打 `v*` 标签(以及手动触发)时,在 GitHub Actions 上构建 macOS DMG 和 Windows 安装器并挂到 Release,因此不需要本地 Windows 机器。

Windows 上的运行要求和 macOS 一致:需要 DeepSeek Harness 检出目录(`git clone` + `pnpm install`)、`node`/`pnpm` 和 `DEEPSEEK_API_KEY`;客户端通过兄弟目录、`DSH_REPO_DIR` 或配置文件定位检出目录。当自动检测找不到检出目录时(例如客户端和检出目录在不同盘),在启动错误页输入检出路径并点"Save & retry":路径会被持久化,之后每次启动都生效。安装器未签名,首次运行时 SmartScreen 会要求确认。

## 可移植性

DMG 是薄外壳而不是独立应用:它承载 harness web 应用,并在需要时从**同一台机器**上的 DeepSeek Harness 检出目录启动 `pnpm dsh web`。因此在另一台 Mac 上,需要一次性准备:

```sh
git clone https://github.com/deepseek-ai/deepseek-harness
cd deepseek-harness
pnpm install
```

外加 `node`/`pnpm`(Homebrew、nvm、fnm 或 volta 均可,客户端的 PATH 自动检测覆盖这些)以及真实 agent 回合所需的 `DEEPSEEK_API_KEY` 环境变量。客户端对 harness 版本无感:它渲染检出目录提供的任何 web 应用,也可以附着到你自己启动的服务器(`pnpm dsh web`),那时完全不需要检出目录。

当前 DMG 为 Apple Silicon(`aarch64`)构建;Intel Mac 需要 `x86_64-apple-darwin` 构建。

## 目录结构

```
dsh-desktop/   the Tauri app (Rust core + loading page)
```

`dsh-desktop/ui/` 下的加载页是本客户端唯一的自有前端资源;启动时它只显示应用图标,并在 harness web 应用加载完成前呈现启动错误。

## 已知限制与待办

- 打包(`tauri build`)需要 Tauri CLI,通过 `npx` 按需获取;`cargo build` / `cargo run` 仍是开发用法。
- `DSH_WEB_COMMAND` 按空白拆分,不支持带引号的参数。
- 应用运行期间服务器退出时,webview 显示连接错误;客户端不会自动重启服务器。
