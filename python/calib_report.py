#!/usr/bin/env python3
"""Turn the calibration CSV into docs/calibration.md (task V3-6).

    python python/calib_report.py calib.csv > docs/calibration.md

Only numpy and the standard library are used: no scipy, no plotting, no significance
tests or bootstrap. The Gumbel numbers are a plain moment fit, exactly as described in the
task card:

    beta  = s * sqrt(6) / pi
    mu    = mean - 0.5772 * beta
    P(z >= t) = 1 - exp(-exp(-(t - mu) / beta))
"""
from __future__ import annotations

import csv
import math
import sys

import numpy as np

NEG_VARIANTS = ["orig", "jpeg75", "half"]
POS_VARIANTS = ["orig", "jpeg75", "half", "crop50"]
THRESHOLDS = [4.0, 4.5, 5.0, 5.5, 6.0, 6.5, 7.0, 7.5, 8.0, 8.5, 9.0]
CRC_RATE = 2.0 ** -16


def load(path: str) -> list[dict]:
    with open(path, newline="", encoding="utf-8") as f:
        return list(csv.DictReader(f))


def zs(rows: list[dict]) -> np.ndarray:
    vals = []
    for r in rows:
        try:
            v = float(r["z_sync"])
        except (KeyError, ValueError):
            continue
        if math.isfinite(v):
            vals.append(v)
    return np.array(vals, dtype=float)


def quantiles(x: np.ndarray) -> dict:
    if x.size == 0:
        return {}
    return {
        "n": int(x.size),
        "p50": float(np.quantile(x, 0.50)),
        "p90": float(np.quantile(x, 0.90)),
        "p99": float(np.quantile(x, 0.99)),
        "p999": float(np.quantile(x, 0.999)),
        "max": float(np.max(x)),
    }


def gumbel_params(x: np.ndarray) -> tuple[float, float] | None:
    """Moment estimates; returns (mu, beta)."""
    if x.size < 2:
        return None
    beta = float(np.std(x, ddof=1) * math.sqrt(6.0) / math.pi)
    if beta <= 0.0:
        return None
    mu = float(np.mean(x) - 0.5772 * beta)
    return mu, beta


def gumbel_sf(t: float, mu: float, beta: float) -> float:
    return 1.0 - math.exp(-math.exp(-(t - mu) / beta))


def main() -> None:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    if len(sys.argv) != 2:
        sys.exit("usage: python python/calib_report.py calib.csv")
    rows = load(sys.argv[1])
    neg = [r for r in rows if r.get("kind") == "neg"]
    pos = [r for r in rows if r.get("kind") == "pos"]

    print("# 误报率标定报告")
    print()
    print("数据由 `examples/calibrate.rs` 采集，本文件由 `python/calib_report.py` 生成。")
    print()

    # 1 -------------------------------------------------------------------
    print("## 1. 样本数")
    print()
    print("| kind | variant | 行数 |")
    print("|---|---|---|")
    for v in NEG_VARIANTS:
        print(f"| neg | {v} | {sum(1 for r in neg if r.get('variant') == v)} |")
    print(f"| neg | 合计 | {len(neg)} |")
    for v in POS_VARIANTS:
        print(f"| pos | {v} | {sum(1 for r in pos if r.get('variant') == v)} |")
    print(f"| pos | 合计 | {len(pos)} |")
    print()
    if rows and "id_ok" in rows[0]:
        wrong = sum(1 for r in pos if (r.get("id_ok") or "") == "0")
        print(f"认错编号（id_ok = 0）的次数：{wrong}")
        print()
    bad = sum(1 for r in neg if not math.isfinite(float(r.get("z_sync", "nan") or "nan")))
    if bad:
        print(
            f"负样本中有 {bad} 行的 `z_sync` 不是有限值（记录为 nan），这些行未参与第 2、3 节。"
            "原因：纯色 / 大面积平涂图没有水印信号，检测器按设计直接判为未找到（V3-6a 的 too_flat）。"
        )
        print()

    # 2 -------------------------------------------------------------------
    print("## 2. 负样本 z_sync 分位数")
    print()
    print("| variant | n | 50% | 90% | 99% | 99.9% | 最大值 |")
    print("|---|---|---|---|---|---|---|")
    neg_all = zs(neg)
    for v in NEG_VARIANTS:
        q = quantiles(zs([r for r in neg if r.get("variant") == v]))
        if q:
            print(f"| {v} | {q['n']} | {q['p50']:.2f} | {q['p90']:.2f} | {q['p99']:.2f} | {q['p999']:.2f} | {q['max']:.2f} |")
    q = quantiles(neg_all)
    if q:
        print(f"| 合并 | {q['n']} | {q['p50']:.2f} | {q['p90']:.2f} | {q['p99']:.2f} | {q['p999']:.2f} | {q['max']:.2f} |")
    print()

    # 3 -------------------------------------------------------------------
    print("## 3. 阈值与误报率外推")
    print()
    params = gumbel_params(neg_all)
    if params is None:
        print("负样本不足 2 行，无法拟合 Gumbel 分布。")
        mu = beta = float("nan")
    else:
        mu, beta = params
        print(f"Gumbel 矩估计（合并负样本，n={neg_all.size}）：β = {beta:.3f}，μ = {mu:.3f}")
    print()
    header = "| t | 负样本实测 P(z≥t) | Gumbel 外推 P(z≥t) |"
    for v in POS_VARIANTS:
        header += f" 正样本 {v} |"
    print(header)
    print("|---|---|---|" + "---|" * len(POS_VARIANTS))
    for t in THRESHOLDS:
        emp = float(np.mean(neg_all >= t)) if neg_all.size else float("nan")
        ext = gumbel_sf(t, mu, beta) if params else float("nan")
        line = f"| {t:.1f} | {emp:.3e} | {ext:.3e} |"
        for v in POS_VARIANTS:
            x = zs([r for r in pos if r.get("variant") == v])
            p = float(np.mean(x >= t)) if x.size else float("nan")
            line += f" {p:.3f} |"
        print(line)
    print()

    # 4 -------------------------------------------------------------------
    print("## 4. 负样本中 method 非空的记录")
    print()
    hit = [r for r in neg if (r.get("method") or "").strip()]
    print(f"共 {len(hit)} 条。理论上 CRC 碰巧通过的概率是 2⁻¹⁶ ≈ {CRC_RATE:.2e}。")
    if hit:
        print()
        print("| image | variant | method | z_sync |")
        print("|---|---|---|---|")
        for r in hit:
            print(f"| {r.get('image', '')} | {r.get('variant', '')} | {r.get('method', '')} | {r.get('z_sync', '')} |")
    print()

    # 5 -------------------------------------------------------------------
    print("## 5. 如何解读")
    print()
    print(
        "> 一张未加水印的图片被判为“找到你的水印”，需要同时满足两件事：同步分数超过阈值，"
        "以及 CRC 碰巧通过（概率 2⁻¹⁶）或登记表碰巧匹配（设计值 10⁻⁶）。"
        "所以整体误报率约等于第 3 节的外推概率 × (2⁻¹⁶ + 10⁻⁶)。"
    )


if __name__ == "__main__":
    main()
