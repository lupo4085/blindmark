"""Affine (rotation / flip / aspect / shear) synchronisation for blindmark — prototype.

Idea: the tiled watermark makes the whitened autocorrelation (ACF) of the residual
show a 2-D lattice of peaks. Any affine distortion maps that lattice to another
lattice, so its two basis vectors give the linear part of the distortion.
The square tile has 8 symmetries (D4); they are resolved after folding by
picking the orientation with the highest sync correlation.
"""
from __future__ import annotations

import numpy as np
import cv2
from scipy.ndimage import gaussian_filter, map_coordinates, maximum_filter

import blindmark as bm

WT = bm.WORK_TILE


def plain_acf(y: np.ndarray, max_work: int = 768):
    """Overlap-normalised (unwhitened) ACF, shifted so lag (0,0) is at the centre."""
    h, w = y.shape
    f0 = min(1.0, max_work / min(h, w))
    y0 = cv2.resize(y.astype(np.float32), (max(1, round(w * f0)), max(1, round(h * f0))),
                    interpolation=cv2.INTER_AREA) if f0 < 1 else y.astype(np.float32)
    r = bm._highpass(y0, 1.5).astype(np.float64)
    r -= r.mean()
    H, W = r.shape
    Z = np.zeros((2 * H, 2 * W)); Z[:H, :W] = r
    A = np.real(np.fft.ifft2(np.abs(np.fft.fft2(Z)) ** 2))
    ky = np.minimum(np.arange(2 * H), 2 * H - np.arange(2 * H))
    kx = np.minimum(np.arange(2 * W), 2 * W - np.arange(2 * W))
    N = np.maximum(np.outer(np.maximum(H - ky, 0), np.maximum(W - kx, 0)), 1)
    A = A / N
    A /= A[0, 0]
    A = np.fft.fftshift(A)                       # (2H, 2W), centre = lag 0
    cy, cx = H, W
    hh, hw = H // 2, W // 2                      # keep lags up to half the image (enough overlap)
    return A[cy - hh:cy + hh, cx - hw:cx + hw], f0


def whitened_acf(y: np.ndarray, max_work: int = 768):
    """Return (shifted whitened ACF, f0). ACF[cy+dy, cx+dx] = correlation at lag (dx, dy)."""
    h, w = y.shape
    f0 = min(1.0, max_work / min(h, w))
    y0 = cv2.resize(y.astype(np.float32), (max(1, round(w * f0)), max(1, round(h * f0))),
                    interpolation=cv2.INTER_AREA) if f0 < 1 else y.astype(np.float32)
    r = bm._highpass(y0, 1.5).astype(np.float64)
    r -= r.mean()
    H, W = r.shape
    win = np.sqrt(np.outer(np.hanning(H), np.hanning(W)))
    P = np.abs(np.fft.fft2(r * win)) ** 2
    Ps = np.fft.ifftshift(gaussian_filter(np.fft.fftshift(P), 4))
    A = np.real(np.fft.ifft2(P / (Ps + 1e-9)))
    A /= A[0, 0]
    return np.fft.fftshift(A), f0


def _lagrange_reduce(b1, b2):
    b1, b2 = np.array(b1, float), np.array(b2, float)
    if b1 @ b1 > b2 @ b2:
        b1, b2 = b2, b1
    for _ in range(50):
        mu = round((b1 @ b2) / (b1 @ b1))
        b2 = b2 - mu * b1
        if b2 @ b2 >= b1 @ b1:
            break
        b1, b2 = b2, b1
    return b1, b2


def find_lattices(A: np.ndarray, pmin: float, top_peaks: int = 16, max_lat: int = 3):
    """Candidate lattice bases (2x2, columns = basis vectors as (dx, dy)) in work-res px."""
    H, W = A.shape
    cy, cx = H // 2, W // 2
    yy, xx = np.mgrid[0:H, 0:W]
    dist = np.hypot(yy - cy, xx - cx)
    lim = 0.5 * min(H, W) - 3
    valid = (dist >= pmin) & (dist <= lim)
    loc = (A == maximum_filter(A, size=5)) & valid
    vals = A[loc]
    if vals.size < 2:
        return []
    noise = np.std(A[valid])
    ys, xs = np.nonzero(loc)
    order = np.argsort(vals)[::-1]
    peaks = []
    for k in order:
        dy, dx = ys[k] - cy, xs[k] - cx
        if dy < 0 or (dy == 0 and dx < 0):        # keep one half-plane (ACF is symmetric)
            continue
        if vals[k] < 3 * noise:
            break
        peaks.append(np.array([dx, dy], float))
        if len(peaks) >= top_peaks:
            break

    def sample(v):
        x, y = cx + v[0], cy + v[1]
        if not (1 <= x < W - 2 and 1 <= y < H - 2):
            return None
        xi, yi = int(round(x)), int(round(y))
        return A[yi - 1:yi + 2, xi - 1:xi + 2].max()

    cands = []
    for i in range(len(peaks)):
        for j in range(i + 1, len(peaks)):
            b1, b2 = peaks[i], peaks[j]
            cr = abs(b1[0] * b2[1] - b1[1] * b2[0])
            if cr < 0.3 * np.linalg.norm(b1) * np.linalg.norm(b2):
                continue
            b1, b2 = _lagrange_reduce(b1, b2)
            if min(np.linalg.norm(b1), np.linalg.norm(b2)) < pmin:
                continue
            pts = [sample(m * b1 + n * b2) for m in range(-max_lat, max_lat + 1)
                   for n in range(-max_lat, max_lat + 1) if (m, n) != (0, 0)]
            pts = [p for p in pts if p is not None]
            if len(pts) < 4:
                continue
            area = abs(b1[0] * b2[1] - b1[1] * b2[0])
            cands.append((float(np.mean(pts)) / noise, area, np.column_stack([b1, b2])))
    if not cands:
        return []
    best = max(c[0] for c in cands)
    good = [c for c in cands if c[0] >= 0.8 * best]
    good.sort(key=lambda c: c[1])
    out = []
    for s, area, B in good + sorted(cands, key=lambda c: -c[0]):
        if all(abs(area - a) / a > 0.05 or np.abs(B - Bo).max() > 2 for _, a, Bo in out):
            out.append((s, area, B))
        if len(out) >= 3:
            break
    return [(s, refine_basis(A, B, max_lat)) for s, _, B in out]


def refine_basis(A, B, max_lat=4):
    """Least-squares refit of the basis from sub-pixel peak positions of far lattice points."""
    H, W = A.shape
    cy, cx = H // 2, W // 2
    M, P = [], []
    for m in range(-max_lat, max_lat + 1):
        for n in range(-max_lat, max_lat + 1):
            if (m, n) == (0, 0):
                continue
            v = B @ np.array([m, n], float)
            x, y = cx + v[0], cy + v[1]
            xi, yi = int(round(x)), int(round(y))
            if not (3 <= xi < W - 3 and 3 <= yi < H - 3):
                continue
            win = A[yi - 2:yi + 3, xi - 2:xi + 3]
            oy, ox = np.unravel_index(np.argmax(win), win.shape)
            py, px = yi - 2 + oy, xi - 2 + ox
            if not (1 <= py < H - 1 and 1 <= px < W - 1):
                continue
            def sub(a, b, c):
                d = a - 2 * b + c
                return 0.0 if d == 0 else float(np.clip((a - c) / (2 * d), -0.5, 0.5))
            sx = sub(A[py, px - 1], A[py, px], A[py, px + 1])
            sy = sub(A[py - 1, px], A[py, px], A[py + 1, px])
            M.append([m, n])
            P.append([px + sx - cx, py + sy - cy])
    if len(M) < 4:
        return B
    sol, *_ = np.linalg.lstsq(np.array(M, float), np.array(P), rcond=None)
    return sol.T                                      # columns = basis vectors


D4 = [lambda t: t, lambda t: np.rot90(t, 1), lambda t: np.rot90(t, 2), lambda t: np.rot90(t, 3),
      lambda t: t.T, lambda t: np.rot90(t.T, 1), lambda t: np.rot90(t.T, 2), lambda t: np.rot90(t.T, 3)]


def rectified_fold(y: np.ndarray, B: np.ndarray):
    """Warp the image so the lattice basis B (native px) becomes WT x WT axis-aligned tiles; fold."""
    h, w = y.shape
    blen = np.sqrt(abs(np.linalg.det(B)))
    r = min(1.0, WT / blen * 1.0)                     # anti-alias by pre-shrinking (never enlarge here)
    ys = cv2.resize(y.astype(np.float32), (max(8, round(w * r)), max(8, round(h * r))),
                    interpolation=cv2.INTER_AREA) if r < 1 else y.astype(np.float32)
    rx, ry = ys.shape[1] / w, ys.shape[0] / h
    Bs = np.diag([rx, ry]) @ B                        # basis in the shrunk image
    M = Bs / WT                                       # rectified px -> shrunk image px
    Minv = np.linalg.inv(M)
    hs, ws = ys.shape
    corners = np.array([[0, 0], [ws, 0], [0, hs], [ws, hs]], float).T
    uc = Minv @ corners
    u0, u1 = np.floor(uc.min(1)), np.ceil(uc.max(1))
    cw, ch = int(u1[0] - u0[0]), int(u1[1] - u0[1])
    if cw * ch > 6000 * 6000 or cw < 16 or ch < 16:
        return None
    uu, vv = np.meshgrid(np.arange(cw) + u0[0], np.arange(ch) + u0[1])
    X = M[0, 0] * uu + M[0, 1] * vv
    Y = M[1, 0] * uu + M[1, 1] * vv
    valid = (X >= 0) & (X <= ws - 1) & (Y >= 0) & (Y <= hs - 1)
    can = map_coordinates(ys, [Y, X], order=1, mode="nearest").astype(np.float32)
    mean = can[valid].mean() if valid.any() else 0
    can[~valid] = mean
    res = bm._highpass(can, 1.5)
    wgt = cv2.erode(valid.astype(np.uint8), np.ones((7, 7), np.uint8)).astype(np.float64)
    # bins are aligned with u = 0 (any constant offset is resolved by the sync correlation)
    iy = (np.arange(ch)[:, None] + int(u0[1])) % WT
    ix = (np.arange(cw)[None, :] + int(u0[0])) % WT
    idx = (iy * WT + ix).ravel()
    acc = np.bincount(idx, weights=(res * wgt).ravel(), minlength=WT * WT)
    cnt = np.bincount(idx, weights=wgt.ravel(), minlength=WT * WT)
    F = (acc / np.maximum(cnt, 1e-9)).reshape(WT, WT)
    return (F - F.mean()) / (F.std() + 1e-9)


def polar_lattices(A: np.ndarray, pmin: float, top: int = 6, max_lat: int = 3):
    """Matched filter over rotated square lattices: score(theta, p) = mean ACF at lattice points.
    More sensitive than peak picking because it sums evidence from many lattice points."""
    H, W = A.shape
    cy, cx = H // 2, W // 2
    Am = maximum_filter(A, size=3)
    yy, xx = np.mgrid[0:H, 0:W]
    ring = np.hypot(yy - cy, xx - cx) > pmin
    noise = np.std(A[ring])
    lim = 0.5 * min(H, W) - 3
    mn = np.array([(m, n) for m in range(0, max_lat + 1) for n in range(-max_lat, max_lat + 1)
                   if (m > 0 or n > 0)], float)                     # half-plane
    th = np.deg2rad(np.arange(0, 90, 0.5))
    ps = np.arange(pmin, lim, 1.0)
    c, s_ = np.cos(th), np.sin(th)
    # lattice point offsets: R(theta) @ (m, n) * p
    ux = c[:, None] * mn[None, :, 0] - s_[:, None] * mn[None, :, 1]   # (T, K)
    uy = s_[:, None] * mn[None, :, 0] + c[:, None] * mn[None, :, 1]
    X = ps[None, :, None] * ux[:, None, :]                              # (T, P, K)
    Y = ps[None, :, None] * uy[:, None, :]
    xi = np.rint(cx + X).astype(int)
    yi = np.rint(cy + Y).astype(int)
    ok = (np.hypot(X, Y) <= lim) & (xi >= 0) & (xi < W) & (yi >= 0) & (yi < H)
    vals = np.where(ok, Am[np.clip(yi, 0, H - 1), np.clip(xi, 0, W - 1)], 0.0)
    nv = ok.sum(-1)
    score = np.where(nv >= 4, vals.sum(-1) / np.maximum(nv, 1), -np.inf) / noise   # (T, P)
    out = []
    flat = np.argsort(score, axis=None)[::-1]
    for f in flat[:4000]:
        ti, pi = np.unravel_index(f, score.shape)
        if not np.isfinite(score[ti, pi]):
            break
        t, p = th[ti], ps[pi]
        if any(abs(p - q) / q < 0.04 and min(abs(t - u), np.pi / 2 - abs(t - u)) < np.deg2rad(3) for _, q, u in out):
            continue
        out.append((float(score[ti, pi]), p, t))
        if len(out) >= top:
            break
    res = []
    for sc, p, t in out:
        B = p * np.array([[np.cos(t), -np.sin(t)], [np.sin(t), np.cos(t)]])
        res.append((sc, refine_basis(A, B, max_lat)))
    return res


def _same_lattice(B1, B2, tol=0.012):
    """True if B1 and B2 generate (almost) the same lattice: B2 = B1 @ U, U integer unimodular."""
    try:
        U = np.linalg.solve(B1, B2)
    except np.linalg.LinAlgError:
        return False
    Ur = np.rint(U)
    return np.abs(U - Ur).max() < tol and abs(abs(np.linalg.det(Ur)) - 1) < 1e-6


def _snap_similarity(B):
    U, S, Vt = np.linalg.svd(B)
    return U @ np.diag([np.sqrt(S[0] * S[1])] * 2) @ Vt


def lattice_candidates(y: np.ndarray, cap: int = 10):
    """Ranked candidate lattice bases (native px) from several detectors.
    Order: axis periods (cheap, exact for unrotated images), polar matched filters
    (whitened, then plain ACF), peak pairs; each followed by its similarity-snapped version."""
    A, f0 = whitened_acf(y)
    pmin = max(10, bm.MIN_PERIOD * f0)
    Ap, fp = plain_acf(y)
    groups = [
        [np.diag([p, p]) for p in bm.estimate_periods(y)[:3]],
        [B / f0 for _, B in polar_lattices(A, pmin, top=4)],
        [B / fp for _, B in polar_lattices(Ap, max(10, bm.MIN_PERIOD * fp), top=3)],
        [B / f0 for _, B in find_lattices(A, pmin)],
    ]
    # interleave the groups so each detector gets a few slots before the cap
    order = []
    for i in range(max(len(g) for g in groups)):
        for g in groups:
            if i < len(g):
                order.append(g[i])
    out = []
    for B in order:
        for C in (_snap_similarity(B), B):
            if abs(np.linalg.det(C)) < (bm.MIN_PERIOD ** 2) * 0.8:
                continue
            if not any(_same_lattice(C, O) for O in out):
                out.append(C)
        if len(out) >= cap:
            break
    return out


def detect_affine(rgb, key, known_ids=None, z_thresh=6.5, return_debug=False):
    y = bm.rgb_to_ycbcr(rgb)[..., 0]
    pat = bm.make_pattern(key)
    fft_sync = bm._sync_fft(pat)

    def best_orientation(B):
        F = rectified_fold(y, B)
        if F is None:
            return (-1e9, None, B, 0)
        b = (-1e9, None, B, 0)
        for k, T in enumerate(D4):
            z, soft = bm._analyse_fold(np.ascontiguousarray(T(F)), pat, fft_sync)
            if z > b[0]:
                b = (z, soft, B, k)
        return b

    def coarse_eval(B):
        return max((best_orientation(B * s) for s in (0.993, 1.0, 1.007)), key=lambda r: r[0])

    coarse = sorted((coarse_eval(B) for B in lattice_candidates(y)), key=lambda r: -r[0])
    best = coarse[0] if coarse else (-1e9, None, None, 0)
    for z0, _, B0, _ in coarse[:2]:                      # fine scale / angle refinement
        for da in (-0.3, 0.0, 0.3):
            t = np.deg2rad(da)
            R = np.array([[np.cos(t), -np.sin(t)], [np.sin(t), np.cos(t)]])
            for s in np.linspace(0.985, 1.015, 13):
                r = best_orientation(R @ B0 * s)
                if r[0] > best[0]:
                    best = r
    z, soft, B, k = best
    if soft is None:
        return bm.Detection(False, False, None, None, float("nan"), None, None)
    det = bm._finish(z, soft, key, known_ids, z_thresh, float(np.sqrt(abs(np.linalg.det(B)))))
    return (det, B, k) if return_debug else det
