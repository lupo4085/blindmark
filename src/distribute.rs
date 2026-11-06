//! One image × one recipient = one copy with its own watermark id.

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::registry::{load_image, save_image, Registry};
use crate::wm;
use crate::unique_output_path;

/// The recipient a copy is addressed to, resolved from the recipient book.
#[derive(Clone, Debug)]
pub struct Target {
    pub id: String,
    pub name: String,
    pub group_id: Option<String>,
    pub group_name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DistOpts {
    /// Root output folder. Per-recipient sub folders are created below it when enabled.
    pub root: PathBuf,
    pub suffix: String,
    pub jpeg_quality: Option<u8>,
    pub strength: f32,
    pub per_recipient_dir: bool,
    pub batch_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Copy {
    pub id: u32,
    pub recipient: String,
    pub path: PathBuf,
    pub psnr: f64,
}

fn folder_name(name: &str) -> String {
    name.chars()
        .map(|c| if r#"\/:*?"<>|"#.contains(c) { '_' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Embed one copy per (file, target). Returns the copies in processing order.
pub fn distribute(files: &[PathBuf], targets: &[Target], key: &[u8], reg: &mut Registry, o: &DistOpts) -> Result<Vec<Copy>> {
    let mut out = Vec::new();
    for f in files {
        let original = load_image(f)?;
        let stem = f.file_stem().unwrap_or_default().to_string_lossy().to_string();
        for t in targets {
            let id = reg.new_id()?;
            let mut marked = original.clone();
            let psnr = wm::embed_rgb(&mut marked.data, marked.w, marked.h, key, id, o.strength);

            let (dir, stem_out) = if o.per_recipient_dir {
                let d = o.root.join(folder_name(&t.name));
                (d, stem.clone())
            } else {
                (o.root.clone(), format!("{stem}-{}", folder_name(&t.name)))
            };
            std::fs::create_dir_all(&dir)?;
            let path = unique_output_path(&dir, Path::new(&format!("{stem_out}.png")), &o.suffix, o.jpeg_quality.is_some());
            save_image(&marked, &path, o.jpeg_quality)?;

            let source = f.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let meta = crate::registry::CopyMeta {
                recipient_id: Some(t.id.clone()),
                recipient_name: Some(t.name.clone()),
                group_id: t.group_id.clone(),
                group_name: t.group_name.clone(),
                batch_id: o.batch_id.clone(),
            };
            reg.register_full(
                id,
                &source,
                &original,
                &marked,
                Some(t.name.clone()),
                o.strength,
                Some(path.display().to_string()),
                Some(&meta),
            )?;
            out.push(Copy { id, recipient: t.name.clone(), path, psnr });
        }
    }
    Ok(out)
}
