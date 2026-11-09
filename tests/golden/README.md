# 黄金样本

这组文件是格式冻结用的固定样本。测试只读它们，绝不改写。

## 禁止重新生成样本

如果某个测试因为样本而失败，说明代码把格式或兼容性改坏了——**去修代码，不要重新生成样本**。
重新生成会让旧数据兼容性的保证在无声中失效。只有当 Lupo 明确决定升级格式时，才允许重新生成，且必须同一次提交里更新本文件。

密钥固定为 `(0u8..32).collect::<Vec<u8>>()`，和 `src/keys.rs` 里的测试向量一致。

## 每个文件的来历

### `cover.png`、`marked.png`

由 `examples/make_golden.rs` 生成（`cargo run --release --example make_golden`，只跑过一次）：

- `cover.png`：320×256 的合成图，横向渐变 + 每 8×8 块一个值的块状噪声（LCG，种子 `0x5EED`）+ 三条斜线。不依赖任何外部图片。
- `marked.png`：`cover.png` 用上面的密钥、编号 `0x0BADF00D`、强度 `2.6` 加水印（`wm::embed_rgb`）后写出的 PNG，PSNR 约 42.0 dB。

### `registry-v02/`

v0.2 格式的登记表，用 **v0.2 时的代码**生成：在提交 `7eb5863` 的独立工作树里构建 CLI，然后

```
blindmark-cli embed cover.png --out out --key golden.key --registry reg
```

把 `reg/` 原样拷过来。条目里**没有** `recipient_id`、`recipient_name`、`group_id`、`group_name`、`batch_id` 这些 v0.3 字段，用来保证新代码能读旧登记表。

### `recipients-v1.json`

alpha 格式的接收人簿，手写：1 个一级分组 `g_0001`（客户）、1 个子分组 `g_0002`（2026 秋季，父组 `g_0001`）、2 个人 `r_0001`（张三）、`r_0002`（李四）。ID 用旧格式，用来保证新代码能按旧 ID 找到人和分组。
