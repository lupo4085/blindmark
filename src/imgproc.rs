//! Minimal single-channel image processing used by blindmark
//! (filters mirror OpenCV defaults: BORDER_REFLECT_101, INTER_AREA / INTER_CUBIC).

use rayon::prelude::*;
use rustfft::{num_complex::Complex, FftPlanner};

#[derive(Clone, Debug)]
pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub d: Vec<f32>,
}

impl Plane {
    pub fn new(w: usize, h: usize) -> Self {
        Plane { w, h, d: vec![0.0; w * h] }
    }
    pub fn from_vec(w: usize, h: usize, d: Vec<f32>) -> Self {
        assert_eq!(d.len(), w * h);
        Plane { w, h, d }
    }
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.d[y * self.w + x]
    }
    pub fn map(&self, f: impl Fn(f32) -> f32 + Sync) -> Plane {
        Plane { w: self.w, h: self.h, d: self.d.par_iter().map(|&v| f(v)).collect() }
    }
    pub fn zip(&self, o: &Plane, f: impl Fn(f32, f32) -> f32 + Sync) -> Plane {
        assert!(self.w == o.w && self.h == o.h);
        Plane { w: self.w, h: self.h, d: self.d.par_iter().zip(&o.d).map(|(&a, &b)| f(a, b)).collect() }
    }
    pub fn crop(&self, x0: usize, y0: usize, w: usize, h: usize) -> Plane {
        let mut out = Plane::new(w, h);
        for y in 0..h {
            out.d[y * w..(y + 1) * w].copy_from_slice(&self.d[(y0 + y) * self.w + x0..(y0 + y) * self.w + x0 + w]);
        }
        out
    }
    pub fn mean(&self) -> f64 {
        self.d.iter().map(|&v| v as f64).sum::<f64>() / self.d.len().max(1) as f64
    }
}

#[inline]
pub fn reflect101(mut i: isize, n: isize) -> usize {
    if n == 1 {
        return 0;
    }
    loop {
        if i < 0 {
            i = -i;
        } else if i >= n {
            i = 2 * n - 2 - i;
        } else {
            return i as usize;
        }
    }
}

/// Separable convolution with a symmetric kernel (len = 2r+1), reflect-101 borders.
pub fn sep_conv(p: &Plane, k: &[f32]) -> Plane {
    let r = (k.len() / 2) as isize;
    let (w, h) = (p.w, p.h);
    // horizontal
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src = &p.d[y * w..(y + 1) * w];
        for x in 0..w {
            let mut s = 0f32;
            for (t, &kv) in k.iter().enumerate() {
                s += kv * src[reflect101(x as isize + t as isize - r, w as isize)];
            }
            row[x] = s;
        }
    });
    // vertical
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let idx: Vec<usize> = (0..k.len()).map(|t| reflect101(y as isize + t as isize - r, h as isize)).collect();
        for x in 0..w {
            let mut s = 0f32;
            for (t, &kv) in k.iter().enumerate() {
                s += kv * tmp[idx[t] * w + x];
            }
            row[x] = s;
        }
    });
    Plane { w, h, d: out }
}

pub fn box_blur(p: &Plane, ksize: usize) -> Plane {
    sep_conv(p, &vec![1.0 / ksize as f32; ksize])
}

pub fn gaussian_kernel(sigma: f64) -> Vec<f32> {
    // OpenCV for float images: ksize = round(sigma*4*2+1) | 1
    let ksize = (((sigma * 8.0 + 1.0).round() as usize) | 1).max(3);
    let r = (ksize / 2) as isize;
    let mut k: Vec<f64> = (-r..=r).map(|x| (-(x * x) as f64 / (2.0 * sigma * sigma)).exp()).collect();
    let s: f64 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= s);
    k.into_iter().map(|v| v as f32).collect()
}

pub fn gaussian_blur(p: &Plane, sigma: f64) -> Plane {
    sep_conv(p, &gaussian_kernel(sigma))
}

pub fn local_std(p: &Plane, k: usize) -> Plane {
    let m = box_blur(p, k);
    let m2 = box_blur(&p.map(|v| v * v), k);
    m.zip(&m2, |a, b| (b - a * a).max(0.0).sqrt())
}

// ---------------------------------------------------------------- resampling

struct Taps {
    start: Vec<usize>,
    weights: Vec<Vec<f32>>,
}

fn area_taps(src: usize, dst: usize) -> Taps {
    let scale = src as f64 / dst as f64; // > 1
    let mut start = Vec::with_capacity(dst);
    let mut weights = Vec::with_capacity(dst);
    for i in 0..dst {
        let a = i as f64 * scale;
        let b = ((i + 1) as f64 * scale).min(src as f64);
        let s0 = a.floor() as usize;
        let s1 = (b.ceil() as usize).min(src);
        let mut ws = Vec::with_capacity(s1 - s0);
        for s in s0..s1 {
            let ov = (b.min((s + 1) as f64) - a.max(s as f64)).max(0.0);
            ws.push((ov / (b - a)) as f32);
        }
        start.push(s0);
        weights.push(ws);
    }
    Taps { start, weights }
}

fn cubic_taps(src: usize, dst: usize) -> Taps {
    let a = -0.75f64; // OpenCV bicubic
    let cw = |x: f64| {
        let x = x.abs();
        if x <= 1.0 {
            ((a + 2.0) * x - (a + 3.0)) * x * x + 1.0
        } else if x < 2.0 {
            ((a * x - 5.0 * a) * x + 8.0 * a) * x - 4.0 * a
        } else {
            0.0
        }
    };
    let scale = dst as f64 / src as f64;
    let mut start = Vec::with_capacity(dst);
    let mut weights = Vec::with_capacity(dst);
    for i in 0..dst {
        let sx = (i as f64 + 0.5) / scale - 0.5;
        let f = sx.floor();
        let t = sx - f;
        let base = f as isize - 1;
        // clamp (replicate) border: materialise up to 4 taps on a contiguous run
        let idx: Vec<usize> = (0..4).map(|k| (base + k).clamp(0, src as isize - 1) as usize).collect();
        let w4: Vec<f64> = (0..4).map(|k| cw(t - (k as f64 - 1.0))).collect();
        let s0 = idx[0];
        let s1 = idx[3];
        let mut ws = vec![0f32; s1 - s0 + 1];
        for k in 0..4 {
            ws[idx[k] - s0] += w4[k] as f32;
        }
        start.push(s0);
        weights.push(ws);
    }
    Taps { start, weights }
}

fn taps_for(src: usize, dst: usize) -> Taps {
    if dst < src { area_taps(src, dst) } else { cubic_taps(src, dst) }
}

/// Resize: area averaging when shrinking, bicubic when enlarging (per axis).
pub fn resize(p: &Plane, nw: usize, nh: usize) -> Plane {
    let nw = nw.max(1);
    let nh = nh.max(1);
    if nw == p.w && nh == p.h {
        return p.clone();
    }
    let tx = taps_for(p.w, nw);
    let ty = taps_for(p.h, nh);
    let mut tmp = vec![0f32; nw * p.h];
    tmp.par_chunks_mut(nw).enumerate().for_each(|(y, row)| {
        let src = &p.d[y * p.w..(y + 1) * p.w];
        for x in 0..nw {
            let s0 = tx.start[x];
            row[x] = tx.weights[x].iter().enumerate().map(|(k, &w)| w * src[s0 + k]).sum();
        }
    });
    let mut out = vec![0f32; nw * nh];
    out.par_chunks_mut(nw).enumerate().for_each(|(y, row)| {
        let s0 = ty.start[y];
        for (k, &w) in ty.weights[y].iter().enumerate() {
            let src = &tmp[(s0 + k) * nw..(s0 + k + 1) * nw];
            for x in 0..nw {
                row[x] += w * src[x];
            }
        }
    });
    Plane { w: nw, h: nh, d: out }
}

// ---------------------------------------------------------------- FFT

pub type C64 = Complex<f64>;

/// In-place 2-D FFT of a row-major (h x w) buffer.
pub fn fft2(data: &mut [C64], h: usize, w: usize, inverse: bool) {
    let mut planner = FftPlanner::<f64>::new();
    let fw = if inverse { planner.plan_fft_inverse(w) } else { planner.plan_fft_forward(w) };
    let fh = if inverse { planner.plan_fft_inverse(h) } else { planner.plan_fft_forward(h) };
    data.par_chunks_mut(w).for_each(|row| fw.process(row));
    let mut t = vec![C64::new(0.0, 0.0); w * h];
    for y in 0..h {
        for x in 0..w {
            t[x * h + y] = data[y * w + x];
        }
    }
    t.par_chunks_mut(h).for_each(|col| fh.process(col));
    let norm = if inverse { 1.0 / (w * h) as f64 } else { 1.0 };
    for y in 0..h {
        for x in 0..w {
            data[y * w + x] = t[x * h + y] * norm;
        }
    }
}

pub fn fftfreq(n: usize) -> Vec<f64> {
    (0..n).map(|i| if i < n.div_ceil(2) { i as f64 / n as f64 } else { (i as f64 - n as f64) / n as f64 }).collect()
}

// ---------------------------------------------------------------- colour

/// BT.601 full-range luma of an interleaved RGB8 buffer.
pub fn luma(rgb: &[u8], w: usize, h: usize) -> Plane {
    let d = rgb
        .par_chunks(3)
        .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
        .collect();
    Plane::from_vec(w, h, d)
}

// ---------------------------------------------------------------- linear warps

/// 2x2 matrix, row-major: [[a, b], [c, d]] maps (x, y) -> (a x + b y, c x + d y).
pub type Mat2 = [[f64; 2]; 2];

pub fn mat_mul(a: &Mat2, b: &Mat2) -> Mat2 {
    [
        [a[0][0] * b[0][0] + a[0][1] * b[1][0], a[0][0] * b[0][1] + a[0][1] * b[1][1]],
        [a[1][0] * b[0][0] + a[1][1] * b[1][0], a[1][0] * b[0][1] + a[1][1] * b[1][1]],
    ]
}

pub fn mat_det(a: &Mat2) -> f64 {
    a[0][0] * a[1][1] - a[0][1] * a[1][0]
}

pub fn mat_inv(a: &Mat2) -> Option<Mat2> {
    let d = mat_det(a);
    if d.abs() < 1e-12 {
        return None;
    }
    Some([[a[1][1] / d, -a[0][1] / d], [-a[1][0] / d, a[0][0] / d]])
}

pub fn mat_scale(a: &Mat2, s: f64) -> Mat2 {
    [[a[0][0] * s, a[0][1] * s], [a[1][0] * s, a[1][1] * s]]
}

pub fn rot(t: f64) -> Mat2 {
    [[t.cos(), -t.sin()], [t.sin(), t.cos()]]
}

#[inline]
fn bilinear(p: &Plane, x: f64, y: f64) -> f32 {
    let x = x.clamp(0.0, (p.w - 1) as f64);
    let y = y.clamp(0.0, (p.h - 1) as f64);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(p.w - 1), (y0 + 1).min(p.h - 1));
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let a = p.d[y0 * p.w + x0] * (1.0 - fx) + p.d[y0 * p.w + x1] * fx;
    let b = p.d[y1 * p.w + x0] * (1.0 - fx) + p.d[y1 * p.w + x1] * fx;
    a * (1.0 - fy) + b * fy
}

/// Resample `src` onto a canvas through a linear map `m`: canvas pixel (u, v), offset by
/// `origin`, samples source pixel m · (u + origin_x, v + origin_y). Returns the canvas and
/// a validity mask (1 where the sample fell inside the source).
pub fn sample_linear(src: &Plane, m: &Mat2, origin: (f64, f64), cw: usize, ch: usize) -> (Plane, Vec<u8>) {
    let mut out = Plane::new(cw, ch);
    let mut mask = vec![0u8; cw * ch];
    let (sw, sh) = ((src.w - 1) as f64, (src.h - 1) as f64);
    out.d.par_chunks_mut(cw).zip(mask.par_chunks_mut(cw)).enumerate().for_each(|(v, (row, mrow))| {
        let vv = v as f64 + origin.1;
        for u in 0..cw {
            let uu = u as f64 + origin.0;
            let x = m[0][0] * uu + m[0][1] * vv;
            let y = m[1][0] * uu + m[1][1] * vv;
            if x >= 0.0 && x <= sw && y >= 0.0 && y <= sh {
                row[u] = bilinear(src, x, y);
                mrow[u] = 1;
            }
        }
    });
    (out, mask)
}

/// Canvas bounds for warping a w x h source with forward map `fwd` (source -> canvas).
pub fn warped_bounds(w: usize, h: usize, fwd: &Mat2) -> ((f64, f64), usize, usize) {
    let cs = [(0.0, 0.0), (w as f64, 0.0), (0.0, h as f64), (w as f64, h as f64)];
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in cs {
        let u = fwd[0][0] * x + fwd[0][1] * y;
        let v = fwd[1][0] * x + fwd[1][1] * y;
        x0 = x0.min(u);
        y0 = y0.min(v);
        x1 = x1.max(u);
        y1 = y1.max(v);
    }
    ((x0.floor(), y0.floor()), (x1.ceil() - x0.floor()) as usize, (y1.ceil() - y0.floor()) as usize)
}

/// Binary erosion with a k x k square (outside the image counts as valid, like OpenCV).
pub fn erode(mask: &[u8], w: usize, h: usize, k: usize) -> Vec<u8> {
    // integral image of invalid pixels
    let mut ii = vec![0u32; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut run = 0u32;
        for x in 0..w {
            run += (mask[y * w + x] == 0) as u32;
            ii[(y + 1) * (w + 1) + x + 1] = ii[y * (w + 1) + x + 1] + run;
        }
    }
    let r = (k / 2) as isize;
    let mut out = vec![0u8; w * h];
    for y in 0..h as isize {
        for x in 0..w as isize {
            let (ya, yb) = ((y - r).max(0) as usize, ((y + r + 1).min(h as isize)) as usize);
            let (xa, xb) = ((x - r).max(0) as usize, ((x + r + 1).min(w as isize)) as usize);
            let s = ii[yb * (w + 1) + xb] + ii[ya * (w + 1) + xa] - ii[ya * (w + 1) + xb] - ii[yb * (w + 1) + xa];
            out[y as usize * w + x as usize] = (s == 0) as u8;
        }
    }
    out
}

pub fn maximum_filter(a: &[f64], h: usize, w: usize, k: usize) -> Vec<f64> {
    let r = (k / 2) as isize;
    let mut tmp = vec![0f64; h * w];
    for y in 0..h {
        for x in 0..w as isize {
            let mut m = f64::MIN;
            for d in -r..=r {
                let xx = (x + d).clamp(0, w as isize - 1) as usize;
                m = m.max(a[y * w + xx]);
            }
            tmp[y * w + x as usize] = m;
        }
    }
    let mut out = vec![0f64; h * w];
    for y in 0..h as isize {
        for x in 0..w {
            let mut m = f64::MIN;
            for d in -r..=r {
                let yy = (y + d).clamp(0, h as isize - 1) as usize;
                m = m.max(tmp[yy * w + x]);
            }
            out[y as usize * w + x] = m;
        }
    }
    out
}
