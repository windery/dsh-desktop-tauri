# DeepSeek Harness Desktop

把 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) 的界面装进一个原生窗口。用系统 WebView 渲染，复用你本机已经装好的 `dsh`，配置直接读 CLI 正在用的 `~/.dsh`。

[![License: MIT](https://img.shields.io/badge/license-MIT-2EA44F)](LICENSE)
[![Platform: macOS](https://img.shields.io/badge/platform-macOS-4493F8)](#环境要求)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%20v2-24C8DB)](https://tauri.app)

> 非官方社区项目，与 DeepSeek 不存在隶属、合作、授权或背书关系。

<!--
截图占位。把界面截图放到 docs/screenshot.png 后取消下面这行的注释。
注意别把真实会话内容截进去 —— 侧边栏的会话标题、工作区路径都会暴露出去。

![DeepSeek Harness Desktop 界面](docs/screenshot.png)
-->

它不是一个另写的 agent，只是一个薄壳：不重写界面、不自带浏览器引擎、不搬一套 Node 运行时。
装好 `dsh` 之后，这个窗口和你在终端敲 `dsh web` 打开的那套东西是**同一个**。

## 特性

- **和 CLI 完全同源** —— 跑的就是你本机那个 `dsh`，所以 harness 版本、插件、模型配置永远和命令行一致，不会出现「桌面端落后一个版本」。
- **一个服务端，两个视图** —— 启动时如果 `dsh web` 已经在跑（比如你自己起的），窗口会**附着**上去而不是再起一个。浏览器和窗口从此共享同一条事件流：一边新建的会话、发出的消息，另一边**实时**可见，不需要刷新，也不需要插件。
- **配置零重复** —— 凭据、会话、技能、设置全部读 `~/.dsh`，和 CLI 共用一份。
- **打开就能用** —— 不管端口上有没有服务、拿不拿得到 token、有没有日志，都不需要你先做任何事。
- **不自带浏览器引擎** —— 用系统 WebView（macOS 上即 WKWebView），安装体积 3 MB 量级。
- **不装后台机制** —— 没有常驻服务，没有登录自启。只在窗口开着的时候占端口，关掉就还给你。
- **退出不留残留** —— 自己起的服务随窗口一起结束，连它派生的一切；别人起的服务一律不动。

## 环境要求

- **macOS**。目前只在 macOS 26 / Apple Silicon 上验证过；代码避开了 macOS 专有 API，其他平台理论上可用，但未经测试。
- **Node.js ≥ 22.15**。harness 的核心插件在模块顶层从 `node:zlib` 具名导入 Zstd 编解码器，而 ESM 在链接期就校验具名导出——所以缺它们的 Node 连模块都加载不了，跟有没有会话无关（那两个编解码器从 22.15 起才有，23 线是 23.8）。应用会自己在常见安装位置里挑一个合格的，`PATH` 上排在前面但版本过旧的会被跳过。
- **已安装 `@deepseek-ai/dsh`**：

  ```sh
  npm i -g @deepseek-ai/dsh
  ```

从源码构建还需要 **Rust stable** 与 **Xcode Command Line Tools**。

## 安装

```sh
git clone https://github.com/windery/dsh-desktop-tauri.git
cd dsh-desktop-tauri
npm install
npm run build
```

产物在 `src-tauri/target/release/bundle/`，包含 `.app` 与 `.dmg`。首次构建要编译整个 Tauri
依赖树，会慢；之后是增量。

**首次打开会被 Gatekeeper 拦一次**，因为构建产物没有签名。拖进 `/Applications` 后清掉隔离标记：

```sh
xattr -dr com.apple.quarantine "/Applications/DeepSeek Harness Desktop.app"
```

## 使用

直接打开，没有首次配置。

窗口出现前有一个短暂的启动画面：如果端口上已经有服务，它一闪而过；如果没有，会停留约 3 秒
等 harness 起来。

### 和本机 `dsh web` 的关系

你可以继续像以前那样在终端敲 `dsh web`，两者不冲突：

| 你想做的事 | 结果 |
| --- | --- |
| 先敲 `dsh web`，再打开窗口 | 窗口附着到你的服务，两个界面实时同步 |
| 直接打开窗口 | 窗口自己起一个服务，关掉窗口时一并结束 |
| 窗口开着时，你起的服务停了 | 窗口接管那个地址继续可用，浏览器也跟着恢复 |
| 窗口开着时，你想自己起服务 | 先退出窗口，端口就还给你了 |

细节和取舍见[设计说明](docs/DESIGN.md#不干扰契约)。

### 快捷键

| 快捷键 | 作用 |
| --- | --- |
| `⌘R` | 重新加载界面 |
| `⌘⇧R` | 重启 harness（改完插件或配置可以用它，不用退出应用） |
| `⌘C` `⌘V` `⌘A` | 标准剪贴板操作（菜单栏里也有） |

窗口**保留原生全屏**（绿灯照常用）。ESC 只做界面该做的事：关弹窗、取消编辑，**不会**把窗口
踢出全屏。壳会在页面加载时把 ESC 标记为已消费，所以 AppKit 不再把它理解成"退出全屏"，而页面
自己的处理器照常收到这个键。理由见[设计说明](docs/DESIGN.md#五个实现决定)。

## 配置

通常不需要任何配置，窗口会自动找到你的 `dsh`。要覆盖时用环境变量：

| 变量 | 默认 | 作用 |
| --- | --- | --- |
| `DSH_DESKTOP_BACKEND` | 自动探测 | 指定 harness 入口：`.js`/`.mjs`/`.cjs` 路径（用 node 跑），或可执行文件 |
| `DSH_DESKTOP_NODE` | 自动探测 | 指定 node 二进制，跳过搜索 |
| `DSH_DESKTOP_PROFILE` | `web` | 使用哪个 profile；默认与本机 `dsh web` 共用 |
| `DSH_DESKTOP_PORT` | `3080` | 共享的回环端口。端口上是 `dsh web` 则附着，空闲则自己起服务，是别的程序则报错停下。端口必须固定，`0` 与无效值一律按默认端口处理 |
| `DSH_HOME` | `~/.dsh` | harness 数据根目录（由上游解释，本应用只透传） |

从 Finder 启动的应用看不到终端环境变量，要从命令行传：

```sh
DSH_DESKTOP_PORT=4000 open -a "DeepSeek Harness Desktop"
```

自动探测会覆盖 fnm、nvm、Homebrew 等常见安装位置，细节见[设计说明](docs/DESIGN.md#后端探测)。

## 常见问题

### 换了个浏览器，打开 `127.0.0.1:3080` 提示 authentication required

认证走的是**每个浏览器各自持有的 cookie**。`dsh web` 启动时会打印一个带 token 的地址，
访问它一次就种下 cookie，此后该浏览器长期有效（cookie 的签名密钥存在 `~/.dsh` 里，对任何
服务端都认）。

所以换浏览器或用隐私窗口时，需要**再访问一次带 token 的地址**。桌面窗口不受影响——它不等
token，会自己签一个 cookie（[原理](docs/DESIGN.md#鉴权壳自己签-cookie不需要-token)）。

最省事的做法是让服务的启动行落盘，token 就随时可查：

```sh
dsh web --no-open 2>&1 | tee -a ~/.dsh/dsh-web.log
```

然后另开一个终端：

```sh
grep -o 'http://127.0.0.1:3080/?token=[^ ]*' ~/.dsh/dsh-web.log | tail -1
```

把打印出来的地址在新浏览器里打开一次即可。

### 提示端口被占用（EADDRINUSE）

先看是谁占着：

```sh
lsof -nP -iTCP:3080 -sTCP:LISTEN
```

如果是桌面窗口起的服务，退出窗口即可；如果是终端里跑的服务，回那个终端结束它。

### 提示找不到 dsh

自动探测没找到时，显式指定入口：

```sh
DSH_DESKTOP_BACKEND=/path/to/node_modules/@deepseek-ai/dsh/lib/bin.js \
  open -a "DeepSeek Harness Desktop"
```

## 开发

```sh
npm install
npm run dev                  # 编译 Rust 壳并开窗
npm run build                # 发布构建

cd src-tauri && cargo test   # 单元测试
```

代码很少：

| 路径 | 职责 |
| --- | --- |
| `src-tauri/src/backend.rs` | 找到 `dsh`、探测端口、起服务、签发会话 cookie |
| `src-tauri/src/main.rs` | 窗口生命周期、菜单、启动与恢复流程 |
| `ui/index.html` | 启动画面（其中的 `ui/icon.png` 是图标的一份副本） |
| `src-tauri/icons/` | 图标资源；`source.png` 是待展开的 1024 源图 |

### 发布

推一个 `v*` tag 就是全部流程：

```sh
git tag v0.1.0
git push origin v0.1.0
```

[`.github/workflows/release.yml`](.github/workflows/release.yml) 会在 arm64 runner 上跑测试、
构建，并把 DMG 挂到 Release 上。

版本号由 tag 戳进三个清单（`package.json`、`src-tauri/tauri.conf.json`、
`src-tauri/Cargo.toml`），所以**不需要先手工改版本再打 tag**。这一步不能省：DMG 的文件名
来自 `tauri.conf.json` 而不是 tag，手工流程下打了 `v0.2.0` 却附上一个仍叫 `0.1.0` 的包，
是那种要到用户看文件名才发现的不一致。

目前只出 Apple Silicon 构建（`macos-latest` 即 arm64）。要出 Intel 或 universal，改
workflow 里的 runner / target 即可。

设计取舍、实测数据，以及每个反直觉决定的理由，都在 [`docs/DESIGN.md`](docs/DESIGN.md)。

## 相关

- [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) —— 上游项目，本仓库只是它的桌面外壳
- [Tauri](https://tauri.app) —— 桌面框架

## 许可证

[MIT](LICENSE)
