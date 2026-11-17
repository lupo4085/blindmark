//! Background jobs shared by every front-end: embedding and detection.
//!
//! This module has no Win32 dependency. It is the single implementation of the two
//! background tasks; the main window calls it from its worker thread and the wizard will
//! call it too once it is rewritten. Errors are returned as [`UserError`], whose `message`
//! follows `docs/specs/ui-copy.md` section 2 and whose `detail` keeps the original error.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::distribute::{self, DistOpts, DistProgress, Target};
use crate::keys;
use crate::recipients::Book;
use crate::registry::{Entry, Registry, RgbImageBuf, ThumbIndex};
use crate::timestamp;
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

/// Read-only view of a registry folder for the wizard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryStatus {
    New,
    Matches { entries: usize },
    Mismatch { fingerprint: String },
}

/// Read-only probe: never creates a folder and never writes a file.
pub fn inspect_registry(dir: &Path, key: Option<&[u8]>) -> Result<RegistryStatus, UserError> {
    let db = dir.join("registry.json");
    if !db.exists() {
        return Ok(RegistryStatus::New);
    }
    let parse_error = |e: anyhow::Error| UserError {
        message: format!("无法打开登记表 {}。请检查这个文件夹是否存在、是否可以写入。", dir.display()),
        detail: format!("{e:#}"),
    };
    let text = std::fs::read_to_string(&db).map_err(|e| parse_error(e.into()))?;
    let entries: std::collections::BTreeMap<String, Entry> = serde_json::from_str(&text).map_err(|e| parse_error(e.into()))?;
    if let Some(key) = key {
        let mp = dir.join("meta.json");
        if mp.exists() {
            let meta_text = std::fs::read_to_string(&mp).map_err(|e| parse_error(e.into()))?;
            let meta: crate::registry::Meta = serde_json::from_str(&meta_text).map_err(|e| parse_error(e.into()))?;
            let fp = keys::fingerprint(key);
            if meta.key_fingerprint != fp {
                return Ok(RegistryStatus::Mismatch { fingerprint: meta.key_fingerprint });
            }
        }
    }
    Ok(RegistryStatus::Matches { entries: entries.len() })
}

fn key_create_message(path: &Path) -> String {
    let dir = path.parent().unwrap_or(path);
    format!("无法在 {} 创建密钥。请换一个位置。", dir.display())
}

/// Create a key file. Refuses to overwrite an existing file; returns the new key.
pub fn create_key_file(path: &Path) -> Result<Vec<u8>, UserError> {
    let fail = |e: anyhow::Error| UserError { message: key_create_message(path), detail: format!("{e:#}") };
    if path.exists() {
        return Err(UserError { message: key_create_message(path), detail: format!("{} already exists", path.display()) });
    }
    let key = keys::generate_key().map_err(|e| fail(e.into()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| fail(e.into()))?;
    }
    std::fs::write(path, hex::encode(key) + "\n").map_err(|e| fail(e.into()))?;
    Ok(key.to_vec())
}

/// Read and validate a key file. The error message starts with the file name.
pub fn read_key_file(path: &Path) -> Result<Vec<u8>, UserError> {
    let name = file_name(path);
    let invalid = |e: anyhow::Error| UserError {
        message: format!("{name} 不是 BlindMark 的密钥文件。"),
        detail: format!("{e:#}"),
    };
    let text = std::fs::read_to_string(path).map_err(|e| invalid(e.into()))?;
    keys::parse_key_file(&text).map_err(|e| invalid(e))
}

/// The 24-word mnemonic of a key, or the user-facing reason it has none.
pub fn key_words(key: &[u8]) -> Result<Vec<String>, UserError> {
    keys::key_to_words(key)
        .map_err(|e| UserError { message: words_error_message(&e), detail: format!("{e:?}") })
}

/// Write `key` to `path`. An existing file is never overwritten: the same key is a
/// no-op, another key is an error.
pub fn write_key_bytes(key: &[u8; 32], path: &Path) -> Result<Vec<u8>, UserError> {
    if path.exists() {
        let existing = read_key_file(path)?;
        let fp = keys::fingerprint(&existing);
        if fp == keys::fingerprint(key) {
            return Ok(existing);
        }
        return Err(UserError {
            message: format!("这里已经有另一把密钥（密钥指纹 {fp}），不会覆盖它。请换一个位置。"),
            detail: format!("{} already exists with key fingerprint {fp}", path.display()),
        });
    }
    let fail = |e: anyhow::Error| UserError { message: key_create_message(path), detail: format!("{e:#}") };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| fail(e.into()))?;
    }
    std::fs::write(path, hex::encode(key) + "\n").map_err(|e| fail(e.into()))?;
    Ok(key.to_vec())
}

/// Recover a key from mnemonic text and write it to `path`.
pub fn restore_key_file(text: &str, path: &Path) -> Result<Vec<u8>, UserError> {
    let key = keys::key_from_words(text)
        .map_err(|e| UserError { message: words_error_message(&e), detail: format!("{e:?}") })?;
    write_key_bytes(&key, path)
}

/// The single place where a [`keys::WordsError`] becomes Chinese copy.
pub fn words_error_message(e: &keys::WordsError) -> String {
    match e {
        keys::WordsError::WrongLength(_) => "这把密钥不是标准长度，不能生成助记词。请直接备份密钥文件。".into(),
        keys::WordsError::WordCount(n) => format!("需要 24 个词，现在是 {n} 个。"),
        keys::WordsError::UnknownWord(i, w) => format!("第 {i} 个词“{w}”不在词表里，请检查拼写。"),
        keys::WordsError::Checksum => "这些词对不上校验，可能有一个词抄错了，或者顺序不对。请逐个核对。".into(),
    }
}

/// Copy the files in `src` into `dst`, returning how many were copied.
pub fn copy_dir(src: &Path, dst: &Path) -> Result<usize, UserError> {
    let fail = |e: anyhow::Error| UserError {
        message: format!("无法保存到 {}。请换一个文件夹，或检查这个文件夹是否只读。", dst.display()),
        detail: format!("{e:#}"),
    };
    std::fs::create_dir_all(dst).map_err(|e| fail(e.into()))?;
    let mut n = 0;
    for entry in std::fs::read_dir(src).map_err(|e| fail(e.into()))? {
        let entry = entry.map_err(|e| fail(e.into()))?;
        if entry.file_type().map_err(|e| fail(e.into()))?.is_file() {
            std::fs::copy(entry.path(), dst.join(entry.file_name())).map_err(|e| fail(e.into()))?;
            n += 1;
        }
    }
    Ok(n)
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
        if cause.downcast_ref::<distribute::UnreadableImage>().is_some() {
            return UserError {
                message: format!("无法打开 {name}：文件可能已损坏，或者不是支持的图片格式。"),
                detail,
            };
        }
        if let Some(ie) = cause.downcast_ref::<image::ImageError>() {
            if matches!(ie, image::ImageError::Decoding(_) | image::ImageError::Unsupported(_)) {
                return UserError {
                    message: format!("无法打开 {name}：文件可能已损坏，或者不是支持的图片格式。"),
                    detail,
                };
            }
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            if matches!(
                io.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem | std::io::ErrorKind::StorageFull
            ) {
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
        let name = file_name(path);
        let out_dir = job.out_dir.clone().unwrap_or_else(|| path.parent().map(Path::to_path_buf).unwrap_or_default());
        let output = crate::unique_output_path(&out_dir, path, &job.suffix, job.jpeg_quality.is_some());
        let result = match std::fs::File::open(path) {
            Ok(_) => crate::embed_file(path, &output, key, &mut reg, job.strength, job.channel.clone(), job.jpeg_quality)
                .map(|r| EmbedOk { id: r.id, psnr: r.psnr, output: output.clone() })
                .map_err(|e| classify(&e, &name, &out_dir)),
            Err(e) => Err(UserError {
                message: format!("无法打开 {name}：文件可能已损坏，或者不是支持的图片格式。"),
                detail: format!("{e:#}"),
            }),
        };
        if result.is_ok() {
            summary.ok += 1;
        } else {
            summary.failed += 1;
        }
        on_event(EmbedEvent::Item { index, result });
    }
    Ok(summary)
}

#[derive(Clone, Debug)]
pub struct DistJob {
    pub files: Vec<PathBuf>,
    pub recipient_ids: Vec<String>,
    pub out_root: PathBuf,
    pub per_recipient_dir: bool,
    pub suffix: String,
    pub strength: f32,
    pub jpeg_quality: Option<u8>,
    pub tsa: Option<String>,
}

#[derive(Clone, Debug)]
pub enum DistEvent {
    Running { done: usize, total: usize, file: String, recipient: String },
    Item { file_index: usize, recipient_id: String, recipient: String, result: Result<DistItemOk, UserError> },
    /// Sent once, right before the trusted-timestamp request starts.
    Stamping,
}

#[derive(Clone, Debug)]
pub struct DistItemOk {
    pub id: u32,
    pub output: PathBuf,
    pub sha256_file: String,
}

#[derive(Clone, Debug)]
pub struct DistSummary {
    pub batch_id: String,
    pub ok: usize,
    pub failed: usize,
    pub cancelled: bool,
    /// Recipients with at least one successful copy.
    pub recipients: usize,
    pub stamp: Option<Result<StampOk, UserError>>,
}

#[derive(Clone, Debug)]
pub struct StampOk {
    pub gen_time_utc: String,
}

/// A timestamp that was not obtained, in the wording from the task card.
fn stamp_user_error(error: Option<&timestamp::StampError>) -> UserError {
    match error {
        Some(timestamp::StampError::Network { host, detail }) => UserError {
            message: format!("暂时没能获取时间戳（无法连接 {host}）。副本已经生成，可以稍后重试。"),
            detail: detail.clone(),
        },
        Some(timestamp::StampError::Rejected { status }) => UserError {
            message: format!("时间戳服务拒绝了这次请求（状态 {status}）。副本已经生成，可以稍后重试。"),
            detail: format!("status {status}"),
        },
        Some(timestamp::StampError::Mismatch { detail }) | Some(timestamp::StampError::Malformed { detail }) => UserError {
            message: "时间戳服务返回的数据和这次请求对不上，已作废。副本已经生成，可以稍后重试。".into(),
            detail: detail.clone(),
        },
        None => UserError {
            message: "时间戳服务返回的数据和这次请求对不上，已作废。副本已经生成，可以稍后重试。".into(),
            detail: "no error was recorded".into(),
        },
    }
}

fn stamp_ok(stamp: timestamp::Stamp) -> Result<StampOk, UserError> {
    if stamp.record.ok {
        Ok(StampOk { gen_time_utc: stamp.record.gen_time_utc.clone().unwrap_or_default() })
    } else {
        Err(stamp_user_error(stamp.record.error.as_ref()))
    }
}

/// File errors while writing the manifest; the card gives no wording for this case.
fn stamp_file_error(e: anyhow::Error) -> UserError {
    UserError { message: "处理这批副本时出了问题。点“详细信息”可以查看原因。".into(), detail: format!("{e:#}") }
}

/// Distribute `job.files` to `job.recipient_ids` through [`distribute::distribute`].
pub fn run_distribute(
    job: &DistJob,
    key: &[u8],
    registry_dir: &Path,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(DistEvent),
) -> Result<DistSummary, UserError> {
    if job.files.is_empty() {
        return Err(UserError { message: "请至少添加一张图片。".into(), detail: "no input files".into() });
    }
    if job.recipient_ids.is_empty() {
        return Err(UserError { message: "请至少选择一位接收人。".into(), detail: "no recipients".into() });
    }
    let mut reg = open_registry_for_write(registry_dir, key)?;
    let book = Book::open(registry_dir).map_err(|e| UserError {
        message: "接收人名单（recipients.json）打不开：文件可能已损坏。".into(),
        detail: format!("{e:#}"),
    })?;
    let mut targets = Vec::with_capacity(job.recipient_ids.len());
    for id in &job.recipient_ids {
        let r = book.recipient(id).filter(|r| !r.archived).ok_or_else(|| UserError {
            message: "有接收人已被修改或归档，请回到上一步重新选择。".into(),
            detail: format!("recipient {id} not found or archived"),
        })?;
        targets.push(Target {
            id: r.id.clone(),
            name: r.name.clone(),
            group_id: r.group_id.clone(),
            group_name: r.group_id.as_deref().map(|g| book.group_path(g)),
        });
    }

    let total = job.files.len() * targets.len();
    let batch_id = distribute::batch_id_now();
    let opts = DistOpts {
        root: job.out_root.clone(),
        per_recipient_dir: job.per_recipient_dir,
        suffix: job.suffix.clone(),
        jpeg_quality: job.jpeg_quality,
        strength: job.strength,
        batch_id: batch_id.clone(),
        tsa: job.tsa.clone(),
    };
    let mut done = 0usize;
    let outcome = distribute::distribute(&job.files, &targets, key, &mut reg, &opts, cancel, &mut |progress| match progress {
        DistProgress::Start { file_index, target } => on_event(DistEvent::Running {
            done,
            total,
            file: file_name(&job.files[file_index]),
            recipient: target.name.clone(),
        }),
        DistProgress::Done { file_index, target, result } => {
            done += 1;
            match result {
                Ok(copy) => on_event(DistEvent::Item {
                    file_index,
                    recipient_id: copy.recipient_id.clone(),
                    recipient: target.name.clone(),
                    result: Ok(DistItemOk {
                        id: copy.id,
                        output: copy.path.clone(),
                        sha256_file: copy.sha256_file.clone(),
                    }),
                }),
                Err(e) => on_event(DistEvent::Item {
                    file_index,
                    recipient_id: target.id.clone(),
                    recipient: target.name.clone(),
                    result: Err(classify(e, &file_name(&job.files[file_index]), &job.out_root)),
                }),
            }
        }
        DistProgress::Stamping => on_event(DistEvent::Stamping),
    });

    let mut recipients = std::collections::HashSet::new();
    for copy in &outcome.copies {
        recipients.insert(copy.recipient_id.as_str());
    }
    Ok(DistSummary {
        batch_id,
        ok: outcome.copies.len(),
        failed: outcome.failed.len(),
        cancelled: outcome.cancelled,
        recipients: recipients.len(),
        stamp: outcome.stamp.map(|stamp| stamp.map_err(stamp_file_error).and_then(stamp_ok)),
    })
}

/// Ask for the timestamp of an existing batch again.
pub fn retry_stamp(registry_dir: &Path, batch_id: &str, tsa_url: &str) -> Result<StampOk, UserError> {
    let stamp = timestamp::retry_batch(registry_dir, batch_id, tsa_url).map_err(stamp_file_error)?;
    stamp_ok(stamp)
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
        let reg = crate::registry::Registry::open(&regdir).unwrap();
        assert_eq!(reg.entries.len(), 2);
        assert_eq!(outputs.len(), 2);
        assert!(outputs.iter().all(|p| p.exists()));
        for entry in reg.entries.values() {
            let bytes = std::fs::read(entry.output.as_ref().unwrap()).unwrap();
            let sha = {
                use sha2::Digest as _;
                hex::encode(sha2::Sha256::digest(&bytes))
            };
            assert_eq!(entry.sha256_file.as_deref(), Some(sha.as_str()), "sha256_file must hash the file on disk");
        }
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
    fn missing_input_is_reported() {
        let base = tmp("missing-input");
        let key = key();
        let regdir = base.join("registry");
        let job = job(vec![base.join("in").join("nope.png")], Some(base.join("out")), None);
        let cancel = AtomicBool::new(false);
        let mut results = vec![];
        let summary = run_embed(&job, &key, &regdir, &cancel, &mut |ev| {
            if let EmbedEvent::Item { index, result } = ev {
                results.push((index, result));
            }
        })
        .unwrap();
        assert_eq!(summary.ok, 0);
        assert_eq!(summary.failed, 1);
        assert_eq!(results.len(), 1);
        match &results[0].1 {
            Err(e) => assert!(e.message.starts_with("无法打开 nope.png"), "{}", e.message),
            Ok(_) => panic!("a missing input must fail"),
        }
    }

    #[test]
    fn inspect_new_dir_creates_nothing() {
        let base = tmp("inspect-new");
        let dir = base.join("missing");
        assert_eq!(inspect_registry(&dir, None).unwrap(), RegistryStatus::New);
        assert!(!dir.exists());
    }

    #[test]
    fn inspect_detects_mismatch() {
        let base = tmp("inspect-mismatch");
        let dir = base.join("registry");
        let key_a = key();
        let key_b = key();
        {
            let mut reg = crate::registry::Registry::open(&dir).unwrap();
            reg.check_key(&key_a).unwrap();
        }
        std::fs::write(dir.join("registry.json"), "{}").unwrap();
        match inspect_registry(&dir, Some(&key_b)).unwrap() {
            RegistryStatus::Mismatch { fingerprint } => assert_eq!(fingerprint, crate::keys::fingerprint(&key_a)),
            other => panic!("expected mismatch, got {other:?}"),
        }
    }

    #[test]
    fn create_key_refuses_overwrite() {
        let base = tmp("create-key");
        let path = base.join("nested").join("blindmark.key");
        let key = create_key_file(&path).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(key.len(), 32);
        let err = create_key_file(&path).unwrap_err();
        assert!(err.message.contains("创建密钥"), "{}", err.message);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
    }

    #[test]
    fn read_key_rejects_garbage() {
        let base = tmp("read-key");
        let path = base.join("bad.key");
        std::fs::write(&path, "not a key").unwrap();
        let err = read_key_file(&path).unwrap_err();
        assert!(err.message.starts_with("bad.key"), "{}", err.message);
    }

    #[test]
    fn restore_same_key_leaves_file_alone() {
        let base = tmp("restore-same");
        let path = base.join("blindmark.key");
        let key = create_key_file(&path).unwrap();
        let fixed: [u8; 32] = key.as_slice().try_into().unwrap();
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        let words = key_words(&fixed).unwrap();
        let restored = restore_key_file(&words.join("\n"), &path).unwrap();
        assert_eq!(restored, key);
        let after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(before, after, "an existing file with the same key must not be written again");
    }

    #[test]
    fn restore_refuses_another_key() {
        let base = tmp("restore-other");
        let path = base.join("blindmark.key");
        let original = create_key_file(&path).unwrap();
        let other = key().to_vec();
        let words = key_words(&other).unwrap();
        let err = restore_key_file(&words.join(" "), &path).unwrap_err();
        assert!(err.message.starts_with("这里已经有另一把密钥（密钥指纹 "), "{}", err.message);
        assert!(err.message.ends_with("），不会覆盖它。请换一个位置。"), "{}", err.message);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), hex::encode(&original) + "\n");
    }

    #[test]
    fn words_errors_use_the_card_copy() {
        assert_eq!(
            words_error_message(&keys::WordsError::WrongLength(16)),
            "这把密钥不是标准长度，不能生成助记词。请直接备份密钥文件。"
        );
        assert_eq!(words_error_message(&keys::WordsError::WordCount(23)), "需要 24 个词，现在是 23 个。");
        assert_eq!(
            words_error_message(&keys::WordsError::UnknownWord(1, "abandom".into())),
            "第 1 个词“abandom”不在词表里，请检查拼写。"
        );
        assert_eq!(
            words_error_message(&keys::WordsError::Checksum),
            "这些词对不上校验，可能有一个词抄错了，或者顺序不对。请逐个核对。"
        );
        let bad = [0u8; 16];
        assert_eq!(key_words(&bad).unwrap_err().message, "这把密钥不是标准长度，不能生成助记词。请直接备份密钥文件。");
    }

    #[test]
    fn copy_dir_copies_files() {
        let base = tmp("copy-dir");
        let src = base.join("src");
        let dst = base.join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.json"), b"a").unwrap();
        std::fs::write(src.join("b.png"), b"b").unwrap();
        assert_eq!(copy_dir(&src, &dst).unwrap(), 2);
        assert_eq!(std::fs::read(dst.join("a.json")).unwrap(), b"a");
        assert_eq!(std::fs::read(dst.join("b.png")).unwrap(), b"b");
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

    fn book_with(dir: &Path, names: &[&str]) -> Vec<String> {
        let mut book = Book::open(dir).unwrap();
        names.iter().map(|n| book.add_recipient(n, None, "").unwrap().id).collect()
    }

    fn dist_job(files: Vec<PathBuf>, ids: Vec<String>, out: PathBuf) -> DistJob {
        DistJob {
            files,
            recipient_ids: ids,
            out_root: out,
            per_recipient_dir: true,
            suffix: "_wm".into(),
            strength: crate::wm::DEFAULT_STRENGTH,
            jpeg_quality: None,
            tsa: None,
        }
    }

    #[test]
    fn distribute_events_and_summary() {
        let base = tmp("distribute");
        let regdir = base.join("registry");
        let ids = book_with(&regdir, &["张三", "李四"]);
        let input = base.join("in");
        std::fs::create_dir_all(&input).unwrap();
        let a = input.join("a.png");
        textured_png(&a);
        let job = dist_job(vec![a], ids, base.join("out"));
        let cancel = AtomicBool::new(false);
        let mut running = vec![];
        let mut items = 0;
        let mut stamping = 0;
        let summary = run_distribute(&job, &key(), &regdir, &cancel, &mut |ev| match ev {
            DistEvent::Running { done, total, .. } => running.push((done, total)),
            DistEvent::Item { result, .. } => {
                assert!(result.is_ok());
                items += 1;
            }
            DistEvent::Stamping => stamping += 1,
        })
        .unwrap();
        assert_eq!(running, vec![(0, 2), (1, 2)]);
        assert_eq!(items, 2);
        assert_eq!(stamping, 0);
        assert_eq!((summary.ok, summary.failed, summary.recipients), (2, 0, 2));
        assert!(summary.batch_id.starts_with("b_"));
        assert!(summary.stamp.is_none());
    }

    #[test]
    fn distribute_bad_image_says_it_cannot_be_opened() {
        let base = tmp("distribute-bad-image");
        let regdir = base.join("registry");
        let ids = book_with(&regdir, &["张三", "李四", "王五"]);
        let input = base.join("in");
        std::fs::create_dir_all(&input).unwrap();
        let bad = input.join("bad.png");
        std::fs::write(&bad, b"not a png").unwrap();
        let job = dist_job(vec![bad], ids, base.join("out"));
        let cancel = AtomicBool::new(false);
        let mut messages = vec![];
        let summary = run_distribute(&job, &key(), &regdir, &cancel, &mut |ev| {
            if let DistEvent::Item { result: Err(e), .. } = ev {
                messages.push(e.message);
            }
        })
        .unwrap();
        assert_eq!(summary.failed, 3);
        assert_eq!(messages.len(), 3);
        assert!(
            messages.iter().all(|m| m.starts_with("无法打开 bad.png：文件可能已损坏，或者不是支持的图片格式。")),
            "{messages:?}"
        );
    }

    #[test]
    fn distribute_rejects_empty_input() {
        let base = tmp("distribute-empty");
        let cancel = AtomicBool::new(false);
        let job = dist_job(vec![], vec!["r_x".into()], base.join("out"));
        let err = run_distribute(&job, &key(), &base.join("registry"), &cancel, &mut |_| {}).unwrap_err();
        assert_eq!(err.message, "请至少添加一张图片。");
        let job = dist_job(vec![base.join("a.png")], vec![], base.join("out"));
        let err = run_distribute(&job, &key(), &base.join("registry"), &cancel, &mut |_| {}).unwrap_err();
        assert_eq!(err.message, "请至少选择一位接收人。");
    }

    #[test]
    fn distribute_rejects_missing_or_archived_recipient() {
        let base = tmp("distribute-recipient");
        let regdir = base.join("registry");
        let ids = book_with(&regdir, &["张三"]);
        {
            let mut book = Book::open(&regdir).unwrap();
            book.set_archived(&ids[0], true).unwrap();
        }
        let input = base.join("in");
        std::fs::create_dir_all(&input).unwrap();
        let a = input.join("a.png");
        textured_png(&a);
        let cancel = AtomicBool::new(false);
        let key = key();
        for id in [ids[0].clone(), "r_00000000".into()] {
            let job = dist_job(vec![a.clone()], vec![id], base.join("out"));
            let err = run_distribute(&job, &key, &regdir, &cancel, &mut |_| {}).unwrap_err();
            assert_eq!(err.message, "有接收人已被修改或归档，请回到上一步重新选择。");
        }
    }
}
