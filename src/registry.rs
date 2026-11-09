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
    // Distribution: who this copy was made for. All optional so old registries load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_id: Option<String>,
}

/// Recipient facts recorded with a copy.
#[derive(Clone, Debug, Default)]
pub struct CopyMeta {
    pub recipient_id: Option<String>,
    pub recipient_name: Option<String>,
    pub group_id: Option<String>,
    pub group_name: Option<String>,
    pub batch_id: Option<String>,
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
        self.register_full(id, source, original, marked, channel, strength, output, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn register_full(
        &mut self,
        id: u32,
        source: &str,
        original: &RgbImageBuf,
        marked: &RgbImageBuf,
        channel: Option<String>,
        strength: f32,
        output: Option<String>,
        meta: Option<&CopyMeta>,
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
            recipient_id: meta.and_then(|m| m.recipient_id.clone()),
            recipient_name: meta.and_then(|m| m.recipient_name.clone()),
            group_id: meta.and_then(|m| m.group_id.clone()),
            group_name: meta.and_then(|m| m.group_name.clone()),
            batch_id: meta.and_then(|m| m.batch_id.clone()),
        };
        self.entries.insert(id.to_string(), e);
        self.save()
    }

    pub fn thumb_path(&self, e: &Entry) -> PathBuf {
        self.dir.join(&e.thumb)
    }

    /// Change (or clear) the channel note of an entry.
    pub fn set_channel(&mut self, id: u32, channel: Option<String>) -> Result<()> {
        let e = self.entries.get_mut(&id.to_string()).context("no such entry")?;
        e.channel = channel.filter(|c| !c.trim().is_empty()).map(|c| c.trim().to_string());
        self.save()
    }

    /// Distinct channel notes, most used first.
    pub fn channels(&self) -> Vec<String> {
        let mut m: BTreeMap<String, usize> = BTreeMap::new();
        for e in self.entries.values() {
            if let Some(c) = &e.channel {
                *m.entry(c.clone()).or_default() += 1;
            }
        }
        let mut v: Vec<(String, usize)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.into_iter().map(|(c, _)| c).collect()
    }

    /// Export all entries as CSV (UTF-8 with BOM so Excel opens it correctly).
    pub fn export_csv(&self, path: &Path) -> Result<()> {
        fn q(s: &str) -> String {
            format!("\"{}\"", s.replace('"', "\"\""))
        }
        let mut out = String::from("\u{feff}id,source,channel,registered_utc,width,height,strength,sha256_original,sha256_marked,output\r\n");
        for (k, e) in &self.entries {
            let id = k.parse::<u32>().map(|i| format!("0x{i:08x}")).unwrap_or_else(|_| k.clone());
            out += &[
                q(&id),
                q(&e.source),
                q(e.channel.as_deref().unwrap_or("")),
                q(&e.registered_utc),
                e.size[1].to_string(),
                e.size[0].to_string(),
                e.strength.map(|s| format!("{s:.1}")).unwrap_or_default(),
                q(&e.sha256_original),
                q(&e.sha256_marked),
                q(e.output.as_deref().unwrap_or("")),
            ]
            .join(",");
            out += "\r\n";
        }
        std::fs::write(path, out)?;
        Ok(())
    }

    /// Copy the whole registry folder into `dest/blindmark-registry-backup-<timestamp>`.
    pub fn backup_to(&self, dest: &Path) -> Result<PathBuf> {
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let target = dest.join(format!("blindmark-registry-backup-{stamp}"));
        std::fs::create_dir_all(&target)?;
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                std::fs::copy(entry.path(), target.join(entry.file_name()))?;
            }
        }
        Ok(target)
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

/// The 8 signed permutation matrices (symmetries of the square tile).
fn d4_mats() -> Vec<imgproc::Mat2> {
    let mut v = vec![];
    for k in 0..4 {
        let r = imgproc::rot(k as f64 * std::f64::consts::FRAC_PI_2);
        let r = [[r[0][0].round(), r[0][1].round()], [r[1][0].round(), r[1][1].round()]];
        v.push(r);
        v.push(imgproc::mat_mul(&r, &[[1.0, 0.0], [0.0, -1.0]]));
    }
    v
}

/// Warp the suspect's luma into the thumbnail frame through `l` (original px -> suspect px)
/// and `ts` (thumbnail px per original px). Invalid area is filled with the mean (flat).
fn to_thumb_frame(sy: &Plane, l: &imgproc::Mat2, ts: f64) -> Option<Plane> {
    let fwd = imgproc::mat_scale(&imgproc::mat_inv(l)?, ts); // suspect px -> thumb px
    let shrink = imgproc::mat_det(&fwd).abs().sqrt();
    let (src, pre) = if shrink < 0.9 {
        let p = resize(sy, ((sy.w as f64 * shrink).round() as usize).max(8), ((sy.h as f64 * shrink).round() as usize).max(8));
        let k = (p.w as f64 / sy.w as f64, p.h as f64 / sy.h as f64);
        (p, k)
    } else {
        (sy.clone(), (1.0, 1.0))
    };
    let fwd2 = [[fwd[0][0] / pre.0, fwd[0][1] / pre.1], [fwd[1][0] / pre.0, fwd[1][1] / pre.1]];
    let back = imgproc::mat_inv(&fwd2)?;
    let (origin, cw, ch) = imgproc::warped_bounds(src.w, src.h, &fwd2);
    if cw < 16 || ch < 16 || cw * ch > 3000 * 3000 {
        return None;
    }
    let (mut can, valid) = imgproc::sample_linear(&src, &back, origin, cw, ch);
    let (mut s, mut n) = (0f64, 0f64);
    for (v, &ok) in can.d.iter().zip(&valid) {
        if ok == 1 {
            s += *v as f64;
            n += 1.0;
        }
    }
    let m = if n > 0.0 { (s / n) as f32 } else { 0.0 };
    for (v, &ok) in can.d.iter_mut().zip(&valid) {
        if ok == 0 {
            *v = m;
        }
    }
    Some(can)
}

/// Similarity between a suspect image and the registered thumbnail. The watermark lattice
/// (`det.basis`, or the axis period) gives scale and rotation up to the 8 tile symmetries,
/// which are resolved by phase-correlation strength; the score is the median block
/// correlation of lightly blurred luminance over the overlap (flat UI areas are skipped).
pub fn content_match(reg: &Registry, id: u32, suspect: &RgbImageBuf, det: &wm::Detection) -> Option<ContentMatch> {
    let e = reg.get(id)?;
    let th = load_image(&reg.thumb_path(e)).ok()?;
    let tile = e.tile_px.unwrap_or_else(|| wm::tile_size_for(e.size[0], e.size[1])) as f64;
    let b = det.basis.or(det.period_px.map(|p| [[p, 0.0], [0.0, p]]))?;
    let ts = th.w as f64 / e.size[1] as f64;
    let sy = imgproc::luma(&suspect.data, suspect.w, suspect.h);
    let t = gaussian_blur(&imgproc::luma(&th.data, th.w, th.h), 1.0);
    let bp = |p: &Plane| p.zip(&gaussian_blur(p, 4.0), |a, b| a - b);
    let tb = bp(&t);
    // 1) orientation at half resolution
    let half = |p: &Plane| resize(p, (p.w / 2).max(8), (p.h / 2).max(8));
    let tb_half = bp(&half(&t));
    let mut orient: Option<(f64, imgproc::Mat2)> = None;
    for s in d4_mats() {
        let l = imgproc::mat_scale(&imgproc::mat_mul(&b, &s), 1.0 / tile);
        let Some(c) = to_thumb_frame(&sy, &l, ts * 0.5) else { continue };
        let c = gaussian_blur(&c, 1.0);
        let (_, _, peak) = phase_corr(&tb_half, &hann_taper(&bp(&c)));
        if orient.as_ref().is_none_or(|o| peak > o.0) {
            orient = Some((peak, l));
        }
    }
    let (_, l0) = orient?;
    // 2) scale refinement + offset at full thumbnail resolution
    let mut best: Option<(f64, Plane, isize, isize)> = None;
    for i in -3..=3 {
        let l = imgproc::mat_scale(&l0, 1.0 + 0.01 * i as f64);
        let Some(c) = to_thumb_frame(&sy, &l, ts) else { continue };
        let s = gaussian_blur(&c, 1.0);
        let (dy, dx, peak) = phase_corr(&tb, &hann_taper(&bp(&s)));
        if best.as_ref().is_none_or(|b| peak > b.0) {
            best = Some((peak, s, dy, dx));
        }
    }
    let (_, s, dy, dx) = best?;
    let (sim, overlap) = overlap_corr(&gaussian_blur(&s, 1.5), &gaussian_blur(&t, 1.5), dy, dx);
    Some(ContentMatch { similarity: sim, overlap })
}

// ------------------------------------------------------------------ registry-assisted geometry

/// ORB features of every registered thumbnail, computed lazily once.
#[derive(Default)]
pub struct ThumbIndex {
    cell: std::sync::OnceLock<Vec<(u32, crate::features::Features, usize, usize)>>,
}

impl ThumbIndex {
    pub fn new() -> Self {
        Self::default()
    }
    fn get(&self, reg: &Registry) -> &[(u32, crate::features::Features, usize, usize)] {
        self.cell.get_or_init(|| {
            use rayon::prelude::*;
            let items: Vec<(u32, PathBuf)> = reg.entries.iter().filter_map(|(k, e)| k.parse().ok().map(|id| (id, reg.thumb_path(e)))).collect();
            items
                .par_iter()
                .filter_map(|(id, p)| {
                    let im = load_image(p).ok()?;
                    let g = imgproc::luma(&im.data, im.w, im.h);
                    Some((*id, crate::features::orb(&g, 1200), im.w, im.h))
                })
                .collect()
        })
    }
}

pub struct Located {
    pub entry: u32,
    pub h: crate::features::Homography, // thumbnail px -> suspect px
    pub inliers: usize,
}

/// Find which registered picture appears in `suspect` and the perspective mapping to it.
pub fn locate(reg: &Registry, index: &ThumbIndex, suspect_gray: &Plane) -> Option<Located> {
    use crate::features::{match_features, orb, ransac_homography};
    let sf = orb(suspect_gray, 3000);
    let mut best: Option<Located> = None;
    for (id, tf, _, _) in index.get(reg) {
        let m = match_features(tf, &sf);
        if m.len() < 12 {
            continue;
        }
        let src: Vec<(f64, f64)> = m.iter().map(|&(i, _)| (tf.pts[i].0 as f64, tf.pts[i].1 as f64)).collect();
        let dst: Vec<(f64, f64)> = m.iter().map(|&(_, j)| (sf.pts[j].0 as f64, sf.pts[j].1 as f64)).collect();
        let tol = (suspect_gray.w.max(suspect_gray.h) as f64 / 400.0).max(3.0);
        if let Some((h, n)) = ransac_homography(&src, &dst, tol, 1500) {
            let ok = n >= 15 && n as f64 >= 0.2 * m.len() as f64;
            if ok && best.as_ref().is_none_or(|b| n > b.inliers) {
                best = Some(Located { entry: *id, h, inliers: n });
            }
        }
    }
    best
}

/// Pull the suspect back into the registered picture's frame (at about the suspect's own
/// resolution, never larger than the original). Returns the rectified luma.
pub fn rectify(reg: &Registry, loc: &Located, suspect_gray: &Plane) -> Option<Plane> {
    use crate::features::apply_h;
    let e = reg.get(loc.entry)?;
    let th = image::image_dimensions(reg.thumb_path(e)).ok()?;
    let (ow, oh) = (e.size[1] as f64, e.size[0] as f64);
    let ts = th.0 as f64 / ow; // thumb px per original px
    // local scale of H at the thumbnail centre (suspect px per thumb px)
    let (cx, cy) = (th.0 as f64 / 2.0, th.1 as f64 / 2.0);
    let p0 = apply_h(&loc.h, cx, cy);
    let px = apply_h(&loc.h, cx + 1.0, cy);
    let py = apply_h(&loc.h, cx, cy + 1.0);
    let det = ((px.0 - p0.0) * (py.1 - p0.1) - (px.1 - p0.1) * (py.0 - p0.0)).abs();
    let s_r = (det.sqrt() * ts).min(1.0); // rectified px per original px
    let (rw, rh) = ((ow * s_r).round() as usize, (oh * s_r).round() as usize);
    if rw < 64 || rh < 64 {
        return None;
    }
    let mut out = Plane::new(rw, rh);
    let mut valid = vec![false; rw * rh];
    let (sw, sh) = ((suspect_gray.w - 1) as f64, (suspect_gray.h - 1) as f64);
    {
        use rayon::prelude::*;
        out.d.par_chunks_mut(rw).zip(valid.par_chunks_mut(rw)).enumerate().for_each(|(v, (row, vrow))| {
            for u in 0..rw {
                let xt = (u as f64 + 0.5) / s_r * ts - 0.5;
                let yt = (v as f64 + 0.5) / s_r * ts - 0.5;
                let (x, y) = apply_h(&loc.h, xt, yt);
                if x >= 0.0 && x <= sw && y >= 0.0 && y <= sh {
                    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
                    let (x1, y1) = ((x0 + 1).min(suspect_gray.w - 1), (y0 + 1).min(suspect_gray.h - 1));
                    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
                    let g = |xx: usize, yy: usize| suspect_gray.d[yy * suspect_gray.w + xx];
                    row[u] = (g(x0, y0) * (1.0 - fx) + g(x1, y0) * fx) * (1.0 - fy) + (g(x0, y1) * (1.0 - fx) + g(x1, y1) * fx) * fy;
                    vrow[u] = true;
                }
            }
        });
    }
    let n = valid.iter().filter(|&&v| v).count();
    if n < rw * rh / 5 {
        return None;
    }
    let mean = out.d.iter().zip(&valid).filter(|p| *p.1).map(|p| *p.0 as f64).sum::<f64>() / n as f64;
    for (x, &v) in out.d.iter_mut().zip(&valid) {
        if !v {
            *x = mean as f32;
        }
    }
    Some(out)
}

/// Content similarity of a rectified (already registered) image against its thumbnail.
pub fn rectified_similarity(reg: &Registry, id: u32, rect: &Plane) -> Option<f64> {
    let e = reg.get(id)?;
    let th = load_image(&reg.thumb_path(e)).ok()?;
    let t = gaussian_blur(&imgproc::luma(&th.data, th.w, th.h), 1.5);
    let s = gaussian_blur(&resize(rect, th.w, th.h), 1.5);
    Some(overlap_corr(&s, &t, 0, 0).0)
}
