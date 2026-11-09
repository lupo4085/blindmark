//! Golden-sample tests: freeze the watermark format and the compatibility with old data.
//!
//! Samples live in `tests/golden/` and are never regenerated to make a test pass — see
//! `tests/golden/README.md`.

use blindmark::recipients::Book;
use blindmark::registry::{self, Registry};
use blindmark::wm;
use blindmark::{detect_image_opts, DetectOptions};
use std::path::{Path, PathBuf};

const ID: u32 = 0x0BADF00D;
const STRENGTH: f32 = 2.6;

/// The key every golden sample was made with, matching the tests in `src/keys.rs`.
fn key() -> Vec<u8> {
    (0u8..32).collect()
}

fn golden() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bm-golden-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), &dst).unwrap();
        }
    }
}

#[test]
fn marked_is_detected() {
    let img = registry::load_image(&golden().join("marked.png")).unwrap();
    let rep = detect_image_opts(&img, &key(), None, None, None, &DetectOptions::default());
    assert!(rep.det.found, "watermark in marked.png was not detected");
    assert_eq!(rep.det.image_id, Some(ID), "wrong image id");
    assert_eq!(rep.det.method.as_deref(), Some("crc"), "wrong detection method");
}

/// 同一平台上应逐字节相同；0.1% 的余量只留给不同平台数学库的个别舍入差异。
#[test]
fn embed_is_stable() {
    let cover = registry::load_image(&golden().join("cover.png")).unwrap();
    let marked = registry::load_image(&golden().join("marked.png")).unwrap();
    let mut again = cover.clone();
    wm::embed_rgb(&mut again.data, again.w, again.h, &key(), ID, STRENGTH);
    assert_eq!((again.w, again.h), (marked.w, marked.h), "image size changed");
    assert_eq!(again.data.len(), marked.data.len(), "pixel count changed");
    let total = marked.data.len();
    let differing = again.data.iter().zip(&marked.data).filter(|(a, b)| a != b).count();
    let worst = again
        .data
        .iter()
        .zip(&marked.data)
        .map(|(&a, &b)| (a as i32 - b as i32).abs())
        .max()
        .unwrap();
    assert!(worst <= 1, "re-embedding differs from marked.png by up to {worst} per channel");
    assert!(differing * 1000 <= total, "re-embedding changed {differing} of {total} bytes (limit 0.1%)");
}

#[test]
fn cover_is_clean() {
    let img = registry::load_image(&golden().join("cover.png")).unwrap();
    let rep = detect_image_opts(&img, &key(), None, None, None, &DetectOptions::default());
    assert!(!rep.det.found, "clean cover.png was reported as watermarked");
}

#[test]
fn registry_v02_loads() {
    let dir = tmp("registry-v02");
    copy_dir(&golden().join("registry-v02"), &dir);
    let mut reg = Registry::open(&dir).unwrap();
    assert_eq!(reg.entries.len(), 1, "v0.2 registry must load as one entry");
    reg.check_key(&key()).unwrap();
}

#[test]
fn recipients_v1_loads() {
    let dir = tmp("recipients-v1");
    std::fs::copy(golden().join("recipients-v1.json"), dir.join("recipients.json")).unwrap();
    let book = Book::open(&dir).unwrap();
    assert_eq!(book.groups.len(), 2, "expected two groups");
    assert_eq!(book.items.len(), 2, "expected two recipients");
    let sub = book.group("g_0002").expect("old group id g_0002 not found");
    assert_eq!(sub.parent.as_deref(), Some("g_0001"), "g_0002 must stay under g_0001");
    assert_eq!(book.group_path("g_0002"), "客户 / 2026 秋季");
    assert!(book.recipient("r_0001").is_some(), "old recipient id r_0001 not found");
    assert!(book.recipient("r_0002").is_some(), "old recipient id r_0002 not found");
    assert_eq!(book.recipients_in(Some("g_0002")).len(), 2);
}

/// Second line of defence for the format: the same Python vectors as `src/keys.rs`.
#[test]
fn keys_vectors() {
    use blindmark::keys::*;
    let key: Vec<u8> = (0u8..32).collect();
    let p = make_pattern(&key);
    let chips16: Vec<i32> = p.chips[..16].iter().map(|&c| c as i32).collect();
    assert_eq!(chips16, vec![-1, -1, -1, -1, -1, -1, 1, 1, 1, -1, -1, -1, 1, 1, 1, -1]);
    let sync: Vec<usize> = (0..NCHIPS).filter(|&k| p.is_sync[k]).take(8).collect();
    assert_eq!(sync, vec![0, 3, 4, 5, 8, 10, 13, 14]);
    assert_eq!(p.is_sync.iter().filter(|&&s| s).count(), NSYNC);
    let bits: String = encode_payload(&key, 0x12345678).iter().map(|b| (b'0' + b) as char).collect();
    assert_eq!(bits, "110001001000110010000011110111001111101001110101");
    for id in [0u32, 1, 0xdeadbeef, u32::MAX] {
        assert_eq!(decode_payload(&key, &encode_payload(&key, id)), (id, true));
    }
}
