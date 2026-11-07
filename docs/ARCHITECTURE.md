# 架构说明（2026-11-07）

## 模块
| 模块 | 作用 | 谁能改 |
|---|---|---|
| `src/keys.rs` `src/wm.rs` `src/affine.rs` `src/features.rs` `src/imgproc.rs` | 水印格式 blindmark/v1 和检测算法 | 只能按 Claude 的设计卡改 |
| `src/registry.rs` | 登记表：`registry.json` + `meta.json` + 缩略图 | 字段改动需要设计卡 |
| `src/recipients.rs` `src/distribute.rs` `src/timestamp.rs` | 接收人簿、分发、时间戳（alpha 分支） | 按任务卡 |
| `src/jobs.rs`（W-0 新建） | **后台任务的唯一实现**：加水印、检测，以后再加分发 | 按任务卡 |
| `src/lib.rs` | `embed_file`、`detect_image_opts`、`Report` | 按任务卡 |
| `src/bin/cli.rs` | 命令行 | 按任务卡 |
| `src/win/*` | Win32 界面，只负责显示和收集输入 | 按任务卡 |

## 状态归属（最重要的规则）
1. **设置**：`App.cfg` 是唯一的内存副本。其他窗口（向导、对话框）拿到的是副本，改动通过返回值交回 App，由 App 写入 `settings.json`。唯一例外是向导直接保存“启动时显示此向导”这个选项。
2. **登记表**：`App.reg` 只用于显示。后台任务在自己的线程里重新打开登记表，并在里面写入。**任何后台任务写完以后，App 都必须调用 `reload_registry()`。**
3. **密钥和登记表必须配对**：凡是要写登记表的任务，都要先调用 `check_key`。这一步统一在 `jobs.rs` 里做，界面代码不要自己重复做。
4. **界面不直接调用 `embed_file`、`detect_image*`、`distribute`**，一律通过 `jobs.rs` 调用。这样向导和主窗口用的是同一份逻辑，不会再出现两边行为不一致。

## 错误
- `jobs.rs` 对外只返回 `UserError { message, detail }`。
- `message` 是给用户看的中文一句话，写法按 `docs/specs/ui-copy.md` 第三节。
- `detail` 是原始错误，只在点“详细信息”时显示，或者写进日志。

## 窗口和线程
- 后台线程通过 `PostMessage`、channel 加定时器把消息交回界面线程。界面控件只能在界面线程里操作。
- 模态窗口关闭时，必须重新启用它的父窗口。对话框正在执行任务时不能直接销毁，必须先问用户，或者先停下任务。
