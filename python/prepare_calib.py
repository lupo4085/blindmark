"""Assemble the image set for false-positive calibration (task V3-6).

Collects images from public datasets you have downloaded, plus your own folders, into

    OUT/<category>/<source>_<name>

and writes OUT/manifest.csv (category, source, original path, sha256) so the exact set can
be reproduced and cited in the calibration report. Images are copied, never modified.
Sampling is deterministic (fixed seed). Nothing here should ever be committed to git.

Examples (Windows):
    python python\\prepare_calib.py --out C:\\calib-images ^
        --div2k  D:\\data\\DIV2K_valid_HR --div2k-n 100 ^
        --coco   D:\\data\\val2017        --coco-n 100 ^
        --rico-n 100 ^
        --own illustration=D:\\my-art --own text=D:\\doc-screens

--rico-n streams screenshots from the Hugging Face dataset
"rootsautomation/RICO-Screen2Words" (CC-BY-4.0) and needs `pip install datasets pillow`.
Every other option only reads local folders.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import random
import shutil
import sys
from pathlib import Path

EXTS = {".png", ".jpg", ".jpeg", ".webp", ".bmp", ".tif", ".tiff"}
SEED = 0xCA11B


def images_in(folder: Path) -> list[Path]:
    return sorted(p for p in folder.rglob("*") if p.suffix.lower() in EXTS and p.is_file())


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def sample(paths: list[Path], n: int, tag: str) -> list[Path]:
    if n <= 0 or n >= len(paths):
        return paths
    rng = random.Random(f"{SEED}-{tag}")
    return sorted(rng.sample(paths, n))


class Writer:
    def __init__(self, out: Path):
        self.out = out
        self.rows: list[list[str]] = []
        self.seen: set[str] = set()

    def add(self, src: Path, category: str, source: str) -> None:
        digest = sha256(src)
        if digest in self.seen:
            return  # same file from two places
        self.seen.add(digest)
        dst_dir = self.out / category
        dst_dir.mkdir(parents=True, exist_ok=True)
        dst = dst_dir / f"{source}_{src.name}"
        if not dst.exists():
            shutil.copy2(src, dst)
        self.rows.append([category, source, str(src), dst.name, digest])

    def save(self) -> None:
        with open(self.out / "manifest.csv", "w", newline="", encoding="utf-8") as f:
            w = csv.writer(f)
            w.writerow(["category", "source", "original", "file", "sha256"])
            w.writerows(sorted(self.rows))


def add_folder(wr: Writer, folder: Path, n: int, category: str, source: str) -> int:
    if not folder.is_dir():
        sys.exit(f"not a folder: {folder}")
    picked = sample(images_in(folder), n, source)
    for p in picked:
        wr.add(p, category, source)
    return len(picked)


def add_rico(wr: Writer, n: int, tmp: Path) -> int:
    try:
        from datasets import load_dataset
    except ImportError:
        sys.exit("--rico-n needs: pip install datasets pillow")
    ds = load_dataset("rootsautomation/RICO-Screen2Words", split="test", streaming=True)
    ds = ds.shuffle(seed=SEED, buffer_size=1000)
    tmp.mkdir(parents=True, exist_ok=True)
    count = 0
    for row in ds:
        if count >= n:
            break
        p = tmp / f"rico_{row['screenId']}.png"
        row["image"].convert("RGB").save(p)
        wr.add(p, "screenshot", "rico")
        count += 1
    return count


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--div2k", type=Path, help="DIV2K_valid_HR folder (photos)")
    ap.add_argument("--div2k-n", type=int, default=100)
    ap.add_argument("--coco", type=Path, help="COCO val2017 folder (photos)")
    ap.add_argument("--coco-n", type=int, default=100)
    ap.add_argument("--rico-n", type=int, default=0, help="RICO screenshots to stream from Hugging Face")
    ap.add_argument("--own", action="append", default=[], metavar="CATEGORY=FOLDER",
                    help="your own images; repeatable, e.g. illustration=D:\\art")
    ap.add_argument("--own-n", type=int, default=0, help="cap per --own folder (0 = all)")
    a = ap.parse_args()

    a.out.mkdir(parents=True, exist_ok=True)
    wr = Writer(a.out)
    if a.div2k:
        print(f"div2k: {add_folder(wr, a.div2k, a.div2k_n, 'photo', 'div2k')}")
    if a.coco:
        print(f"coco: {add_folder(wr, a.coco, a.coco_n, 'photo', 'coco')}")
    if a.rico_n:
        print(f"rico: {add_rico(wr, a.rico_n, a.out / '_rico_tmp')}")
    for spec in a.own:
        if "=" not in spec:
            sys.exit(f"--own expects CATEGORY=FOLDER, got {spec}")
        cat, folder = spec.split("=", 1)
        print(f"own {cat}: {add_folder(wr, Path(folder), a.own_n, cat.strip(), 'own')}")
    shutil.rmtree(a.out / "_rico_tmp", ignore_errors=True)
    wr.save()

    counts: dict[str, int] = {}
    for row in wr.rows:
        counts[row[0]] = counts.get(row[0], 0) + 1
    print("\nper category:", ", ".join(f"{k} {v}" for k, v in sorted(counts.items())))
    print(f"total {len(wr.rows)}  ->  {a.out / 'manifest.csv'}")
    small = [k for k, v in counts.items() if v < 50]
    if small:
        print("note: fewer than 50 images in:", ", ".join(small), "(conclusions for these will be weak)")


if __name__ == "__main__":
    main()
