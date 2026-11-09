//! False-positive calibration data collector (task V3-6).
//!
//! For every image it measures the raw synchronisation score `z_sync` that the detector
//! would produce on an unwatermarked copy ("neg") or on a watermarked copy ("pos"), always
//! with the threshold set to -inf so the CRC and registry steps run and their outcome is
//! recorded in `method`.
//!
//! ```text
//! cargo run --release --example calibrate -- --images <DIR> --out calib.csv \
//!     [--keys 10] [--registry-size 1000] [--resume] [--images-limit N]
//! ```
//!
//! `--images` is searched recursively. Each top-level sub folder is a category
//! (`photo/`, `illustration/`, `screenshot/`, `text/`); images directly in the root are
//! `other`, and the 30 generated images are `synthetic`. Non-image files (manifest.csv, ...)
//! are skipped. `--images-limit N` keeps only the first N images after sorting by path, for
//! quick trial runs. Positive samples are the first 40 images, taken round-robin from the
//! categories.
//!
//! Output rows are appended and flushed one by one; `--resume` skips rows already present
//! (keyed by kind, image, variant, key_index).

use anyhow::{Context, Result};
use blindmark::imgproc::{self, Plane};
use blindmark::registry::{from_dynamic, load_image, RgbImageBuf};
use blindmark::wm::{self, KnownIds};
use clap::Parser;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff"];
const MAX_SIDE: usize = 1600;
const POSITIVES: usize = 40;
const SEED: u64 = 0xCA11B;
const CSV_HEADER: &str = "kind,image,category,variant,key_index,w,h,z_sync,method,z_id,seconds";

#[derive(Parser)]
#[command(name = "calibrate", about = "Collect z_sync scores for false-positive calibration")]
struct Args {
    /// Folder with the images; sub folders are categories.
    #[arg(long)]
    images: PathBuf,
    /// CSV to append to.
    #[arg(long, default_value = "calib.csv")]
    out: PathBuf,
    /// How many detection keys to test (deterministic, seed 0xCA11B).
    #[arg(long, default_value_t = 10)]
    keys: usize,
    /// Random registry ids built per key.
    #[arg(long, default_value_t = 1000)]
    registry_size: usize,
    /// Skip the rows already present in the CSV.
    #[arg(long)]
    resume: bool,
    /// Only process the first N images after sorting by path (trial runs).
    #[arg(long)]
    images_limit: Option<usize>,
}

fn main() -> Result<()> {
    let a = Args::parse();
    let mut images = collect_images(&a.images)?;
    images.sort_by(|x, y| rel_string(&a.images, x).cmp(&rel_string(&a.images, y)));
    if let Some(n) = a.images_limit {
        images.truncate(n);
    }
    if images.is_empty() {
        anyhow::bail!("no images found under {}", a.images.display());
    }

    let keys = generate_keys(a.keys);
    let known: Vec<KnownIds> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| KnownIds::new(k, registry_ids(i, a.registry_size)))
        .collect();
    let positives = pick_positives(&a.images, &images, POSITIVES);

    let total_rows = images.len() * 3 * a.keys + 30 * 3 * a.keys + positives.len() * 4;
    let done_set = if a.resume { read_done(&a.out)? } else { HashSet::new() };
    let done_rows = done_set.len();
    let mut out = OutCsv::open(&a.out, !a.resume || !a.out.exists())?;
    let mut stats = Stats { written: 0, skipped: 0, panics: 0, total: total_rows, start: Instant::now() };

    // ---------------------------------------------------------------- negatives
    for img in &images {
        let rel = rel_string(&a.images, img);
        let category = category_of(&rel);
        let base = downscale(load_image(img).with_context(|| format!("load {}", rel))?);
        let variants = negative_variants(&base)?;
        for (variant, rgb) in &variants {
            for (ki, key) in keys.iter().enumerate() {
                process(&mut out, &done_set, &mut stats, "neg", &rel, &category, variant, ki, rgb, key, &known[ki])?;
            }
        }
    }

    // ---------------------------------------------------------------- synthetic negatives
    for (name, base) in synthetic_images() {
        let variants = negative_variants(&base)?;
        for (variant, rgb) in &variants {
            for (ki, key) in keys.iter().enumerate() {
                process(&mut out, &done_set, &mut stats, "neg", &name, "synthetic", variant, ki, rgb, key, &known[ki])?;
            }
        }
    }

    // ---------------------------------------------------------------- positives
    for (pi, img) in positives.iter().enumerate() {
        let rel = rel_string(&a.images, img);
        let category = category_of(&rel);
        let base = downscale(load_image(img).with_context(|| format!("load {}", rel))?);
        let (pkey, pid) = positive_key_and_id(pi);
        let mut marked = base.clone();
        wm::embed_rgb(&mut marked.data, marked.w, marked.h, &pkey, pid, wm::DEFAULT_STRENGTH);
        let variants = positive_variants(&marked)?;
        let pknown = KnownIds::new(&pkey, registry_ids(1000 + pi, a.registry_size));
        for (variant, rgb) in &variants {
            process(&mut out, &done_set, &mut stats, "pos", &rel, &category, variant, pi, rgb, &pkey, &pknown)?;
        }
    }

    let elapsed = stats.start.elapsed().as_secs_f64();
    eprintln!(
        "done: {} new rows, {} already present, {:.0}s total (target {}), {} detector panics",
        stats.written, done_rows, elapsed, total_rows, stats.panics
    );
    if stats.panics > 0 {
        eprintln!(
            "WARNING: {} rows have z_sync=nan because the detector panicked on flat images \
             (src/wm.rs median_filter_nearest); the report counts them separately and they are \
             excluded from quantiles and extrapolation",
            stats.panics
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn process(
    out: &mut OutCsv,
    done: &HashSet<String>,
    stats: &mut Stats,
    kind: &str,
    image: &str,
    category: &str,
    variant: &str,
    key_index: usize,
    rgb: &RgbImageBuf,
    key: &[u8],
    known: &KnownIds,
) -> Result<()> {
    let id = format!("{kind}|{image}|{variant}|{key_index}");
    if done.contains(&id) {
        stats.skipped += 1;
        return Ok(());
    }
    let t = Instant::now();
    let y = imgproc::luma(&rgb.data, rgb.w, rgb.h);
    // Flat images currently make the detector panic inside `median_filter_nearest`
    // (sort with NaN). V3-6 may not change `src/`, so keep collecting: record the row with
    // z_sync = nan and report the crash count instead of aborting the whole run.
    let det = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wm::detect_luma(&y, key, Some(known), f64::NEG_INFINITY)
    })) {
        Ok(d) => d,
        Err(_) => {
            let secs = t.elapsed().as_secs_f64();
            stats.panics += 1;
            eprintln!("warning: detector panicked on {image} [{variant}] key {key_index}; recorded z_sync=nan");
            out.row(&[
                kind,
                image,
                category,
                variant,
                &key_index.to_string(),
                &rgb.w.to_string(),
                &rgb.h.to_string(),
                "nan",
                "",
                "",
                &format!("{secs:.3}"),
            ])?;
            stats.written += 1;
            stats.progress();
            return Ok(());
        }
    };
    let secs = t.elapsed().as_secs_f64();

    let z_id = det.z_id.map(|v| format!("{v:.6}")).unwrap_or_default();
    out.row(&[
        kind,
        image,
        category,
        variant,
        &key_index.to_string(),
        &rgb.w.to_string(),
        &rgb.h.to_string(),
        &format!("{:.6}", det.z_sync),
        det.method.as_deref().unwrap_or(""),
        &z_id,
        &format!("{secs:.3}"),
    ])?;
    stats.written += 1;
    stats.progress();
    Ok(())
}

struct Stats {
    written: usize,
    skipped: usize,
    panics: usize,
    total: usize,
    start: Instant,
}

impl Stats {
    fn progress(&self) {
        if self.written % 50 != 0 {
            return;
        }
        let elapsed = self.start.elapsed().as_secs_f64();
        let done = self.written + self.skipped;
        let remaining = self.total.saturating_sub(done) as f64;
        let rate = if done > 0 { elapsed / done as f64 } else { 0.0 };
        eprintln!("{done}/{} rows, {elapsed:.0}s elapsed, ETA {:.0}s", self.total, remaining * rate);
    }
}

// ------------------------------------------------------------------ CSV

struct OutCsv {
    file: std::fs::File,
}

impl OutCsv {
    fn open(path: &Path, truncate: bool) -> Result<Self> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        let mut file = if truncate {
            let f = std::fs::File::create(path)?;
            f
        } else {
            std::fs::OpenOptions::new().append(true).open(path)?
        };
        if truncate {
            writeln!(file, "{CSV_HEADER}")?;
            file.flush()?;
        }
        Ok(OutCsv { file })
    }

    fn row(&mut self, fields: &[&str]) -> Result<()> {
        let line = fields.iter().map(|f| csv_escape(f)).collect::<Vec<_>>().join(",");
        writeln!(self.file, "{line}")?;
        self.file.flush()?;
        Ok(())
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            }
            '"' => quoted = true,
            ',' if !quoted => {
                out.push(std::mem::take(&mut field));
            }
            c => field.push(c),
        }
    }
    out.push(field);
    out
}

/// Keys already in the CSV, as `kind|image|variant|key_index`.
fn read_done(path: &Path) -> Result<HashSet<String>> {
    let mut set = HashSet::new();
    if !path.exists() {
        return Ok(set);
    }
    let text = std::fs::read_to_string(path)?;
    for (i, line) in text.lines().enumerate() {
        if i == 0 || line.trim().is_empty() {
            continue;
        }
        let f = parse_csv_line(line);
        if f.len() >= 5 {
            set.insert(format!("{}|{}|{}|{}", f[0], f[1], f[3], f[4]));
        }
    }
    Ok(set)
}

// ------------------------------------------------------------------ images

fn collect_images(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let rd = std::fs::read_dir(root).with_context(|| format!("cannot read {}", root.display()))?;
    for e in rd {
        let e = e?;
        let p = e.path();
        if p.is_dir() {
            out.extend(collect_images(&p)?);
        } else if is_image(&p) {
            out.push(p);
        }
    }
    Ok(out)
}

fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn rel_string(root: &Path, p: &Path) -> String {
    let rel = p.strip_prefix(root).unwrap_or(p);
    rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

fn category_of(rel: &str) -> String {
    match rel.split_once('/') {
        Some((dir, _)) => dir.to_string(),
        None => "other".to_string(),
    }
}

/// First `n` images, one after another from every category (categories in sorted order).
fn pick_positives(root: &Path, images: &[PathBuf], n: usize) -> Vec<PathBuf> {
    let mut by_cat: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for img in images {
        by_cat.entry(category_of(&rel_string(root, img))).or_default().push(img.clone());
    }
    let cats: Vec<Vec<PathBuf>> = by_cat.into_values().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while out.len() < n {
        let mut added = false;
        for v in &cats {
            if let Some(p) = v.get(i) {
                out.push(p.clone());
                added = true;
                if out.len() >= n {
                    break;
                }
            }
        }
        if !added {
            break;
        }
        i += 1;
    }
    out
}

fn downscale(img: RgbImageBuf) -> RgbImageBuf {
    let long = img.w.max(img.h);
    if long <= MAX_SIDE {
        return img;
    }
    let s = MAX_SIDE as f64 / long as f64;
    let nw = ((img.w as f64 * s).round() as usize).max(1);
    let nh = ((img.h as f64 * s).round() as usize).max(1);
    resize_rgb(&img, nw, nh)
}

fn resize_rgb(img: &RgbImageBuf, nw: usize, nh: usize) -> RgbImageBuf {
    if nw == img.w && nh == img.h {
        return img.clone();
    }
    let mut out = RgbImageBuf { w: nw, h: nh, data: vec![0u8; nw * nh * 3], alpha: None };
    for c in 0..3 {
        let plane = Plane::from_vec(img.w, img.h, img.data.iter().skip(c).step_by(3).map(|&v| v as f32).collect());
        let r = imgproc::resize(&plane, nw, nh);
        for (i, &v) in r.d.iter().enumerate() {
            out.data[i * 3 + c] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn jpeg_roundtrip(img: &RgbImageBuf, quality: u8) -> Result<RgbImageBuf> {
    let mut buf = Vec::new();
    {
        let mut cur = std::io::Cursor::new(&mut buf);
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cur, quality);
        enc.encode(&img.data, img.w as u32, img.h as u32, image::ExtendedColorType::Rgb8)
            .context("jpeg encode")?;
    }
    let dynimg = image::load_from_memory(&buf).context("jpeg decode")?;
    Ok(from_dynamic(dynimg))
}

fn crop_half_area(img: &RgbImageBuf) -> RgbImageBuf {
    let s = 0.5f64.sqrt();
    let nw = ((img.w as f64 * s).round() as usize).clamp(1, img.w);
    let nh = ((img.h as f64 * s).round() as usize).clamp(1, img.h);
    let x0 = (img.w - nw) / 2;
    let y0 = (img.h - nh) / 2;
    let mut out = RgbImageBuf { w: nw, h: nh, data: vec![0u8; nw * nh * 3], alpha: None };
    for y in 0..nh {
        let src = ((y0 + y) * img.w + x0) * 3;
        out.data[y * nw * 3..(y + 1) * nw * 3].copy_from_slice(&img.data[src..src + nw * 3]);
    }
    out
}

fn negative_variants(img: &RgbImageBuf) -> Result<Vec<(&'static str, RgbImageBuf)>> {
    let half = resize_rgb(img, (img.w / 2).max(1), (img.h / 2).max(1));
    Ok(vec![("orig", img.clone()), ("jpeg75", jpeg_roundtrip(img, 75)?), ("half", half)])
}

fn positive_variants(marked: &RgbImageBuf) -> Result<Vec<(&'static str, RgbImageBuf)>> {
    let half = resize_rgb(marked, (marked.w / 2).max(1), (marked.h / 2).max(1));
    Ok(vec![
        ("orig", marked.clone()),
        ("jpeg75", jpeg_roundtrip(marked, 75)?),
        ("half", half),
        ("crop50", crop_half_area(marked)),
    ])
}

// ------------------------------------------------------------------ deterministic inputs

struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn fill(&mut self, buf: &mut [u8]) {
        for c in buf.chunks_mut(8) {
            let v = self.next_u64().to_le_bytes();
            c.copy_from_slice(&v[..c.len()]);
        }
    }
}

fn generate_keys(n: usize) -> Vec<[u8; 32]> {
    let mut rng = SplitMix64(SEED);
    (0..n)
        .map(|_| {
            let mut k = [0u8; 32];
            rng.fill(&mut k);
            k
        })
        .collect()
}

fn registry_ids(key_index: usize, n: usize) -> Vec<u32> {
    let mut rng = SplitMix64(SEED ^ ((key_index as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)));
    (0..n).map(|_| rng.next_u64() as u32).collect()
}

fn positive_key_and_id(index: usize) -> ([u8; 32], u32) {
    let mut rng = SplitMix64(SEED ^ 0x5EED_5EED ^ ((index as u64 + 1).wrapping_mul(0xD1B5_4A32_D192_ED03)));
    let mut key = [0u8; 32];
    rng.fill(&mut key);
    let id = rng.next_u64() as u32;
    (key, id)
}

// ------------------------------------------------------------------ synthetic negatives

/// 30 generated images: 6 solid, 6 gradient, 6 noise, 6 checkerboard, 6 random text blocks.
fn synthetic_images() -> Vec<(String, RgbImageBuf)> {
    let (w, h) = (800usize, 600usize);
    let mut rng = SplitMix64(SEED ^ 0x5EED_CA11B);
    let mut out = Vec::new();

    for i in 0..6 {
        let v = rng.next_u64();
        let c = [(v >> 40) as u8, (v >> 24) as u8, (v >> 8) as u8];
        out.push((format!("synthetic/solid_{i}"), solid(w, h, c)));
    }
    for i in 0..6 {
        let v = rng.next_u64();
        let c0 = [(v >> 40) as u8, (v >> 24) as u8, (v >> 8) as u8];
        let v = rng.next_u64();
        let c1 = [(v >> 40) as u8, (v >> 24) as u8, (v >> 8) as u8];
        out.push((format!("synthetic/gradient_{i}"), gradient(w, h, c0, c1, i % 2 == 1)));
    }
    for i in 0..6 {
        out.push((format!("synthetic/noise_{i}"), noise(w, h, &mut rng)));
    }
    for i in 0..6 {
        let cell = [8usize, 16, 24, 32, 48, 64][i];
        let v = rng.next_u64();
        let c0 = [(v >> 40) as u8, (v >> 24) as u8, (v >> 8) as u8];
        out.push((format!("synthetic/checker_{i}"), checker(w, h, cell, [250, 250, 250], c0)));
    }
    for i in 0..6 {
        out.push((format!("synthetic/text_{i}"), text_blocks(w, h, &mut rng)));
    }
    out
}

fn solid(w: usize, h: usize, c: [u8; 3]) -> RgbImageBuf {
    let mut data = vec![0u8; w * h * 3];
    for px in data.chunks_mut(3) {
        px.copy_from_slice(&c);
    }
    RgbImageBuf { w, h, data, alpha: None }
}

fn gradient(w: usize, h: usize, c0: [u8; 3], c1: [u8; 3], diagonal: bool) -> RgbImageBuf {
    let mut data = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let t = if diagonal {
                (x as f64 / w as f64 + y as f64 / h as f64) / 2.0
            } else {
                x as f64 / (w - 1).max(1) as f64
            };
            let i = (y * w + x) * 3;
            for c in 0..3 {
                data[i + c] = (c0[c] as f64 * (1.0 - t) + c1[c] as f64 * t).round() as u8;
            }
        }
    }
    RgbImageBuf { w, h, data, alpha: None }
}

fn noise(w: usize, h: usize, rng: &mut SplitMix64) -> RgbImageBuf {
    let mut data = vec![0u8; w * h * 3];
    rng.fill(&mut data);
    RgbImageBuf { w, h, data, alpha: None }
}

fn checker(w: usize, h: usize, cell: usize, c0: [u8; 3], c1: [u8; 3]) -> RgbImageBuf {
    let mut data = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let c = if ((x / cell) + (y / cell)) % 2 == 0 { c0 } else { c1 };
            let i = (y * w + x) * 3;
            data[i..i + 3].copy_from_slice(&c);
        }
    }
    RgbImageBuf { w, h, data, alpha: None }
}

/// White page with dark rectangles arranged like lines of text — no font dependency.
fn text_blocks(w: usize, h: usize, rng: &mut SplitMix64) -> RgbImageBuf {
    let mut data = vec![255u8; w * h * 3];
    let line_h = 18usize;
    let pad = 40usize;
    let mut y = pad;
    while y + line_h + 4 < h - pad {
        let mut x = pad;
        while x < w - pad {
            let word = 12 + (rng.next_u64() % 70) as usize;
            let gap = 6 + (rng.next_u64() % 14) as usize;
            let shade = 20 + (rng.next_u64() % 60) as u8;
            for yy in y..(y + line_h) {
                for xx in x..(x + word).min(w - pad) {
                    let i = (yy * w + xx) * 3;
                    data[i..i + 3].fill(shade);
                }
            }
            x += word + gap;
        }
        y += line_h + 10;
    }
    RgbImageBuf { w, h, data, alpha: None }
}
