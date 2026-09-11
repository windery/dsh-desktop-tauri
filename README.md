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

### 不干扰契约

壳刻意**不装任何常驻后台机制**：没有 launchd 服务，没有登录自启，没有后台守护。理由
很直接——你可能想自己敲 `dsh web`，一个常驻服务会把端口占死，让你的命令直接 EADDRINUSE。

所以它的行为边界是：

| 场景 | 行为 |
| --- | --- |
| 端口上已有服务 | 附着，**绝不移开或取代它** |
| 端口空闲 | 起一个，而且这个服务**随窗口关闭一起结束** |
| 窗口开着，而它正显示的服务停了 | **接管那个地址**（下面单独说） |
| 窗口关闭 | 自己起的服务一并结束；附着来的服务原样不动 |

**接管是唯一一处壳会占端口的情形，需要说清楚。** 如果你起的服务在窗口开着的时候停了
（比如你关掉了那个终端），看门狗会接管那个地址、自己起一个。好处是窗口不会烂在死页面
上；而且因为接管复用同一个地址和同一个 harness home，浏览器手里那个 origin 的 cookie
依然有效，**浏览器也跟着一起恢复**。代价是从那一刻起该端口归壳所有，直到窗口关闭——
在那之前你手敲 `dsh web` 会撞上 EADDRINUSE。

不想要接管的话，删掉 `watch_serving` 里的恢复分支即可（`src-tauri/src/main.rs`）；代价是
服务死后窗口只能停在死页面上，需要手动 ⌘⇧R。这是有意的取舍：**默认保证窗口可用，而不是
默认不碰端口。**

顺带解释一下启动手感的差别，这不是可以"优化掉"的东西：

| 3080 上有没有服务 | 打开窗口时发生什么 | 体感 |
| --- | --- | --- |
| 有（你自己起的） | 直接附着，一个窗口 + 一次加载 | 几乎立刻 |
| 没有 | 起一个，等它打印出地址 | 多等一次 harness 启动 |

实测 harness 冷启动稳定在 **约 3.0 秒**（三次 2.97 / 2.97 / 3.01），这段由 Node 加载插件树
决定，壳无法优化——能省掉它的唯一办法是让服务先在那儿等着，而那正需要常驻机制，也就是
上面刻意不做的事。这是有意的取舍：**优先不打扰你，而不是抢先启动。**

如果你确实想要常驻，自己起就行（前台跑着、tmux 里挂着、或者你自己写 launchd），壳都能
附着上去。真要用 launchd 的话注意三点：`ProgramArguments` 全用绝对路径且别走 fnm 的
multishell shim（那是每个 shell 一份的临时链接，shell 一关就失效）；`WorkingDirectory`
必须显式钉死（默认 workspace 根就是进程的 cwd，不钉会给成 `/`）；`PATH` 必须给全
（launchd 不读 shell 配置，agent 派生的工具要靠它）。

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

## 四个实现决定

**用创建窗口而不是 `navigate()`。** 最初实现对 splash 窗口调用
`WebviewWindow::navigate(url)`，实测 webview 从未发起过连接（harness 端口上零连接）。
改成直接以 `WebviewUrl::External` **创建**主窗口，连接立即正常建立
（可见 SSE + API 两条 ESTABLISHED）。导航到跨源地址这条路径在 macOS WKWebView 上不可靠。

**自己签 cookie，而不是找 token。** 见上一节。这是「没有 token 就自动处理」的全部内容，
也是唯一一个依赖上游内部格式的地方，所以它设计成**可失败**：读不到密钥就退回 token，
token 也没有就退回裸 URL，任何一步失败都只是降级，不会让窗口打不开。

**看门狗：附着的服务死了就自己接管。** 附着的 `dsh web` 不归壳管——用户关掉那个终端，
服务就跟着走了，窗口会一直停在死页面上，除非用户恰好知道 ⌘⇧R。壳自己起的服务也可能
这样死。两种情况的恢复动作是同一个：重跑启动流程，此时端口已空，于是接管它。因为接管
复用了同一个地址和同一个 harness home，浏览器手里那个 origin 的会话 cookie 依然有效，
所以**浏览器也跟着一起恢复**。

三道保险防止它帮倒忙：连续两次探测失败才算死（避免瞬时抖动误判）；启动流程进行中暂不
干预（避免打断有意的重启）；重试次数用尽就放弃（真的起不来时把它留在错误页上，而不是
无限重启）。

实测：杀掉附着的服务后 **6 秒**恢复；**恢复前签的 cookie 在恢复后依然返回 200**。

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

## 图标

鲸鱼由 `scripts/make-icon.mjs` 生成：无依赖、可复现，跑一次得到 1024×1024 的
`source.png`，再用 `npx tauri icon` 展开成各平台尺寸。`tauri icon` 会顺带生成
Android / iOS 目录，对这个 macOS 壳没用，删掉即可（仓库里没有保留）。

造型上没有画路径，而是把几个圆角实体**平滑并集（smooth-min）**成一整块。硬叠会在
交界处留下一道折痕，看起来像两个圆摞在一起——第一版就是这样，一眼像鱼。融合半径是
全部诀窍：大的那个把头部和身体融成没有「腰」的整体；小的那个让尾鳍和胸鳍像肢体一样
长出来，同时保住两片尾鳍之间的**凹口**，那个凹口才是「鲸鱼尾巴」而不是「耳朵」的
关键。胸鳍刻意短而后掠，长的垂在身下会读成一条腿，让鲸鱼显得站着。

刻意画的是原创造型而非复刻任何一家的商标：品牌联想由蓝色底板承担，鲸鱼只需要让人
认出是鲸鱼。

启动画面 `ui/index.html` 里是同一只鲸鱼，用 SVG 的 **blur + alpha 阈值** 复现同样的
融合效果——省掉一条要手工维护的路径。

## 许可证

[MIT](LICENSE)。
