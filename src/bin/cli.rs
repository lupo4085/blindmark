use anyhow::{bail, Context, Result};
use blindmark::jobs::{run_distribute, run_embed, DistEvent, DistJob, EmbedEvent, EmbedJob};
use blindmark::recipients::{Book, GroupFilter};
use blindmark::{keys, registry::Registry, wm, EmbedCheck};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "blindmark-cli", about = "Keyed blind image watermark (command line)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new secret key file
    Keygen {
        #[arg(long)]
        key: PathBuf,
    },
    Key {
        #[command(subcommand)]
        cmd: KeyCmd,
    },
    /// Watermark one or more images into an output folder
    Embed {
        inputs: Vec<PathBuf>,
        #[arg(long, short)]
        out: PathBuf,
        #[arg(long)]
        key: PathBuf,
        #[arg(long, default_value = "registry")]
        registry: PathBuf,
        #[arg(long, default_value_t = wm::DEFAULT_STRENGTH)]
        strength: f32,
        /// Free-text note stored with each ID (e.g. where you post it)
        #[arg(long)]
        channel: Option<String>,
        /// Write JPEG (quality 95) instead of PNG
        #[arg(long)]
        jpeg: bool,
        /// Distribute to these recipients (comma separated). One copy per recipient.
        #[arg(long)]
        to: Option<String>,
        /// Do not create a folder per recipient; add the name to the file name instead.
        #[arg(long)]
        flat: bool,
        /// Request an RFC 3161 timestamp for this batch (off by default).
        #[arg(long)]
        timestamp: bool,
        /// Time-stamp authority URL; only used together with --timestamp.
        #[arg(long, default_value = blindmark::timestamp::DEFAULT_TSA)]
        tsa: String,
    },
    /// Check images for your watermark
    Detect {
        inputs: Vec<PathBuf>,
        #[arg(long)]
        key: PathBuf,
        #[arg(long, default_value = "registry")]
        registry: PathBuf,
    },
    /// Manage the recipient book (stored next to the registry)
    Recipients {
        #[command(subcommand)]
        cmd: RecCmd,
    },
    /// Manage trusted timestamps of batches (watermarking or distribution)
    Stamp {
        #[command(subcommand)]
        cmd: StampCmd,
    },
}

#[derive(Subcommand)]
enum KeyCmd {
    Words {
        #[arg(long)]
        key: PathBuf,
    },
    Restore {
        #[arg(long)]
        key: PathBuf,
    },
}

#[derive(Subcommand)]
enum RecCmd {
    /// List recipients
    List {
        /// Only this group (a top-level group also lists its subgroups)
        #[arg(long)]
        group: Option<String>,
        /// Include archived recipients
        #[arg(long)]
        archived: bool,
        #[arg(long, default_value = "registry")]
        registry: PathBuf,
    },
    /// List groups in tree order (ID, then the full path of the group)
    Groups {
        #[arg(long, default_value = "registry")]
        registry: PathBuf,
    },
    /// Add a recipient
    Add {
        name: String,
        /// Group id
        #[arg(long)]
        group: Option<String>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long, default_value = "registry")]
        registry: PathBuf,
    },
    /// Edit a recipient
    Edit {
        id: String,
        #[arg(long)]
        name: Option<String>,
        /// Group id, or `none` to remove the recipient from its group
        #[arg(long)]
        group: Option<String>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long, default_value = "registry")]
        registry: PathBuf,
    },
    /// Archive a recipient; --undo restores it
    Archive {
        id: String,
        #[arg(long)]
        undo: bool,
        #[arg(long, default_value = "registry")]
        registry: PathBuf,
    },
    /// Add a group (two levels: --parent takes a group id)
    Group {
        name: String,
        #[arg(long)]
        parent: Option<String>,
        #[arg(long, default_value = "registry")]
        registry: PathBuf,
    },
    /// Rename a group
    RenameGroup {
        id: String,
        name: String,
        #[arg(long, default_value = "registry")]
        registry: PathBuf,
    },
}

#[derive(Subcommand)]
enum StampCmd {
    /// List batches and whether each timestamp was obtained
    List {
        #[arg(long, default_value = "registry")]
        registry: PathBuf,
    },
    /// Request the timestamp again for an existing batch
    Retry {
        batch_id: String,
        #[arg(long, default_value = "registry")]
        registry: PathBuf,
        #[arg(long, default_value = blindmark::timestamp::DEFAULT_TSA)]
        tsa: String,
    },
}

fn read_key(p: &PathBuf) -> Result<Vec<u8>> {
    keys::parse_key_file(&std::fs::read_to_string(p).with_context(|| format!("cannot read key {}", p.display()))?)
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| path.display().to_string())
}

/// Print the warning for a self-check result; returns 1 when the watermark is weak.
fn warn_check(name: &str, check: EmbedCheck) -> usize {
    match check {
        EmbedCheck::Ok => 0,
        EmbedCheck::Weak => {
            eprintln!("warning: {name}: watermark is weak and may not survive re-compression or screenshots.");
            1
        }
        EmbedCheck::Unreadable => 0,
    }
}

fn main() -> Result<()> {
    run(Cli::parse())
}

fn run(cli: Cli) -> Result<()> {
    match cli.cmd {
        Cmd::Keygen { key } => {
            anyhow::ensure!(!key.exists(), "{} exists; refusing to overwrite", key.display());
            let k = keys::generate_key()?;
            std::fs::write(&key, hex::encode(k) + "\n")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600))?;
            }
            println!("wrote {} (fingerprint {}) — back it up, never share it", key.display(), keys::fingerprint(&k));
        }
        Cmd::Key { cmd } => match cmd {
            KeyCmd::Words { key } => {
                let k = read_key(&key)?;
                let words = blindmark::jobs::key_words(&k).map_err(|e| anyhow::anyhow!(e.message))?;
                eprintln!("任何人拿到这 24 个词，就等于拿到了你的密钥。请抄在纸上，不要截图或发给别人。");
                for (i, w) in words.iter().enumerate() {
                    println!("{}. {}", i + 1, w);
                }
            }
            KeyCmd::Restore { key } => {
                let text = std::io::read_to_string(std::io::stdin())?;
                let k = blindmark::jobs::restore_key_file(&text, &key).map_err(|e| anyhow::anyhow!(e.message))?;
                println!("restored {} (fingerprint {})", key.display(), keys::fingerprint(&k));
            }
        },
        Cmd::Embed { inputs, out, key, registry, strength, channel, jpeg, to, flat, timestamp, tsa } => {
            let k = read_key(&key)?;
            let t = std::time::Instant::now();
            let tsa = timestamp.then(|| tsa.clone());

            if let Some(list) = to {
                let mut reg = Registry::open(&registry)?;
                reg.check_key(&k)?;
                drop(reg);
                let mut book = Book::open(&registry)?;
                let mut recipient_ids = Vec::new();
                for name in list.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    recipient_ids.push(book.recipient_by_name_or_create(name)?.id);
                }
                drop(book);
                let job = DistJob {
                    files: inputs,
                    recipient_ids,
                    out_root: out,
                    per_recipient_dir: !flat,
                    suffix: "_wm".into(),
                    strength,
                    jpeg_quality: jpeg.then_some(95),
                    tsa,
                };
                let cancel = std::sync::atomic::AtomicBool::new(false);
                let mut rows = Vec::new();
                let mut weak = 0usize;
                let summary = run_distribute(&job, &k, &registry, &cancel, &mut |ev| match ev {
                    DistEvent::Running { .. } | DistEvent::Stamping => {}
                    DistEvent::Item { recipient_id, recipient, result: Ok(ok), .. } => {
                        rows.push((ok, recipient_id, recipient))
                    }
                    DistEvent::Item { result: Err(e), .. } => eprintln!("{}", e.message),
                })
                .map_err(|e| anyhow::anyhow!("{}", e.message))?;

                for (ok, recipient_id, recipient) in rows {
                    let name = file_name(&ok.output);
                    weak += warn_check(&name, ok.check);
                    println!(
                        "{}",
                        serde_json::json!({
                            "id": format!("0x{:08x}", ok.id),
                            "recipient": recipient,
                            "recipient_id": recipient_id,
                            "output": ok.output,
                            "sha256_file": ok.sha256_file,
                            "check": ok.check,
                        })
                    );
                }
                if let Some(Err(e)) = &summary.stamp {
                    eprintln!("{}", e.message);
                }
                println!(
                    "{}",
                    serde_json::json!({
                        "batch_id": summary.batch_id,
                        "ok": summary.ok,
                        "failed": summary.failed,
                        "stamp": match &summary.stamp {
                            None => serde_json::Value::Null,
                            Some(Ok(_)) => serde_json::json!("ok"),
                            Some(Err(_)) => serde_json::json!("failed"),
                        },
                    })
                );
                if weak + summary.failed > 0 {
                    eprintln!("{weak} weak, {} failed", summary.failed);
                }
            } else {
                let job = EmbedJob {
                    files: inputs,
                    out_dir: Some(out),
                    suffix: "_wm".into(),
                    strength,
                    jpeg_quality: jpeg.then_some(95),
                    channel,
                    tsa,
                };
                let files = job.files.clone();
                let cancel = std::sync::atomic::AtomicBool::new(false);
                let mut weak = 0usize;
                let summary = run_embed(&job, &k, &registry, &cancel, &mut |ev| match ev {
                    EmbedEvent::Running { .. } | EmbedEvent::Stamping => {}
                    EmbedEvent::Item { index, result: Ok(ok) } => {
                        let name = file_name(&ok.output);
                        weak += warn_check(&name, ok.check);
                        let input = files.get(index).cloned().unwrap_or_default();
                        println!(
                            "{}",
                            serde_json::json!({
                                "input": input,
                                "output": ok.output,
                                "id": format!("0x{:08x}", ok.id),
                                "psnr_db": (ok.psnr * 100.0).round() / 100.0,
                                "check": ok.check,
                                "sha256_file": ok.sha256_file,
                                "sec": t.elapsed().as_secs_f64(),
                            })
                        );
                    }
                    EmbedEvent::Item { index, result: Err(e) } => {
                        let input = files.get(index).cloned().unwrap_or_default();
                        eprintln!("error: {}: {}", input.display(), e.detail);
                    }
                })
                .map_err(|e| anyhow::anyhow!("{}", e.message))?;

                if let Some(Err(e)) = &summary.stamp {
                    eprintln!("{}", e.message);
                }
                println!(
                    "{}",
                    serde_json::json!({
                        "batch_id": summary.batch_id,
                        "ok": summary.ok,
                        "failed": summary.failed,
                        "stamp": match &summary.stamp {
                            None => serde_json::Value::Null,
                            Some(Ok(_)) => serde_json::json!("ok"),
                            Some(Err(_)) => serde_json::json!("failed"),
                        },
                    })
                );
                if weak + summary.failed > 0 {
                    eprintln!("{weak} weak, {} failed", summary.failed);
                }
            }
        }
        Cmd::Detect { inputs, key, registry } => {
            let k = read_key(&key)?;
            let reg = Registry::open(&registry).ok();
            let known = reg.as_ref().map(|r| wm::KnownIds::new(&k, r.ids()));
            let thumbs = blindmark::registry::ThumbIndex::new();
            for inp in inputs {
                let t = std::time::Instant::now();
                let img = blindmark::registry::load_image(&inp)?;
                let rep = blindmark::detect_image(&img, &k, reg.as_ref(), known.as_ref(), Some(&thumbs));
                let mut v = serde_json::to_value(&rep)?;
                if let (Some(id), Some(r)) = (rep.det.image_id, reg.as_ref()) {
                    if let Some(e) = r.entries.get(&id.to_string()) {
                        v["recipient"] = serde_json::json!(e.recipient_name);
                        v["group"] = serde_json::json!(e.group_name);
                        v["batch"] = serde_json::json!(e.batch_id);
                        v["channel"] = serde_json::json!(e.channel);
                    }
                }
                v["input"] = serde_json::json!(inp);
                v["sec"] = serde_json::json!(t.elapsed().as_secs_f64());
                println!("{}", serde_json::to_string(&v)?);
            }
        }
        Cmd::Recipients { cmd } => match cmd {
            RecCmd::List { group, archived, registry } => {
                let book = Book::open(&registry)?;
                let filter = match group.as_deref() {
                    Some(id) => GroupFilter::Group(id),
                    None => GroupFilter::All,
                };
                for r in book.list(filter, archived) {
                    let g = r.group_id.as_deref().map(|g| book.group_path(g)).unwrap_or_else(|| "未分组".into());
                    let archived_mark = if r.archived { "\t已归档" } else { "" };
                    println!("{}\t{}\t{}\t{}{}", r.id, r.name, g, r.note, archived_mark);
                }
            }
            RecCmd::Groups { registry } => {
                let book = Book::open(&registry)?;
                for (top, subs) in book.groups_tree() {
                    println!("{}\t{}", top.id, book.group_path(&top.id));
                    for sub in subs {
                        println!("{}\t{}", sub.id, book.group_path(&sub.id));
                    }
                }
            }
            RecCmd::Add { name, group, note, registry } => {
                let mut book = Book::open(&registry)?;
                let r = book.add_recipient(&name, group.as_deref(), note.as_deref().unwrap_or(""))?;
                println!("{}", serde_json::json!({"id": r.id, "name": r.name, "group": r.group_id}));
            }
            RecCmd::Edit { id, name, group, note, registry } => {
                let mut book = Book::open(&registry)?;
                let cur = match book.recipient(&id) {
                    Some(r) => r.clone(),
                    None => bail!("找不到这位接收人，可能已经被修改，请重新打开。"),
                };
                let new_name = name.as_deref().unwrap_or(&cur.name);
                let new_group = match group.as_deref() {
                    Some("none") => None,
                    Some(g) => Some(g.to_string()),
                    None => cur.group_id.clone(),
                };
                let new_note = note.as_deref().unwrap_or(&cur.note);
                book.update_recipient(&id, new_name, new_group.as_deref(), new_note)?;
            }
            RecCmd::Archive { id, undo, registry } => {
                let mut book = Book::open(&registry)?;
                book.set_archived(&id, !undo)?;
            }
            RecCmd::Group { name, parent, registry } => {
                let mut book = Book::open(&registry)?;
                let g = book.add_group(&name, parent.as_deref())?;
                println!("{}", serde_json::json!({"id": g.id, "name": g.name, "parent": g.parent}));
            }
            RecCmd::RenameGroup { id, name, registry } => {
                let mut book = Book::open(&registry)?;
                book.rename_group(&id, &name)?;
            }
        },
        Cmd::Stamp { cmd } => match cmd {
            StampCmd::List { registry } => {
                for (batch, rec) in blindmark::timestamp::list_batches(&registry)? {
                    match rec {
                        Some(r) if r.ok => println!("{batch}\tok\t{}", r.gen_time_utc.as_deref().unwrap_or("")),
                        Some(r) => println!("{batch}\t未获取\t{}", stamp_error_kind(&r.error)),
                        None => println!("{batch}\t未获取"),
                    }
                }
            }
            StampCmd::Retry { batch_id, registry, tsa } => {
                let st = blindmark::timestamp::retry_batch(&registry, &batch_id, &tsa)?;
                if st.record.ok {
                    println!("{batch_id}\tok\t{}", st.record.gen_time_utc.as_deref().unwrap_or(""));
                } else {
                    println!("{batch_id}\t未获取\t{}", stamp_error_kind(&st.record.error));
                }
            }
        },
    }
    Ok(())
}

fn stamp_error_kind(error: &Option<blindmark::timestamp::StampError>) -> &'static str {
    use blindmark::timestamp::StampError::*;
    match error {
        Some(Network { .. }) => "network",
        Some(Rejected { .. }) => "rejected",
        Some(Mismatch { .. }) => "mismatch",
        Some(Malformed { .. }) => "malformed",
        None => "-",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("blindmark-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 512×512 procedural texture (never a flat colour), as PNG.
    fn textured_png(path: &std::path::Path) {
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

    /// The second run must not overwrite the first output.
    #[test]
    fn embedding_twice_keeps_both_outputs() {
        let base = tmp("twice");
        let input = base.join("a.png");
        textured_png(&input);
        let key_path = base.join("blindmark.key");
        let k = keys::generate_key().unwrap();
        std::fs::write(&key_path, hex::encode(k) + "\n").unwrap();
        let out = base.join("out");
        let embed = || Cmd::Embed {
            inputs: vec![input.clone()],
            out: out.clone(),
            key: key_path.clone(),
            registry: base.join("registry"),
            strength: wm::DEFAULT_STRENGTH,
            channel: None,
            jpeg: false,
            to: None,
            flat: false,
            timestamp: false,
            tsa: blindmark::timestamp::DEFAULT_TSA.into(),
        };

        run(Cli { cmd: embed() }).unwrap();
        let first = out.join("a_wm.png");
        assert!(first.exists(), "the first output must be a_wm.png");
        let before = std::fs::read(&first).unwrap();

        run(Cli { cmd: embed() }).unwrap();
        let second = out.join("a_wm2.png");
        assert!(second.exists(), "the second output must get its own name");
        assert_ne!(before, std::fs::read(&second).unwrap(), "the two outputs must differ");
        assert_eq!(before, std::fs::read(&first).unwrap(), "the first output must not change");
        let _ = std::fs::remove_dir_all(&base);
    }
}
