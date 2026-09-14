# Issue tracker：本地 Markdown

本仓库的议题与 spec 以 markdown 文件形式存在 `.scratch/` 下，**随仓库一起版本化**。

## 约定

- 一个 feature 一个目录：`.scratch/<feature-slug>/`
- spec 是 `.scratch/<feature-slug>/spec.md`
- 实现工单一个文件一张，位于 `.scratch/<feature-slug>/issues/<NN>-<slug>.md`，从 `01` 起编号 ——
  永远不要写成一个合并的 tickets 文件
- triage 状态记在每张工单靠近顶部的 `Status:` 行（角色字符串见 `triage-labels.md`）
- 评论与对话历史追加到文件底部的 `## Comments` 标题下

## 当技能说「publish to the issue tracker」

在 `.scratch/<feature-slug>/` 下建新文件，目录不存在就连目录一起建。

## 当技能说「fetch the relevant ticket」

读取所指向路径的文件。用户通常会直接给你路径或工单编号。

## Wayfinding 操作

供 `/wayfinder` 使用。**map** 是一个文件，每张工单是它的一个 **child** 文件。

- **Map**：`.scratch/<effort>/map.md`（承载 Notes / Decisions-so-far / Fog 正文）。
- **Child 工单**：`.scratch/<effort>/issues/NN-<slug>.md`，从 `01` 起编号，正文写问题本身。
  `Type:` 行记工单类型（`research`/`prototype`/`grilling`/`task`）；`Status:` 行记
  `claimed`/`resolved`。
- **阻塞**：靠近顶部的 `Blocked by: NN, NN` 行。所列文件全部 `resolved` 时，该工单才解除阻塞。
- **Frontier**：扫 `.scratch/<effort>/issues/`，取开放、未阻塞、未认领的文件；编号小的优先。
- **Claim**：动手之前先置 `Status: claimed` 并保存。
- **Resolve**：在 `## Answer` 标题下追加答案，置 `Status: resolved`，然后把一条 context pointer
  （要点 + 链接）追加到 `map.md` 的 Decisions-so-far。
