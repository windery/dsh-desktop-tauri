# DSH Desktop (Tauri)

一个极薄的 DeepSeek Harness 桌面壳。它不实现任何 agent 逻辑，只做这几件事：

1. 找到你**本机已经装好的** `dsh`；
2. 探共享的回环端口（默认 3080）——有人在那儿服务就**附着**上去，没人就自己起一个；
3. 拿到能进去的凭证（自己起的服务用它的 token，别人的服务就**现签一个 cookie**）；
4. 把界面交给系统 webview，并在退出时干掉自己起的那整个进程组。

界面、会话、设置、插件全部是上游的，配置全部来自你 CLI 正在用的 `~/.dsh`——
壳自己不存任何配置。

**任何时候打开都是可用的**：端口被占、没有 token、没有日志，都不需要你做任何事。

## 为什么是 Tauri

| | Tauri（本项目） | 官方 Electron 桌面端 |
| --- | --- | --- |
| 渲染引擎 | 系统 WKWebView | 内置 Chromium |
| 运行时 | 复用你已装的 Node + dsh | 自带 Node.js + pnpm |
| harness 版本 | 跟你的 CLI 完全一致 | 与 shell 绑定发布，可能落后 |
| profile | 与本地 `dsh web` 共用 `web` | 独占 `desktop`（CLI 拒绝启动该名字） |
| 通信 | 回环 HTTP，走上游已有的鉴权 | `dsh-app://` 协议 + 分帧字节管道，不开端口 |

Tauri 省掉的是「自带一个浏览器引擎」这份分发与内存开销。它**不会**让 harness
本身的启动变快：窗口打开是即时的，但插件树加载仍由 Node 决定。

## 环境要求

- Node.js ≥ 22.15（会话用 Zstd 存，`node:zlib` 到 22.15 才有 `createZstdDecompress`）
- 已安装 `@deepseek-ai/dsh`（`npm i -g @deepseek-ai/dsh`）
- Rust stable + Xcode Command Line Tools

## 使用

```sh
npm install
npm run dev      # 开发模式，编译 Rust 壳后开窗
npm run build    # 产出 .app 与 .dmg（src-tauri/target/release/bundle/）
```

首次 `npm run build` 要编译整个 Tauri 依赖树，会慢；之后是增量。

产物未签名，macOS 首次打开会被 Gatekeeper 拦。拖进 `/Applications` 后清一次隔离标记：

```sh
xattr -dr com.apple.quarantine "/Applications/DSH Desktop.app"
```

## 环境变量

| 变量 | 默认 | 作用 |
| --- | --- | --- |
| `DSH_DESKTOP_BACKEND` | 自动探测 | 指定 harness 入口：一个 `.js`/`.mjs`/`.cjs` 路径（用 node 跑），或一个可执行文件 |
| `DSH_DESKTOP_NODE` | 自动探测 | 指定 node 二进制，跳过搜索 |
| `DSH_DESKTOP_PROFILE` | `web` | 使用哪个 profile；默认与本机 `dsh web` 服务共用 |
| `DSH_DESKTOP_PORT` | `3080` | 共享的回环端口；被占则附着，空闲则自己起服务。设 `0` 退回随机端口 |
| `DSH_HOME` | `~/.dsh` | harness 数据根目录（由上游解释，壳只透传） |

### 后端探测顺序

1. `$DSH_DESKTOP_BACKEND`
2. 各 Node 安装根下的全局安装，**新版本优先**：fnm、nvm、`/opt/homebrew`、`/usr/local`、
   `~/.npm-global`、`~/.local`、`~/Library/pnpm/global`
3. 搜索路径上的 `dsh`

GUI 应用从 Finder 启动时只继承 launchd 的最小 `PATH`
（`/usr/bin:/bin:/usr/sbin:/sbin`），看不到 fnm / nvm / Homebrew。所以上面这些位置是
**显式枚举**的，并且会把找到 node 的目录前置进子进程的 `PATH`，让 agent 派生的工具也能解析到。

## 一个端口，一个服务端

壳不假设自己该起服务。启动时先探共享端口：

| 情况 | 行为 |
| --- | --- |
| 端口空闲 | 自己起 `dsh web --profile web --port 3080` |
| 端口被占 | **附着**上去，不起第二个进程 |

附着是重点：浏览器和窗口从此是**同一个服务端**——同一个 origin、同一条事件流、同一份
会话 cookie。所以一边新建的会话、发出去的消息，另一边是**实时的**，不需要轮询，也不
需要插件。你自己起的 `dsh web` 永远不被打扰，窗口只是变成它的第二个视图。

设 `DSH_DESKTOP_PORT=0` 可退回「每次随机端口、自己起服务」的老行为，代价是放弃共享。

## 鉴权：壳自己签 cookie，不需要 token

`dsh web` 的 `/` 不带凭证返回 **401**，启动时它会打印：

```
dsh web: http://127.0.0.1:<port>/?token=<launch-token>
```

带这个 token 的请求会种下 cookie 并 303 跳到干净的 `/`。**但这个 token 只出现在服务自己的
stdout 上**——别人起的服务，你读不到它。

所以壳不去找 token，它**自己签一个**。关键在于 cookie 的签名密钥：

- 持久化在共享 harness home 的凭据库里（`~/.dsh/.credentials.yaml`），任何客户端都能读；
- cookie 的 audience 是 `host:port`，**与 token 无关**。

壳照 harness 的格式现签一个 —— `v1.<base64url(payload)>.<base64url(HMAC-SHA256)>` ——
用 Tauri 的 `Webview::set_cookie` 装进 webview，再重新发起请求。

两个必须对上的细节，猜错任何一个都会签出服务端不认的 cookie：

1. 凭据库里存的密钥是 **base64url 文本**，要先解码成 32 字节原始密钥再当 HMAC key。
   harness 的 `canonicalSecret()` 就是这么做的。
2. payload 的生命周期 `expiresAt - issuedAt` 不能超过 harness 的 `cookieMaxAgeDays`
   （默认 30 天），否则 `isAuthenticated` 直接判否。壳用 29 天。

于是启动路径是：

| 情况 | 行为 |
| --- | --- |
| 端口空闲 | 自己起服务，用它的 token 加载（token 在手上，最直接） |
| 端口被占 | 现签 cookie 装进 webview —— **没有 token 也进得去，你什么都不用做** |
| 凭据库读不到 | 退回读日志（`~/.dsh/dsh-web.log`、`dsh-web-launchd.log`）里的 token，再退回裸 URL |

三项实测（都对着真实服务跑过）：

- 壳签的 cookie 服务端认可：不带凭证 **401**，带上 → **200**
- 全新端口（webview 里不可能有它的 cookie）、零 token 参与 → 自动认证，2 条 ESTABLISHED
- 对着手动裸跑的 3080（**无日志、无历史 cookie**）→ 自动认证成功

> 边界：若某个 profile 把 `cookieMaxAgeDays` 调到低于 29 天，签名会被拒，此时自动退回
> token / 裸 URL 路径 —— 不会崩，只是可能回到需要凭证的状态。

## profile 归属

壳默认使用 `web` —— 和本机 `dsh web` 服务同一个 profile，一份插件集、一份配置。

即使退回两个独立服务端，同跑两个 harness 进程也是安全的，因为上游对共享数据做了跨进程
保护。以下都是实测结果，不是从文档推断的：

| 数据 | 机制 | 效果 |
| --- | --- | --- |
| 会话 | `flock(2)` 排他锁（`<会话目录>/session.lock`） | 同一会话同时只有一个写者；第二个得到 `SessionAlreadyOwnedError` |
| storage / settings / credentials | `tmp` + `rename` 原子写 | 最后写入者生效，不会写坏 |
| attachments | 内容寻址 | 天然安全 |
| `settings.yaml`、`skills/` | 文件监听（`watch` + 自写抑制） | **跨进程实时同步** |
| 会话列表 | 无监听，只有进程内事件推送 | 换服务端时需要刷新页面 |

锁的排他性是实测的：进程 A 持有时，B 和 C 尝试同一把锁都返回 `EAGAIN`，A 释放后即可获取。

想换 profile 时设 `DSH_DESKTOP_PROFILE`。**不要用 `desktop`**：它被官方 Electron 应用
独占，CLI 会直接拒绝启动。

注意：如果有人**同时**改插件，`dsh plugin` 会动 profile 的 `node_modules`，两个进程都
需要重启才能看到变化。

## 三个实现决定

**用创建窗口而不是 `navigate()`。** 最初实现对 splash 窗口调用
`WebviewWindow::navigate(url)`，实测 webview 从未发起过连接（harness 端口上零连接）。
改成直接以 `WebviewUrl::External` **创建**主窗口，连接立即正常建立
（可见 SSE + API 两条 ESTABLISHED）。导航到跨源地址这条路径在 macOS WKWebView 上不可靠。

**自己签 cookie，而不是找 token。** 见上一节。这是「没有 token 就自动处理」的全部内容，
也是唯一一个依赖上游内部格式的地方，所以它设计成**可失败**：读不到密钥就退回 token，
token 也没有就退回裸 URL，任何一步失败都只是降级，不会让窗口打不开。

**信号处理和 `RunEvent` 都要挂。** 关窗 / ⌘Q 会走 `RunEvent::ExitRequested`，
但 `kill`、Ctrl-C、终端挂断**不会**触发它——直接 `kill` 壳进程会留下孤儿的 harness
和它派生的一切。所以 `SIGTERM`/`SIGINT`/`SIGHUP` 单独装了处理器，用
`killpg` 打整个进程组。附着模式下壳没有子进程，退出时自然不会误杀别人的服务。

## 自检

```sh
cargo test
```

除了解析、版本排序、cookie 格式的单测，还有三个测试守住本项目的成立前提：
`finds_the_harness_this_shell_is_meant_to_reuse`（本机能找到 dsh）、
`mints_a_cookie_the_harness_accepts`（密钥与 cookie 格式可复现）、
`never_claims_the_electron_owned_profile`（不碰 `desktop`）。
