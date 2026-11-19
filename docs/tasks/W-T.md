# W-T 设置与测试基础：测试数据文件夹、设置文件读写安全、时间戳服务地址

- **基线分支**：`main`（`5d862bd`），工作分支 `t-w-t`
- **预计**：S（约 200 行，含测试）
- **范围**：`src/win/config.rs`、`src/win/setpage.rs`、`src/win/mod.rs`（只改启动时读设置、窗口标题、保存失败提示）。**不改核心库**（`src/` 下非 `win/` 的文件）。
- **背景**：W-5 走查时踩到三个坑：（1）走查只能用 Lupo 的真实数据目录，要先备份再改名；（2）手改 `settings.json` 后，程序运行或退出时又把它覆盖回去，而且文件读不出来时程序会**悄悄全部恢复默认**，连密钥和登记表的位置都会丢；（3）时间戳服务地址只能手改文件。

## 1. 测试数据文件夹 `BLINDMARK_DATA_DIR`
```rust
/// 纯函数，便于测试：env 有值且不为空（去掉首尾空白后）时用它，否则用 dirs::config_dir()/blindmark。
fn app_dir_from(env: Option<std::ffi::OsString>) -> PathBuf;
pub fn app_dir() -> PathBuf { app_dir_from(std::env::var_os("BLINDMARK_DATA_DIR")) }
pub fn test_data_dir() -> Option<PathBuf>;   // 环境变量生效时返回该路径，否则 None
```
- 环境变量生效时，设置、默认密钥位置、默认登记表位置都在这个文件夹里（现有代码都经过 `app_dir()`，只改这一处）。文件夹不存在时由程序按需创建，和现在一样。
- 生效时，主窗口标题改为：`blindmark 盲水印（测试数据：{路径}）`。这是唯一的界面提示，防止测试的人以为在用真实数据。
- 不要在测试里调用 `std::env::set_var`（edition 2024 下是 unsafe，而且会影响并行测试）。测试 `app_dir_from` 这个纯函数即可。

## 2. 设置文件的读取
```rust
pub enum LoadProblem {
    /// 文件存在但读不出来或不是有效的设置。backup = 另存后的文件路径（另存失败时为 None）。
    Unreadable { backup: Option<PathBuf>, detail: String },
}
/// 纯函数版本，便于测试。
pub fn load_from(path: &Path) -> (Settings, Option<LoadProblem>);
pub fn load() -> (Settings, Option<LoadProblem>);   // = load_from(&Settings::path())
```
规则：
| 情况 | 返回的设置 | LoadProblem | 对文件做什么 |
|---|---|---|---|
| 文件不存在 | 默认 | None | 不动（第一次运行） |
| 文件存在，JSON 有效 | 文件内容（缺的字段用默认值，多余字段忽略，和现在一样） | None | 不动 |
| 文件存在，读取失败或 JSON 无效 | 默认 | Unreadable | **先把原文件复制**为同目录下的 `settings.json.bad`；如果已存在，用 `settings.json.bad-YYYYMMDD-HHMMSS`（本地时间）。复制失败时 backup = None，原文件不动 |

- `mod.rs` 启动时调用 `load()`。有 `LoadProblem` 时，主窗口显示出来以后弹一次提示（用现有的 `warn`），文案照抄：
  - 另存成功：`设置文件（settings.json）读不出来，已改用默认设置。\n\n原文件另存为：{backup 完整路径}\n里面记录的密钥和登记表位置，可以在“设置”选项卡里照着改回来。`
  - 另存失败：`设置文件（settings.json）读不出来，已改用默认设置。原文件没能另存，也没有改动：{settings.json 完整路径}`
- 提示只弹一次；之后正常使用，下一次保存会写出一份有效的新 settings.json（坏文件已另存，不怕覆盖）。**另存失败时，本次运行不要保存设置**（避免覆盖唯一的原文件）：用一个标志位让 `save()` 直接返回 `Err`。

## 3. 设置文件的保存
```rust
pub fn save(&self) -> Result<(), String>;   // 原来是 ()，返回值改为 Result
```
- 写到 `settings.json.tmp`，再 rename 成 `settings.json`（同目录，原子替换）。
- 所有调用 `cfg.save()` 的地方（mod.rs 里 7 处、setpage.rs 1 处）：失败时在状态栏显示 `设置没能保存：{原因}`。不弹窗（避免关闭窗口时弹框）。成功时保持各处现有的状态栏文字不变。

## 4. 设置页：时间戳服务地址
- `Settings` 加：
  ```rust
  /// Trusted-timestamp service used when the user ticks “申请可信时间戳”.
  #[serde(default = "default_tsa_url")]
  pub tsa_url: String,
  fn default_tsa_url() -> String { blindmark::timestamp::DEFAULT_TSA.to_string() }
  ```
  `Default` 里同样用 `default_tsa_url()`。确认 `Settings` 没有 `deny_unknown_fields`（旧文件里 alpha 留下的 `"timestamp": true` 要能读）。
- 设置页“常规”一组（或现有布局里最合适的一组，按现有 `layout` 风格排版）加一行：
  - 标签：`可信时间戳服务`
  - 输入框（显示 `tsa_url`），右边按钮「恢复默认」（把输入框改回 `DEFAULT_TSA`，不立即保存）
  - 下面一行灰色说明：`只有在加水印或分发时勾选“申请可信时间戳”才会联网，并且只发送这批文件的摘要。`
- 保存（现有的 `apply` → `read_controls`）时校验：去掉首尾空白后必须以 `https://` 或 `http://` 开头，并且后面不为空；否则返回错误 `时间戳服务地址请填写以 https:// 开头的网址。`（沿用现有的 `warn` 弹窗方式）。
- `controls_from` / `load` 里把 `tsa_url` 填进输入框。

## 测试（写在 `src/win/config.rs` 的 `#[cfg(test)]` 里；它们不依赖 Win32，确认在 `cargo test --release --lib` 或现有的 win 测试方式里能跑到；跑不到就在审阅包里说明怎么跑的）
- `app_dir_from(None)` 以 `blindmark` 结尾；`app_dir_from(Some("D:\\x".into()))` == `D:\x`；`Some("  ")` 当作没设。
- `load_from`：文件不存在 → 默认、None；有效 JSON（含未知字段 `"timestamp": true`、不含 `tsa_url`）→ 读到原值且 `tsa_url == DEFAULT_TSA`；无效 JSON → 默认、Unreadable，且 `settings.json.bad` 内容与原文件逐字节相同；再来一次无效 → 生成带时间戳后缀的 `.bad-…`，第一份 `.bad` 不变。
- `save` 之后再 `load_from` 能读回同样的 `tsa_url`，目录里不残留 `.tmp`。
- 测试用的目录放在 `target\tmp` 下（见派卡说明的平台一节），不要写到系统临时目录以外的地方。

## 验收（Lupo 手动）
1. PowerShell：`$env:BLINDMARK_DATA_DIR = "D:\Blind_Mark\_testdata\W-T"`，再运行复制出来的 exe → 标题显示“（测试数据：D:\Blind_Mark\_testdata\W-T）”，首次设置向导弹出，完成后该文件夹里有 settings.json、blindmark.key、registry。`%APPDATA%\blindmark` 没有任何变化。
2. 关掉程序，用记事本把测试文件夹里的 settings.json 改坏（删掉一个右花括号），再启动 → 弹出“设置文件读不出来…”，文件夹里多了 settings.json.bad，内容就是改坏的那份。
3. 设置页把时间戳服务改成 `example.invalid`（不带 https）→ 保存时提示要以 https:// 开头；改成 `https://example.invalid/` → 保存成功；关掉再打开，输入框里还是这个地址。点「恢复默认」再保存 → 变回 freetsa。
