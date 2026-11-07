# HF-1 向导热修复

- **基线分支**：`feat/wizard`，工作分支：`t-hf-1`
- **目标**：先止血。关闭向导不能再把整个程序带着一起退出；修掉向导会写坏登记表、选项设了不生效的问题。这样 feat/wizard 才能安全合并进 main。**向导的界面和文案先不改**，它们将在 W-1 到 W-5 中整个重写。
- **预计规模**：S，diff 应该在 80 行以内

## 范围
- 只改这两个文件：`src/win/wizard.rs`、`src/win/mod.rs`
- 删除这 5 个误提交的调试文件：`_fresh.err`、`_fresh.out`、`_probe.out`、`_ui.err`、`_ui.out`
- 把 `_fresh.*`、`_probe.*`、`_ui.*`、`build.log` 加进 `.gitignore`
- 其他文件一律不动

## 要改的地方

### 1. 二次释放（致命问题）
在 `wizard::open` 里：
- 现在的写法：`let mut boxed = Box::new(State {...});`，然后 `let raw: *mut State = &mut *boxed;`
- 改成：`let raw: *mut State = Box::into_raw(Box::new(State {...}));`
- 函数后面已有的 `let done = unsafe { Box::from_raw(raw) };` 保持原样。改完以后整个函数里**只剩这一个** `Box::from_raw`，`boxed` 这个变量不能再出现。

### 2. 启动时先不自动弹出向导
在 `mod.rs` 的 `run()` 里，删掉 `if config::Settings::is_first_run() { a.open_wizard(); }` 这三行。菜单项和快捷键保留。启动时弹出的功能留到 W-1 重新做。

### 3. 向导加水印时没有校验密钥和登记表是否配对
在 `start_run` 的 `Mode::Embed` 分支里，打开登记表之后、进入循环之前，加上：
```rust
if let Err(e) = reg.check_key(&key) {
    let _ = tx.send(Msg::Fatal(format!("这个登记表属于另一把密钥，不能和当前密钥一起使用。（{e:#}）")));
    return;
}
```
提示文字照抄上面这句，W-0 会把它换成规范里的写法。

### 4. 检测选项不生效
在 `Mode::Detect` 分支里，把 `blindmark::detect_image(&img, &key, reg.as_ref(), known.as_ref(), Some(&thumbs))` 改成 `blindmark::detect_image_opts(&img, &key, reg.as_ref(), known.as_ref(), Some(&thumbs), &cfg.detect_options())`。`cfg` 就是线程闭包里已经有的那个 `s.cfg.clone()`。

### 5. 向导里选的密钥和登记表没有保存
在 `wizard::open` 的 `if accepted { ... }` 里，在 `cfg.save()` 之前加两行：
```rust
cfg.key_path = done.cfg.key_path.clone();
cfg.registry_dir = done.cfg.registry_dir.clone();
```

### 6. 向导悄悄改掉了全局默认值
在 `build_embed` 里：
- 强度滑块：`trackbar(h, ID_STRENGTH, 5, 60)` 改成 `trackbar(h, ID_STRENGTH, 15, 40)`，和主窗口保持一致。创建之后立刻把数值标签设成 `format!("{:.1}", st().cfg.strength)`，不要等用户拖动滑块才显示。
- 格式下拉框：`cb_set_sel(fmt, 0)` 改成 `cb_set_sel(fmt, if st().cfg.jpeg { 1 } else { 0 })`。
- 质量下拉框：不要再固定选第 4 项。要选中和 `st().cfg.jpeg_quality` 相同的那一项（列表是 60、65 … 100，95 在列表里）。如果没有相同的，就选 95。

## 验收
- [ ] `grep -n "from_raw\|into_raw\|boxed" src/win/wizard.rs`：`from_raw` 和 `into_raw` 各只有 1 处，`boxed` 一处都没有
- [ ] 三条自检命令全部通过（见 AGENTS.md）
- [ ] 需要 Lupo 手动验证：
  - 打开向导后，分别用取消、✕、Esc、在执行页点完成这四种方式关闭，**每种都要单独重新打开一次再关**。关闭后主窗口都还在。
  - 第一次启动时不会自动弹出向导。
  - 在向导里选一把和当前登记表不配对的密钥，然后加水印：结果里显示“属于另一把密钥”，并且登记表的条数没有增加。
  - 在向导里取消勾选“仿射同步”，再检查一张旋转过 90° 的带水印图片：结果是检测不到。这说明选项确实生效了。
  - 什么都不改，直接走完一遍加水印，然后打开“设置”页：JPEG 质量仍然是原来的值（默认是 95）。

## 不要做
- 不要改任何文案和布局，也不要重构；那些属于 W-1 到 W-5。
- 不要改 `Cargo.toml` 里的版本号。
