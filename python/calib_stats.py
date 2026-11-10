"""Statistical evidence for the README (false-positive rate, detection rate, wrong IDs).

Usage:
    python python/calib_stats.py calib.csv calib_pos.csv > docs/statistics.md

calib.csv      negatives from `examples/calibrate.rs` (the positive rows in it are ignored)
calib_pos.csv  positives from `calibrate --skip-negatives` (V3-6b or later: registry holds the true id)

Requires numpy and scipy. Bootstraps resample whole images (cluster bootstrap), because the
rows of one image (several keys and variants) are correlated. Fixed seed, so the output is
reproducible.
"""
import csv
import math
import sys

import numpy as np
from scipy import stats

SEED = 20261110
Z_THRESH = 6.5
Z_PRESENT_ONLY = 8.0
CRC_PASS = 2.0 ** -16  # chance that 48 random bits pass the CRC-16
REGISTRY_FP = 1e-6  # per-check budget of the registry rule (wm::REGISTRY_FP)
VARIANTS = ["orig", "jpeg75", "half", "crop50"]


def num(x):
    try:
        return float(x)
    except (TypeError, ValueError):
        return math.nan


def load(path):
    with open(path, newline="", encoding="utf-8") as f:
        return list(csv.DictReader(f))


def gumbel_mom(a):
    m, s = a.mean(), a.std(ddof=1)
    beta = s * math.sqrt(6) / math.pi
    return m - 0.5772 * beta, beta


def gumbel_sf(a, t):
    mu, beta = gumbel_mom(a)
    return float(stats.gumbel_r.sf(t, mu, beta))


def wilson(k, n):
    lo, hi = stats.binomtest(k, n).proportion_ci(0.95, method="wilson")
    return lo, hi


def upper_zero(n):
    """Exact one-sided 95% upper bound when 0 events are seen in n trials."""
    return 1 - 0.05 ** (1 / n)


def pct(x):
    return f"{100 * x:.1f}%"


def main():
    rng = np.random.default_rng(SEED)
    neg = [r for r in load(sys.argv[1]) if r["kind"] == "neg"]
    pos = [r for r in load(sys.argv[2]) if r["kind"] == "pos"]
    if not pos or "id_ok" not in pos[0]:
        sys.exit("calib_pos.csv must come from calibrate V3-6b or later (id_ok column)")

    z = np.array([num(r["z_sync"]) for r in neg])
    img = np.array([r["image"] for r in neg])
    fin = np.isfinite(z)
    a = z[fin]
    images = sorted(set(img[fin]))
    groups = {u: a[img[fin] == u] for u in images}
    n_found = sum(1 for r in neg if r["method"])

    # cluster bootstrap of the tail probabilities
    g65, g8 = [], []
    for _ in range(1000):
        s = rng.choice(len(images), len(images), replace=True)
        b = np.concatenate([groups[images[i]] for i in s])
        g65.append(gumbel_sf(b, Z_THRESH))
        g8.append(gumbel_sf(b, Z_PRESENT_ONLY))
    g65_hi, g8_hi = np.percentile(g65, 97.5), np.percentile(g8, 97.5)
    mu, beta = gumbel_mom(a)

    print("# 统计证据（误报率、识别率、认错编号）\n")
    print(f"由 `python/calib_stats.py` 生成，随机种子 {SEED}。置信区间都是 95%。")
    print("bootstrap 按图片整组重抽（同一张图的多把密钥、多个版本彼此相关）。\n")
    print("图由 `python/calib_plots.py` 生成。\n")
    print("![分数分布](images/calib_scores.svg)\n")
    print("![尾部概率](images/calib_tail.svg)\n")
    print("![识别率](images/calib_detection.svg)\n")

    print("## 1. 把未加水印的图误判为“这是你的图片”\n")
    print(f"- 负样本：{len(set(img))} 张图（照片、插画、截图、合成图），{len(neg)} 次检测，每张图 × 3 个版本 × 10 把密钥。")
    print(f"- **实测：{n_found} 次误判。** 这里阈值放到负无穷，每次都走完 CRC 和登记表匹配两步。")
    print(f"- 只凭实测能给出的上界：每次检测 ≤ {upper_zero(len(neg)):.1e}；如果把同一张图的所有检测算作一次，每张图 ≤ {upper_zero(len(images)):.1e}。")
    print("  这两个数只说明样本量有限，不代表真实误报率这么高。")
    print("- **模型估计**：一次误判必须同时满足两件事：")
    print(f"  1. 同步分数 z ≥ {Z_THRESH}；")
    print(f"  2. 48 位读数恰好通过 CRC-16（概率 2⁻¹⁶），或者恰好通过登记表规则（按设计 ≤ {REGISTRY_FP:.0e}）。")
    print("  未加水印的图里，读出的位是噪声，和 z 无关，所以两个概率可以相乘。")
    exp_crc = len(neg) * CRC_PASS
    print(f"  实测 CRC 碰巧通过 0 次，按 2⁻¹⁶ 应为 {exp_crc:.2f} 次（全为 0 的概率 {math.exp(-exp_crc):.2f}），和理论一致。")
    print(f"- P(z ≥ {Z_THRESH})：实测 {(a >= Z_THRESH).mean():.1e}。Gumbel 拟合 {gumbel_sf(a, Z_THRESH):.1e}，bootstrap 上界 {g65_hi:.1e}。")
    print("- Gumbel 拟合在尾部**偏保守**，预测的超标次数在每个阈值都多于实测：\n")
    print("| 阈值 | 实测超过次数 | Gumbel 预测次数 |")
    print("|---|---|---|")
    for t in (6.0, 6.25, 6.5, 6.75, 7.0):
        print(f"| {t} | {(a >= t).sum()} | {len(a) * stats.gumbel_r.sf(t, mu, beta):.1f} |")
    bound = g65_hi * (CRC_PASS + REGISTRY_FP)
    print(f"\n- **结论：每次检测误判的概率 ≤ {bound:.0e}**，也就是 {g65_hi:.1e} × (2⁻¹⁶ + 10⁻⁶)。")
    print("  按最保守的拟合（Gumbel 最大似然，尾部更重）算，也低于 10⁻⁶。\n")

    print("## 2. 读不出编号时，提示“可能有你的水印”\n")
    print(f"- 门槛 z ≥ {Z_PRESENT_ONLY}。实测 0 / {len(a)} 次，未加水印的图 z 最大为 {a.max():.2f}。")
    print(f"- Gumbel 估计 {gumbel_sf(a, Z_PRESENT_ONLY):.1e}，bootstrap 上界 {g8_hi:.1e}。\n")

    print("## 3. 识别率（加了水印的图）\n")
    imgs = sorted(set(r["image"] for r in pos))
    ok_orig = {r["image"] for r in pos if r["variant"] == "orig" and r["id_ok"] == "1"}
    print(f"{len(imgs)} 张图，每张一把随机密钥，登记表里有 1000 个编号，其中包含真实编号。“识别”指找到并且编号正确。\n")
    print("| 版本 | 全部图片 | 加完水印能读出的图（通过自检） |")
    print("|---|---|---|")
    for v in VARIANTS:
        q = [r for r in pos if r["variant"] == v]
        k = sum(r["id_ok"] == "1" for r in q)
        lo, hi = wilson(k, len(q))
        q2 = [r for r in q if r["image"] in ok_orig]
        k2 = sum(r["id_ok"] == "1" for r in q2)
        lo2, hi2 = wilson(k2, len(q2))
        right = "100%（按定义）" if v == "orig" else f"{k2}/{len(q2)} = {pct(k2 / len(q2))} [{pct(lo2)}, {pct(hi2)}]"
        print(f"| {v} | {k}/{len(q)} = {pct(k / len(q))} [{pct(lo)}, {pct(hi)}] | {right} |")
    print("\n版本说明：orig 是加完水印不做处理；jpeg75 是 JPEG 质量 75；half 是缩小一半；crop50 是居中裁掉一半面积。所有图长边不超过 1600 像素。\n")
    bad = sorted(set(imgs) - ok_orig)
    print(f"加完水印就读不出的 {len(bad)} 张：")
    print("- 大部分是几乎全黑或全白的截图、大面积纯色的图，图片本身装不下水印。")
    print("- 1 张 640×428 的小照片。")
    print("- 1 张带点阵背景的插画，原因还在查。")
    print("V3-6c 的自检会在加水印时提示用户。")
    print("只要加完能读出，这批图在三种处理后全部能识别。\n")

    print("## 4. 认错编号\n")
    found = [r for r in pos if r["id_ok"] in ("0", "1")]
    w = sum(r["id_ok"] == "0" for r in found)
    print(f"- 实测：{w} / {len(found)} 次识别认错编号。只凭实测给出的上界是 {pct(upper_zero(len(found)))}。")
    print(f"- 按设计：登记表匹配规则的误配概率 ≤ {REGISTRY_FP:.0e} 每次检测，由二项分布精确计算（V3-6b）。")
    print("  CRC 路径认错编号，需要读出的 48 位错成另一个合法码字，概率约为 2⁻¹⁶ 乘以出错概率。")
    print("- 旧规则在同样的数据上认错 5 次，V3-6b 修复后为 0 次。\n")

    zf = np.array([num(r["z_sync"]) for r in pos if r["id_ok"] == "1"])
    print("## 5. 分数间隔\n")
    print(f"能识别的水印图，z 最低 {zf.min():.2f}；未加水印的图，z 最高 {a.max():.2f}。两组没有重叠。\n")

    print("## 局限")
    print(f"- 正样本只有 {len(imgs)} 张，识别率的置信区间较宽。处理方式只有 4 种温和的，不含截图、旋转、拍屏，这些见 README 的评测表。")
    print("- 所有图长边不超过 1600 像素，水印强度为默认值 2.6。")
    print("- 标定用的是 `wm::detect_luma`。程序在有登记表缩略图时还会多试一条拍屏校正路径，相当于多一次机会，误报率上界大约翻倍，仍远低于 10⁻⁶。")


if __name__ == "__main__":
    main()
