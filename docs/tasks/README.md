# 任务索引

按顺序执行。前一张卡由 Lupo 合并之后，才开始下一张。

| 卡号 | 标题 | 基线分支 | 状态 |
|---|---|---|---|
| HF-1 | 向导热修复：关闭时崩溃，以及四个数据问题 | feat/wizard | 已合并 |
| W-0 | 抽出 jobs.rs（加水印、检测） | main（HF-1 合并之后） | 已合并 |
| W-1 | 向导外壳、欢迎页、选择任务页、启动时弹出 | main（W-0 合并之后） | 已合并 |
| W-2 | 首次设置 S1–S3（先读 W-COMMON.md） | main（W-1 合并之后） | 进行中 |
| W-3 | 加水印流程 E1–E4 | t-w-2（叠加） | 进行中 |
| W-4 | 检查流程 C1–C3，删除旧向导 | t-w-3（叠加） | 进行中 |
| A-fix / D-fix | alpha 分支的修复 | alpha（变基到 main 之后） | 卡未写 |
| FMT | 整个仓库 cargo fmt，并加 rustfmt.toml | main（等 alpha 合并之后） | 卡未写 |
| W-5 | 分发流程 D1–D5 | main（等 alpha 合并之后） | 卡未写 |

参考文档：
- 规格：`docs/specs/wizard.md`、`docs/specs/ui-copy.md`
- 规则：`AGENTS.md`、`docs/ARCHITECTURE.md`
