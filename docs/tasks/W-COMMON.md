# W-2 到 W-4 的共同约定（三张卡都要遵守）

## 页面 ID
| 流程 | 页面 |
|---|---|
| 首次设置 | S1 220、S2 221、S3 222 |
| 加水印 | E1 230、E2 231、E3 232、E4 233 |
| 检查 | C1 240、C2 241、C3 242 |
| 分发 | 继续使用占位页 219，等 W-5 再做 |

- 每加一个页面，就要在 `assets/blindmark.rc.in` 里添加一个空模板（写法照抄 210），并且在 `start::run` 的 `pages` 列表里加入这一页。
- S3、E4、C3 是**完成页**，用外页样式。其余都是内页，用内页样式。

## 跳转规则（全部写在 PSN_WIZNEXT 里，用 history 栈）
- 选择任务页的“下一步”：
  - Setup → 220
  - Embed → 有密钥时去 230，没有密钥时去 220
  - Check → 有密钥时去 240，没有密钥时去 220
  - Distribute → 219
- S2 提交成功以后：`task == Setup` 时去 222，否则去该任务的第一页（230 或 240）。
- 进度页（232、241）完成后，程序自动按“下一步”进入完成页：
  1. `set_buttons(h, PSWIZB_NEXT)`
  2. `PostMessageW(sheet, PSM_PRESSBUTTON, PSBTN_NEXT, 0)`
  - 进入进度页前要 `history.clear()`。在进度页和完成页上，PSN_WIZBACK 一律返回 -1，“上一步”按钮也不显示。

## 步骤提示（只加在内页的副标题前面）
格式为 `第 {x} 步，共 {y} 步 · {副标题}`。
- 只有首次设置：S1=1，S2=2，y=2。
- 加水印：E1=1，E2=2，E3=3，y=3。如果前面插入了 S1、S2，就是 S1=1，S2=2，E1=3，E2=4，E3=5，y=5。
- 检查：C1=1，C2=2，y=2。如果前面插入了 S1、S2，就是 S1=1，S2=2，C1=3，C2=4，y=4。
- y 在一次向导内不变。实现方法：在 `St` 里加 `prefix_setup: bool`，在选择任务页点“下一步”时就确定下来。

## 提交按钮的文字
- 进入提交页（S2、E2、C1）时，在 PSN_SETACTIVE 里发送 `PSM_SETNEXTTEXT`（`WM_USER + 137`，wParam 为 0，lParam 指向文字），文字照 ui-copy 写。
- 离开这一页时（PSN_KILLACTIVE），把文字恢复成「下一步(&N) >」。
- 同时在这三页正文的最后一行，加上 ui-copy 里的“提交说明”那一行。这样即使 LITE 模式下改按钮文字没有生效，用户也能看明白。

## 后台任务
- 一律调用 `blindmark::jobs`。界面代码里**不允许**出现 `embed_file`、`detect_image`、`Registry::open`。
- 后台线程通过 `std::sync::mpsc` 把消息发回界面线程。进度页用 `SetTimer(h, 2, 100)` 定时去取。
- 进度页上用户点“取消”或 ✕ 时会触发 PSN_QUERYCANCEL。这时用 `TaskDialogIndirect` 弹出确认，两个自定义按钮分别是「停止」和「继续处理」，文案见 ui-copy。
  - 选“停止”：把 cancel 标志设为 true，然后 `DWLP_MSGRESULT = 1` 并返回 1，阻止向导关闭。等后台任务结束后，照常自动进入完成页。
  - 选“继续处理”：同样返回 1，阻止向导关闭。

## 完成页按钮的返回值
- 「在登记表中查看」：`outcome = ShowInRegistry(ids)`，然后 `PostMessageW(sheet, PSM_PRESSBUTTON, PSBTN_FINISH, 0)`。
- 「转到“…”选项卡」：`outcome = ShowTab(TAB_x)`，然后同样按 Finish。
- 「打开所在文件夹」和「导出检测报告…」：只执行这个操作，不关闭向导。

## App 端（在 W-2 里一次性写好）
在 `App::open_start` 的 `match` 里：
- `SettingsChanged { key_path, registry_dir }`：写入 `self.cfg`，执行 `save()`，然后重新加载。注意：重新加载必须放在 match **之后**，不能在 match 之前做。
- `ShowTab(i)`：执行 `self.select_tab(i)`。
- `ShowInRegistry(ids)`：执行 `self.select_tab(TAB_REG)`；如果 `ids.first()` 有值，调用 `regpage::select_id(self, id)`；如果不止一条，状态栏显示“本次共 {n} 条，已选中第一条”。

## 共同的验收要求
- `grep -n "embed_file\|detect_image\|Registry::open" src/win/start.rs`：零处
- `grep -nw "from_raw\|into_raw\|thread_local" src/win/start.rs`：仍然是 1 / 1 / 0
- 每个新增的页面过程都要经过 `guard`
- 两条自检命令都通过
