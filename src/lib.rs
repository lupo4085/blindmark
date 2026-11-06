//! blindmark — keyed, content-adaptive, screenshot-tolerant blind image watermark.
pub mod affine;
pub mod features;
pub mod distribute;
pub mod imgproc;
pub mod recipients;
pub mod keys;
pub mod registry;
pub mod wm;

use anyhow::Result;
use registry::{load_image, save_image, Registry, RgbImageBuf};
use std::path::Path;

pub struct EmbedResult {
    pub id: u32,
    pub psnr: f64,
}

/// Load -> embed -> save -> register. Used by both the GUI and the CLI.
pub fn embed_file(
    input: &Path,
    output: &Path,
    key: &[u8],
    reg: &mut Registry,
    strength: f32,
    channel: Option<String>,
    jpeg_quality: Option<u8>,
) -> Result<EmbedResult> {
    let original = load_image(input)?;
    let id = reg.new_id()?;
    let mut marked = original.clone();
    let psnr = wm::embed_rgb(&mut marked.data, marked.w, marked.h, key, id, strength);
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    save_image(&marked, output, jpeg_quality)?;
    let src = input.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    reg.register(id, &src, &original, &marked, channel, strength, Some(output.display().to_string()))?;
    Ok(EmbedResult { id, psnr })
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
