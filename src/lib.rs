//! blindmark — keyed, content-adaptive, screenshot-tolerant blind image watermark.
pub mod imgproc;
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct Report {
    #[serde(flatten)]
    pub det: wm::Detection,
    pub id_hex: Option<String>,
    pub content_similarity: Option<f64>,
    pub content_overlap: Option<f64>,
}

pub fn detect_image(img: &RgbImageBuf, key: &[u8], reg: Option<&Registry>, known: Option<&wm::KnownIds>) -> Report {
    let det = wm::detect_rgb(&img.data, img.w, img.h, key, known);
    let mut rep = Report { id_hex: det.image_id.map(|i| format!("0x{i:08x}")), det, content_similarity: None, content_overlap: None };
    if let (Some(id), Some(p), Some(r)) = (rep.det.image_id, rep.det.period_px, reg) {
        if let Some(cm) = registry::content_match(r, id, img, p) {
            rep.content_similarity = Some(cm.similarity);
            rep.content_overlap = Some(cm.overlap);
        }
    }
    rep
}
