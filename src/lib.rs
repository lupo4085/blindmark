//! blindmark — keyed, content-adaptive, screenshot-tolerant blind image watermark.
pub mod affine;
pub mod distribute;
pub mod features;
pub mod imgproc;
pub mod jobs;
pub mod keys;
pub mod recipients;
pub mod registry;
pub mod timestamp;
pub mod wm;

use anyhow::Result;
use registry::{load_image, save_image, Registry, RgbImageBuf};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbedCheck {
    /// Readable from the saved file and after a JPEG-75 round trip.
    Ok,
    /// Readable from the saved file, but not after JPEG-75: likely lost when re-compressed or screenshotted.
    Weak,
    /// Not readable even from the saved file: the picture is too flat (large pure black / white areas).
    Unreadable,
}

/// The watermark could not be read back from the saved file: the picture is too flat
/// (large pure black / white areas). The output was deleted and nothing was registered.
#[derive(Debug)]
pub struct WatermarkNotReadable {
    /// The input file name.
    pub file: String,
}

impl std::fmt::Display for WatermarkNotReadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "the picture is too flat (large pure black or white areas) to hold a watermark; it could not be read back, so nothing was saved.",
        )
    }
}

impl std::error::Error for WatermarkNotReadable {}

#[derive(Debug)]
pub struct EmbedResult {
    pub id: u32,
    pub psnr: f64,
    pub check: EmbedCheck,
    pub sha256_file: String,
}

/// Re-detect a freshly saved watermarked file. `id` is the ID just embedded.
pub fn check_embedded(saved: &Path, key: &[u8], id: u32) -> Result<EmbedCheck> {
    let saved_img = load_image(saved)?;
    let known = wm::KnownIds::new(key, vec![id]);
    let y = imgproc::luma(&saved_img.data, saved_img.w, saved_img.h);
    let det = wm::detect_luma(&y, key, Some(&known), wm::Z_THRESH);
    if !det.found || det.image_id != Some(id) {
        return Ok(EmbedCheck::Unreadable);
    }
    let rt = registry::jpeg_roundtrip(&saved_img, 75)?;
    let y_rt = imgproc::luma(&rt.data, rt.w, rt.h);
    let det_rt = wm::detect_luma(&y_rt, key, Some(&known), wm::Z_THRESH);
    if det_rt.found && det_rt.image_id == Some(id) {
        Ok(EmbedCheck::Ok)
    } else {
        Ok(EmbedCheck::Weak)
    }
}

/// Load -> embed -> save -> read back -> register. Used by both the GUI and the CLI.
pub fn embed_file(
    input: &Path,
    output: &Path,
    key: &[u8],
    reg: &mut Registry,
    strength: f32,
    channel: Option<String>,
    jpeg_quality: Option<u8>,
) -> Result<EmbedResult> {
    embed_file_with(input, output, key, reg, strength, channel, jpeg_quality, None)
}

/// Same as [`embed_file`], with `batch_id` written into the registry entry when it is set.
#[allow(clippy::too_many_arguments)]
pub fn embed_file_with(
    input: &Path,
    output: &Path,
    key: &[u8],
    reg: &mut Registry,
    strength: f32,
    channel: Option<String>,
    jpeg_quality: Option<u8>,
    batch_id: Option<&str>,
) -> Result<EmbedResult> {
    let original = load_image(input)?;
    let file = input.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let id = reg.new_id()?;
    let mut marked = original.clone();
    let psnr = wm::embed_rgb(&mut marked.data, marked.w, marked.h, key, id, strength);
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    save_image(&marked, output, jpeg_quality)?;
    let sha256_file = {
        use sha2::Digest as _;
        hex::encode(sha2::Sha256::digest(std::fs::read(output)?))
    };
    let check = match check_embedded(output, key, id) {
        Ok(check) => check,
        Err(e) => {
            let _ = std::fs::remove_file(output);
            return Err(e.context("could not read back the saved file"));
        }
    };
    if check == EmbedCheck::Unreadable {
        let _ = std::fs::remove_file(output);
        return Err(anyhow::Error::new(WatermarkNotReadable { file }));
    }
    let meta = registry::CopyMeta {
        sha256_file: Some(sha256_file.clone()),
        batch_id: batch_id.map(str::to_string),
        ..Default::default()
    };
    reg.register_full(
        id,
        &file,
        &original,
        &marked,
        channel,
        strength,
        Some(output.display().to_string()),
        Some(&meta),
    )?;
    Ok(EmbedResult { id, psnr, check, sha256_file })
}

/// `dir/stem{suffix}.{ext}`, adding 2, 3, ... until the name is free.
pub fn unique_output_path(dir: &Path, input: &Path, suffix: &str, jpeg: bool) -> std::path::PathBuf {
    let stem = input.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let ext = if jpeg { "jpg" } else { "png" };
    let mut out = dir.join(format!("{stem}{suffix}.{ext}"));
    let mut n = 2;
    while out.exists() {
        out = dir.join(format!("{stem}{suffix}{n}.{ext}"));
        n += 1;
    }
    out
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Report {
    #[serde(flatten)]
    pub det: wm::Detection,
    pub id_hex: Option<String>,
    pub content_similarity: Option<f64>,
    pub content_overlap: Option<f64>,
    /// "feature-homography" when the perspective was undone using the registered thumbnail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geometry: Option<String>,
    /// Registered picture whose content was found in the image (feature matching).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub located_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub located_inliers: Option<usize>,
}

/// Which detection stages to run.
#[derive(Debug, Clone, Copy)]
pub struct DetectOptions {
    /// Try affine synchronisation (rotation / mirror / aspect / shear) when the fast path fails.
    pub affine: bool,
    /// Locate the registered picture by feature matching and undo perspective (screen photos).
    pub photo: bool,
    /// Compare the image with the registered thumbnail after a successful decode.
    pub content: bool,
    /// Sync-score threshold (standard 6.5, strict 7.5).
    pub z_thresh: f64,
}

impl Default for DetectOptions {
    fn default() -> Self {
        DetectOptions { affine: true, photo: true, content: true, z_thresh: wm::Z_THRESH }
    }
}

/// Blind detection first; if that fails and a registry is available, locate the registered
/// picture in the image by feature matching, undo the perspective and read the watermark.
pub fn detect_image(
    img: &RgbImageBuf,
    key: &[u8],
    reg: Option<&Registry>,
    known: Option<&wm::KnownIds>,
    thumbs: Option<&registry::ThumbIndex>,
) -> Report {
    detect_image_opts(img, key, reg, known, thumbs, &DetectOptions::default())
}

pub fn detect_image_opts(
    img: &RgbImageBuf,
    key: &[u8],
    reg: Option<&Registry>,
    known: Option<&wm::KnownIds>,
    thumbs: Option<&registry::ThumbIndex>,
    opts: &DetectOptions,
) -> Report {
    let y = imgproc::luma(&img.data, img.w, img.h);
    let det = if opts.affine {
        wm::detect_luma(&y, key, known, opts.z_thresh)
    } else {
        wm::detect_axis(&y, key, known, opts.z_thresh)
    };
    let mut rep = Report {
        id_hex: det.image_id.map(|i| format!("0x{i:08x}")),
        det,
        content_similarity: None,
        content_overlap: None,
        geometry: None,
        located_id: None,
        located_inliers: None,
    };
    if let (Some(id), Some(r)) = (rep.det.image_id, reg) {
        if !opts.content {
            return rep;
        }
        if let Some(cm) = registry::content_match(r, id, img, &rep.det) {
            rep.content_similarity = Some(cm.similarity);
            rep.content_overlap = Some(cm.overlap);
        }
        return rep;
    }
    let (Some(r), Some(ti), true) = (reg, thumbs, opts.photo) else { return rep };
    let Some(loc) = registry::locate(r, ti, &y) else { return rep };
    rep.located_id = Some(loc.entry);
    rep.located_inliers = Some(loc.inliers);
    let Some(rect) = registry::rectify(r, &loc, &y) else { return rep };
    let d2 = wm::detect_axis(&rect, key, known, opts.z_thresh);
    if d2.found {
        rep.content_similarity = if opts.content { d2.image_id.and_then(|id| registry::rectified_similarity(r, id, &rect)) } else { None };
        rep.content_overlap = None;
        rep.id_hex = d2.image_id.map(|i| format!("0x{i:08x}"));
        rep.det = d2;
        rep.geometry = Some("feature-homography".into());
    }
    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("blindmark-lib-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn key() -> [u8; 32] {
        crate::keys::generate_key().unwrap()
    }

    /// 720×1600, almost all black. The bottom 60 rows carry noise so `too_flat` does not
    /// reject the picture before the watermark is considered.
    fn black_screenshot(path: &Path) {
        let (w, h) = (720usize, 1600usize);
        let mut data = vec![0u8; w * h * 3];
        let mut s: u32 = 0x9e37_79b9;
        for y in (h - 60)..h {
            for x in 0..w {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                let v = (s >> 24) as u8;
                let i = (y * w + x) * 3;
                data[i] = v;
                data[i + 1] = v;
                data[i + 2] = v;
            }
        }
        image::save_buffer(path, &data, w as u32, h as u32, image::ColorType::Rgb8).unwrap();
    }

    /// 800×600 gradient plus noise, never a flat colour.
    fn textured(path: &Path) {
        let (w, h) = (800usize, 600usize);
        let mut s: u32 = 0x1234_5678;
        let mut data = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                let n = (s >> 24) as f32 / 255.0 * 40.0;
                let v = 110.0 + 60.0 * ((x as f32 * 0.03).sin() * (y as f32 * 0.04).cos()) + n;
                let i = (y * w + x) * 3;
                for c in 0..3 {
                    data[i + c] = (v + c as f32 * 9.0).clamp(0.0, 255.0) as u8;
                }
            }
        }
        image::save_buffer(path, &data, w as u32, h as u32, image::ColorType::Rgb8).unwrap();
    }

    #[test]
    fn flat_screenshot_is_not_saved() {
        let base = tmp("flat");
        let input = base.join("black.png");
        black_screenshot(&input);
        let mut reg = Registry::open(&base.join("registry")).unwrap();
        let out = base.join("black_wm.png");
        let k = key();
        let err = embed_file(&input, &out, &k, &mut reg, wm::DEFAULT_STRENGTH, None, None).unwrap_err();
        assert!(err.downcast_ref::<WatermarkNotReadable>().is_some(), "{err:#}");
        assert!(!out.exists(), "a copy that cannot be read back must not stay on disk");
        assert_eq!(reg.entries.len(), 0, "nothing may be registered for an unreadable copy");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn textured_png_is_ok() {
        let base = tmp("textured-png");
        let input = base.join("textured.png");
        textured(&input);
        let mut reg = Registry::open(&base.join("registry")).unwrap();
        let out = base.join("textured_wm.png");
        let k = key();
        let r = embed_file(&input, &out, &k, &mut reg, wm::DEFAULT_STRENGTH, None, None).unwrap();
        assert_eq!(r.check, EmbedCheck::Ok);
        let bytes = std::fs::read(&out).unwrap();
        let sha = {
            use sha2::Digest as _;
            hex::encode(sha2::Sha256::digest(&bytes))
        };
        assert_eq!(r.sha256_file, sha, "sha256_file must hash the file on disk");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn textured_jpeg95_is_not_unreadable() {
        let base = tmp("textured-jpeg");
        let input = base.join("textured.png");
        textured(&input);
        let mut reg = Registry::open(&base.join("registry")).unwrap();
        let out = base.join("textured_wm.jpg");
        let k = key();
        let r = embed_file(&input, &out, &k, &mut reg, wm::DEFAULT_STRENGTH, None, Some(95)).unwrap();
        assert!(matches!(r.check, EmbedCheck::Ok | EmbedCheck::Weak), "{:?}", r.check);
        let _ = std::fs::remove_dir_all(&base);
    }
}
