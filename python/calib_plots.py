"""Figures for docs/statistics.md and the README.

Usage:
    python python/calib_plots.py calib.csv calib_pos.csv docs/images

Writes calib_scores.svg, calib_tail.svg, calib_detection.svg (and .png copies).
Requires numpy, scipy, matplotlib and a CJK font (e.g. Noto Sans CJK).
"""
import csv
import math
import sys
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np
from scipy import stats

BLUE, ORANGE = "#2a78d6", "#eb6834"
INK, INK2, GRID, SURFACE = "#0b0b0b", "#52514e", "#e1e0d9", "#fcfcfb"
Z_THRESH, Z_PRESENT_ONLY = 6.5, 8.0
VARIANTS = [("orig", "原图\nunchanged"), ("jpeg75", "JPEG 75"), ("half", "缩小一半\nhalf size"), ("crop50", "裁掉一半\nhalf-area crop")]

plt.rcParams.update({
    "font.family": ["Noto Sans CJK SC", "Noto Sans CJK TC", "Noto Sans CJK JP", "DejaVu Sans"],
    "svg.fonttype": "path",
    "font.size": 10,
    "axes.edgecolor": GRID,
    "axes.labelcolor": INK2,
    "xtick.color": INK2,
    "ytick.color": INK2,
    "axes.titlecolor": INK,
    "axes.titlesize": 12,
    "axes.titleweight": "bold",
    "axes.titlelocation": "left",
    "axes.spines.top": False,
    "axes.spines.right": False,
    "figure.facecolor": SURFACE,
    "axes.facecolor": SURFACE,
})


def num(x):
    try:
        return float(x)
    except (TypeError, ValueError):
        return math.nan


def load(path):
    with open(path, newline="", encoding="utf-8") as f:
        return list(csv.DictReader(f))


def grid(ax, axis="y"):
    ax.grid(axis=axis, color=GRID, linewidth=0.6)
    ax.set_axisbelow(True)


def save(fig, out, name):
    for ext in ("svg", "png"):
        fig.savefig(out / f"{name}.{ext}", dpi=200, bbox_inches="tight")
    plt.close(fig)


def threshold_line(ax, x, text, ytext):
    ax.axvline(x, color=INK2, linewidth=1, linestyle=(0, (3, 3)))
    ax.text(x + 0.25, ytext, text, color=INK2, fontsize=9, va="top")


def fig_scores(neg_z, pos_z, out):
    fig, ax = plt.subplots(figsize=(8, 3.6))
    bins = np.arange(3.5, 41, 0.5)
    ax.hist(neg_z, bins=bins, color=BLUE, edgecolor=SURFACE, linewidth=1, label=f"未加水印 unmarked (n={len(neg_z):,}, max {neg_z.max():.2f})")
    ax.hist(pos_z, bins=bins, color=ORANGE, edgecolor=SURFACE, linewidth=1, label=f"加了水印并识别 marked & identified (n={len(pos_z)}, min {pos_z.min():.2f})")
    ax.set_yscale("log")
    ax.set_xlabel("同步分数 z_sync · sync score")
    ax.set_ylabel("次数 · count (log)")
    threshold_line(ax, Z_THRESH, "判定阈值 6.5\nthreshold", 4000)
    threshold_line(ax, Z_PRESENT_ONLY, "“可能有水印” 8.0\npresent-only", 300)
    ax.set_title("两组分数没有重叠 · Scores do not overlap")
    ax.legend(frameon=False, loc="upper right", labelcolor=INK)
    grid(ax)
    save(fig, out, "calib_scores")


def fig_tail(neg_z, out):
    a = np.sort(neg_z)
    n = len(a)
    m, s = a.mean(), a.std(ddof=1)
    beta = s * math.sqrt(6) / math.pi
    mu = m - 0.5772 * beta
    fig, ax = plt.subplots(figsize=(8, 3.6))
    t = np.linspace(5.5, 9.0, 300)
    emp = np.array([(a >= x).mean() for x in t])
    mask = emp > 0
    ax.step(t[mask], emp[mask], where="post", color=BLUE, linewidth=2, label="实测 measured")
    ax.plot(t, stats.gumbel_r.sf(t, mu, beta), color=ORANGE, linewidth=2, linestyle=(0, (5, 3)), label="Gumbel 拟合（偏保守）fit, conservative")
    ax.axhline(1 / n, color=GRID, linewidth=1)
    ax.text(7.05, 1 / n * 0.5, f"1 / {n:,}：样本能测到的最小比例 · smallest measurable", color=INK2, fontsize=8, va="top")
    ax.set_yscale("log")
    ax.set_ylim(1e-7, 1.2)
    ax.set_xlabel("阈值 t · threshold")
    ax.set_ylabel("P(z_sync ≥ t)，未加水印 · unmarked")
    threshold_line(ax, Z_THRESH, "6.5", 0.5)
    threshold_line(ax, Z_PRESENT_ONLY, "8.0", 0.5)
    ax.set_title("尾部概率：拟合线始终在实测之上 · Tail: the fit stays above the data")
    ax.legend(frameon=False, loc="lower left", labelcolor=INK)
    grid(ax)
    save(fig, out, "calib_tail")


def fig_detection(pos, out):
    ok_orig = {r["image"] for r in pos if r["variant"] == "orig" and r["id_ok"] == "1"}
    groups = [
        ("全部 40 张 all 40 images", lambda r: True, BLUE, -0.12),
        (f"通过自检的 {len(ok_orig)} 张 passed self-check", lambda r: r["image"] in ok_orig, ORANGE, 0.12),
    ]
    fig, ax = plt.subplots(figsize=(8, 3.4))
    for label, keep, color, dy in groups:
        for i, (v, _) in enumerate(VARIANTS):
            if v == "orig" and color == ORANGE:
                ax.text(99.5, i + dy, "按定义 100% · by definition", color=INK2, fontsize=8.5, va="center", ha="right")
                continue
            q = [r for r in pos if r["variant"] == v and keep(r)]
            k = sum(r["id_ok"] == "1" for r in q)
            lo, hi = stats.binomtest(k, len(q)).proportion_ci(0.95, method="wilson")
            y = i + dy
            ax.plot([lo * 100, hi * 100], [y, y], color=color, linewidth=2, solid_capstyle="round")
            ax.plot([k / len(q) * 100], [y], "o", color=color, markersize=8, markeredgecolor=SURFACE, markeredgewidth=2,
                    label=label if i == 1 else None)
            ax.text(hi * 100 + 0.8 if hi < 0.995 else lo * 100 - 0.8, y, f"{k}/{len(q)}", color=INK2, fontsize=8.5,
                    va="center", ha="left" if hi < 0.995 else "right")
    ax.set_yticks(range(len(VARIANTS)), [lab for _, lab in VARIANTS])
    ax.invert_yaxis()
    ax.set_xlim(60, 101)
    ax.set_xlabel("识别率（点）和 95% 置信区间（线） · detection rate and 95% CI (%)")
    ax.set_title("识别率 · Detection rate")
    ax.legend(frameon=False, loc="upper center", bbox_to_anchor=(0.5, -0.2), ncol=2, labelcolor=INK)
    grid(ax, "x")
    save(fig, out, "calib_detection")


def main():
    neg = [r for r in load(sys.argv[1]) if r["kind"] == "neg"]
    pos = [r for r in load(sys.argv[2]) if r["kind"] == "pos"]
    out = Path(sys.argv[3])
    out.mkdir(parents=True, exist_ok=True)
    neg_z = np.array([num(r["z_sync"]) for r in neg])
    neg_z = neg_z[np.isfinite(neg_z)]
    pos_z = np.array([num(r["z_sync"]) for r in pos if r["id_ok"] == "1"])
    fig_scores(neg_z, pos_z, out)
    fig_tail(neg_z, out)
    fig_detection(pos, out)


if __name__ == "__main__":
    main()
