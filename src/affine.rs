//! Affine synchronisation (rotation, mirror, aspect change, shear).
//!
//! The tiled watermark turns the residual's autocorrelation (ACF) into a 2-D lattice of
//! peaks; an affine distortion maps that lattice to another lattice. We collect candidate
//! lattice bases from several detectors, warp the image so the candidate basis becomes
//! axis-aligned WORK_TILE tiles, fold, and resolve the 8 symmetries of the square tile (D4)
//! by the sync correlation. Port of `python/affine.py`.

use crate::imgproc::{self, fft2, gaussian_blur, mat_det, mat_inv, mat_mul, mat_scale, maximum_filter, resize, rot, Mat2, Plane, C64};
use crate::keys::NBITS;
use crate::wm::{self, finish, highpass, hann, roll2, standardize, Detection, KnownIds, SyncRef, MIN_PERIOD, WORK_TILE as WT};
use rayon::prelude::*;

/// Shifted ACF: value at lag (dx, dy) is a[(cy + dy) * w + cx + dx].
struct Acf {
    a: Vec<f64>,
    h: usize,
    w: usize,
    f0: f64,
}

impl Acf {
    fn cy(&self) -> usize {
        self.h / 2
    }
    fn cx(&self) -> usize {
        self.w / 2
    }
}

fn work_residual(y: &Plane) -> (Vec<f64>, usize, usize, f64) {
    let (h, w) = (y.h, y.w);
    let f0 = (768.0 / h.min(w) as f64).min(1.0);
    let y0 = if f0 < 1.0 {
        resize(y, ((w as f64 * f0).round() as usize).max(1), ((h as f64 * f0).round() as usize).max(1))
    } else {
        y.clone()
    };
    let r = highpass(&y0, 1.5);
    let m = r.mean();
    (r.d.iter().map(|&v| v as f64 - m).collect(), r.h, r.w, f0)
}

fn whitened_acf(y: &Plane) -> Acf {
    let (rv, hh, ww, f0) = work_residual(y);
    let (hy, hx) = (hann(hh), hann(ww));
    let mut buf: Vec<C64> = (0..hh * ww).map(|i| C64::new(rv[i] * (hy[i / ww] * hx[i % ww]).sqrt(), 0.0)).collect();
    fft2(&mut buf, hh, ww, false);
    let p: Vec<f64> = buf.iter().map(|c| c.norm_sqr()).collect();
    let shifted = roll2(&p, hh, ww, hh.div_ceil(2), ww.div_ceil(2));
    let sm = gaussian_blur(&Plane::from_vec(ww, hh, shifted.iter().map(|&v| v as f32).collect()), 4.0);
    let smv: Vec<f64> = sm.d.iter().map(|&v| v as f64).collect();
    let ps = roll2(&smv, hh, ww, hh / 2, ww / 2);
    let mut buf: Vec<C64> = p.iter().zip(&ps).map(|(&a, &b)| C64::new(a / (b + 1e-9), 0.0)).collect();
    fft2(&mut buf, hh, ww, true);
    let a0 = buf[0].re;
    let a: Vec<f64> = buf.iter().map(|c| c.re / a0).collect();
    Acf { a: roll2(&a, hh, ww, hh.div_ceil(2), ww.div_ceil(2)), h: hh, w: ww, f0 }
}

fn plain_acf(y: &Plane) -> Acf {
    let (rv, hh, ww, f0) = work_residual(y);
    let (h2, w2) = (2 * hh, 2 * ww);
    let mut z = vec![C64::new(0.0, 0.0); h2 * w2];
    for yy in 0..hh {
        for xx in 0..ww {
            z[yy * w2 + xx] = C64::new(rv[yy * ww + xx], 0.0);
        }
    }
    fft2(&mut z, h2, w2, false);
    z.iter_mut().for_each(|c| *c = C64::new(c.norm_sqr(), 0.0));
    fft2(&mut z, h2, w2, true);
    let mut a = vec![0.0; h2 * w2];
    for ky in 0..h2 {
        let dy = ky.min(h2 - ky);
        for kx in 0..w2 {
            let dx = kx.min(w2 - kx);
            let n = (hh.saturating_sub(dy) * ww.saturating_sub(dx)).max(1) as f64;
            a[ky * w2 + kx] = z[ky * w2 + kx].re / n;
        }
    }
    let a0 = a[0];
    a.iter_mut().for_each(|v| *v /= a0);
    let s = roll2(&a, h2, w2, hh, ww); // fftshift for even sizes: centre at (hh, ww)
    let (qh, qw) = (hh / 2, ww / 2);
    let (oh, ow) = (2 * qh, 2 * qw);
    let mut out = vec![0.0; oh * ow];
    for y in 0..oh {
        for x in 0..ow {
            out[y * ow + x] = s[(hh - qh + y) * w2 + (ww - qw + x)];
        }
    }
    Acf { a: out, h: oh, w: ow, f0 }
}

fn ring_noise(acf: &Acf, rmin: f64, rmax: f64) -> f64 {
    let (cy, cx) = (acf.cy() as f64, acf.cx() as f64);
    let (mut n, mut s, mut s2) = (0f64, 0f64, 0f64);
    for y in 0..acf.h {
        for x in 0..acf.w {
            let d = ((y as f64 - cy).powi(2) + (x as f64 - cx).powi(2)).sqrt();
            if d > rmin && d <= rmax {
                let v = acf.a[y * acf.w + x];
                n += 1.0;
                s += v;
                s2 += v * v;
            }
        }
    }
    if n < 2.0 {
        return 1.0;
    }
    (s2 / n - (s / n).powi(2)).max(1e-18).sqrt()
}

/// Columns of a basis: b1 = (m[0][0], m[1][0]), b2 = (m[0][1], m[1][1]).
fn basis(b1: [f64; 2], b2: [f64; 2]) -> Mat2 {
    [[b1[0], b2[0]], [b1[1], b2[1]]]
}

fn refine_basis(acf: &Acf, b: &Mat2, max_lat: i32) -> Mat2 {
    let (h, w) = (acf.h as i64, acf.w as i64);
    let (cy, cx) = (acf.cy() as f64, acf.cx() as f64);
    let (mut mtm, mut mtp) = ([[0f64; 2]; 2], [[0f64; 2]; 2]);
    let mut count = 0;
    for m in -max_lat..=max_lat {
        for n in -max_lat..=max_lat {
            if m == 0 && n == 0 {
                continue;
            }
            let vx = b[0][0] * m as f64 + b[0][1] * n as f64;
            let vy = b[1][0] * m as f64 + b[1][1] * n as f64;
            let (xi, yi) = ((cx + vx).round() as i64, (cy + vy).round() as i64);
            if !(3..w - 3).contains(&xi) || !(3..h - 3).contains(&yi) {
                continue;
            }
            let (mut best, mut py, mut px) = (f64::MIN, 0i64, 0i64);
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let v = acf.a[((yi + dy) * w + xi + dx) as usize];
                    if v > best {
                        best = v;
                        py = yi + dy;
                        px = xi + dx;
                    }
                }
            }
            if !(1..h - 1).contains(&py) || !(1..w - 1).contains(&px) {
                continue;
            }
            let at = |y: i64, x: i64| acf.a[(y * w + x) as usize];
            let sub = |a: f64, b: f64, c: f64| {
                let d = a - 2.0 * b + c;
                if d == 0.0 { 0.0 } else { ((a - c) / (2.0 * d)).clamp(-0.5, 0.5) }
            };
            let sx = sub(at(py, px - 1), at(py, px), at(py, px + 1));
            let sy = sub(at(py - 1, px), at(py, px), at(py + 1, px));
            let p = [px as f64 + sx - cx, py as f64 + sy - cy];
            let mn = [m as f64, n as f64];
            for i in 0..2 {
                for j in 0..2 {
                    mtm[i][j] += mn[i] * mn[j];
                    mtp[i][j] += mn[i] * p[j];
                }
            }
            count += 1;
        }
    }
    if count < 4 {
        return *b;
    }
    let Some(inv) = mat_inv(&mtm) else { return *b };
    let sol = mat_mul(&inv, &mtp); // rows: b1, b2
    basis(sol[0], sol[1])
}

fn lagrange_reduce(mut b1: [f64; 2], mut b2: [f64; 2]) -> ([f64; 2], [f64; 2]) {
    let dot = |a: [f64; 2], b: [f64; 2]| a[0] * b[0] + a[1] * b[1];
    if dot(b1, b1) > dot(b2, b2) {
        std::mem::swap(&mut b1, &mut b2);
    }
    for _ in 0..50 {
        let mu = (dot(b1, b2) / dot(b1, b1)).round();
        b2 = [b2[0] - mu * b1[0], b2[1] - mu * b1[1]];
        if dot(b2, b2) >= dot(b1, b1) {
            break;
        }
        std::mem::swap(&mut b1, &mut b2);
    }
    (b1, b2)
}

/// Candidate lattices from pairs of strong ACF peaks (handles aspect change / shear).
fn peak_pair_lattices(acf: &Acf, pmin: f64) -> Vec<Mat2> {
    let (h, w) = (acf.h, acf.w);
    let (cy, cx) = (acf.cy() as f64, acf.cx() as f64);
    let lim = 0.5 * h.min(w) as f64 - 3.0;
    let mx = maximum_filter(&acf.a, h, w, 5);
    let noise = ring_noise(acf, pmin, lim);
    let mut peaks: Vec<(f64, [f64; 2])> = vec![];
    for y in 0..h {
        for x in 0..w {
            let (dy, dx) = (y as f64 - cy, x as f64 - cx);
            let d = (dx * dx + dy * dy).sqrt();
            let v = acf.a[y * w + x];
            if d < pmin || d > lim || v != mx[y * w + x] || dy < 0.0 || (dy == 0.0 && dx < 0.0) || v < 3.0 * noise {
                continue;
            }
            peaks.push((v, [dx, dy]));
        }
    }
    peaks.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    peaks.truncate(16);
    let sample = |v: [f64; 2]| -> Option<f64> {
        let (x, y) = (cx + v[0], cy + v[1]);
        if !(1.0..(w - 2) as f64).contains(&x) || !(1.0..(h - 2) as f64).contains(&y) {
            return None;
        }
        let (xi, yi) = (x.round() as usize, y.round() as usize);
        let mut m = f64::MIN;
        for yy in yi - 1..=yi + 1 {
            for xx in xi - 1..=xi + 1 {
                m = m.max(acf.a[yy * w + xx]);
            }
        }
        Some(m)
    };
    let norm = |v: [f64; 2]| (v[0] * v[0] + v[1] * v[1]).sqrt();
    let mut cands: Vec<(f64, f64, Mat2)> = vec![];
    for i in 0..peaks.len() {
        for j in i + 1..peaks.len() {
            let (a, b) = (peaks[i].1, peaks[j].1);
            if (a[0] * b[1] - a[1] * b[0]).abs() < 0.3 * norm(a) * norm(b) {
                continue;
            }
            let (b1, b2) = lagrange_reduce(a, b);
            if norm(b1).min(norm(b2)) < pmin {
                continue;
            }
            let mut vals = vec![];
            for m in -3..=3 {
                for n in -3..=3 {
                    if m == 0 && n == 0 {
                        continue;
                    }
                    if let Some(s) = sample([m as f64 * b1[0] + n as f64 * b2[0], m as f64 * b1[1] + n as f64 * b2[1]]) {
                        vals.push(s);
                    }
                }
            }
            if vals.len() < 4 {
                continue;
            }
            let score = vals.iter().sum::<f64>() / vals.len() as f64 / noise;
            let area = (b1[0] * b2[1] - b1[1] * b2[0]).abs();
            cands.push((score, area, basis(b1, b2)));
        }
    }
    if cands.is_empty() {
        return vec![];
    }
    let best = cands.iter().map(|c| c.0).fold(f64::MIN, f64::max);
    let mut good: Vec<_> = cands.iter().filter(|c| c.0 >= 0.8 * best).cloned().collect();
    good.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let mut rest = cands.clone();
    rest.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let mut out: Vec<(f64, Mat2)> = vec![];
    for (_, area, b) in good.into_iter().chain(rest) {
        let dup = out.iter().any(|(a0, b0)| {
            (area - a0).abs() / a0 <= 0.05 && (0..2).all(|i| (0..2).all(|j| (b[i][j] - b0[i][j]).abs() <= 2.0))
        });
        if !dup {
            out.push((area, b));
        }
        if out.len() >= 3 {
            break;
        }
    }
    out.into_iter().map(|(_, b)| refine_basis(acf, &b, 3)).collect()
}

/// Matched filter over rotated square lattices (sums evidence over many lattice points).
fn polar_lattices(acf: &Acf, pmin: f64, top: usize) -> Vec<Mat2> {
    let (h, w) = (acf.h, acf.w);
    let (cy, cx) = (acf.cy() as f64, acf.cx() as f64);
    let am = maximum_filter(&acf.a, h, w, 3);
    let lim = 0.5 * h.min(w) as f64 - 3.0;
    let noise = ring_noise(acf, pmin, f64::MAX);
    let mn: Vec<(f64, f64)> = (0..=3)
        .flat_map(|m| (-3..=3).map(move |n| (m, n)))
        .filter(|&(m, n)| m > 0 || n > 0)
        .map(|(m, n)| (m as f64, n as f64))
        .collect();
    let thetas: Vec<f64> = (0..180).map(|i| (i as f64 * 0.5).to_radians()).collect();
    let ps: Vec<f64> = {
        let mut v = vec![];
        let mut p = pmin;
        while p < lim {
            v.push(p);
            p += 1.0;
        }
        v
    };
    let scores: Vec<(f64, usize, usize)> = thetas
        .par_iter()
        .enumerate()
        .flat_map_iter(|(ti, &t)| {
            let (c, s) = (t.cos(), t.sin());
            let am = &am;
            let mn = &mn;
            ps.iter().enumerate().filter_map(move |(pi, &p)| {
                let (mut sum, mut nv) = (0f64, 0usize);
                for &(m, n) in mn.iter() {
                    let x = p * (c * m - s * n);
                    let y = p * (s * m + c * n);
                    if (x * x + y * y).sqrt() > lim {
                        continue;
                    }
                    let (xi, yi) = ((cx + x).round() as isize, (cy + y).round() as isize);
                    if xi < 0 || yi < 0 || xi >= w as isize || yi >= h as isize {
                        continue;
                    }
                    sum += am[yi as usize * w + xi as usize];
                    nv += 1;
                }
                (nv >= 4).then(|| (sum / nv as f64 / noise, ti, pi))
            })
        })
        .collect();
    let mut order = scores;
    order.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let mut picked: Vec<(f64, f64)> = vec![]; // (p, theta)
    for &(_, ti, pi) in order.iter().take(4000) {
        let (t, p) = (thetas[ti], ps[pi]);
        let close = picked.iter().any(|&(q, u)| {
            let dt = (t - u).abs();
            (p - q).abs() / q < 0.04 && dt.min(std::f64::consts::FRAC_PI_2 - dt) < 3f64.to_radians()
        });
        if close {
            continue;
        }
        picked.push((p, t));
        if picked.len() >= top {
            break;
        }
    }
    picked.into_iter().map(|(p, t)| refine_basis(acf, &mat_scale(&rot(t), p), 3)).collect()
}

/// Nearest similarity (rotation or reflection, uniform scale) to `b`.
fn snap_similarity(b: &Mat2) -> Mat2 {
    let refl = mat_det(b) < 0.0;
    let f: Mat2 = [[1.0, 0.0], [0.0, -1.0]];
    let bb = if refl { mat_mul(b, &f) } else { *b };
    let t = (bb[1][0] - bb[0][1]).atan2(bb[0][0] + bb[1][1]);
    let s = mat_det(b).abs().sqrt();
    let r = mat_scale(&rot(t), s);
    if refl { mat_mul(&r, &f) } else { r }
}

fn same_lattice(b1: &Mat2, b2: &Mat2) -> bool {
    let Some(inv) = mat_inv(b1) else { return false };
    let u = mat_mul(&inv, b2);
    let ur = [[u[0][0].round(), u[0][1].round()], [u[1][0].round(), u[1][1].round()]];
    let err = (0..2).flat_map(|i| (0..2).map(move |j| (i, j))).map(|(i, j)| (u[i][j] - ur[i][j]).abs()).fold(0.0, f64::max);
    err < 0.012 && (mat_det(&ur).abs() - 1.0).abs() < 1e-6
}

fn tick(label: &str, t: &mut std::time::Instant) {
    if std::env::var_os("BLINDMARK_PROFILE").is_some() {
        eprintln!("  {label}: {:.3}s", t.elapsed().as_secs_f64());
    }
    *t = std::time::Instant::now();
}

pub fn lattice_candidates(y: &Plane, cap: usize) -> Vec<Mat2> {
    let mut t = std::time::Instant::now();
    let aw = whitened_acf(y);
    tick("whitened_acf", &mut t);
    let pmin = (MIN_PERIOD * aw.f0).max(10.0);
    let ap = plain_acf(y);
    tick("plain_acf", &mut t);
    let pminp = (MIN_PERIOD * ap.f0).max(10.0);
    let g0: Vec<Mat2> = wm::estimate_periods(y).into_iter().take(3).map(|p| [[p, 0.0], [0.0, p]]).collect();
    tick("axis periods", &mut t);
    let g1: Vec<Mat2> = polar_lattices(&aw, pmin, 4).iter().map(|b| mat_scale(b, 1.0 / aw.f0)).collect();
    tick("polar whitened", &mut t);
    let g2: Vec<Mat2> = polar_lattices(&ap, pminp, 3).iter().map(|b| mat_scale(b, 1.0 / ap.f0)).collect();
    tick("polar plain", &mut t);
    let g3: Vec<Mat2> = peak_pair_lattices(&aw, pmin).iter().map(|b| mat_scale(b, 1.0 / aw.f0)).collect();
    tick("peak pairs", &mut t);
    let groups = vec![g0, g1, g2, g3];
    let maxlen = groups.iter().map(|g| g.len()).max().unwrap_or(0);
    let mut out: Vec<Mat2> = vec![];
    'outer: for i in 0..maxlen {
        for g in &groups {
            if let Some(b) = g.get(i) {
                for c in [snap_similarity(b), *b] {
                    if mat_det(&c).abs() < MIN_PERIOD * MIN_PERIOD * 0.8 {
                        continue;
                    }
                    if !out.iter().any(|o| same_lattice(&c, o)) {
                        out.push(c);
                    }
                }
                if out.len() >= cap {
                    break 'outer;
                }
            }
        }
    }
    out
}

/// Tiles per axis used for affine folding (more tiles = more averaging, but slower).
const AFFINE_TILES: f64 = 8.0;

/// Source prepared for folding with bases close to `b0`: anti-alias shrink (computed once)
/// and, when the tiles are small, a central window of at most MAX_TILES x MAX_TILES tiles.
pub struct Prepared {
    ys: Plane,
    // native px -> prepared px: x' = sx * (x - x0)
    sx: f64,
    sy: f64,
}

pub fn prepare(y: &Plane, b0: &Mat2) -> Prepared {
    let blen = mat_det(b0).abs().sqrt();
    let lim = (AFFINE_TILES * blen) as usize;
    let (x0, y0) = (y.w.saturating_sub(lim) / 2, y.h.saturating_sub(lim) / 2);
    let src = if y.w > lim || y.h > lim { y.crop(x0, y0, lim.min(y.w - x0), lim.min(y.h - y0)) } else { y.clone() };
    let r = (WT as f64 / blen).min(1.0);
    let ys = if r < 1.0 {
        resize(&src, ((src.w as f64 * r).round() as usize).max(8), ((src.h as f64 * r).round() as usize).max(8))
    } else {
        src.clone()
    };
    let sx = ys.w as f64 / src.w as f64;
    let sy = ys.h as f64 / src.h as f64;
    Prepared { ys, sx, sy }
}

/// Warp so basis `b` (native px) becomes axis-aligned WT x WT tiles, then fold.
pub fn rectified_fold(p: &Prepared, b: &Mat2) -> Option<Vec<f64>> {
    let ys = &p.ys;
    let bs: Mat2 = [[b[0][0] * p.sx, b[0][1] * p.sx], [b[1][0] * p.sy, b[1][1] * p.sy]];
    let m = mat_scale(&bs, 1.0 / WT as f64); // canvas px -> prepared px
    let minv = mat_inv(&m)?;
    let (origin, cw, ch) = imgproc::warped_bounds(ys.w, ys.h, &minv);
    if cw < 16 || ch < 16 || cw * ch > 2600 * 2600 {
        return None;
    }
    let (mut can, valid) = imgproc::sample_linear(ys, &m, origin, cw, ch);
    let (mut s, mut n) = (0f64, 0f64);
    for (v, &ok) in can.d.iter().zip(&valid) {
        if ok == 1 {
            s += *v as f64;
            n += 1.0;
        }
    }
    let mean = if n > 0.0 { (s / n) as f32 } else { 0.0 };
    for (v, &ok) in can.d.iter_mut().zip(&valid) {
        if ok == 0 {
            *v = mean;
        }
    }
    let res = highpass(&can, 1.5);
    let wgt = imgproc::erode(&valid, cw, ch, 7);
    let n = WT * WT;
    let (mut acc, mut cnt) = (vec![0f64; n], vec![0f64; n]);
    let (ox, oy) = (origin.0 as i64, origin.1 as i64);
    for v in 0..ch {
        let iy = (v as i64 + oy).rem_euclid(WT as i64) as usize;
        for u in 0..cw {
            if wgt[v * cw + u] == 0 {
                continue;
            }
            let ix = (u as i64 + ox).rem_euclid(WT as i64) as usize;
            acc[iy * WT + ix] += res.d[v * cw + u] as f64;
            cnt[iy * WT + ix] += 1.0;
        }
    }
    Some(standardize(acc.iter().zip(&cnt).map(|(a, &c)| if c > 0.0 { a / c } else { 0.0 }).collect()))
}

/// The 8 symmetries of the square applied to a WT x WT tile (numpy rot90 / transpose order).
fn d4(t: &[f64], k: usize) -> Vec<f64> {
    let n = WT;
    let mut out = vec![0f64; n * n];
    for i in 0..n {
        for j in 0..n {
            // source indices for rot90^r of (transposed if k >= 4)
            let (mut a, mut b) = (i, j);
            match k % 4 {
                0 => {}
                1 => (a, b) = (j, n - 1 - i),
                2 => (a, b) = (n - 1 - i, n - 1 - j),
                _ => (a, b) = (n - 1 - j, i),
            }
            if k >= 4 {
                std::mem::swap(&mut a, &mut b);
            }
            out[i * n + j] = t[a * n + b];
        }
    }
    out
}

struct Eval {
    z: f64,
    soft: [f64; NBITS],
    b: Mat2,
}

fn best_orientation(p: &Prepared, b: &Mat2, sref: &SyncRef) -> Option<Eval> {
    let f = rectified_fold(p, b)?;
    (0..8)
        .map(|k| {
            let (z, soft) = sref.analyse(&d4(&f, k));
            Eval { z, soft, b: *b }
        })
        .max_by(|a, b| a.z.partial_cmp(&b.z).unwrap())
}

pub fn detect_affine(y: &Plane, key: &[u8], known: Option<&KnownIds>, z_thresh: f64) -> Detection {
    let sref = SyncRef::new(key);
    let mut t = std::time::Instant::now();
    let cands = lattice_candidates(y, 10);
    tick("candidates total", &mut t);
    let mut coarse: Vec<(Eval, usize)> = cands
        .par_iter()
        .enumerate()
        .filter_map(|(ci, b)| {
            let p = prepare(y, b);
            [0.993, 1.0, 1.007]
                .iter()
                .filter_map(|&s| best_orientation(&p, &mat_scale(b, s), &sref))
                .max_by(|a, b| a.z.partial_cmp(&b.z).unwrap())
                .map(|e| (e, ci))
        })
        .collect();
    coarse.sort_by(|a, b| b.0.z.partial_cmp(&a.0.z).unwrap());
    // fine search: scale first (13 steps), then angle (5 steps) at the best scale
    let fine = coarse
        .iter()
        .take(2)
        .filter_map(|(e, _)| {
            let p = prepare(y, &e.b);
            let scales: Vec<Mat2> = (0..13).map(|i| mat_scale(&e.b, 0.985 + 0.0025 * i as f64)).collect();
            let bs = scales.par_iter().filter_map(|b| best_orientation(&p, b, &sref)).max_by(|a, b| a.z.partial_cmp(&b.z).unwrap())?;
            let angles: Vec<Mat2> = [-0.4f64, -0.2, 0.2, 0.4].iter().map(|da| mat_mul(&rot(da.to_radians()), &bs.b)).collect();
            let ba = angles.par_iter().filter_map(|b| best_orientation(&p, b, &sref)).max_by(|a, b| a.z.partial_cmp(&b.z).unwrap());
            Some(match ba {
                Some(a) if a.z > bs.z => a,
                _ => bs,
            })
        })
        .max_by(|a, b| a.z.partial_cmp(&b.z).unwrap());
    tick("coarse+fine folds", &mut t);
    let coarse: Vec<Eval> = coarse.into_iter().map(|(e, _)| e).collect();
    let best = match (coarse.into_iter().next(), fine) {
        (Some(c), Some(f)) => if f.z > c.z { f } else { c },
        (Some(c), None) => c,
        (None, Some(f)) => f,
        (None, None) => return Detection { found: false, present: false, image_id: None, method: None, z_sync: f64::NAN, z_id: None, period_px: None, basis: None },
    };
    let mut d = finish(best.z, &best.soft, mat_det(&best.b).abs().sqrt(), key, known, z_thresh);
    d.basis = Some(best.b);
    d
}
