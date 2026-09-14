# 设计说明

给维护者看的：这个壳为什么长这样、哪些地方是刻意取舍、以及每个反直觉的决定背后
实测到了什么。使用层面的东西在 [README](../README.md)。

## 定位

壳不实现任何 agent 逻辑。它只做四件事：

1. 找到本机已经装好的 `dsh`；
2. 探共享的回环端口——有人在那儿服务就**附着**上去，没人就自己起一个；
3. 拿到能进去的凭证（自己起的服务用它的 token，别人的服务就**现签一个 cookie**）；
4. 把界面交给系统 webview，并在退出时干掉自己起的那整个进程组。

界面、会话、设置、插件全部是上游的，配置全部来自 CLI 正在用的 `~/.dsh`——壳自己不存
任何配置。

## 为什么是 Tauri

| | Tauri（本项目） | 官方 Electron 桌面端 |
| --- | --- | --- |
| 渲染引擎 | 系统 WKWebView | 内置 Chromium |
| 运行时 | 复用已装的 Node + dsh | 自带 Node.js + pnpm |
| harness 版本 | 跟 CLI 完全一致 | 与 shell 绑定发布，可能落后 |
| profile | 与本地 `dsh web` 共用 `web` | 独占 `desktop`（CLI 拒绝启动该名字） |
| 通信 | 回环 HTTP，走上游已有的鉴权 | `dsh-app://` 协议 + 分帧字节管道，不开端口 |

Tauri 省掉的是「自带一个浏览器引擎」这份分发与内存开销。它**不会**让 harness 本身的启动
变快：窗口打开是即时的，但插件树加载仍由 Node 决定。

## 运行模型：一个端口，一个服务端

壳不假设自己该起服务。启动时先探共享端口（默认 3080）：

| 情况 | 行为 |
| --- | --- |
| 端口空闲 | 自己起 `dsh web --profile web --port 3080` |
| 端口上是 `dsh web` | **附着**上去，不起第二个进程 |
| 端口上是别的程序 | 报错并停下 —— 附着会把陌生页面装进窗口，硬起会撞 EADDRINUSE |

"端口被占"的问法不是"有没有人监听"，而是"监听的是不是 harness"：一次 TCP 连接分不出来，
而 3080 是常见端口。判别依据是 harness 自己的未认证应答（`401` + `dsh web authentication
required`），见 `probe_port`。

附着是重点：浏览器和窗口从此是**同一个服务端**——同一个 origin、同一条事件流、同一份
会话 cookie。所以一边新建的会话、发出去的消息，另一边是**实时的**，不需要轮询，也不需要
插件。用户自己起的 `dsh web` 永远不被打扰，窗口只是变成它的第二个视图。

端口必须**固定**：附着、看门狗、自签 cookie 都指向同一个端口值，所以没有"随机端口"的
逃生口。`DSH_DESKTOP_PORT=0` 或任何无效值一律按默认端口处理。

### 不干扰契约

壳刻意**不装任何常驻后台机制**：没有 launchd 服务，没有登录自启，没有后台守护。理由很
直接——用户可能想自己敲 `dsh web`，一个常驻服务会把端口占死，让那条命令直接 EADDRINUSE。

行为边界：

| 场景 | 行为 |
| --- | --- |
| 端口上已有服务 | 附着，**绝不移开或取代它** |
| 端口空闲 | 起一个，而且这个服务**随窗口关闭一起结束** |
| 窗口开着，而它正显示的服务停了 | **接管那个地址**（下面单独说） |
| 窗口关闭 | 自己起的服务一并结束；附着来的服务原样不动 |

**接管是唯一一处壳会占端口的情形。** 用户起的服务在窗口开着的时候停了（比如关掉了那个
终端），看门狗会接管那个地址、自己起一个。好处是窗口不会烂在死页面上；而且因为接管复用
同一个地址和同一个 harness home，浏览器手里那个 origin 的 cookie 依然有效，**浏览器也
跟着一起恢复**。代价是从那一刻起该端口归壳所有，直到窗口关闭——在那之前手敲 `dsh web`
会撞上 EADDRINUSE。

不想要接管的话，删掉 `watch_serving` 里的恢复分支即可（`src-tauri/src/main.rs`）；代价是
服务死后窗口只能停在死页面上，需要手动 ⌘⇧R。这是有意的取舍：**默认保证窗口可用，而不是
默认不碰端口。**

### 启动手感

| 3080 上有没有服务 | 打开窗口时发生什么 | 体感 |
| --- | --- | --- |
| 有 | 直接附着，一个窗口 + 一次加载 | 几乎立刻 |
| 没有 | 起一个，等它打印出地址 | 多等一次 harness 启动 |

实测 harness 冷启动稳定在 **约 3.0 秒**（三次 2.97 / 2.97 / 3.01），这段由 Node 加载插件
树决定，壳无法优化——能省掉它的唯一办法是让服务先在那儿等着，而那正需要常驻机制，也就是
上面刻意不做的事。这是有意的取舍：**优先不打扰用户，而不是抢先启动。**

用户想要常驻的话自己起就行（前台跑着、tmux 里挂着、自己写 launchd），壳都能附着上去。
用 launchd 的话注意三点：`ProgramArguments` 全用绝对路径且别走 fnm 的 multishell shim
（那是每个 shell 一份的临时链接，shell 一关就失效）；`WorkingDirectory` 必须显式钉死
（默认 workspace 根就是进程的 cwd，不钉会给成 `/`）；`PATH` 必须给全（launchd 不读 shell
配置，agent 派生的工具要靠它）。

## 鉴权：壳自己签 cookie，不需要 token

`dsh web` 的 `/` 不带凭证返回 **401**，启动时它会打印：

```
dsh web: http://127.0.0.1:<port>/?token=<launch-token>
```

带这个 token 的请求会种下 cookie 并 303 跳到干净的 `/`。**但这个 token 只出现在服务自己的
stdout 上**——别人起的服务读不到。

所以壳不去找 token，它**自己签一个** —— 这是本项目的设计，不是绕开上游的临时手段。cookie 的
audience 是 `host:port`，**与 token 无关**；密钥持久化在共享 harness home 的凭据库里
（`~/.dsh/.credentials.yaml`），任何本机客户端都读得到。于是窗口附着到用户自己起的服务上时，
不需要 token、不需要日志、不需要用户做任何事。

（另一条路是自带一份 harness、连服务都自己起 —— 那样 token 就在手上，完全不必碰凭据库，代价是
失去"窗口与终端 CLI 永远同源"这个前提。作为 `dsh web` 的平替，这里选复用本机那份。）

壳照 harness 的格式现签一个 —— `v1.<base64url(payload)>.<base64url(HMAC-SHA256)>` ——
装进 webview 再重新发起请求。

**三个前提。** 算法本身是自足的、与 stdout 无关；但前两条它保证不了：

1. **密钥必须是服务端加载的那一个** —— 即两边解析出同一个 `$DSH_HOME`。这是唯一算法无法保证的
   条件：home 不同则密钥不同，签得再对也必被拒。它也是唯一"读 stdout 反而能救"的场景，
   因为 token 与密钥无关。
2. **`authority` 必须与实际加载的地址一致** —— cookie 名是
   `cookiePrefix + b64url(sha256(authority))`，payload 里也带 authority，两处都要匹配。所以加载
   地址必须始终是 `127.0.0.1:<port>`；改成 `localhost` 会同时废掉两处，直接 401。
3. 格式细节：库里的密钥是 **base64url 文本**，要先解码成 32 字节原始密钥再当 HMAC key
   （harness 的 `canonicalSecret()` 就是这么做的）；生命周期 `expiresAt - issuedAt` 不能超过
   `cookieMaxAgeDays`（默认 30 天），壳用 29 天。

于是启动路径是：

| 情况 | 行为 |
| --- | --- |
| 端口空闲 | 自己起服务，用它的 token 加载（token 在手上，最直接） |
| 端口上是 `dsh web` | 现签 cookie，**验证通过后**才装进 webview —— 没有 token 也进得去 |
| 签出来的 cookie 被拒 | 换更短的有效期再签一次；两次都不过就在启动画面说明原因，不静默停在 401 页 |

三项实测（都对着真实服务跑过）：

- 壳签的 cookie 服务端认可：不带凭证 **401**，带上 → **200**
- 全新端口（webview 里不可能有它的 cookie）、零 token 参与 → 自动认证，2 条 ESTABLISHED
- 对着手动裸跑、无日志、无历史 cookie 的 3080 → 自动认证成功

**为什么签完必须验证。** 不验证的话，一个服务端不认的 cookie 就是一个停在 401 页、却什么
都不解释的窗口 —— 对"有服务就能用"这个目标来说，最糟的失败不是做不到，而是做不到还说不清。
验证把静默失败变成已知失败。也正因为有验证，`cookieMaxAgeDays` 被调低这件事**不需要**壳去
读 profile 配置：先用 29 天试，被拒就换 1 天再试 —— **试出来比读出来少一层上游格式耦合**。

壳**不再**去读 `dsh-web.log` / `dsh-web-launchd.log` 找 token：那要求用户事先 `tee` 过，不满足
"零用户动作"。README 里那条 `tee` 配方保留 —— 它服务的是**浏览器**换 cookie 的场景，与壳无关。

> 边界：若签名两次都被拒（最可能是那个服务用了不同的 `DSH_HOME`，两边 secret 不同），
> 壳会在启动画面说明原因并停下，而不是打开一个用不了的 401 页面。`⌘⇧R` 可重试。

## 配置与数据归属

壳默认使用 `web` profile —— 和本机 `dsh web` 服务同一个，一份插件集、一份配置。

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

改 profile 用 `DSH_DESKTOP_PROFILE`。**不要用 `desktop`**：它被官方 Electron 应用独占，
CLI 会直接拒绝启动。

注意：如果有人**同时**改插件，`dsh plugin` 会动 profile 的 `node_modules`，两个进程都需要
重启才能看到变化。

## 五个实现决定

**用创建窗口而不是 `navigate()`。** 最初实现对 splash 窗口调用
`WebviewWindow::navigate(url)`，实测 webview 从未发起过连接（harness 端口上零连接）。
改成直接以 `WebviewUrl::External` **创建**主窗口，连接立即正常建立（可见 SSE + API 两条
ESTABLISHED）。导航到跨源地址这条路径在 macOS WKWebView 上不可靠。

**自己签 cookie，而不是找 token。** 见上文。这是唯一一个依赖上游内部格式的地方。它的失败
面比看起来小：凭据文件是 `dsh web` 启动时**急切写入**的（`client-connection` 激活时
`BrowserAuth.create` → `initializeSecret`），且服务端在 URL 公告**之前**就已把它落盘，所以
"读不到密钥"在公告后的服务上不可达。剩下的失败只有一类：**服务端加载的 `DSH_HOME` 与壳读的
不是同一个**，签得再对也必被拒 —— 那时壳在启动画面说明原因并停下，而不是打开一个用不了的
401 页面（见上文「为什么签完必须验证」）。

**ESC 不当"退出全屏"用。** 原生全屏下按 ESC 会退出全屏，这是 AppKit 的
`NSWindow.cancelOperation:` —— 壳里没有这个处理器（全仓没有 ESC 相关代码）。问题出在**网页的
ESC 处理器不声明"这个键我用了"**：它们有 23 处（`Modal` / `Menu` / 设置面板 / 目录选择器 …），
都是按表面挂载的，且只有 6 处调 `preventDefault()`。于是这个键看起来没被处理，继续沿响应链
上浮到窗口，一次按键干了两件事。

壳的做法是**在页面侧把它标记为已消费**：建窗时用 `initialization_script` 注入一个 capture
阶段的 `keydown`，对 ESC 只调 `preventDefault()`、**不调** `stopPropagation()`。页面自己的
处理器照常收到键、照常关弹窗，被挡掉的只有 AppKit 那一半。`initialization_script` 在每次顶层
导航都会重跑，所以刷新和页内跳转都不会把它丢掉。

这条路径**还没有实测**：它成立的前提是 WebKit 在 DOM 事件 `preventDefault()` 之后不再把该键
交给响应链，而这是运行时属性，静态分析给不出答案。按一下就知道 —— 原生全屏下不打开任何弹窗
按 ESC：不退全屏即成立。

**若它不成立**，退路依次是：① 原生 `NSEvent` 局部监听吞掉 ESC，再把合成的 `keydown` 派发回
页面（保住全屏，代价是多一处原生代码，且合成事件不 trusted）；② 干脆不让窗口进原生全屏
（`ns_window()` + `objc2-app-kit` 把 `FullScreenPrimary` 换成 `FullScreenNone`，代码已写过一版
并撤掉）。②零未知，但绿灯只做 zoom。**否决过的第三条路是"在壳里拦下 ESC 不往页面传"**：那会
让 ESC 变成无操作，弹窗照样关不掉，比现状更差。

**看门狗：附着的服务死了就自己接管。** 附着的 `dsh web` 不归壳管——用户关掉那个终端，
服务就跟着走了，窗口会一直停在死页面上，除非用户恰好知道 ⌘⇧R。壳自己起的服务也可能这样
死。两种情况的恢复动作是同一个：重跑启动流程，此时端口已空，于是接管它。因为接管复用了
同一个地址和同一个 harness home，浏览器手里那个 origin 的会话 cookie 依然有效，所以
**浏览器也跟着一起恢复**。

三道保险防止它帮倒忙：连续两次探测失败才算死（避免瞬时抖动误判）；启动流程进行中暂不
干预（避免打断有意的重启）；重试次数用尽就放弃（真的起不来时把它留在错误页上，而不是
无限重启）。

实测：杀掉附着的服务后 **6 秒**恢复；**恢复前签的 cookie 在恢复后依然返回 200**。

**信号处理和 `RunEvent` 都要挂。** 关窗 / ⌘Q 会走 `RunEvent::ExitRequested`，但 `kill`、
Ctrl-C、终端挂断**不会**触发它——直接 `kill` 壳进程会留下孤儿的 harness 和它派生的一切。
所以 `SIGTERM`/`SIGINT`/`SIGHUP` 单独装了处理器，用 `killpg` 打整个进程组。附着模式下壳
没有子进程，退出时自然不会误杀别人的服务。

## 后端探测

顺序：

1. `$DSH_DESKTOP_BACKEND`
2. 各 Node 安装根下的全局安装，**新版本优先**：fnm、nvm、`/opt/homebrew`、`/usr/local`、
   `~/.npm-global`、`~/.local`、`~/Library/pnpm/global`
3. 搜索路径上的 `dsh`

GUI 应用从 Finder 启动时只继承 launchd 的最小 `PATH`（`/usr/bin:/bin:/usr/sbin:/sbin`），
看不到 fnm / nvm / Homebrew。所以上面这些位置是**显式枚举**的，并且会把找到 node 的目录
前置进子进程的 `PATH`，让 agent 派生的工具也能解析到。

### 解释器版本门槛

harness 的两个硬性下限都来自**顶层的 ESM 具名导入**，而不是某个可选的运行时特性：

- `node:zlib` 的 `createZstdCompress` / `createZstdDecompress` 到 22.15 才有（23 线是
  23.8），而一个核心插件在模块顶层按名字导入它们：

  ```js
  import { constants, createZstdCompress, createZstdDecompress, zstdCompress, zstdDecompress, zstdDecompressSync } from "node:zlib";
  ```

  **ESM 的具名导入在链接期解析**，所以运行时没有这些导出时，这个插件**根本加载不了** ——
  在任何会话被读写之前就失败。这正是它构成「门槛」而不是能靠懒加载绕开的原因：全新安装、
  零会话，一样失败。
- `node:util` 的 `parseEnv`（启动即 import）需要 20.12，被前者覆盖。

两者失败都发生在 harness **内部**，报错只提某个导出名、完全不提 Node。v20.10 上的实测原文：

```
SyntaxError: The requested module 'node:zlib' does not provide an export named 'createZstdCompress'
```

（对照：同一个文件里只导入 `constants`，v20.10 下立刻正常 —— 说明差异确实来自这几个
zstd 具名导出，不是别的。）

这些符号也确实被调用（不是死导入）：会话就是以 `.jsonl.zstd` 帧存储的。

解释器是壳选的，所以必须在壳里拦住。规则：

- 每个候选都跑一次 `node --version` 探测，**跳过不达标的继续往后找** —— 一个老 node 排在
  `PATH` 前面（遗留的系统安装、废弃的 nvm 默认）不该遮住后面合适的那个。这是本项目修掉的
  一个真实缺口：原先取的是 `binary_dirs()` 里第一个 `node`，而 `PATH` 排在最前，所以旧
  版本会直接胜出。
- 探测失败的候选**不算被拒**。不回应 `--version` 的包装脚本仍给机会，让 harness 自己报；
  只有**确实读到**版本且低于下限才拒绝。
- 落在某个安装根旁边的解释器是自然配对，但**配对只是便利**：任何合格 node 都能跑 JS 程序。
  所以某个根的 node 太旧**不会**否决它的 `dsh` —— 保住最新的 dsh、另找一个能跑它的解释器，
  比退到更旧的安装要好。
- `DSH_DESKTOP_NODE` 是**决定而非提示**：它不合格就直接报错，而不是悄悄换一个用。配置被
  "遵守"的方式是被忽略，是最难察觉的那类问题。

开销：本机去重后只有 2 个候选（v24.11.1 与 v20.10.0），单个探测约 17 ms，`resolve()` 整体
约 30 ms —— 相对 harness 约 3.0 秒的启动可以忽略。

## 图标

`src-tauri/icons/` 是一套完整资源，由 `npx tauri icon <1024 源图>` 展开；`source.png`
保存那张 1024 源图。该命令会顺带生成 Android / iOS 目录，对桌面壳没用，删掉即可（仓库里
没有保留）。

启动画面用的是同一张图的 128px 版本（`ui/icon.png`）——只有 `ui/` 会被送进 webview，所以
副本放那里，图标和启动画面因此同源，不会各自漂移。

> 早先版本另有 `scripts/make-icon.mjs`，用 SDF 平滑并集现场画一只扁平的鲸鱼。它产出的
> 造型和现在的出货图标不同，留着会让「图标生成器」这个名义产生误导，已删除（需要时可在
> git 历史里找回）。

## 测试

```sh
cd src-tauri && cargo test
```

除了解析、版本排序、cookie 格式这类纯逻辑单测，还有三个测试守住本项目的成立前提：

- `finds_the_harness_this_shell_is_meant_to_reuse` —— 本机能找到 dsh
- `mints_a_well_formed_session_cookie` —— cookie 格式可复现（端口可用 `DSH_TEST_PORT`
  覆盖，因为 authority 是签进 payload 的，回放必须对准签发时的端口）
- `never_claims_the_electron_owned_profile` —— 不碰 `desktop`

前两个是**本机冒烟测试**，不是代码单测：它们要求本机装了 `dsh`、且有 `~/.dsh` 凭据。这类
前置条件不是改代码能满足的，所以缺了就**打印原因并跳过** —— 否则 CI 上永远跑不过，而一个
永远红的测试提供的信息量是零。

`picks_an_interpreter_the_harness_can_run_on` 反过来保持严格：它依赖的 node 是仓库自己
声明的前置条件（README 里列了），失败意味着环境没搭对，是**可行动的**，不该被静默跳过。
