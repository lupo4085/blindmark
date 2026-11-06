//! Minimal ORB-style features (FAST-9 + Harris ranking + intensity-centroid orientation +
//! steered BRIEF-256 on an image pyramid), brute-force Hamming matching and a RANSAC
//! homography. Used to locate a registered picture inside a photo of a screen so the
//! perspective can be undone before reading the watermark.

use crate::imgproc::{gaussian_blur, resize, Plane};
use rayon::prelude::*;

pub type Desc = [u64; 4];

pub struct Features {
    pub pts: Vec<(f32, f32)>, // level-0 pixel coordinates
    pub desc: Vec<Desc>,
}

const PATCH_R: i32 = 15;
const MARGIN: i32 = 19;
const CIRCLE: [(i32, i32); 16] = [
    (0, -3), (1, -3), (2, -2), (3, -1), (3, 0), (3, 1), (2, 2), (1, 3),
    (0, 3), (-1, 3), (-2, 2), (-3, 1), (-3, 0), (-3, -1), (-2, -2), (-1, -3),
];

fn brief_pairs() -> Vec<(f32, f32, f32, f32)> {
    // deterministic Gaussian sampling (sigma = 31/5) clipped to the patch circle
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut uni = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        ((s >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    let mut gauss = || {
        let (u1, u2) = (uni(), uni());
        ((-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos() * 31.0 / 5.0) as f32
    };
    let r = PATCH_R as f32 - 0.5;
    let mut v = Vec::with_capacity(256);
    while v.len() < 256 {
        let (a, b, c, d) = (gauss(), gauss(), gauss(), gauss());
        if a * a + b * b <= r * r && c * c + d * d <= r * r {
            v.push((a, b, c, d));
        }
    }
    v
}

fn fast9(p: &Plane, t: f32) -> Vec<(i32, i32)> {
    let (w, h) = (p.w as i32, p.h as i32);
    if w <= 2 * MARGIN || h <= 2 * MARGIN {
        return vec![];
    }
    (MARGIN..h - MARGIN)
        .into_par_iter()
        .flat_map_iter(|y| {
            let mut out = vec![];
            for x in MARGIN..w - MARGIN {
                let c = p.d[(y * w + x) as usize];
                let at = |k: usize| p.d[((y + CIRCLE[k].1) * w + x + CIRCLE[k].0) as usize];
                // quick rejection on the 4 compass points
                let q = [at(0), at(4), at(8), at(12)];
                let nb = q.iter().filter(|&&v| v > c + t).count();
                let nd = q.iter().filter(|&&v| v < c - t).count();
                if nb < 3 && nd < 3 {
                    continue;
                }
                let mut ring = [0i8; 32];
                for k in 0..16 {
                    let v = at(k);
                    let s = if v > c + t { 1 } else if v < c - t { -1 } else { 0 };
                    ring[k] = s;
                    ring[k + 16] = s;
                }
                let mut ok = false;
                for sign in [1i8, -1] {
                    let mut run = 0;
                    for &s in ring.iter() {
                        if s == sign {
                            run += 1;
                            if run >= 9 {
                                ok = true;
                                break;
                            }
                        } else {
                            run = 0;
                        }
                    }
                    if ok {
                        break;
                    }
                }
                if ok {
                    out.push((x, y));
                }
            }
            out
        })
        .collect()
}

fn harris(p: &Plane, x: i32, y: i32) -> f32 {
    let w = p.w as i32;
    let (mut a, mut b, mut c) = (0f32, 0f32, 0f32);
    for dy in -3..=3 {
        for dx in -3..=3 {
            let i = ((y + dy) * w + x + dx) as usize;
            let gx = p.d[i + 1] - p.d[i - 1];
            let gy = p.d[i + w as usize] - p.d[i - w as usize];
            a += gx * gx;
            b += gy * gy;
            c += gx * gy;
        }
    }
    a * b - c * c - 0.04 * (a + b) * (a + b)
}

fn orientation(p: &Plane, x: i32, y: i32) -> f32 {
    let w = p.w as i32;
    let (mut m01, mut m10) = (0f32, 0f32);
    for dy in -PATCH_R..=PATCH_R {
        for dx in -PATCH_R..=PATCH_R {
            if dx * dx + dy * dy > PATCH_R * PATCH_R {
                continue;
            }
            let v = p.d[((y + dy) * w + x + dx) as usize];
            m10 += dx as f32 * v;
            m01 += dy as f32 * v;
        }
    }
    m01.atan2(m10)
}

/// ORB-like features of a grey image (0..255 values). `n` = target number of keypoints.
pub fn orb(gray: &Plane, n: usize) -> Features {
    let pairs = brief_pairs();
    let levels = 7;
    let factor = 1.25f64;
    let mut out_pts = vec![];
    let mut out_desc = vec![];
    let total_area: f64 = (0..levels).map(|l| factor.powi(-2 * l)).sum();
    let mut cur = gray.clone();
    for l in 0..levels {
        let scale = factor.powi(l);
        if l > 0 {
            let nw = (gray.w as f64 / scale).round() as usize;
            let nh = (gray.h as f64 / scale).round() as usize;
            if nw < 2 * MARGIN as usize + 8 || nh < 2 * MARGIN as usize + 8 {
                break;
            }
            cur = resize(gray, nw, nh);
        }
        let want = ((n as f64) * factor.powi(-2 * l) / total_area).ceil() as usize;
        let mut corners = fast9(&cur, 12.0);
        if corners.len() < want {
            corners = fast9(&cur, 7.0);
        }
        let mut scored: Vec<(f32, i32, i32)> = corners.par_iter().map(|&(x, y)| (harris(&cur, x, y), x, y)).collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        // greedy non-maximum suppression on a coarse grid (3 px)
        let mut taken = std::collections::HashSet::new();
        let mut keep = vec![];
        for (_, x, y) in scored {
            let k = (x / 3, y / 3);
            if taken.contains(&k) {
                continue;
            }
            for dy in -1..=1 {
                for dx in -1..=1 {
                    taken.insert((k.0 + dx, k.1 + dy));
                }
            }
            keep.push((x, y));
            if keep.len() >= want {
                break;
            }
        }
        let smooth = gaussian_blur(&cur, 2.0);
        let w = cur.w as i32;
        let res: Vec<((f32, f32), Desc)> = keep
            .par_iter()
            .map(|&(x, y)| {
                let a = orientation(&cur, x, y);
                let (c, s) = (a.cos(), a.sin());
                let mut d = [0u64; 4];
                for (i, &(px, py, qx, qy)) in pairs.iter().enumerate() {
                    let rp = ((c * px - s * py).round() as i32, (s * px + c * py).round() as i32);
                    let rq = ((c * qx - s * qy).round() as i32, (s * qx + c * qy).round() as i32);
                    let vp = smooth.d[((y + rp.1) * w + x + rp.0) as usize];
                    let vq = smooth.d[((y + rq.1) * w + x + rq.0) as usize];
                    if vp < vq {
                        d[i / 64] |= 1 << (i % 64);
                    }
                }
                (((x as f64 + 0.5) * scale - 0.5) as f32, ((y as f64 + 0.5) * scale - 0.5) as f32)
                    .pipe(|pt| (pt, d))
            })
            .collect();
        for (pt, d) in res {
            out_pts.push(pt);
            out_desc.push(d);
        }
    }
    Features { pts: out_pts, desc: out_desc }
}

trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}
impl<T> Pipe for T {}

#[inline]
fn hamming(a: &Desc, b: &Desc) -> u32 {
    (a[0] ^ b[0]).count_ones() + (a[1] ^ b[1]).count_ones() + (a[2] ^ b[2]).count_ones() + (a[3] ^ b[3]).count_ones()
}

/// Mutual nearest neighbours passing a ratio test. Returns index pairs (a, b).
pub fn match_features(a: &Features, b: &Features) -> Vec<(usize, usize)> {
    let best = |x: &[Desc], y: &[Desc]| -> Vec<(usize, u32, u32)> {
        x.par_iter()
            .map(|d| {
                let (mut b1, mut d1, mut d2) = (0usize, u32::MAX, u32::MAX);
                for (j, e) in y.iter().enumerate() {
                    let h = hamming(d, e);
                    if h < d1 {
                        d2 = d1;
                        d1 = h;
                        b1 = j;
                    } else if h < d2 {
                        d2 = h;
                    }
                }
                (b1, d1, d2)
            })
            .collect()
    };
    if a.desc.is_empty() || b.desc.is_empty() {
        return vec![];
    }
    let ab = best(&a.desc, &b.desc);
    let ba = best(&b.desc, &a.desc);
    ab.iter()
        .enumerate()
        .filter(|(i, (j, d1, d2))| *d1 <= 64 && (*d1 as f32) < 0.85 * *d2 as f32 && ba[*j].0 == *i)
        .map(|(i, (j, _, _))| (i, *j))
        .collect()
}

pub type Homography = [f64; 9];

pub fn apply_h(h: &Homography, x: f64, y: f64) -> (f64, f64) {
    let w = h[6] * x + h[7] * y + h[8];
    ((h[0] * x + h[1] * y + h[2]) / w, (h[3] * x + h[4] * y + h[5]) / w)
}

/// Least-squares homography (h33 = 1) from >= 4 correspondences, with Hartley normalisation.
fn fit_h(src: &[(f64, f64)], dst: &[(f64, f64)]) -> Option<Homography> {
    let norm = |p: &[(f64, f64)]| {
        let n = p.len() as f64;
        let (mx, my) = (p.iter().map(|q| q.0).sum::<f64>() / n, p.iter().map(|q| q.1).sum::<f64>() / n);
        let d = p.iter().map(|q| ((q.0 - mx).powi(2) + (q.1 - my).powi(2)).sqrt()).sum::<f64>() / n;
        let s = if d > 1e-9 { std::f64::consts::SQRT_2 / d } else { 1.0 };
        (mx, my, s)
    };
    let (sx, sy, ss) = norm(src);
    let (dx, dy, ds) = norm(dst);
    let mut ata = [[0f64; 8]; 8];
    let mut atb = [0f64; 8];
    for (p, q) in src.iter().zip(dst) {
        let (x, y) = ((p.0 - sx) * ss, (p.1 - sy) * ss);
        let (u, v) = ((q.0 - dx) * ds, (q.1 - dy) * ds);
        let rows = [([x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y], u), ([0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y], v)];
        for (r, b) in rows {
            for i in 0..8 {
                atb[i] += r[i] * b;
                for j in 0..8 {
                    ata[i][j] += r[i] * r[j];
                }
            }
        }
    }
    // Gaussian elimination with partial pivoting
    let mut m = ata;
    let mut b = atb;
    for c in 0..8 {
        let piv = (c..8).max_by(|&i, &j| m[i][c].abs().partial_cmp(&m[j][c].abs()).unwrap())?;
        if m[piv][c].abs() < 1e-12 {
            return None;
        }
        m.swap(c, piv);
        b.swap(c, piv);
        for r in 0..8 {
            if r != c {
                let f = m[r][c] / m[c][c];
                for k in c..8 {
                    m[r][k] -= f * m[c][k];
                }
                b[r] -= f * b[c];
            }
        }
    }
    let hn: Vec<f64> = (0..8).map(|i| b[i] / m[i][i]).collect();
    let hn = [hn[0], hn[1], hn[2], hn[3], hn[4], hn[5], hn[6], hn[7], 1.0];
    // denormalise: H = Td^-1 * Hn * Ts
    let ts = [ss, 0.0, -ss * sx, 0.0, ss, -ss * sy, 0.0, 0.0, 1.0];
    let tdi = [1.0 / ds, 0.0, dx, 0.0, 1.0 / ds, dy, 0.0, 0.0, 1.0];
    let mul = |a: &[f64; 9], b: &[f64; 9]| {
        let mut r = [0f64; 9];
        for i in 0..3 {
            for j in 0..3 {
                r[i * 3 + j] = (0..3).map(|k| a[i * 3 + k] * b[k * 3 + j]).sum();
            }
        }
        r
    };
    let h = mul(&tdi, &mul(&hn, &ts));
    if h[8].abs() < 1e-12 {
        return None;
    }
    Some(h.map(|v| v / h[8]))
}

/// RANSAC homography src -> dst. Returns (H, inlier count).
pub fn ransac_homography(src: &[(f64, f64)], dst: &[(f64, f64)], thresh: f64, iters: usize) -> Option<(Homography, usize)> {
    let n = src.len();
    if n < 8 {
        return None;
    }
    let mut s: u64 = 0x2545_F491_4F6C_DD1D ^ n as u64;
    let mut rnd = move |m: usize| {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s % m as u64) as usize
    };
    let count = |h: &Homography| -> Vec<usize> {
        (0..n)
            .filter(|&i| {
                let (u, v) = apply_h(h, src[i].0, src[i].1);
                ((u - dst[i].0).powi(2) + (v - dst[i].1).powi(2)).sqrt() < thresh
            })
            .collect()
    };
    let mut best: Option<(Homography, Vec<usize>)> = None;
    for _ in 0..iters {
        let mut idx = [0usize; 4];
        let mut k = 0;
        while k < 4 {
            let c = rnd(n);
            if !idx[..k].contains(&c) {
                idx[k] = c;
                k += 1;
            }
        }
        let (ps, pd): (Vec<_>, Vec<_>) = idx.iter().map(|&i| (src[i], dst[i])).unzip();
        let Some(h) = fit_h(&ps, &pd) else { continue };
        // reject orientation-reversing / degenerate models
        let det = h[0] * h[4] - h[1] * h[3];
        if det <= 0.0 {
            continue;
        }
        let inl = count(&h);
        if best.as_ref().is_none_or(|b| inl.len() > b.1.len()) {
            best = Some((h, inl));
        }
    }
    let (mut h, mut inl) = best?;
    for _ in 0..2 {
        let (ps, pd): (Vec<_>, Vec<_>) = inl.iter().map(|&i| (src[i], dst[i])).unzip();
        if let Some(h2) = fit_h(&ps, &pd) {
            let inl2 = count(&h2);
            if inl2.len() >= inl.len() {
                h = h2;
                inl = inl2;
            }
        }
    }
    Some((h, inl.len()))
}
