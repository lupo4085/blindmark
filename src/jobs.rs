//! Background jobs shared by every front-end: embedding and detection.
//!
//! This module has no Win32 dependency. It is the single implementation of the two
//! background tasks; the main window calls it from its worker thread and the wizard will
//! call it too once it is rewritten. Errors are returned as [`UserError`], whose `message`
//! follows `docs/specs/ui-copy.md` section 2 and whose `detail` keeps the original error.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::keys;
use crate::registry::{Entry, Registry, RgbImageBuf, ThumbIndex};
use crate::wm;

/// A failure as the user should see it: one Chinese sentence plus the raw error.
#[derive(Clone, Debug)]
pub struct UserError {
    /// Chinese one-liner, copied from `docs/specs/ui-copy.md` section 2.
    pub message: String,
    /// Original error (`format!("{e:#}")`), for the log or a details pane.
    pub detail: String,
}

impl std::fmt::Display for UserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

fn registry_mismatch(registry_fingerprint: &str) -> UserError {
    UserError {
        message: format!(
            "这个登记表属于另一把密钥（密钥指纹 {registry_fingerprint}），不能和当前密钥一起使用。请换一个登记表文件夹，或改用对应的密钥。"
        ),
        detail: format!("registry key fingerprint: {registry_fingerprint}"),
    }
}

fn registry_unavailable(dir: &Path, e: &anyhow::Error) -> UserError {
    UserError {
        message: format!("无法打开登记表 {}。请检查这个文件夹是否存在、是否可以写入。", dir.display()),
        detail: format!("{e:#}"),
    }
}

/// Open a registry for writing. The key fingerprint is compared first; a mismatch is an
/// error and nothing is written. A registry without `meta.json` is initialised here.
pub fn open_registry_for_write(dir: &Path, key: &[u8]) -> Result<Registry, UserError> {
    let mut reg = Registry::open(dir).map_err(|e| registry_unavailable(dir, &e))?;
    let fp = keys::fingerprint(key);
    let mismatch = reg.meta.as_ref().filter(|m| m.key_fingerprint != fp).map(|m| m.key_fingerprint.clone());
    if let Some(registry_fingerprint) = mismatch {
        return Err(registry_mismatch(&registry_fingerprint));
    }
    if reg.meta.is_none() {
        reg.check_key(key).map_err(|e| registry_unavailable(dir, &e))?;
    }
    Ok(reg)
}

#[derive(Clone, Debug)]
pub struct EmbedJob {
    pub files: Vec<PathBuf>,
    /// `None` writes next to the source image.
    pub out_dir: Option<PathBuf>,
    pub suffix: String,
    pub strength: f32,
    /// `None` means PNG output.
    pub jpeg_quality: Option<u8>,
    pub channel: Option<String>,
}

#[derive(Clone, Debug)]
pub struct EmbedOk {
    pub id: u32,
    pub psnr: f64,
    pub output: PathBuf,
}

#[derive(Clone, Debug)]
pub enum EmbedEvent {
    Running { index: usize },
    Item { index: usize, result: Result<EmbedOk, UserError> },
}

#[derive(Clone, Debug, Default)]
pub struct EmbedSummary {
    pub ok: usize,
    pub failed: usize,
    pub cancelled: bool,
}

/// Classify a single-image failure into the copy from `ui-copy.md` section 2.
fn classify(e: &anyhow::Error, name: &str, out_dir: &Path) -> UserError {
    let detail = format!("{e:#}");
    for cause in e.chain() {
        if let Some(ie) = cause.downcast_ref::<image::ImageError>() {
            if matches!(ie, image::ImageError::Decoding(_) | image::ImageError::Unsupported(_)) {
                return UserError {
                    message: format!("无法打开 {name}：文件可能已损坏，或者不是支持的图片格式。"),
                    detail,
                };
            }
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            if io.kind() == std::io::ErrorKind::PermissionDenied || !matches!(io.kind(), std::io::ErrorKind::NotFound) {
                return UserError {
                    message: format!("无法保存到 {}。请换一个文件夹，或检查这个文件夹是否只读。", out_dir.display()),
                    detail,
                };
            }
        }
    }
    UserError {
        message: format!("处理 {name} 时出了问题。点“详细信息”可以查看原因。"),
        detail,
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

/// Process `job.files` in order. Only a registry that cannot be opened or does not belong
/// to `key` returns `Err`, and in that case no image is processed. `cancel` is checked
/// before every image.
pub fn run_embed(
    job: &EmbedJob,
    key: &[u8],
    registry_dir: &Path,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(EmbedEvent),
) -> Result<EmbedSummary, UserError> {
    let mut reg = open_registry_for_write(registry_dir, key)?;
    let mut summary = EmbedSummary::default();
    for (index, path) in job.files.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            summary.cancelled = true;
            break;
        }
        on_event(EmbedEvent::Running { index });
        let out_dir = job.out_dir.clone().unwrap_or_else(|| path.parent().map(Path::to_path_buf).unwrap_or_default());
        let output = crate::unique_output_path(&out_dir, path, &job.suffix, job.jpeg_quality.is_some());
        let result = crate::embed_file(path, &output, key, &mut reg, job.strength, job.channel.clone(), job.jpeg_quality)
            .map(|r| EmbedOk { id: r.id, psnr: r.psnr, output: output.clone() })
            .map_err(|e| classify(&e, &file_name(path), &out_dir));
        if result.is_ok() {
            summary.ok += 1;
        } else {
            summary.failed += 1;
        }
        on_event(EmbedEvent::Item { index, result });
    }
    Ok(summary)
}

/// Registry and key material prepared for detection.
pub struct DetectContext {
    registry: Option<Registry>,
    known: Option<wm::KnownIds>,
}

pub struct DetectResult {
    pub report: crate::Report,
    /// The registry entry for the decoded ID (or the ID found by content matching).
    pub entry: Option<Entry>,
}

impl DetectContext {
    /// `registry_dir` of `None` means no registry is used. A registry that belongs to a
    /// different key is an error.
    pub fn open(registry_dir: Option<&Path>, key: &[u8]) -> Result<DetectContext, UserError> {
        let Some(dir) = registry_dir else {
            return Ok(DetectContext { registry: None, known: None });
        };
        let reg = Registry::open(dir).map_err(|e| registry_unavailable(dir, &e))?;
        let fp = keys::fingerprint(key);
        if let Some(meta) = &reg.meta {
            if meta.key_fingerprint != fp {
                return Err(registry_mismatch(&meta.key_fingerprint));
            }
        }
        let ids = reg.ids();
        let known = (!ids.is_empty()).then(|| wm::KnownIds::new(key, ids));
        Ok(DetectContext { registry: Some(reg), known })
    }

    pub fn registry(&self) -> Option<&Registry> {
        self.registry.as_ref()
    }

    /// Run detection and look up the decoded (or located) ID in the registry.
    pub fn detect(&self, img: &RgbImageBuf, key: &[u8], thumbs: Option<&ThumbIndex>, opts: &crate::DetectOptions) -> DetectResult {
        let report = crate::detect_image_opts(img, key, self.registry.as_ref(), self.known.as_ref(), thumbs, opts);
        let id = report.det.image_id.or(report.located_id);
        let entry = id.and_then(|i| self.registry.as_ref().and_then(|r| r.get(i)).cloned());
        DetectResult { report, entry }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("blindmark-jobs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 512×512 procedural texture (never a flat colour), as PNG.
    fn textured_png(path: &Path) {
        let (w, h) = (512usize, 512usize);
        let mut s: u32 = 0x1234_5678;
        let mut data = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                let n = (s >> 24) as f32 / 255.0 * 30.0;
                let v = 120.0 + 50.0 * ((x as f32 * 0.05).sin() * (y as f32 * 0.03).cos()) + n;
                let i = (y * w + x) * 3;
                for c in 0..3 {
                    data[i + c] = (v + c as f32 * 10.0).clamp(0.0, 255.0) as u8;
                }
            }
        }
        image::save_buffer(path, &data, w as u32, h as u32, image::ColorType::Rgb8).unwrap();
    }

    fn key() -> [u8; 32] {
        crate::keys::generate_key().unwrap()
    }

    fn job(files: Vec<PathBuf>, out_dir: Option<PathBuf>, channel: Option<&str>) -> EmbedJob {
        EmbedJob {
            files,
            out_dir,
            suffix: "_wm".into(),
            strength: crate::wm::DEFAULT_STRENGTH,
            jpeg_quality: None,
            channel: channel.map(str::to_string),
        }
    }

    #[test]
    fn embed_two_files() {
        let base = tmp("embed-two");
        let input = base.join("in");
        let out = base.join("out");
        std::fs::create_dir_all(&input).unwrap();
        let a = input.join("a.png");
        let b = input.join("b.png");
        textured_png(&a);
        textured_png(&b);
        let key = key();
        let regdir = base.join("registry");
        let job = job(vec![a, b], Some(out.clone()), Some("测试渠道"));
        let cancel = AtomicBool::new(false);
        let (mut running, mut items) = (0usize, 0usize);
        let mut outputs = vec![];
        let summary = run_embed(&job, &key, &regdir, &cancel, &mut |ev| match ev {
            EmbedEvent::Running { .. } => running += 1,
            EmbedEvent::Item { result, .. } => {
                items += 1;
                if let Ok(ok) = result {
                    outputs.push(ok.output);
                }
            }
        })
        .unwrap();
        assert_eq!(summary.ok, 2);
        assert_eq!(summary.failed, 0);
        assert_eq!((running, items), (2, 2));
        assert_eq!(crate::registry::Registry::open(&regdir).unwrap().entries.len(), 2);
        assert_eq!(outputs.len(), 2);
        assert!(outputs.iter().all(|p| p.exists()));
    }

    #[test]
    fn wrong_key_is_rejected() {
        let base = tmp("wrong-key");
        let input = base.join("in");
        let out = base.join("out");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let a = input.join("a.png");
        textured_png(&a);
        let key_a = key();
        let key_b = key();
        let regdir = base.join("registry");
        {
            let mut reg = crate::registry::Registry::open(&regdir).unwrap();
            reg.check_key(&key_a).unwrap();
        }
        let job = job(vec![a], Some(out.clone()), None);
        let cancel = AtomicBool::new(false);
        let err = run_embed(&job, &key_b, &regdir, &cancel, &mut |_| {}).unwrap_err();
        assert!(err.message.contains("另一把密钥"), "{}", err.message);
        assert_eq!(crate::registry::Registry::open(&regdir).unwrap().entries.len(), 0);
        let produced: Vec<_> = std::fs::read_dir(&out).unwrap().filter_map(|e| e.ok()).collect();
        assert!(produced.is_empty(), "output directory should stay empty");
    }

    #[test]
    fn cancel_before_start() {
        let base = tmp("cancel");
        let input = base.join("in");
        std::fs::create_dir_all(&input).unwrap();
        let a = input.join("a.png");
        textured_png(&a);
        let key = key();
        let regdir = base.join("registry");
        let job = job(vec![a], Some(base.join("out")), None);
        let cancel = AtomicBool::new(true);
        let mut items = 0;
        let summary = run_embed(&job, &key, &regdir, &cancel, &mut |ev| {
            if let EmbedEvent::Item { .. } = ev {
                items += 1;
            }
        })
        .unwrap();
        assert!(summary.cancelled);
        assert_eq!(summary.ok, 0);
        assert_eq!(items, 0);
    }

    #[test]
    fn bad_image_is_reported() {
        let base = tmp("bad-image");
        let input = base.join("in");
        std::fs::create_dir_all(&input).unwrap();
        let bad = input.join("x.png");
        std::fs::write(&bad, b"this is not a png").unwrap();
        let good = input.join("good.png");
        textured_png(&good);
        let key = key();
        let regdir = base.join("registry");
        let job = job(vec![bad, good], Some(base.join("out")), None);
        let cancel = AtomicBool::new(false);
        let mut results = vec![];
        let summary = run_embed(&job, &key, &regdir, &cancel, &mut |ev| {
            if let EmbedEvent::Item { index, result } = ev {
                results.push((index, result));
            }
        })
        .unwrap();
        assert_eq!(summary.ok, 1);
        assert_eq!(summary.failed, 1);
        results.sort_by_key(|(i, _)| *i);
        match &results[0].1 {
            Err(e) => assert!(e.message.starts_with("无法打开 x.png"), "{}", e.message),
            Ok(_) => panic!("a text file named x.png must fail"),
        }
        assert!(results[1].1.is_ok());
    }

    #[test]
    fn detect_finds_embedded() {
        let base = tmp("detect");
        let input = base.join("in");
        let out = base.join("out");
        std::fs::create_dir_all(&input).unwrap();
        let a = input.join("a.png");
        textured_png(&a);
        let key = key();
        let regdir = base.join("registry");
        let channel = "测试渠道";
        let job = job(vec![a], Some(out), Some(channel));
        let cancel = AtomicBool::new(false);
        let mut output = None;
        run_embed(&job, &key, &regdir, &cancel, &mut |ev| {
            if let EmbedEvent::Item { result: Ok(ok), .. } = ev {
                output = Some(ok.output);
            }
        })
        .unwrap();
        let output = output.expect("embedding produced no output");
        let img = crate::registry::load_image(&output).unwrap();
        let ctx = DetectContext::open(Some(regdir.as_path()), &key).unwrap();
        let result = ctx.detect(&img, &key, None, &crate::DetectOptions::default());
        assert!(result.report.det.found, "{:?}", result.report.det);
        let entry = result.entry.expect("decoded id should be in the registry");
        assert_eq!(entry.channel.as_deref(), Some(channel));
    }
}
