//! Build script: build info, third-party licence list, and (Windows only) the
//! icon / manifest / version resources.
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=assets");

    let git = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(&root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=BM_GIT_HASH={git}");
    println!("cargo:rustc-env=BM_BUILD_DATE={}", chrono::Utc::now().format("%Y-%m-%d"));

    fs::write(out.join("third_party.txt"), third_party(&root)).unwrap();

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let ver = env::var("CARGO_PKG_VERSION").unwrap();
        let comma = version_words(&ver).join(",");
        let fwd = |p: PathBuf| p.display().to_string().replace('\\', "/");
        let rc = fs::read_to_string(root.join("assets/blindmark.rc.in"))
            .unwrap()
            .replace("@ICON@", &fwd(root.join("assets/blindmark.ico")))
            .replace("@MANIFEST@", &fwd(root.join("assets/blindmark.manifest")))
            .replace("@WM_BMP@", &fwd(root.join("assets/wizard-watermark.bmp")))
            .replace("@HD_BMP@", &fwd(root.join("assets/wizard-header.bmp")))
            .replace("@VER_COMMA@", &comma)
            .replace("@VER@", &ver);
        // rc.exe takes one to four comma-separated 16-bit words here, and reports a fifth
        // one as a bogus "unrecognized VERSIONINFO field" on the first field line.
        for field in ["FILEVERSION", "PRODUCTVERSION"] {
            let value = rc
                .lines()
                .find_map(|l| l.trim_start().strip_prefix(field))
                .map(|rest| rest.trim())
                .unwrap_or_else(|| panic!("{field} is missing from assets/blindmark.rc.in"));
            let words = value.split(',').count();
            assert!(words <= 4, "{field} {value} has {words} words; rc.exe accepts at most 4");
        }
        let rc_path = out.join("blindmark.rc");
        fs::write(&rc_path, rc).unwrap();
        embed_resource::compile_for(&rc_path, ["blindmark"], embed_resource::NONE).manifest_required().unwrap();
    }
}

/// `FILEVERSION`/`PRODUCTVERSION` words: at most four 16-bit numbers from `major.minor.patch`.
fn version_words(ver: &str) -> Vec<String> {
    let mut words: Vec<u32> = ver
        .split(['-', '+'])
        .next()
        .unwrap_or("0")
        .split('.')
        .map(|s| s.parse().unwrap_or(0))
        .collect();
    words.resize(4, 0);
    words.iter().map(|w| w.to_string()).collect()
}

/// "name version — licence" for every registry dependency in Cargo.lock.
fn third_party(root: &Path) -> String {
    let lock = fs::read_to_string(root.join("Cargo.lock")).unwrap_or_default();
    let cargo_home = env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| {
        env::var_os("USERPROFILE").or_else(|| env::var_os("HOME")).map(|h| PathBuf::from(h).join(".cargo"))
    });
    let src_dirs: Vec<PathBuf> = cargo_home
        .map(|h| h.join("registry").join("src"))
        .and_then(|d| fs::read_dir(d).ok())
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    let mut rows = vec![];
    for block in lock.split("[[package]]").skip(1) {
        let field = |k: &str| {
            block.lines().find_map(|l| l.strip_prefix(&format!("{k} = \"")).map(|v| v.trim_end_matches('"').to_string()))
        };
        let (Some(name), Some(version), Some(source)) = (field("name"), field("version"), field("source")) else { continue };
        if !source.starts_with("registry+") {
            continue;
        }
        let licence = src_dirs
            .iter()
            .map(|d| d.join(format!("{name}-{version}")).join("Cargo.toml"))
            .find(|p| p.exists())
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|t| {
                let pkg = t.split("\n[").next().unwrap_or("").to_string();
                let t2 = t.split("[package]").nth(1).map(|s| s.split("\n[").next().unwrap_or("").to_string()).unwrap_or(pkg);
                t2.lines().find_map(|l| {
                    let l = l.trim();
                    l.strip_prefix("license = ").map(|v| v.trim_matches('"').to_string())
                        .or_else(|| l.strip_prefix("license-file = ").map(|v| format!("见 {}", v.trim_matches('"'))))
                })
            })
            .unwrap_or_else(|| "（见该组件的源码包）".into());
        rows.push(format!("{name} {version}\r\n    {licence}"));
    }
    rows.sort_by_key(|r| r.to_lowercase());
    let mut s = String::from(
        "blindmark 使用了以下开源组件。每个组件的许可证全文随其源码包分发，可在 https://crates.io 上按名称查到。\r\n\r\n",
    );
    s += &rows.join("\r\n");
    s += "\r\n";
    s
}
