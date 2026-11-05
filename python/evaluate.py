"""Benchmark blindmark: image quality + detection under screenshot-like attacks.

Usage: python evaluate.py testimgs/ results/ [--reps 4]
"""
import argparse, csv, json, sys, time
from pathlib import Path

import numpy as np
import cv2
from skimage.metrics import structural_similarity

sys.path.insert(0, str(Path(__file__).parent))
import blindmark as bm
import attacks as A

RANDOM_ATTACKS = {"crop_50%", "crop_25%", "ui_frame", "screenshot_mild", "screenshot_typical", "screenshot_hard"}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("imgdir"); ap.add_argument("outdir")
    ap.add_argument("--reps", type=int, default=4)
    ap.add_argument("--strength", type=float, default=2.6)
    ap.add_argument("--n_issued", type=int, default=10000)
    a = ap.parse_args()
    out = Path(a.outdir); out.mkdir(parents=True, exist_ok=True)
    key = bytes.fromhex("5a" * 32)
    rng = np.random.default_rng(7)
    attacks = A.suite(rng)
    rows, quality = [], []
    # simulate an owner who has issued many IDs; the test images use some of them
    issued = [int(v) for v in rng.integers(0, 2**32, a.n_issued)]
    for p in sorted(Path(a.imgdir).glob("*.png")):
        im = bm.load_rgb(p)
        iid = int(issued[len(quality)])
        wm = bm.embed(im, key, iid, a.strength)
        psnr = cv2.PSNR(im, wm)
        ssim = structural_similarity(im, wm, channel_axis=2, data_range=255)
        quality.append({"image": p.stem, "h": im.shape[0], "w": im.shape[1], "psnr": psnr, "ssim": ssim})
        print(f"{p.stem}: {im.shape[1]}x{im.shape[0]} PSNR {psnr:.1f} SSIM {ssim:.4f}", flush=True)
        for name, fn in attacks.items():
            reps = a.reps if name in RANDOM_ATTACKS else 1
            for r in range(reps):
                state = rng.bit_generator.state
                x = fn(wm)
                rng.bit_generator.state = state      # same geometry for the clean control
                x0 = fn(im)
                t = time.time(); d = bm.detect(x, key, known_ids=issued); dt = time.time() - t
                d0 = bm.detect(x0, key, known_ids=issued)
                rows.append({"image": p.stem, "attack": name, "rep": r,
                             "correct": int(d.found and d.image_id == iid),
                             "wrong_id": int(d.found and d.image_id != iid),
                             "method": d.method or "",
                             "z": round(d.z_sync, 2), "fp_on_clean": int(d0.found),
                             "z_clean": round(d0.z_sync, 2), "sec": round(dt, 2)})
    with open(out / "detections.csv", "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=list(rows[0])); w.writeheader(); w.writerows(rows)
    summ = {}
    for name in attacks:
        rs = [r for r in rows if r["attack"] == name]
        summ[name] = {"n": len(rs),
                      "detect_rate": float(np.mean([r["correct"] for r in rs])),
                      "wrong_id": int(sum(r["wrong_id"] for r in rs)),
                      "via_crc": int(sum(r["method"] == "crc" and r["correct"] for r in rs)),
                      "via_registry": int(sum(r["method"] == "registry" and r["correct"] for r in rs)),
                      "false_pos_clean": int(sum(r["fp_on_clean"] for r in rs)),
                      "median_z": float(np.median([r["z"] for r in rs]))}
    res = {"quality": quality, "attacks": summ,
           "max_z_on_clean": float(max(r["z_clean"] for r in rows))}
    (out / "summary.json").write_text(json.dumps(res, indent=2))
    print(json.dumps(summ, indent=1))
    print("max clean z:", res["max_z_on_clean"])


if __name__ == "__main__":
    main()
