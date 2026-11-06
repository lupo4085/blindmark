use anyhow::{Context, Result};
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
    Keygen { #[arg(long)] key: PathBuf },
    /// Watermark one or more images into an output folder
    Embed {
        inputs: Vec<PathBuf>,
        #[arg(long, short)] out: PathBuf,
        #[arg(long)] key: PathBuf,
        #[arg(long, default_value = "registry")] registry: PathBuf,
        #[arg(long, default_value_t = wm::DEFAULT_STRENGTH)] strength: f32,
        /// Free-text note stored with each ID (e.g. where you post it)
        #[arg(long)] channel: Option<String>,
        /// Write JPEG (quality 95) instead of PNG
        #[arg(long)] jpeg: bool,
    },
    /// Check images for your watermark
    Detect {
        inputs: Vec<PathBuf>,
        #[arg(long)] key: PathBuf,
        #[arg(long, default_value = "registry")] registry: PathBuf,
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
        Cmd::Embed { inputs, out, key, registry, strength, channel, jpeg } => {
            let k = read_key(&key)?;
            let mut reg = Registry::open(&registry)?;
            reg.check_key(&k)?;
            for inp in inputs {
                let stem = inp.file_stem().unwrap_or_default().to_string_lossy();
                let o = out.join(format!("{stem}_wm.{}", if jpeg { "jpg" } else { "png" }));
                let t = std::time::Instant::now();
                match blindmark::embed_file(&inp, &o, &k, &mut reg, strength, channel.clone(), jpeg.then_some(95)) {
                    Ok(r) => println!("{}", serde_json::json!({"input": inp, "output": o, "id": format!("0x{:08x}", r.id), "psnr_db": (r.psnr * 100.0).round() / 100.0, "sec": t.elapsed().as_secs_f64()})),
                    Err(e) => eprintln!("{}: {e:#}", inp.display()),
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
                v["input"] = serde_json::json!(inp);
                v["sec"] = serde_json::json!(t.elapsed().as_secs_f64());
                println!("{}", serde_json::to_string(&v)?);
            }
        }
    }
    Ok(())
}
