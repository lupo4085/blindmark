//! Embedding and blind detection (port of the Python reference `blindmark.py`).

use crate::imgproc::{self, fft2, fftfreq, gaussian_blur, local_std, resize, Plane, C64};
use crate::keys::{self, Pattern, G, NBITS, NCHIPS, NSYNC};
use rayon::prelude::*;

pub const WORK_TILE: usize = 128; // detector tile size (2 px per chip)
pub const MIN_PERIOD: f64 = 36.0; // smallest native period searched
pub const MAX_TILES: f64 = 10.0; // per axis, when the detector upsamples / for affine folding // per axis, when the detector upsamples
pub const DEFAULT_STRENGTH: f32 = 2.6;
pub const Z_THRESH: f64 = 6.5;
pub const Z_THRESH_STRICT: f64 = 7.5;

pub fn tile_size_for(h: usize, w: usize) -> usize {
    (2 * G).max((h.min(w) as f64 / 4.0).round() as usize)
}

/// Bilinear wrap-around upsampling of a g x g tile to period `t` pixels.
pub fn periodic_upsample(tile: &[f32], g: usize, h: usize, w: usize, t: f64) -> Plane {
    let coords = |n: usize| -> Vec<(usize, usize, f32)> {
        (0..n)
            .map(|i| {
                let u = (i as f64 + 0.5) * g as f64 / t - 0.5;
                let i0 = u.floor();
                let f = (u - i0) as f32;
                let i0 = (i0 as i64).rem_euclid(g as i64) as usize;
                (i0, (i0 + 1) % g, f)
            })
            .collect()
    };
    let cy = coords(h);
    let cx = coords(w);
    let mut out = Plane::new(w, h);
    out.d.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (y0, y1, fy) = cy[y];
        for x in 0..w {
            let (x0, x1, fx) = cx[x];
            let t00 = tile[y0 * g + x0];
            let t01 = tile[y0 * g + x1];
            let t10 = tile[y1 * g + x0];
            let t11 = tile[y1 * g + x1];
            row[x] = (1.0 - fy) * ((1.0 - fx) * t00 + fx * t01) + fy * ((1.0 - fx) * t10 + fx * t11);
        }
    });
    out
}

/// Content-adaptive strength map: texture masking x luminance masking.
pub fn jnd_strength(y: &Plane, base: f32) -> Plane {
    let ls = local_std(y, 7);
    let s = y.zip(&ls, |yv, sd| {
        let tex = (0.5 + sd / 12.0).clamp(0.5, 2.5);
        let lum = ((yv + 10.0) / 50.0).min((265.0 - yv) / 50.0).clamp(0.3, 1.0);
        base * tex * lum
    });
    gaussian_blur(&s, 2.0)
}

/// Embed in place into an interleaved RGB8 buffer. Returns PSNR (dB).
/// Only luma changes; adding dY to Y with Cb/Cr fixed adds dY to R, G and B.
pub fn embed_rgb(rgb: &mut [u8], w: usize, h: usize, key: &[u8], id: u32, strength: f32) -> f64 {
    let t = tile_size_for(h, w);
    let pat = keys::make_pattern(key);
    let bits = keys::encode_payload(key, id);
    let tile = keys::tile_values(&pat, Some(&bits));
    let y = imgproc::luma(rgb, w, h);
    let s = jnd_strength(&y, strength);
    let wm = periodic_upsample(&tile, G, h, w, t as f64);
    let se: f64 = rgb
        .par_chunks_mut(3)
        .zip(s.d.par_iter().zip(wm.d.par_iter()))
        .map(|(px, (&sv, &wv))| {
            let dy = sv * wv;
            let mut e = 0f64;
            for c in px.iter_mut() {
                let nv = (*c as f32 + dy).round().clamp(0.0, 255.0) as u8;
                let diff = nv as f64 - *c as f64;
                e += diff * diff;
                *c = nv;
            }
            e
        })
        .sum();
    let mse = se / (w * h * 3) as f64;
    if mse == 0.0 { f64::INFINITY } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

// ------------------------------------------------------------------ detection

pub(crate) fn highpass(y: &Plane, sigma: f64) -> Plane {
    let r = y.zip(&gaussian_blur(y, sigma), |a, b| a - b);
    let ls = local_std(&r, 15);
    r.zip(&ls, |a, s| a / (s + 1.0))
}

pub(crate) fn hann(n: usize) -> Vec<f64> {
    if n == 1 {
        return vec![1.0];
    }
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos()).collect()
}

pub(crate) fn median_filter_nearest(s: &[f64], size: usize) -> Vec<f64> {
    let r = (size / 2) as isize;
    let n = s.len() as isize;
    (0..n)
        .map(|i| {
            let mut win: Vec<f64> = (i - r..=i + r).map(|j| s[j.clamp(0, n - 1) as usize]).collect();
            win.sort_by(|a, b| a.partial_cmp(b).unwrap());
            win[win.len() / 2]
        })
        .collect()
}

pub(crate) fn roll2(src: &[f64], h: usize, w: usize, sy: usize, sx: usize) -> Vec<f64> {
    let mut out = vec![0.0; h * w];
    for y in 0..h {
        for x in 0..w {
            out[y * w + x] = src[((y + sy) % h) * w + (x + sx) % w];
        }
    }
    out
}

/// Candidate tile periods (native px): whitened + plain autocorrelation peaks, plus harmonics.
pub fn estimate_periods(y: &Plane) -> Vec<f64> {
    let (h, w) = (y.h, y.w);
    let f0 = (768.0 / h.min(w) as f64).min(1.0);
    let y0 = if f0 < 1.0 {
        resize(y, ((w as f64 * f0).round() as usize).max(1), ((h as f64 * f0).round() as usize).max(1))
    } else {
        y.clone()
    };
    let r = highpass(&y0, 1.5);
    let mean = r.mean();
    let rv: Vec<f64> = r.d.iter().map(|&v| v as f64 - mean).collect();
    let (hh, ww) = (r.h, r.w);
    let pmax = (hh.min(ww) as f64 * 0.75) as usize;
    let pmin = 12usize.max((MIN_PERIOD * f0) as usize);
    if pmax <= pmin + 8 {
        return vec![];
    }
    let ps: Vec<usize> = (pmin..pmax).collect();

    let peaks = |a: &[f64], aw: usize, top_n: usize, base_win: usize| -> Vec<f64> {
        let s: Vec<f64> = ps
            .iter()
            .map(|&p| a[p] + a[p * aw] + 0.5 * (a[p * aw + p] + a[p * aw + (aw - p)]))
            .collect();
        let med = median_filter_nearest(&s, base_win);
        let prom: Vec<f64> = s.iter().zip(&med).map(|(a, b)| a - b).collect();
        let mut order: Vec<usize> = (0..s.len()).collect();
        order.sort_by(|&i, &j| prom[j].partial_cmp(&prom[i]).unwrap());
        let mut found = vec![];
        for i in order {
            if prom[i] <= 0.0 || found.len() >= top_n {
                break;
            }
            if i > 0 && i + 1 < s.len() && prom[i] >= prom[i - 1] && prom[i] >= prom[i + 1] {
                let (a0, b0, c0) = (s[i - 1], s[i], s[i + 1]);
                let den = a0 - 2.0 * b0 + c0;
                let d = if den != 0.0 { (a0 - c0) / (2.0 * den) } else { 0.0 };
                found.push((ps[i] as f64 + d.clamp(-0.5, 0.5)) / f0);
            }
        }
        found
    };

    // (a) spectrally whitened ACF
    let (hy, hx) = (hann(hh), hann(ww));
    let mut buf: Vec<C64> = (0..hh * ww)
        .map(|i| C64::new(rv[i] * (hy[i / ww] * hx[i % ww]).sqrt(), 0.0))
        .collect();
    fft2(&mut buf, hh, ww, false);
    let p: Vec<f64> = buf.iter().map(|c| c.norm_sqr()).collect();
    // fftshift -> gaussian(4) -> ifftshift
    let shifted = roll2(&p, hh, ww, hh.div_ceil(2), ww.div_ceil(2));
    let sm = gaussian_blur(&Plane::from_vec(ww, hh, shifted.iter().map(|&v| v as f32).collect()), 4.0);
    let smv: Vec<f64> = sm.d.iter().map(|&v| v as f64).collect();
    let ps_spec = roll2(&smv, hh, ww, hh / 2, ww / 2);
    let mut buf: Vec<C64> = p.iter().zip(&ps_spec).map(|(&a, &b)| C64::new(a / (b + 1e-9), 0.0)).collect();
    fft2(&mut buf, hh, ww, true);
    let a0 = buf[0].re;
    let aw: Vec<f64> = buf.iter().map(|c| c.re / a0).collect();

    // (b) plain ACF with overlap normalisation
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
    let mut ap = vec![0.0; h2 * w2];
    for ky in 0..h2 {
        let dy = if ky < hh { ky } else { h2 - ky };
        for kx in 0..w2 {
            let dx = if kx < ww { kx } else { w2 - kx };
            let n = (hh.saturating_sub(dy) * ww.saturating_sub(dx)).max(1) as f64;
            ap[ky * w2 + kx] = z[ky * w2 + kx].re / n;
        }
    }
    let ap0 = ap[0];
    ap.iter_mut().for_each(|v| *v /= ap0);

    let mut cand: Vec<f64> = vec![];
    for p in peaks(&aw, ww, 8, 9).into_iter().chain(peaks(&ap, w2, 3, 15)) {
        if cand.iter().all(|&q| (p - q).abs() / q > 0.03) {
            cand.push(p);
        }
    }
    let mut out = cand.clone();
    let lim = 0.75 * h.min(w) as f64;
    for &p in cand.iter().take(3) {
        for q in [p / 2.0, p * 2.0] {
            if q >= MIN_PERIOD && q <= lim && out.iter().all(|&o| (q - o).abs() / o > 0.03) {
                out.push(q);
            }
        }
    }
    out
}

fn residual_at(y: &Plane, period: f64) -> Plane {
    let r = WORK_TILE as f64 / period;
    let src;
    let yy = if r > 1.0 {
        let lim = (MAX_TILES * period) as usize;
        let y0 = y.h.saturating_sub(lim) / 2;
        let x0 = y.w.saturating_sub(lim) / 2;
        src = y.crop(x0, y0, lim.min(y.w - x0), lim.min(y.h - y0));
        &src
    } else {
        y
    };
    let nh = ((yy.h as f64 * r).round() as usize).max(8);
    let nw = ((yy.w as f64 * r).round() as usize).max(8);
    highpass(&resize(yy, nw, nh), 1.5)
}

fn fold(res: &Plane, period_res: f64) -> Vec<f64> {
    let n = WORK_TILE * WORK_TILE;
    let k = WORK_TILE as f64 / period_res;
    let iy: Vec<usize> = (0..res.h).map(|y| ((y as f64 % period_res) * k) as usize % WORK_TILE).collect();
    let ix: Vec<usize> = (0..res.w).map(|x| ((x as f64 % period_res) * k) as usize % WORK_TILE).collect();
    let mut acc = vec![0f64; n];
    let mut cnt = vec![0u32; n];
    for y in 0..res.h {
        let base = iy[y] * WORK_TILE;
        let row = &res.d[y * res.w..(y + 1) * res.w];
        for x in 0..res.w {
            acc[base + ix[x]] += row[x] as f64;
            cnt[base + ix[x]] += 1;
        }
    }
    let f: Vec<f64> = acc.iter().zip(&cnt).map(|(a, &c)| a / c.max(1) as f64).collect();
    standardize(f)
}

pub(crate) fn standardize(mut v: Vec<f64>) -> Vec<f64> {
    let n = v.len() as f64;
    let m = v.iter().sum::<f64>() / n;
    let sd = (v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / n).sqrt() + 1e-9;
    v.iter_mut().for_each(|x| *x = (*x - m) / sd);
    v
}

pub struct SyncRef {
    pat: Pattern,
    fft: Vec<C64>,
}

impl SyncRef {
    pub fn new(key: &[u8]) -> Self {
        let pat = keys::make_pattern(key);
        let t = periodic_upsample(&keys::tile_values(&pat, None), G, WORK_TILE, WORK_TILE, WORK_TILE as f64);
        // circular gaussian high-pass, same as the residual's
        let mut buf: Vec<C64> = t.d.iter().map(|&v| C64::new(v as f64, 0.0)).collect();
        fft2(&mut buf, WORK_TILE, WORK_TILE, false);
        let f = fftfreq(WORK_TILE);
        let s = 1.5f64;
        let mut low = buf.clone();
        for yy in 0..WORK_TILE {
            for xx in 0..WORK_TILE {
                let g = (-2.0 * (std::f64::consts::PI * s).powi(2) * (f[yy] * f[yy] + f[xx] * f[xx])).exp();
                low[yy * WORK_TILE + xx] *= g;
            }
        }
        fft2(&mut low, WORK_TILE, WORK_TILE, true);
        let hp: Vec<f64> = t.d.iter().zip(&low).map(|(&a, b)| a as f64 - b.re).collect();
        let mut fft: Vec<C64> = standardize(hp).into_iter().map(|v| C64::new(v, 0.0)).collect();
        fft2(&mut fft, WORK_TILE, WORK_TILE, false);
        SyncRef { pat, fft }
    }

    /// Align a folded tile to the sync template -> (z_sync, per-bit soft values).
    pub(crate) fn analyse(&self, f: &[f64]) -> (f64, [f64; NBITS]) {
        let n = WORK_TILE;
        let mut buf: Vec<C64> = f.iter().map(|&v| C64::new(v, 0.0)).collect();
        fft2(&mut buf, n, n, false);
        for (b, s) in buf.iter_mut().zip(&self.fft) {
            *b *= s.conj();
        }
        fft2(&mut buf, n, n, true);
        let (mut best, mut bi) = (f64::MIN, 0);
        for (i, c) in buf.iter().enumerate() {
            if c.re > best {
                best = c.re;
                bi = i;
            }
        }
        let (sy, sx) = (bi / n, bi % n);
        let fa = roll2(f, n, n, sy, sx);
        let k = WORK_TILE / G;
        let mut chipv = vec![0f64; NCHIPS];
        for cy in 0..G {
            for cx in 0..G {
                let mut s = 0.0;
                for dy in 0..k {
                    for dx in 0..k {
                        s += fa[(cy * k + dy) * n + cx * k + dx];
                    }
                }
                chipv[cy * G + cx] = s / (k * k) as f64;
            }
        }
        let chipv = standardize(chipv);
        let mut z = 0.0;
        let mut soft = [0f64; NBITS];
        for i in 0..NCHIPS {
            let v = chipv[i] * self.pat.chips[i] as f64;
            if self.pat.is_sync[i] {
                z += v;
            } else {
                soft[self.pat.bit_of[i] as usize] += v;
            }
        }
        let nd = (NCHIPS - NSYNC) as f64;
        soft.iter_mut().for_each(|s| *s /= nd.sqrt());
        (z / (NSYNC as f64).sqrt(), soft)
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Detection {
    pub found: bool,
    pub present: bool,
    pub image_id: Option<u32>,
    pub method: Option<String>,
    pub z_sync: f64,
    pub z_id: Option<f64>,
    pub period_px: Option<f64>,
    /// Lattice basis (native px per tile, columns = tile axes) when found by affine sync.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basis: Option<[[f64; 2]; 2]>,
}

/// Expected +/-1 bit signs for issued IDs (rows of NBITS).
pub fn payload_matrix(key: &[u8], ids: &[u32]) -> Vec<[i8; NBITS]> {
    ids.par_iter()
        .map(|&id| {
            let b = keys::encode_payload(key, id);
            let mut r = [0i8; NBITS];
            for i in 0..NBITS {
                r[i] = if b[i] == 1 { 1 } else { -1 };
            }
            r
        })
        .collect()
}

pub struct KnownIds {
    pub ids: Vec<u32>,
    pub rows: Vec<[i8; NBITS]>,
}

impl KnownIds {
    pub fn new(key: &[u8], ids: Vec<u32>) -> Self {
        let rows = payload_matrix(key, &ids);
        KnownIds { ids, rows }
    }
}

pub fn detect_rgb(rgb: &[u8], w: usize, h: usize, key: &[u8], known: Option<&KnownIds>) -> Detection {
    let y = imgproc::luma(rgb, w, h);
    detect_luma(&y, key, known, Z_THRESH)
}

/// Fast axis-aligned path first; if it does not identify the image, affine synchronisation
/// (rotation / mirror / aspect / shear) is tried and the better result is returned.
pub fn detect_luma(y: &Plane, key: &[u8], known: Option<&KnownIds>, z_thresh: f64) -> Detection {
    let t = std::time::Instant::now();
    let d = detect_axis(y, key, known, z_thresh);
    if std::env::var_os("BLINDMARK_PROFILE").is_some() {
        eprintln!("  axis path: {:.3}s (image {}x{})", t.elapsed().as_secs_f64(), y.w, y.h);
    }
    if d.found {
        return d;
    }
    let a = crate::affine::detect_affine(y, key, known, z_thresh);
    if a.found || a.z_sync > d.z_sync || d.z_sync.is_nan() { a } else { d }
}

pub fn detect_axis(y: &Plane, key: &[u8], known: Option<&KnownIds>, z_thresh: f64) -> Detection {
    let sref = SyncRef::new(key);
    let cands = estimate_periods(y);
    // coarse pass (parallel over candidates)
    let mut scored: Vec<(f64, f64, Plane)> = cands
        .par_iter()
        .map(|&p0| {
            let res = residual_at(y, p0);
            let zb = [-0.01, 0.0, 0.01]
                .iter()
                .map(|d| sref.analyse(&fold(&res, WORK_TILE as f64 * (1.0 + d))).0)
                .fold(f64::MIN, f64::max);
            (zb, p0, res)
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    // fine pass around the best two
    let sref = &sref;
    let fine: Vec<(f64, [f64; NBITS], f64)> = scored
        .iter()
        .take(2)
        .flat_map(|(_, p0, res)| {
            (0..13).map(move |i| {
                let d = -0.015 + 0.0025 * i as f64;
                let (z, soft) = sref.analyse(&fold(res, WORK_TILE as f64 * (1.0 + d)));
                (z, soft, p0 * (1.0 + d))
            })
        })
        .collect();
    let Some((z, soft, period)) = fine.into_iter().max_by(|a, b| a.0.partial_cmp(&b.0).unwrap()) else {
        return Detection { found: false, present: false, image_id: None, method: None, z_sync: f64::NAN, z_id: None, period_px: None, basis: None };
    };
    finish(z, &soft, period, key, known, z_thresh)
}

/// Decision step shared by all synchronisation paths: CRC decode, then registry ML decode.
pub(crate) fn finish(z: f64, soft: &[f64; NBITS], period: f64, key: &[u8], known: Option<&KnownIds>, z_thresh: f64) -> Detection {
    let present = z >= z_thresh;
    let mut bits = [0u8; NBITS];
    for i in 0..NBITS {
        bits[i] = (soft[i] > 0.0) as u8;
    }
    let (id, ok) = keys::decode_payload(key, &bits);
    if present && ok {
        return Detection { found: true, present, image_id: Some(id), method: Some("crc".into()), z_sync: z, z_id: None, period_px: Some(period), basis: None };
    }
    if present {
        if let Some(k) = known.filter(|k| !k.ids.is_empty()) {
            let scores: Vec<f64> = k.rows.iter().map(|r| r.iter().zip(soft).map(|(&s, &v)| s as f64 * v).sum()).collect();
            let (j, &best) = scores.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap();
            let thr = norm_isf(1e-6 / k.ids.len() as f64).max(5.0);
            let hamming = k.rows[j].iter().zip(soft).filter(|(s, v)| (**s > 0) != (**v > 0.0)).count();
            if best >= thr && hamming <= NBITS / 3 {
                return Detection { found: true, present, image_id: Some(k.ids[j]), method: Some("registry".into()), z_sync: z, z_id: Some(best), period_px: Some(period), basis: None };
            }
            return Detection { found: false, present, image_id: None, method: None, z_sync: z, z_id: Some(best), period_px: Some(period), basis: None };
        }
    }
    Detection { found: false, present, image_id: None, method: None, z_sync: z, z_id: None, period_px: Some(period), basis: None }
}

/// Inverse survival function of N(0,1) (Acklam's rational approximation).
pub fn norm_isf(p: f64) -> f64 {
    -norm_ppf(p)
}

pub fn norm_ppf(p: f64) -> f64 {
    const A: [f64; 6] = [-3.969683028665376e+01, 2.209460984245205e+02, -2.759285104469687e+02, 1.383577518672690e+02, -3.066479806614716e+01, 2.506628277459239e+00];
    const B: [f64; 5] = [-5.447609879822406e+01, 1.615858368580409e+02, -1.556989798598866e+02, 6.680131188771972e+01, -1.328068155288572e+01];
    const C: [f64; 6] = [-7.784894002430293e-03, -3.223964580411365e-01, -2.400758277161838e+00, -2.549732539343734e+00, 4.374664141464968e+00, 2.938163982698783e+00];
    const D: [f64; 4] = [7.784695709041462e-03, 3.224671290700398e-01, 2.445134137142996e+00, 3.754408661907416e+00];
    let pl = 0.02425;
    if p < pl {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5]) / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - pl {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        -norm_ppf(1.0 - p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Procedural textured test image (no external files needed).
    fn synthetic(w: usize, h: usize) -> Vec<u8> {
        let mut s: u32 = 12345;
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                let n = (s >> 24) as f32 / 255.0 * 30.0;
                let v = 120.0 + 50.0 * ((x as f32 * 0.05).sin() * (y as f32 * 0.03).cos()) + n;
                for c in 0..3 {
                    rgb[(y * w + x) * 3 + c] = (v + c as f32 * 10.0).clamp(0.0, 255.0) as u8;
                }
            }
        }
        rgb
    }

    #[test]
    fn roundtrip_and_crop_and_scale() {
        let key = b"unit-test-key-0123456789abcdef!!".to_vec();
        let (w, h) = (640, 480);
        let mut img = synthetic(w, h);
        let psnr = embed_rgb(&mut img, w, h, &key, 0xC0FFEE, DEFAULT_STRENGTH);
        assert!(psnr > 35.0, "psnr {psnr}");
        let d = detect_rgb(&img, w, h, &key, None);
        assert!(d.found && d.image_id == Some(0xC0FFEE), "{d:?}");
        // crop + downscale
        let y = imgproc::luma(&img, w, h);
        let c = y.crop(100, 60, 400, 300);
        let c = resize(&c, 280, 210);
        let d = detect_luma(&c, &key, Some(&KnownIds::new(&key, vec![1, 2, 0xC0FFEE, 3])), Z_THRESH);
        assert!(d.found && d.image_id == Some(0xC0FFEE), "{d:?}");
        // rotation by 30 degrees and a mirror flip (affine synchronisation)
        let t = 30f64.to_radians();
        let m: imgproc::Mat2 = [[t.cos(), t.sin()], [-t.sin(), t.cos()]]; // canvas -> source (inverse rotation)
        let fwd = imgproc::mat_inv(&m).unwrap();
        let (origin, cw, ch) = imgproc::warped_bounds(w, h, &fwd);
        let (r, _) = imgproc::sample_linear(&y, &m, origin, cw, ch);
        let d = detect_luma(&r, &key, Some(&KnownIds::new(&key, vec![7, 0xC0FFEE])), Z_THRESH);
        assert!(d.found && d.image_id == Some(0xC0FFEE) && d.basis.is_some(), "rotated: {d:?}");
        let mut mir = y.clone();
        for row in mir.d.chunks_mut(w) {
            row.reverse();
        }
        let d = detect_luma(&mir, &key, None, Z_THRESH);
        assert!(d.found && d.image_id == Some(0xC0FFEE), "mirrored: {d:?}");
        // wrong key / unmarked image must not be detected
        let d = detect_rgb(&img, w, h, b"another-key-0123456789abcdefgh!!", None);
        assert!(!d.present, "{d:?}");
        let d = detect_rgb(&synthetic(w, h), w, h, &key, None);
        assert!(!d.present, "{d:?}");
    }
}
