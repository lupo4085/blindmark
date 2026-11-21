# AGENTS.md — 给编码 agent（DeepSeek Harness）的规则

本仓库由两个 agent 协作开发：
- **Claude**：负责设计和审阅，编写规格和任务卡，在主仓库审阅你推送的分支。**不改 `src/`。**
- **DeepSeek Harness（你）**：在 `blindmark-ds` 克隆里严格按任务卡实现，**不做任何设计决策**。

只有 Lupo 能把代码合并进 `main`。

## 开工前必读（每次都读）
1. 本文件
2. `docs/ARCHITECTURE.md`
3. 你要做的那张任务卡：`docs/tasks/<卡号>.md`
4. 任务卡里引用的规格。界面卡一律以 `docs/specs/ui.md` 为准。

## 铁律
1. **只做任务卡写明的事。** 发现规格有错、有缺口，或者需要扩大范围，就写进审阅包的「问题」一栏，**不要自己动手改**。
2. **文案一字不改。** 界面上出现的所有文字，必须原样取自 `docs/specs/ui.md` 或任务卡。需要新文案时先在审阅包里提问。
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
6. **做完必须自检**：运行 `scripts/check`（Windows 用 `scripts/check.ps1`，其他平台用 `scripts/check.sh`），必须打印 `ALL CHECKS PASSED`。

7. **暂时不跑 `cargo fmt`**：现有代码是手工排版，不符合 rustfmt 的规则。整个仓库的格式化以后单独开一张卡来做（还没排期）。在那之前**禁止运行 `cargo fmt`**，新写的代码模仿周围代码的风格就可以。
8. **Windows 本机的规矩**（Harness 沙箱里）：
   - 运行 `cargo test` 或 `scripts/check.ps1` 之前，先执行 `$tmp = Join-Path (Get-Location) 'target\tmp'; New-Item -ItemType Directory -Force $tmp | Out-Null; $env:TEMP = $tmp; $env:TMP = $tmp`（沙箱里写不进系统的临时文件夹）。
   - 所有临时文件、演示输出、基准测试都放在 `target\tmp` 下。不许在 `blindmark-ds` 之外创建文件或目录，不许 `git worktree add`。
   - 运行图形程序之前，必须先设 `BLINDMARK_DATA_DIR` 指向 `target\tmp` 下的一个文件夹，并用 `echo $env:BLINDMARK_DATA_DIR` 确认不为空。不设就不要运行（否则会读写 Lupo 的真实数据）。
   - 不要安装或升级任何 Python 包。

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

推送分支以后，把派卡指令要求的“最后回复”写进 `review/<卡号>.reply.md`（不提交），再在对话里回复同样内容。
