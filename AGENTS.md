# DeepSeek Harness Desktop

能力全在本机那个 `dsh` 里；这个仓库只负责找到它、启动它、把它显示出来。
用系统 WebView 渲染，界面、插件、模型配置一律沿用本机那份，不做第二套。

取舍与理由的单一真源是 [`docs/DESIGN.md`](docs/DESIGN.md)，面向用户的说明是 [`README.md`](README.md)。
动到下面任何一条契约之前，先读对应那一节。

## 不能破的契约

**薄壳** —— 这个仓库不产生 agent 能力：界面、浏览器引擎、Node 运行时都复用本机的。
唯一的前端产物是 `ui/index.html` 这个静态启动页，改完刷新窗口即可，没有构建步骤。

**不干扰** —— 端口上已有服务就附着上去，只有自己起的服务才终止。
改启动路径时，「已存在则附着」和「退出只收自己的」两条同时成立才算对。
见 [`docs/DESIGN.md#不干扰契约`](docs/DESIGN.md#不干扰契约)。

**退出不留残留** —— 关窗和 ⌘Q 走 `RunEvent::ExitRequested`，但 `kill`、Ctrl-C、终端挂断
**不触发**它。所以 `SIGTERM`/`SIGINT`/`SIGHUP` 另装处理器，用 `killpg` 打整个进程组
（`BACKEND_PGID`）。这条路径上只能用 async-signal-safe 调用。
见 [`docs/DESIGN.md#五个实现决定`](docs/DESIGN.md#五个实现决定)。

**版本戳** —— 版本号由 git tag 戳进三个清单，手工改会被下次发布覆盖。
见 [`README.md#发布`](README.md#发布)。

## 环境

harness 侧要求 **Node ≥ 22.15**：核心插件在模块链接期就校验 `node:zlib` 的 Zstd 具名导出，
缺了连模块都加载不了，与有没有会话无关。探测顺序、以及「版本读不到时不拒绝」的取舍，
见 [`docs/DESIGN.md#后端探测`](docs/DESIGN.md#后端探测)。

## 验证

```sh
cd src-tauri && cargo test     # 单元测试
npm run build                  # 发布构建；首次要编整棵 Tauri 依赖树，很慢
```

守成立前提的三个测试见 [`docs/DESIGN.md#测试`](docs/DESIGN.md#测试)。

改动落在启动 / 附着 / 退出路径上时，`cargo test` 之后再手验一遍：
先起 `dsh web`，再开窗口，确认它**附着**上去而不是抢端口。

## Agent skills

### Issue tracker

议题与 spec 以 markdown 文件存在 `.scratch/` 下，随仓库版本化。见 `docs/agents/issue-tracker.md`。

### Triage labels

用五个默认角色标签（`needs-triage` / `needs-info` / `ready-for-agent` / `ready-for-human` / `wontfix`）。见 `docs/agents/triage-labels.md`。

### Domain docs

Single-context：根 `CONTEXT.md` + `docs/adr/`。见 `docs/agents/domain.md`。
