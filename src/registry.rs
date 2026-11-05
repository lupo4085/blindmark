//! Local registry of issued IDs (JSON, compatible with the Python reference) and
//! the content check that ties a decoded ID back to the registered picture.

use crate::imgproc::{self, fft2, gaussian_blur, resize, Plane, C64};
use crate::keys;
use crate::wm;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub source: String,
    pub sha256_original: String,
    pub sha256_marked: String,
    pub size: [usize; 2], // [h, w]
    pub thumb: String,
    pub registered_utc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile_px: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Meta {
    pub format: String,
    pub key_fingerprint: String,
}

pub struct Registry {
    pub dir: PathBuf,
    pub entries: BTreeMap<String, Entry>,
    pub meta: Option<Meta>,
}

impl Registry {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let db = dir.join("registry.json");
        let entries = if db.exists() {
            serde_json::from_str(&std::fs::read_to_string(&db)?).context("registry.json is not valid")?
        } else {
            BTreeMap::new()
        };
        let mp = dir.join("meta.json");
        let meta = if mp.exists() { serde_json::from_str(&std::fs::read_to_string(&mp)?).ok() } else { None };
        Ok(Registry { dir: dir.to_path_buf(), entries, meta })
    }

    /// Returns Err if this registry was created with a different key.
    pub fn check_key(&mut self, key: &[u8]) -> Result<()> {
        let fp = keys::fingerprint(key);
        match &self.meta {
            Some(m) if m.key_fingerprint != fp => anyhow::bail!(
                "this registry belongs to key {} but the loaded key is {}",
                m.key_fingerprint,
                fp
            ),
            Some(_) => Ok(()),
            None => {
                let m = Meta { format: keys::FORMAT.into(), key_fingerprint: fp };
                std::fs::write(self.dir.join("meta.json"), serde_json::to_string_pretty(&m)?)?;
                self.meta = Some(m);
                Ok(())
            }
        }
    }

    pub fn ids(&self) -> Vec<u32> {
        self.entries.keys().filter_map(|k| k.parse().ok()).collect()
    }

    pub fn get(&self, id: u32) -> Option<&Entry> {
        self.entries.get(&id.to_string())
    }

    fn save(&self) -> Result<()> {
        let tmp = self.dir.join("registry.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&self.entries)?)?;
        std::fs::rename(&tmp, self.dir.join("registry.json"))?;
        Ok(())
    }

    /// Fresh random ID not yet in the registry.
    pub fn new_id(&self) -> Result<u32> {
        loop {
            let id = getrandom::u32().map_err(|e| anyhow::anyhow!("RNG: {e}"))?;
            if !self.entries.contains_key(&id.to_string()) {
                return Ok(id);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn register(
        &mut self,
        id: u32,
        source: &str,
        original: &RgbImageBuf,
        marked: &RgbImageBuf,
        channel: Option<String>,
        strength: f32,
        output: Option<String>,
    ) -> Result<()> {
        let thumb_name = format!("{id:08x}.png");
        let th = thumbnail(marked, 512);
        image::save_buffer(self.dir.join(&thumb_name), &th.data, th.w as u32, th.h as u32, image::ColorType::Rgb8)?;
        let e = Entry {
            source: source.to_string(),
            sha256_original: hex::encode(Sha256::digest(&original.data)),
            sha256_marked: hex::encode(Sha256::digest(&marked.data)),
            size: [original.h, original.w],
            thumb: thumb_name,
            registered_utc: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            tile_px: Some(wm::tile_size_for(original.h, original.w)),
            channel: channel.filter(|c| !c.trim().is_empty()),
            strength: Some(strength),
            output,
        };
        self.entries.insert(id.to_string(), e);
        self.save()
    }

    pub fn thumb_path(&self, e: &Entry) -> PathBuf {
        self.dir.join(&e.thumb)
    }
}

// ------------------------------------------------------------------ images

#[derive(Clone)]
pub struct RgbImageBuf {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,          // interleaved RGB8
    pub alpha: Option<Vec<u8>>, // kept as-is when the source had transparency
}

pub fn load_image(path: &Path) -> Result<RgbImageBuf> {
    use image::ImageDecoder;
    let reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let mut dec = reader.into_decoder().with_context(|| format!("unsupported image: {}", path.display()))?;
    let orient = dec.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(dec)?;
    img.apply_orientation(orient);
    Ok(from_dynamic(img))
}

pub fn from_dynamic(img: image::DynamicImage) -> RgbImageBuf {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if img.color().has_alpha() {
        let rgba = img.to_rgba8().into_raw();
        let mut data = Vec::with_capacity(w * h * 3);
        let mut alpha = Vec::with_capacity(w * h);
        for p in rgba.chunks(4) {
            data.extend_from_slice(&p[..3]);
            alpha.push(p[3]);
        }
        RgbImageBuf { w, h, data, alpha: Some(alpha) }
    } else {
        RgbImageBuf { w, h, data: img.to_rgb8().into_raw(), alpha: None }
    }
}

pub fn save_image(img: &RgbImageBuf, path: &Path, jpeg_quality: Option<u8>) -> Result<()> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "jpg" || ext == "jpeg" {
        let f = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(f, jpeg_quality.unwrap_or(95));
        enc.encode(&img.data, img.w as u32, img.h as u32, image::ExtendedColorType::Rgb8)?;
        return Ok(());
    }
    match &img.alpha {
        Some(a) => {
            let mut rgba = Vec::with_capacity(img.w * img.h * 4);
            for (p, &al) in img.data.chunks(3).zip(a) {
                rgba.extend_from_slice(p);
                rgba.push(al);
            }
            image::save_buffer(path, &rgba, img.w as u32, img.h as u32, image::ColorType::Rgba8)?;
        }
        None => image::save_buffer(path, &img.data, img.w as u32, img.h as u32, image::ColorType::Rgb8)?,
    }
    Ok(())
}

pub fn thumbnail(img: &RgbImageBuf, side: usize) -> RgbImageBuf {
    let s = side as f64 / img.w.max(img.h) as f64;
    if s >= 1.0 {
        return RgbImageBuf { alpha: None, ..img.clone() };
    }
    let (nw, nh) = (((img.w as f64 * s).round() as usize).max(1), ((img.h as f64 * s).round() as usize).max(1));
    let mut planes = vec![];
    for c in 0..3 {
        let p = Plane::from_vec(img.w, img.h, img.data.iter().skip(c).step_by(3).map(|&v| v as f32).collect());
        planes.push(resize(&p, nw, nh));
    }
    let mut data = vec![0u8; nw * nh * 3];
    for i in 0..nw * nh {
        for c in 0..3 {
            data[i * 3 + c] = planes[c].d[i].round().clamp(0.0, 255.0) as u8;
        }
    }
    RgbImageBuf { w: nw, h: nh, data, alpha: None }
}

// ------------------------------------------------------------------ content check

/// Similarity between a suspect image and the registered thumbnail.
/// The watermark's tile period gives an initial scale (refined by a small search);
/// phase correlation gives the offset; the score is the Pearson correlation of
/// lightly blurred luminance over the overlapping area.
pub struct ContentMatch {
    pub similarity: f64,
    pub overlap: f64, // fraction of the suspect covered by the registered picture
}

fn hann_taper(p: &Plane) -> Plane {
    let wy: Vec<f32> = (0..p.h).map(|i| (std::f32::consts::PI * (i as f32 + 0.5) / p.h as f32).sin()).collect();
    let wx: Vec<f32> = (0..p.w).map(|i| (std::f32::consts::PI * (i as f32 + 0.5) / p.w as f32).sin()).collect();
    let mut o = p.clone();
    for y in 0..p.h {
        for x in 0..p.w {
            o.d[y * p.w + x] *= wy[y] * wx[x];
        }
    }
    o
}

/// Phase correlation of `s` against `t`: best offset (dy, dx) with s(y) ~ t(y + d), and peak strength.
fn phase_corr(t: &Plane, s: &Plane) -> (isize, isize, f64) {
    let (hp, wp) = (t.h + s.h, t.w + s.w);
    let pad = |p: &Plane| {
        let m = p.mean();
        let mut v = vec![C64::new(0.0, 0.0); hp * wp];
        for y in 0..p.h {
            for x in 0..p.w {
                v[y * wp + x] = C64::new(p.d[y * p.w + x] as f64 - m, 0.0);
            }
        }
        v
    };
    let (mut ft, mut fs) = (pad(t), pad(s));
    fft2(&mut ft, hp, wp, false);
    fft2(&mut fs, hp, wp, false);
    let mut r: Vec<C64> = ft
        .iter()
        .zip(&fs)
        .map(|(a, b)| {
            let c = a * b.conj();
            let n = c.norm();
            if n > 1e-12 { c / n } else { C64::new(0.0, 0.0) }
        })
        .collect();
    fft2(&mut r, hp, wp, true);
    let n = r.len() as f64;
    let mean = r.iter().map(|c| c.re).sum::<f64>() / n;
    let sd = (r.iter().map(|c| (c.re - mean).powi(2)).sum::<f64>() / n).sqrt().max(1e-12);
    let bi = (0..r.len()).max_by(|&i, &j| r[i].re.partial_cmp(&r[j].re).unwrap()).unwrap();
    let (ky, kx) = ((bi / wp) as isize, (bi % wp) as isize);
    let dy = if ky < t.h as isize { ky } else { ky - hp as isize };
    let dx = if kx < t.w as isize { kx } else { kx - wp as isize };
    (dy, dx, (r[bi].re - mean) / sd)
}

/// Block-wise correlation over the overlap. Flat blocks (UI bars, margins) are skipped and
/// the median over textured blocks is returned, so app chrome around a partial capture
/// does not dilute the score. Returns (median block correlation, overlap fraction).
fn overlap_corr(s: &Plane, t: &Plane, dy: isize, dx: isize) -> (f64, f64) {
    const B: usize = 24;
    let mut covered = 0usize;
    let mut cors = vec![];
    for by in (0..s.h).step_by(B) {
        for bx in (0..s.w).step_by(B) {
            let (mut n, mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
            for y in by..(by + B).min(s.h) {
                let ty = y as isize + dy;
                if ty < 0 || ty >= t.h as isize {
                    continue;
                }
                for x in bx..(bx + B).min(s.w) {
                    let tx = x as isize + dx;
                    if tx < 0 || tx >= t.w as isize {
                        continue;
                    }
                    let a = s.d[y * s.w + x] as f64;
                    let b = t.d[ty as usize * t.w + tx as usize] as f64;
                    n += 1.0;
                    sa += a;
                    sb += b;
                    saa += a * a;
                    sbb += b * b;
                    sab += a * b;
                }
            }
            covered += n as usize;
            if n < (B * B / 2) as f64 {
                continue;
            }
            let va = saa / n - (sa / n).powi(2);
            let vb = sbb / n - (sb / n).powi(2);
            if va < 4.0 || vb < 4.0 {
                continue; // flat in either image: no evidence either way
            }
            cors.push((sab / n - sa / n * sb / n) / (va * vb).sqrt());
        }
    }
    let overlap = covered as f64 / (s.w * s.h) as f64;
    if cors.len() < 4 {
        return (0.0, overlap);
    }
    cors.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (cors[cors.len() / 2], overlap)
}

pub fn content_match(reg: &Registry, id: u32, suspect: &RgbImageBuf, period_px: f64) -> Option<ContentMatch> {
    let e = reg.get(id)?;
    let th = load_image(&reg.thumb_path(e)).ok()?;
    let tile = e.tile_px.unwrap_or_else(|| wm::tile_size_for(e.size[0], e.size[1])) as f64;
    let scale = period_px / tile; // suspect px per original px
    let base = th.w as f64 / e.size[1] as f64 / scale;
    let sy = imgproc::luma(&suspect.data, suspect.w, suspect.h);
    let t = gaussian_blur(&imgproc::luma(&th.data, th.w, th.h), 1.0);
    let bp = |p: &Plane| p.zip(&gaussian_blur(p, 4.0), |a, b| a - b);
    let tb = bp(&t);
    let mut best: Option<(f64, f64, Plane, isize, isize)> = None; // (peak, factor, s, dy, dx)
    for i in -4..=4 {
        let f = base * (1.0 + 0.008 * i as f64);
        let (nw, nh) = ((sy.w as f64 * f).round() as usize, (sy.h as f64 * f).round() as usize);
        if nw < 16 || nh < 16 {
            continue;
        }
        let s = gaussian_blur(&resize(&sy, nw, nh), 1.0);
        let (dy, dx, peak) = phase_corr(&tb, &hann_taper(&bp(&s)));
        if best.as_ref().is_none_or(|b| peak > b.0) {
            best = Some((peak, f, s, dy, dx));
        }
    }
    let (_, _, s, dy, dx) = best?;
    // small residual misalignment is tolerated by comparing lightly blurred luminance
    let (sim, overlap) = overlap_corr(&gaussian_blur(&s, 1.5), &gaussian_blur(&t, 1.5), dy, dx);
    Some(ContentMatch { similarity: sim, overlap })
}
