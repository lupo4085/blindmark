use anyhow::{bail, Context, Result};
use blindmark::jobs::{run_distribute, DistEvent, DistJob};
use blindmark::recipients::{Book, GroupFilter};
use blindmark::{keys, registry::Registry, wm};
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
    /// Manage trusted timestamps of distribution batches
    Stamp {
        #[command(subcommand)]
        cmd: StampCmd,
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



fn main() -> Result<()> {
    match Cli::parse().cmd {
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
        Cmd::Embed { inputs, out, key, registry, strength, channel, jpeg, to, flat, timestamp, tsa } => {
            let k = read_key(&key)?;
            let mut reg = Registry::open(&registry)?;
            reg.check_key(&k)?;
            let t = std::time::Instant::now();

            if let Some(list) = to {
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
                    tsa: timestamp.then(|| tsa.clone()),
                };
                let cancel = std::sync::atomic::AtomicBool::new(false);
                let mut rows = Vec::new();
                let summary = run_distribute(&job, &k, &registry, &cancel, &mut |ev| match ev {
                    DistEvent::Running { .. } | DistEvent::Stamping => {}
                    DistEvent::Item { recipient_id, recipient, result: Ok(ok), .. } => {
                        rows.push((ok, recipient_id, recipient))
                    }
                    DistEvent::Item { result: Err(e), .. } => eprintln!("{}", e.message),
                })
                .map_err(|e| anyhow::anyhow!("{}", e.message))?;

                for (ok, recipient_id, recipient) in rows {
                    println!(
                        "{}",
                        serde_json::json!({
                            "id": format!("0x{:08x}", ok.id),
                            "recipient": recipient,
                            "recipient_id": recipient_id,
                            "output": ok.output,
                            "sha256_file": ok.sha256_file,
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
            } else {
                for inp in inputs {
                    let stem = inp.file_stem().unwrap_or_default().to_string_lossy();
                    let o = out.join(format!("{stem}_wm.{}", if jpeg { "jpg" } else { "png" }));
                    match blindmark::embed_file(&inp, &o, &k, &mut reg, strength, channel.clone(), jpeg.then_some(95)) {
                        Ok(r) => println!(
                            "{}",
                            serde_json::json!({"input": inp, "output": o, "id": format!("0x{:08x}", r.id), "psnr_db": (r.psnr * 100.0).round() / 100.0, "sec": t.elapsed().as_secs_f64()})
                        ),
                        Err(e) => eprintln!("{}: {e:#}", inp.display()),
                    }
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
