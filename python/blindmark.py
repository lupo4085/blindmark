#!/usr/bin/env python3
"""
blindmark v0 — keyed, content-adaptive, screenshot-tolerant blind image watermark.

Design (classical, no training needed):
  * Payload: 32-bit image ID + CRC-16, encrypted with a keyed 48-bit Feistel
    cipher so that the embedded bits look random from image to image.
  * Pattern: a G x G grid of +/-1 chips derived from a secret key. Half the
    chips are a fixed SYNC pattern (used for detection and alignment); the
    other half carry the 48 payload bits (each bit spread over ~42 chips).
  * Tiling: the grid is bilinearly upsampled to a tile of T pixels
    (T ~ short_side/4) and repeated over the whole image, so any reasonably
    large crop contains complete tiles (self-referencing watermark).
  * Content adaptivity (JND): embedding strength follows local texture
    (local std of luminance) — stronger in busy regions, weaker in flat ones.
    Only the luminance channel (Y) is modified.
  * Blind detection:
      1. high-pass residual -> 2-D autocorrelation -> tile period (= scale)
      2. resample so one tile = 128 px, fold all tiles into one 128x128 tile
      3. circular cross-correlation with the SYNC template -> translation
      4. matched-filter the payload chips -> decrypt -> CRC check
      5. if CRC fails but the sync is clearly present: maximum-likelihood
         decoding over the IDs you issued (registry) — uses all data chips
         jointly, far more robust than bit-by-bit decoding
  * Ownership evidence: a local registry stores, per ID, the SHA-256 of the
    original, a thumbnail and a timestamp. After decoding an ID the suspect
    image is matched against that thumbnail (ORB + RANSAC) to confirm it is
    the same picture (prevents "copy the watermark onto another image").

CLI:
  python blindmark.py keygen  --key my.key
  python blindmark.py embed   in.png out.png --key my.key --registry reg/
  python blindmark.py detect  suspect.png   --key my.key --registry reg/
"""
from __future__ import annotations

import argparse
import datetime as _dt
import hashlib
import hmac
import json
import os
import secrets
from dataclasses import dataclass, asdict
from pathlib import Path

import cv2
import numpy as np
from PIL import Image
from scipy.ndimage import median_filter, gaussian_filter
from scipy.stats import norm

G = 64                 # chip grid size (G x G chips per tile)
NBITS = 48             # 32-bit ID + 16-bit CRC
WORK_TILE = 128        # tile size (px) used inside the detector (2 px per chip)
SYNC_FRACTION = 0.5
MIN_PERIOD = 36        # smallest tile period (native px) the detector searches
MAX_TILES = 10         # per axis, used when the detector has to upsample


# ----------------------------------------------------------------------------
# key-derived material
# ----------------------------------------------------------------------------
FORMAT = "blindmark/v1"


def _drbg(key: bytes, label: str):
    """Deterministic byte stream: HMAC-SHA256(key, label || ctr_be64), ctr = 0, 1, ..."""
    ctr = 0
    while True:
        yield from hmac.new(key, label.encode() + ctr.to_bytes(8, "big"), hashlib.sha256).digest()
        ctr += 1


def _kdf(key: bytes, label: str) -> bytes:
    return hmac.new(key, label.encode(), hashlib.sha256).digest()


@dataclass
class Pattern:
    chips: np.ndarray      # (G*G,) +/-1
    is_sync: np.ndarray    # (G*G,) bool
    bit_of: np.ndarray     # (G*G,) int, -1 for sync chips


def make_pattern(key: bytes) -> Pattern:
    """Byte-exact spec shared with the Rust implementation (see FORMAT)."""
    st = _drbg(key, FORMAT + "/pattern")
    n = G * G
    raw = bytes(next(st) for _ in range(n // 8))
    chips = np.array([1.0 if (raw[k >> 3] >> (7 - (k & 7))) & 1 else -1.0 for k in range(n)])
    perm = list(range(n))
    for i in range(n - 1, 0, -1):          # Fisher-Yates with rejection sampling
        m = i + 1
        limit = (1 << 32) - ((1 << 32) % m)
        while True:
            x = int.from_bytes(bytes(next(st) for _ in range(4)), "big")
            if x < limit:
                break
        j = x % m
        perm[i], perm[j] = perm[j], perm[i]
    perm = np.array(perm)
    nsync = int(n * SYNC_FRACTION)
    is_sync = np.zeros(n, bool)
    is_sync[perm[:nsync]] = True
    bit_of = -np.ones(n, int)
    data_idx = perm[nsync:]
    bit_of[data_idx] = np.arange(len(data_idx)) % NBITS
    return Pattern(chips, is_sync, bit_of)


def tile_values(p: Pattern, bits: np.ndarray | None) -> np.ndarray:
    """Chip values for one tile. bits=None -> sync chips only."""
    v = np.zeros(G * G)
    v[p.is_sync] = p.chips[p.is_sync]
    if bits is not None:
        d = ~p.is_sync
        v[d] = p.chips[d] * (2.0 * bits[p.bit_of[d]] - 1.0)
    return v.reshape(G, G)


# ----------------------------------------------------------------------------
# payload: ID + CRC16, Feistel-encrypted
# ----------------------------------------------------------------------------
def crc16(data: bytes) -> int:
    crc = 0xFFFF
    for b in data:
        crc ^= b << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) if crc & 0x8000 else (crc << 1)
            crc &= 0xFFFF
    return crc


def _feistel(key: bytes, x: int, decrypt: bool) -> int:
    """4-round Feistel on 48 bits; round fn = first 3 bytes of HMAC(fk, [i] || half)."""
    fk = _kdf(key, FORMAT + "/feistel")
    rf = lambda i, h: int.from_bytes(hmac.new(fk, bytes([i]) + h.to_bytes(3, "big"), hashlib.sha256).digest()[:3], "big")
    L, R = (x >> 24) & 0xFFFFFF, x & 0xFFFFFF
    if not decrypt:
        for i in range(4):
            L, R = R, L ^ rf(i, R)
    else:
        for i in reversed(range(4)):
            L, R = R ^ rf(i, L), L
    return (L << 24) | R


def encode_payload(key: bytes, image_id: int) -> np.ndarray:
    idb = image_id.to_bytes(4, "big")
    plain = (image_id << 16) | crc16(idb)
    c = _feistel(key, plain, decrypt=False)
    return np.array([(c >> (NBITS - 1 - i)) & 1 for i in range(NBITS)], dtype=float)


def decode_payload(key: bytes, bits: np.ndarray) -> tuple[int, bool]:
    c = 0
    for b in bits:
        c = (c << 1) | int(b)
    plain = _feistel(key, c, decrypt=True)
    image_id, crc = plain >> 16, plain & 0xFFFF
    return image_id, crc == crc16(image_id.to_bytes(4, "big"))


# ----------------------------------------------------------------------------
# image helpers
# ----------------------------------------------------------------------------
def rgb_to_ycbcr(rgb: np.ndarray) -> np.ndarray:
    rgb = rgb.astype(np.float64)
    m = np.array([[0.299, 0.587, 0.114],
                  [-0.168736, -0.331264, 0.5],
                  [0.5, -0.418688, -0.081312]])
    ycc = rgb @ m.T
    ycc[..., 1:] += 128.0
    return ycc


def ycbcr_to_rgb(ycc: np.ndarray) -> np.ndarray:
    y, cb, cr = ycc[..., 0], ycc[..., 1] - 128.0, ycc[..., 2] - 128.0
    r = y + 1.402 * cr
    g = y - 0.344136 * cb - 0.714136 * cr
    b = y + 1.772 * cb
    return np.stack([r, g, b], -1)


def load_rgb(path) -> np.ndarray:
    return np.asarray(Image.open(path).convert("RGB"))


def periodic_upsample(tile: np.ndarray, H: int, W: int, T: float,
                      off: tuple[float, float] = (0.0, 0.0)) -> np.ndarray:
    """Bilinear, wrap-around upsampling of a G x G tile to period T pixels."""
    g = tile.shape[0]

    def coords(n, o):
        u = (np.arange(n) + 0.5 + o) * g / T - 0.5
        i0 = np.floor(u).astype(int)
        f = u - i0
        return i0 % g, (i0 + 1) % g, f

    y0, y1, fy = coords(H, off[0])
    x0, x1, fx = coords(W, off[1])
    t00 = tile[np.ix_(y0, x0)]; t01 = tile[np.ix_(y0, x1)]
    t10 = tile[np.ix_(y1, x0)]; t11 = tile[np.ix_(y1, x1)]
    fy = fy[:, None]; fx = fx[None, :]
    return (1 - fy) * ((1 - fx) * t00 + fx * t01) + fy * ((1 - fx) * t10 + fx * t11)


def local_std(y: np.ndarray, k: int = 7) -> np.ndarray:
    y = y.astype(np.float32)
    m = cv2.blur(y, (k, k))
    m2 = cv2.blur(y * y, (k, k))
    return np.sqrt(np.maximum(m2 - m * m, 0))


def jnd_strength(y: np.ndarray, base: float) -> np.ndarray:
    """Content-adaptive strength map (texture + luminance masking)."""
    tex = np.clip(0.5 + local_std(y, 7) / 12.0, 0.5, 2.5)
    lum = np.clip(np.minimum((y + 10) / 50.0, (265 - y) / 50.0), 0.3, 1.0)
    s = base * tex * lum
    return cv2.GaussianBlur(s, (0, 0), 2.0)


def tile_size_for(h: int, w: int) -> int:
    return int(max(2 * G, round(min(h, w) / 4)))


# ----------------------------------------------------------------------------
# embedding
# ----------------------------------------------------------------------------
def embed(rgb: np.ndarray, key: bytes, image_id: int, strength: float = 2.6,
          tile_px: int | None = None) -> np.ndarray:
    h, w = rgb.shape[:2]
    T = tile_px or tile_size_for(h, w)
    pat = make_pattern(key)
    bits = encode_payload(key, image_id)
    wm = periodic_upsample(tile_values(pat, bits), h, w, T)
    ycc = rgb_to_ycbcr(rgb)
    ycc[..., 0] += jnd_strength(ycc[..., 0], strength) * wm
    out = ycbcr_to_rgb(ycc)
    return np.clip(np.round(out), 0, 255).astype(np.uint8)


# ----------------------------------------------------------------------------
# detection
# ----------------------------------------------------------------------------
def _highpass(y: np.ndarray, sigma: float) -> np.ndarray:
    r = y - cv2.GaussianBlur(y, (0, 0), sigma)
    return r / (local_std(r, 15) + 1.0)


def _fft_gauss_hp(t: np.ndarray, sigma: float) -> np.ndarray:
    """Circular (wrap-around) version of _highpass's first step, for templates."""
    n = t.shape[0]
    f = np.fft.fftfreq(n)
    g = np.exp(-2 * (np.pi * sigma) ** 2 * (f[:, None] ** 2 + f[None, :] ** 2))
    return t - np.real(np.fft.ifft2(np.fft.fft2(t) * g))


def estimate_periods(y: np.ndarray, max_work: int = 768, top: int = 8) -> list[float]:
    """Candidate tile periods (native px) from a spectrally whitened autocorrelation
    of the high-pass residual. Whitening flattens the image's own 1/f spectrum so the
    narrow peaks produced by the tiled watermark stand out."""
    h, w = y.shape
    f0 = min(1.0, max_work / min(h, w))
    y0 = cv2.resize(y, (max(1, round(w * f0)), max(1, round(h * f0))), interpolation=cv2.INTER_AREA) if f0 < 1 else y
    r = _highpass(y0.astype(np.float32), 1.5).astype(np.float64)
    r -= r.mean()
    H, W = r.shape
    pmax = int(min(H, W) * 0.75)
    pmin = max(12, int(MIN_PERIOD * f0))
    if pmax <= pmin + 8:
        return []
    ps = np.arange(pmin, pmax)

    def peaks(A, top_n, base_win):
        s = A[0, ps] + A[ps, 0] + 0.5 * (A[ps, ps] + A[ps, -ps])
        prom = s - median_filter(s, size=base_win, mode="nearest")
        found: list[float] = []
        for i in np.argsort(prom)[::-1]:
            if prom[i] <= 0 or len(found) >= top_n:
                break
            if 0 < i < len(s) - 1 and prom[i] >= prom[i - 1] and prom[i] >= prom[i + 1]:
                a, b, c = s[i - 1], s[i], s[i + 1]
                den = a - 2 * b + c
                d = (a - c) / (2 * den) if den != 0 else 0.0
                found.append((ps[i] + float(np.clip(d, -0.5, 0.5))) / f0)
        return found

    # (a) spectrally whitened ACF: flattens the image's 1/f spectrum
    win = np.sqrt(np.outer(np.hanning(H), np.hanning(W)))
    P = np.abs(np.fft.fft2(r * win)) ** 2
    Ps = np.fft.ifftshift(gaussian_filter(np.fft.fftshift(P), 4))
    Aw = np.real(np.fft.ifft2(P / (Ps + 1e-9)))
    Aw /= Aw[0, 0]
    # (b) plain ACF with overlap normalisation (better on dark / sparse images)
    Z = np.zeros((2 * H, 2 * W)); Z[:H, :W] = r
    Mk = np.zeros_like(Z); Mk[:H, :W] = 1
    Ap = np.real(np.fft.ifft2(np.abs(np.fft.fft2(Z)) ** 2))
    Ap /= np.maximum(np.real(np.fft.ifft2(np.abs(np.fft.fft2(Mk)) ** 2)), 1)
    Ap /= Ap[0, 0]
    cand: list[float] = []
    for p in peaks(Aw, top, 9) + peaks(Ap, 3, 15):
        if all(abs(p - q) / q > 0.03 for q in cand):
            cand.append(p)
    # harmonic guard: the true period may show up as 2x or 1/2 of a strong peak
    out = list(cand)
    lim = 0.75 * min(h, w)
    for p in cand[:3]:
        for q in (p / 2, p * 2):
            if MIN_PERIOD <= q <= lim and all(abs(q - o) / o > 0.03 for o in out):
                out.append(q)
    return out


@dataclass
class Detection:
    found: bool                 # watermark from this key present AND an ID was recovered
    present: bool               # sync pattern from this key detected (z_sync >= z_thresh)
    image_id: int | None
    method: str | None          # "crc" (blind decode) or "registry" (ML over issued IDs)
    z_sync: float               # sync correlation, ~N(0,1) without watermark
    z_id: float | None          # ID correlation (registry method), ~N(0,1) for wrong IDs
    period_px: float | None


def _residual_at(y: np.ndarray, period: float) -> np.ndarray:
    """Resample so one tile ~= WORK_TILE px and return the whitened residual."""
    r = WORK_TILE / period
    if r > 1:   # small screenshot: only a central window of MAX_TILES^2 tiles is needed
        h, w = y.shape
        lim = int(MAX_TILES * period)
        y0, x0 = max(0, (h - lim) // 2), max(0, (w - lim) // 2)
        y = y[y0:y0 + lim, x0:x0 + lim]
    h, w = y.shape
    nh, nw = max(8, round(h * r)), max(8, round(w * r))
    interp = cv2.INTER_AREA if r < 1 else cv2.INTER_CUBIC
    yr = cv2.resize(y.astype(np.float32), (nw, nh), interpolation=interp)
    return _highpass(yr, 1.5)


def _fold(res: np.ndarray, period_res: float) -> np.ndarray:
    """Fold a residual with (fractional) period `period_res` into WORK_TILE^2 bins."""
    nh, nw = res.shape
    iy = (np.mod(np.arange(nh), period_res) * (WORK_TILE / period_res)).astype(int) % WORK_TILE
    ix = (np.mod(np.arange(nw), period_res) * (WORK_TILE / period_res)).astype(int) % WORK_TILE
    idx = (iy[:, None] * WORK_TILE + ix[None, :]).ravel()
    n = WORK_TILE * WORK_TILE
    acc = np.bincount(idx, weights=res.ravel().astype(np.float64), minlength=n)
    cnt = np.bincount(idx, minlength=n)
    F = (acc / np.maximum(cnt, 1)).reshape(WORK_TILE, WORK_TILE)
    return (F - F.mean()) / (F.std() + 1e-9)


def _analyse_fold(F: np.ndarray, pat: Pattern, fft_sync: np.ndarray):
    """Align a folded tile to the sync template; return (z_sync, per-bit soft values)."""
    C = np.real(np.fft.ifft2(np.fft.fft2(F) * np.conj(fft_sync)))
    sy, sx = np.unravel_index(np.argmax(C), C.shape)
    Fa = np.roll(F, (-sy, -sx), axis=(0, 1))
    k = WORK_TILE // G
    chipv = Fa.reshape(G, k, G, k).mean(axis=(1, 3)).ravel()   # chip-level values
    chipv = (chipv - chipv.mean()) / (chipv.std() + 1e-9)
    s_idx = pat.is_sync
    z = float(np.dot(chipv[s_idx], pat.chips[s_idx]) / np.sqrt(s_idx.sum()))
    d_idx = ~s_idx
    soft = np.zeros(NBITS)
    np.add.at(soft, pat.bit_of[d_idx], chipv[d_idx] * pat.chips[d_idx])
    return z, soft / np.sqrt(d_idx.sum())


def _sync_fft(pat: Pattern) -> np.ndarray:
    t = periodic_upsample(tile_values(pat, None), WORK_TILE, WORK_TILE, WORK_TILE)
    t = _fft_gauss_hp(t, 1.5)
    return np.fft.fft2((t - t.mean()) / t.std())


def payload_matrix(key: bytes, ids) -> np.ndarray:
    """+/-1 expected bit signs for each issued ID (rows)."""
    return np.array([2 * encode_payload(key, int(i)) - 1 for i in ids]).reshape(-1, NBITS)


def detect(rgb: np.ndarray, key: bytes, known_ids=None, z_thresh: float = 6.5,
           extra_periods: list[float] | None = None) -> Detection:
    """Blind detection. `known_ids` (IDs you issued, from the registry) enables
    maximum-likelihood ID decoding when blind CRC decoding fails."""
    y = rgb_to_ycbcr(rgb)[..., 0]
    pat = make_pattern(key)
    fft_sync = _sync_fft(pat)

    # coarse pass over all period candidates, fine pass around the best two
    cands = estimate_periods(y) + list(extra_periods or [])
    scored = []
    residuals = {}
    for p0 in cands:
        res = _residual_at(y, p0)
        residuals[p0] = res
        zb = max(_analyse_fold(_fold(res, WORK_TILE * (1 + d)), pat, fft_sync)[0]
                 for d in (-0.01, 0.0, 0.01))
        scored.append((zb, p0))
    scored.sort(reverse=True)
    best = (-1e9, None, None)                      # (z, soft, period)
    for _, p0 in scored[:2]:
        res = residuals[p0]
        for d in np.linspace(-0.015, 0.015, 13):
            z, soft = _analyse_fold(_fold(res, WORK_TILE * (1 + d)), pat, fft_sync)
            if z > best[0]:
                best = (z, soft, p0 * (1 + d))
    z, soft, period = best
    if soft is None:
        return Detection(False, False, None, None, float("nan"), None, None)
    present = z >= z_thresh

    # 1) blind decode: hard bits -> decrypt -> CRC
    iid, ok = decode_payload(key, (soft > 0).astype(float))
    if present and ok:
        return Detection(True, True, iid, "crc", z, None, period)

    # 2) registry-assisted ML decode over issued IDs
    if present and known_ids is not None and len(known_ids):
        ids = list(known_ids)
        M = payload_matrix(key, ids)
        scores = M @ soft                              # ~N(0,1) for unrelated IDs
        j = int(np.argmax(scores))
        thr = max(5.0, float(norm.isf(1e-6 / len(ids))))
        hamming = int(np.sum((M[j] > 0) != (soft > 0)))
        if scores[j] >= thr and hamming <= NBITS // 3:
            return Detection(True, True, int(ids[j]), "registry", z, float(scores[j]), period)
        return Detection(False, True, None, None, z, float(scores[j]), period)
    return Detection(False, present, None, None, z, None, period)


# ----------------------------------------------------------------------------
# registry + content check
# ----------------------------------------------------------------------------
def _thumb(rgb: np.ndarray, side: int = 512) -> np.ndarray:
    h, w = rgb.shape[:2]
    s = side / max(h, w)
    if s < 1:
        rgb = cv2.resize(rgb, (round(w * s), round(h * s)), interpolation=cv2.INTER_AREA)
    return rgb


def register(reg_dir: Path, image_id: int, src_name: str, original: np.ndarray, marked: np.ndarray):
    reg_dir.mkdir(parents=True, exist_ok=True)
    db_path = reg_dir / "registry.json"
    db = json.loads(db_path.read_text()) if db_path.exists() else {}
    thumb_name = f"{image_id:08x}.png"
    Image.fromarray(_thumb(marked)).save(reg_dir / thumb_name)
    db[str(image_id)] = {
        "source": src_name,
        "sha256_original": hashlib.sha256(original.tobytes()).hexdigest(),
        "sha256_marked": hashlib.sha256(marked.tobytes()).hexdigest(),
        "size": list(original.shape[:2]),
        "thumb": thumb_name,
        "registered_utc": _dt.datetime.now(_dt.timezone.utc).isoformat(timespec="seconds"),
    }
    db_path.write_text(json.dumps(db, indent=2, ensure_ascii=False))


def content_match(reg_dir: Path, image_id: int, suspect: np.ndarray) -> int | None:
    """Number of RANSAC-consistent ORB matches between suspect and stored thumbnail."""
    db_path = reg_dir / "registry.json"
    if not db_path.exists():
        return None
    entry = json.loads(db_path.read_text()).get(str(image_id))
    if entry is None:
        return None
    ref = load_rgb(reg_dir / entry["thumb"])
    g1 = cv2.cvtColor(ref, cv2.COLOR_RGB2GRAY)
    g2 = cv2.cvtColor(_thumb(suspect, 1024), cv2.COLOR_RGB2GRAY)
    orb = cv2.ORB_create(3000)
    k1, d1 = orb.detectAndCompute(g1, None)
    k2, d2 = orb.detectAndCompute(g2, None)
    if d1 is None or d2 is None or len(k1) < 8 or len(k2) < 8:
        return 0
    m = cv2.BFMatcher(cv2.NORM_HAMMING).knnMatch(d1, d2, k=2)
    good = [a for a, *b in m if b and a.distance < 0.75 * b[0].distance]
    if len(good) < 8:
        return len(good)
    src = np.float32([k1[g.queryIdx].pt for g in good])
    dst = np.float32([k2[g.trainIdx].pt for g in good])
    _, inl = cv2.estimateAffinePartial2D(src, dst, method=cv2.RANSAC, ransacReprojThreshold=4.0)
    return int(inl.sum()) if inl is not None else 0


# ----------------------------------------------------------------------------
# CLI
# ----------------------------------------------------------------------------
def _read_key(path: str) -> bytes:
    return bytes.fromhex(Path(path).read_text().strip())


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    k = sub.add_parser("keygen"); k.add_argument("--key", required=True)
    e = sub.add_parser("embed")
    e.add_argument("input"); e.add_argument("output")
    e.add_argument("--key", required=True); e.add_argument("--registry", default="registry")
    e.add_argument("--id", type=lambda s: int(s, 0), default=None)
    e.add_argument("--strength", type=float, default=2.6)
    d = sub.add_parser("detect")
    d.add_argument("input"); d.add_argument("--key", required=True)
    d.add_argument("--registry", default="registry")
    a = ap.parse_args()

    if a.cmd == "keygen":
        if os.path.exists(a.key):
            raise SystemExit(f"{a.key} exists; refusing to overwrite")
        Path(a.key).write_text(secrets.token_hex(32) + "\n")
        os.chmod(a.key, 0o600)
        print(f"wrote secret key to {a.key} — back it up, never share it")
    elif a.cmd == "embed":
        key = _read_key(a.key)
        rgb = load_rgb(a.input)
        iid = a.id if a.id is not None else secrets.randbits(32)
        out = embed(rgb, key, iid, a.strength)
        Image.fromarray(out).save(a.output)
        register(Path(a.registry), iid, os.path.basename(a.input), rgb, out)
        psnr = cv2.PSNR(rgb, out)
        print(json.dumps({"id": f"0x{iid:08x}", "psnr_db": round(psnr, 2), "output": a.output}))
    else:
        key = _read_key(a.key)
        rgb = load_rgb(a.input)
        db_path = Path(a.registry) / "registry.json"
        known = [int(k) for k in json.loads(db_path.read_text())] if db_path.exists() else None
        det = detect(rgb, key, known_ids=known)
        res = asdict(det)
        if det.image_id is not None:
            res["id_hex"] = f"0x{det.image_id:08x}"
            res["content_inliers"] = content_match(Path(a.registry), det.image_id, rgb)
        print(json.dumps(res, indent=2))


if __name__ == "__main__":
    main()
