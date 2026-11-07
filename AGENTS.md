# AGENTS.md — 给编码 agent（DeepSeek Harness）的规则

本仓库由两个 agent 协作开发，用同一个工作目录：
- **Claude**：负责设计和审阅，编写 `docs/` 下的规格和任务卡。**不改 `src/`。**
- **DeepSeek Harness（你）**：严格按任务卡实现，**不做任何设计决策**。

只有 Lupo 能把代码合并进 `main`。

## 开工前必读（每次都读）
1. 本文件
2. `docs/ARCHITECTURE.md`
3. 你要做的那张任务卡：`docs/tasks/<卡号>.md`
4. 任务卡里引用的规格：`docs/specs/*.md`

## 铁律
1. **只做任务卡写明的事。** 发现规格有错、有缺口，或者需要扩大范围，就写进审阅包的「问题」一栏，**不要自己动手改**。
2. **文案一字不改。** 界面上出现的所有文字，必须原样取自 `docs/specs/ui-copy.md` 或任务卡。需要新文案时先在审阅包里提问。
3. **不准碰的地方**（除非任务卡明确允许）：
   - `src/wm.rs`、`src/keys.rs`、`src/affine.rs`、`src/features.rs`、`src/imgproc.rs`：水印算法和格式
   - 登记表、接收人簿、密钥这三种文件的格式
   - 新增联网代码或新增依赖
   - `Cargo.toml` 里的版本号
4. **git 规则**：
   - 每张卡单独开分支，名字是 `t-<卡号小写>`，从任务卡指定的基线分支拉出来
   - 提交信息写成 `<卡号>: 一句话`
   - **禁止**：`git push` 到 main、`git push --force`、`git reset --hard`、改写已推送的历史、删除分支
   - 只提交任务卡要求的文件。不要提交调试用的临时文件（`_*.out`、`_*.err`、`build.log`）
5. **unsafe 和 Win32**：
   - 每个窗口过程或对话框过程的函数体都要用 `std::panic::catch_unwind` 包起来。panic 绝不能穿过 FFI 边界。
   - 通过 `Box::into_raw` 交出去的指针，必须且只能用 `Box::from_raw` 收回一次。
6. **做完必须自检**，下面三条都要通过：
   - `cargo fmt --check`
   - `cargo test --release --lib`
   - `cargo check --features win32-check --bins`（非 Windows 环境），或在 Windows 上 `cargo build --release --bins`

## 交付：审阅包
写到 `review/<卡号>.md`，这个文件不提交。总长度不超过 800 行，包含以下各节：

```
# <卡号> 审阅包
## 做了什么（3–5 句）
## 偏离任务卡的地方（没有就写“无”）
## git diff --stat <基线>..HEAD
## 关键 diff（只放逻辑改动；纯布局和机械性改动只列文件名）
## 自检输出（每条命令的最后 20 行）
## 验收清单逐条结果（✓ / ✗ / 需要 Lupo 手动验证）
## 问题
```
