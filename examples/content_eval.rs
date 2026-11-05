//! Re-run the content check on benchmark detections: content_eval <run_dir>
use blindmark::registry::{content_match, load_image, Registry};
fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let reg = Registry::open(&dir.join("reg")).unwrap();
    let man: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    let tags: std::collections::HashMap<String, (String, String)> = man.as_array().unwrap().iter().map(|m| {
        (m["path"].as_str().unwrap().to_string(), (m["tag"].as_str().unwrap().to_string(), m["attack"].as_str().unwrap().to_string()))
    }).collect();
    for line in std::fs::read_to_string(dir.join("results.jsonl")).unwrap().lines() {
        let r: serde_json::Value = serde_json::from_str(line).unwrap();
        if !r["found"].as_bool().unwrap() { continue; }
        let p = r["input"].as_str().unwrap();
        let full = dir.join(p).display().to_string();
        let (tag, att) = tags.get(&full).cloned().unwrap_or_default();
        let img = load_image(&dir.join(p)).unwrap();
        let cm = content_match(&reg, r["image_id"].as_u64().unwrap() as u32, &img, r["period_px"].as_f64().unwrap());
        if let Some(cm) = cm { println!("{tag}\t{att}\t{:.3}\t{:.2}", cm.similarity, cm.overlap); }
    }
}
