use anyhow::{Context, Result};
use blindmark::distribute::{distribute, DistOpts, Target};
use blindmark::recipients::Book;
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
}

#[derive(Subcommand)]
enum RecCmd {
    /// Add a recipient
    Add {
        name: String,
        #[arg(long)]
        group: Option<String>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long, default_value = "registry")]
        registry: PathBuf,
    },
    /// Add a group (two levels: --parent takes a group name)
    Group {
        name: String,
        #[arg(long)]
        parent: Option<String>,
        #[arg(long, default_value = "registry")]
        registry: PathBuf,
    },
    /// List groups and recipients
    List {
        #[arg(long, default_value = "registry")]
        registry: PathBuf,
    },
}

fn read_key(p: &PathBuf) -> Result<Vec<u8>> {
    keys::parse_key_file(&std::fs::read_to_string(p).with_context(|| format!("cannot read key {}", p.display()))?)
}

fn batch_id() -> String {
    format!("b_{}", chrono::Local::now().format("%Y%m%d_%H%M%S"))
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
        Cmd::Embed { inputs, out, key, registry, strength, channel, jpeg, to, flat } => {
            let k = read_key(&key)?;
            let mut reg = Registry::open(&registry)?;
            reg.check_key(&k)?;
            let t = std::time::Instant::now();

            if let Some(list) = to {
                let mut book = Book::open(&registry)?;
                let mut targets = Vec::new();
                for name in list.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    let r = book.recipient_by_name_or_create(name)?;
                    let group_name = r.group_id.as_deref().map(|g| book.group_path(g));
                    targets.push(Target { id: r.id, name: r.name, group_id: r.group_id, group_name });
                }
                let opts = DistOpts {
                    root: out,
                    suffix: "_wm".into(),
                    jpeg_quality: jpeg.then_some(95),
                    strength,
                    per_recipient_dir: !flat,
                    batch_id: Some(batch_id()),
                };
                let batch = opts.batch_id.clone();
                for c in distribute(&inputs, &targets, &k, &mut reg, &opts)? {
                    println!(
                        "{}",
                        serde_json::json!({
                            "recipient": c.recipient,
                            "output": c.path,
                            "id": format!("0x{:08x}", c.id),
                            "psnr_db": (c.psnr * 100.0).round() / 100.0,
                            "batch": batch,
                        })
                    );
                }
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
            RecCmd::Add { name, group, note, registry } => {
                let mut book = Book::open(&registry)?;
                let gid = group.as_deref().and_then(|g| book.groups.iter().find(|x| x.name == g).map(|x| x.id.clone()));
                let r = book.add_recipient(&name, gid.as_deref(), note.as_deref().unwrap_or(""))?;
                println!("{}", serde_json::json!({"id": r.id, "name": r.name, "group": r.group_id}));
            }
            RecCmd::Group { name, parent, registry } => {
                let mut book = Book::open(&registry)?;
                let pid = parent.as_deref().and_then(|p| book.groups.iter().find(|g| g.name == p).map(|g| g.id.clone()));
                let g = book.add_group(&name, pid.as_deref())?;
                println!("{}", serde_json::json!({"id": g.id, "name": g.name, "parent": g.parent}));
            }
            RecCmd::List { registry } => {
                let book = Book::open(&registry)?;
                for g in &book.groups {
                    println!("[group] {} ({}) parent={:?}", g.name, g.id, g.parent);
                }
                for r in &book.items {
                    let g = r.group_id.as_deref().map(|g| book.group_path(g)).unwrap_or_else(|| "未分组".into());
                    println!("[recipient] {} ({}) group={} note={}", r.name, r.id, g, r.note);
                }
            }
        },
    }
    Ok(())
}
