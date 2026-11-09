//! One-off generator for the golden samples. Run it once:
//!
//!     cargo run --release --example make_golden
//!
//! It overwrites `tests/golden/cover.png` and `tests/golden/marked.png`. The samples are
//! frozen after that: never re-run it to make a failing golden test pass (see
//! `tests/golden/README.md`).

use blindmark::registry::{save_image, RgbImageBuf};
use blindmark::wm;
use std::path::PathBuf;

const W: usize = 320;
const H: usize = 256;
const ID: u32 = 0x0BADF00D;
const STRENGTH: f32 = 2.6;

/// The key every golden sample is made with, matching the tests in `src/keys.rs`.
fn key() -> Vec<u8> {
    (0u8..32).collect()
}

/// Deterministic LCG; the sample must never depend on an external image or an OS RNG.
struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        self.0
    }
}

/// 320x256: horizontal gradient, 8x8 blocky noise from seed 0x5EED, three diagonal lines.
fn cover() -> RgbImageBuf {
    let mut rng = Lcg(0x5EED);
    let mut data = vec![0u8; W * H * 3];
    for by in (0..H).step_by(8) {
        for bx in (0..W).step_by(8) {
            let noise = (rng.next() >> 8) % 41; // 0..=40
            for y in by..(by + 8).min(H) {
                for x in bx..(bx + 8).min(W) {
                    let g = 20 + x * 200 / (W - 1); // 20..=220 left to right
                    let v = (g + noise as usize).saturating_sub(20);
                    let i = (y * W + x) * 3;
                    data[i] = v as u8;
                    data[i + 1] = v as u8;
                    data[i + 2] = v as u8;
                }
            }
        }
    }
    for c in [-20i64, 60, 140] {
        for x in 0..W as i64 {
            let y = x + c;
            if y >= 0 && (y as usize) < H {
                let i = (y as usize * W + x as usize) * 3;
                data[i] = 250;
                data[i + 1] = 250;
                data[i + 2] = 250;
            }
        }
    }
    RgbImageBuf { w: W, h: H, data, alpha: None }
}

fn main() -> anyhow::Result<()> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    std::fs::create_dir_all(&dir)?;

    let cover = cover();
    save_image(&cover, &dir.join("cover.png"), None)?;

    let mut marked = cover.clone();
    let psnr = wm::embed_rgb(&mut marked.data, marked.w, marked.h, &key(), ID, STRENGTH);
    save_image(&marked, &dir.join("marked.png"), None)?;

    println!("wrote {}/cover.png and {}/marked.png (id 0x{ID:08x}, strength {STRENGTH}, psnr {psnr:.2} dB)", dir.display(), dir.display());
    Ok(())
}
