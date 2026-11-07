//! 检测 page: preview, verdict banner, result table, technical details and evidence export.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::WindowsAndMessaging::BN_CLICKED;

use super::controls::{banner, banner_set, image_box, image_hint, image_set, Tone};
use super::util::*;
use super::{post, App, WorkerMsg, TAB_REG};
use blindmark::jobs::DetectContext;
use blindmark::registry::{load_image, save_image, RgbImageBuf};
use blindmark::{keys, Report};

const OPEN: u16 = 1201;
const PASTE: u16 = 1202;
const LIST: u16 = 1203;
const SHOW_IN_REG: u16 = 1204;
const COPY: u16 = 1205;
const EXPORT: u16 = 1206;

/// Content similarity needed to call the picture "the same as registered".
pub const SIM_OK: f64 = 0.6;

pub struct DetectPage {
    open: HWND,
    paste: HWND,
    preview: HWND,
    src: HWND,
    banner: HWND,
    list: HWND,
    thumb_label: HWND,
    thumb: HWND,
    show_in_reg: HWND,
    grp: HWND,
    tech: HWND,
    copy: HWND,
    export: HWND,
    pub busy: bool,
    token: u64,
    img: Option<Arc<RgbImageBuf>>,
    source: Option<PathBuf>,
    source_label: String,
    report: Option<Report>,
    elapsed: f64,
}

impl DetectPage {
    pub fn create(p: HWND) -> DetectPage {
        let open = button(p, OPEN, "打开图片…");
        let paste = button(p, PASTE, "从剪贴板粘贴");
        let preview = image_box(p, "把要检测的图片拖到这里\n或点“打开图片…”、按 Ctrl+V 粘贴截图");
        let src = label(p, "");
        let banner = banner(p);
        banner_set(banner, Tone::Neutral, "尚未检测", "打开或粘贴一张图片，检测会自动开始。");
        let list = listview(p, LIST, true);
        lv_column(list, 0, "项目", 110, false);
        lv_column(list, 1, "值", 320, false);
        let thumb_label = label(p, "登记时的样子");
        let thumb = image_box(p, "（无）");
        let show_in_reg = button(p, SHOW_IN_REG, "在登记表中显示");
        let grp = group(p, "技术细节");
        let tech = edit_readonly_multi(p, 0);
        let copy = button(p, COPY, "复制报告");
        let export = button(p, EXPORT, "导出证据…");
        let d = DetectPage {
            open,
            paste,
            preview,
            src,
            banner,
            list,
            thumb_label,
            thumb,
            show_in_reg,
            grp,
            tech,
            copy,
            export,
            busy: false,
            token: 0,
            img: None,
            source: None,
            source_label: String::new(),
            report: None,
            elapsed: 0.0,
        };
        d.enable_result_buttons();
        d
    }

    fn enable_result_buttons(&self) {
        let has = self.report.is_some();
        enable(self.copy, has);
        enable(self.export, has);
        enable(self.show_in_reg, self.report.as_ref().and_then(|r| r.det.image_id).is_some());
    }
}

pub fn fonts(a: &App, f: Fonts) {
    set_font(a.detect.tech, f.mono);
}

pub fn layout(a: &App, w: i32, h: i32) {
    let d = &a.detect;
    let m = sc(10);
    let bh = sc(28);
    let left_w = ((w as f64 * 0.40) as i32).max(sc(300));
    mv(d.open, m, m, sc(100), bh);
    mv(d.paste, m + sc(106), m, sc(120), bh);
    let py = m + bh + sc(8);
    mv(d.preview, m, py, left_w - m, h - py - m - sc(24));
    mv(d.src, m, h - m - sc(20), left_w - m, sc(20));

    let rx = left_w + m;
    let rw = w - rx - m;
    mv(d.banner, rx, m, rw, sc(68));
    let ty = m + sc(68) + sc(10);
    let bottom = h - m - bh - sc(8);
    let tech_h = sc(130);
    let tech_y = bottom - tech_h;
    let thumb_w = sc(180);
    let list_w = rw - thumb_w - sc(10);
    mv(d.list, rx, ty, list_w, tech_y - ty - sc(8));
    let tx = rx + list_w + sc(10);
    mv(d.thumb_label, tx, ty, thumb_w, sc(20));
    let th = (tech_y - ty - sc(8) - sc(22) - bh - sc(6)).clamp(sc(60), thumb_w);
    mv(d.thumb, tx, ty + sc(22), thumb_w, th);
    mv(d.show_in_reg, tx, ty + sc(22) + th + sc(6), thumb_w, bh);
    mv(d.grp, rx, tech_y, rw, tech_h);
    mv(d.tech, rx + sc(10), tech_y + sc(20), rw - sc(20), tech_h - sc(28));
    mv(d.export, w - m - sc(100), h - m - bh, sc(100), bh);
    mv(d.copy, w - m - sc(100) - sc(8) - sc(90), h - m - bh, sc(90), bh);
    lv_set_col_width(d.list, 1, (list_w - sc(110) - sc(24)).max(sc(100)));
}

pub fn command(a: &mut App, id: u16, code: u32, _ctl: HWND) {
    if code != BN_CLICKED {
        return;
    }
    match id {
        OPEN => open_dialog(a),
        PASTE => from_clipboard(a, false),
        SHOW_IN_REG => {
            if let Some(id) = a.detect.report.as_ref().and_then(|r| r.det.image_id) {
                a.select_tab(TAB_REG);
                super::regpage::select_id(a, id);
            }
        }
        COPY => copy_report(a),
        EXPORT => export_evidence(a),
        _ => {}
    }
}

pub fn notify(_a: &mut App, _nm: &NMHDR) -> LRESULT {
    0
}

pub fn open_dialog(a: &mut App) {
    if let Some(p) = open_files(a.hwnd, "选择要检测的图片", IMAGE_FILTER, false).into_iter().next() {
        open_path(a, &p);
    }
}

pub fn open_path(a: &mut App, p: &Path) {
    match load_image(p) {
        Ok(img) => {
            let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            start(a, img, Some(p.to_path_buf()), name);
        }
        Err(e) => error(a.hwnd, &format!("无法打开图片：\n{}\n\n{e:#}", p.display())),
    }
}

/// `quiet`: don't complain when the clipboard holds no image (auto-paste on tab switch).
pub fn from_clipboard(a: &mut App, quiet: bool) {
    match clipboard_image(a.hwnd) {
        Some(ClipImage::Image(img)) => start(a, img, None, "剪贴板图片".into()),
        Some(ClipImage::File(p)) => open_path(a, &p),
        None => {
            if !quiet {
                info(a.hwnd, "剪贴板里没有图片。\n\n先截图或复制一张图片，再按 Ctrl+V。");
            }
        }
    }
}

fn start(a: &mut App, img: RgbImageBuf, source: Option<PathBuf>, label_text: String) {
    if a.detect.busy {
        a.set_status("上一张还在检测，请稍候");
        return;
    }
    let Some(key) = a.key.clone() else {
        warn(a.hwnd, "还没有加载密钥。请先在“设置”里新建或选择密钥。");
        return;
    };
    let d = &mut a.detect;
    d.token += 1;
    d.busy = true;
    d.report = None;
    d.source = source;
    d.source_label = format!("{label_text} · {} × {}", img.w, img.h);
    set_text(d.src, &d.source_label);
    image_set(d.preview, Some(&img));
    image_set(d.thumb, None);
    image_hint(d.thumb, "（无）");
    lv_clear(d.list);
    set_text(d.tech, "");
    banner_set(d.banner, Tone::Busy, "正在检测…", "旋转、翻转过的图片需要多花几秒。");
    enable(d.open, false);
    enable(d.paste, false);
    d.enable_result_buttons();
    let img = Arc::new(img);
    d.img = Some(img.clone());
    let token = d.token;
    a.set_status("正在检测…");

    let tx = a.tx.clone();
    let hw = a.hwnd as isize;
    let regdir = a.cfg.registry_dir.clone();
    let use_reg = a.reg.is_some();
    let opts = a.cfg.detect_options();
    let thumbs = a.thumbs.clone();
    std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(Report, f64), blindmark::jobs::UserError> {
            let ctx = DetectContext::open(if use_reg { Some(regdir.as_path()) } else { None }, &key)?;
            let thumbs_arg = if ctx.registry().is_some() { Some(thumbs.as_ref()) } else { None };
            let dr = ctx.detect(&img, &key, thumbs_arg, &opts);
            Ok((dr.report, t0.elapsed().as_secs_f64()))
        }))
        .map_err(|_| "检测过程中出现内部错误".to_string())
        .and_then(|r| {
            r.map_err(|e| {
                eprintln!("{}", e.detail);
                e.message
            })
        });
        post(&tx, hw, WorkerMsg::Detect { token, res });
    });
}

/// Verdict shown in the banner: tone, title, explanation.
fn verdict(a: &App, r: &Report) -> (Tone, String, String) {
    let entry = r.det.image_id.and_then(|id| a.reg.as_ref().and_then(|g| g.get(id)));
    match (r.det.found, r.det.image_id) {
        (true, Some(id)) => {
            let idh = format!("0x{id:08x}");
            match (entry, r.content_similarity) {
                (Some(_), Some(s)) if s >= SIM_OK => (
                    Tone::Good,
                    format!("确认：这是你的图片（编号 {idh}）"),
                    format!("水印读出成功，画面与登记的原图相符（相似度 {:.0}%）。", s * 100.0),
                ),
                (Some(_), Some(s)) => (
                    Tone::Warn,
                    format!("读出你的水印（编号 {idh}），但画面与登记不符"),
                    format!("相似度只有 {:.0}%。可能被大幅改动、裁成局部，或水印被转移到了别的画面上。", s * 100.0),
                ),
                (Some(_), None) => (
                    Tone::Good,
                    format!("检测到你的水印（编号 {idh}）"),
                    "这个编号在登记表里。没有做画面比对（设置里关闭了，或画面无法对齐）。".into(),
                ),
                (None, _) => (
                    Tone::Good,
                    format!("检测到你的水印（编号 {idh}）"),
                    "水印由你的密钥生成，但当前登记表里没有这个编号——可能来自另一份登记表。".into(),
                ),
            }
        }
        _ if r.det.present => (
            Tone::Warn,
            "有你的水印迹象，但读不出编号".into(),
            "同步信号存在，但编号已经损坏。图片可能被严重压缩、缩小或涂改。".into(),
        ),
        _ => match r.located_id {
            Some(id) => (
                Tone::Warn,
                "没有读出水印，但画面像登记过的图片".into(),
                format!("画面与登记图片 0x{id:08x} 的特征相符，可能是没加水印的版本，或水印已被破坏。"),
            ),
            None => (
                Tone::Neutral,
                "未检测到水印".into(),
                "这张图里没有用你的密钥加的水印，或者它被改动得太多了。".into(),
            ),
        },
    }
}

fn rows(a: &App, r: &Report, elapsed: f64) -> Vec<(String, String)> {
    let mut v = vec![];
    let (_, title, _) = verdict(a, r);
    v.push(("结论".into(), title));
    v.push(("图片编号".into(), r.id_hex.clone().unwrap_or_else(|| "—".into())));
    let entry = r.det.image_id.and_then(|id| a.reg.as_ref().and_then(|g| g.get(id)));
    if let Some(e) = entry {
        v.push(("渠道".into(), e.channel.clone().unwrap_or_else(|| "（未填写）".into())));
        v.push(("原始文件".into(), e.source.clone()));
        v.push(("登记时间".into(), local_time(&e.registered_utc)));
        v.push(("原始尺寸".into(), format!("{} × {}", e.size[1], e.size[0])));
    }
    if let Some(s) = r.content_similarity {
        let ov = r.content_overlap.map(|o| format!("，覆盖 {:.0}%", o * 100.0)).unwrap_or_default();
        v.push(("画面比对".into(), format!("相似度 {:.0}%{ov}", s * 100.0)));
    }
    v.push(("同步强度".into(), format!("z = {:.1}（阈值 {:.1}）", r.det.z_sync, a.cfg.detect_options().z_thresh)));
    let mut method = match r.det.method.as_deref() {
        Some("crc") => "直接解码（CRC 校验）".to_string(),
        Some("registry") => "登记表最大似然匹配".to_string(),
        Some(m) => m.to_string(),
        None => "—".to_string(),
    };
    if r.det.basis.is_some() {
        method += " · 仿射同步（旋转/翻转）";
    }
    if r.geometry.is_some() {
        method += " · 拍屏透视校正";
    }
    v.push(("检测方式".into(), method));
    v.push(("耗时".into(), format!("{elapsed:.1} 秒")));
    v
}

pub fn on_result(a: &mut App, token: u64, res: Result<(Report, f64), String>) {
    if token != a.detect.token {
        return;
    }
    a.detect.busy = false;
    enable(a.detect.open, true);
    enable(a.detect.paste, true);
    match res {
        Ok((rep, secs)) => {
            let (tone, title, sub) = verdict(a, &rep);
            banner_set(a.detect.banner, tone, &title, &sub);
            lv_clear(a.detect.list);
            for (k, v) in rows(a, &rep, secs) {
                lv_add(a.detect.list, &[&k, &v], 0);
            }
            set_text(a.detect.tech, &crlf(&serde_json::to_string_pretty(&rep).unwrap_or_default()));
            let shown = rep.det.image_id.or(rep.located_id);
            if let (Some(id), Some(reg)) = (shown, &a.reg) {
                if let Some(e) = reg.get(id) {
                    match load_image(&reg.thumb_path(e)) {
                        Ok(t) => image_set(a.detect.thumb, Some(&t)),
                        Err(_) => image_hint(a.detect.thumb, "缩略图缺失"),
                    }
                }
            }
            a.set_status(&format!("检测完成：{title}"));
            a.detect.report = Some(rep);
            a.detect.elapsed = secs;
        }
        Err(e) => {
            banner_set(a.detect.banner, Tone::Bad, "检测失败", &e);
            a.set_status("检测失败");
        }
    }
    a.detect.enable_result_buttons();
}

fn report_text(a: &App) -> Option<String> {
    let r = a.detect.report.as_ref()?;
    let mut s = String::from("blindmark 检测报告\n");
    s += &format!("时间：{}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    s += &format!("图片：{}\n", a.detect.source.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| a.detect.source_label.clone()));
    if let Some(k) = &a.key {
        s += &format!("密钥指纹：{}\n", keys::fingerprint(k));
    }
    s += &format!("格式：{} · blindmark {}\n\n", keys::FORMAT, env!("CARGO_PKG_VERSION"));
    let (_, _, sub) = verdict(a, r);
    for (k, v) in rows(a, r, a.detect.elapsed) {
        s += &format!("{k}：{v}\n");
    }
    s += &format!("说明：{sub}\n\n技术细节：\n{}\n", serde_json::to_string_pretty(r).unwrap_or_default());
    Some(s)
}

pub fn copy_report(a: &mut App) {
    match report_text(a) {
        Some(t) => {
            if set_clipboard_text(a.hwnd, &t) {
                a.set_status("报告已复制到剪贴板");
            }
        }
        None => a.set_status("还没有检测结果"),
    }
}

fn export_evidence(a: &mut App) {
    let (Some(text), Some(rep)) = (report_text(a), a.detect.report.clone()) else { return };
    let Some(dest) = pick_folder(a.hwnd, "选择证据保存位置") else { return };
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let tag = rep.det.image_id.map(|i| format!("{i:08x}")).unwrap_or_else(|| "none".into());
    let dir = dest.join(format!("blindmark-evidence-{tag}-{stamp}"));
    let res = (|| -> anyhow::Result<()> {
        use sha2::{Digest, Sha256};
        std::fs::create_dir_all(&dir)?;
        let mut text = text.clone();
        if let Some(src) = &a.detect.source {
            let bytes = std::fs::read(src)?;
            let name = src.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "suspect".into());
            std::fs::write(dir.join(format!("suspect-original-{name}")), &bytes)?;
            text += &format!("\n可疑文件 SHA-256：{}\n", hex::encode(Sha256::digest(&bytes)));
        } else if let Some(img) = &a.detect.img {
            save_image(img, &dir.join("suspect-clipboard.png"), None)?;
        }
        if let (Some(id), Some(reg)) = (rep.det.image_id, &a.reg) {
            if let Some(e) = reg.get(id) {
                let _ = std::fs::copy(reg.thumb_path(e), dir.join(format!("registered-{id:08x}.png")));
                std::fs::write(dir.join("registry-entry.json"), serde_json::to_string_pretty(e)?)?;
            }
        }
        std::fs::write(dir.join("report.txt"), crlf(&text).as_bytes())?;
        std::fs::write(dir.join("report.json"), serde_json::to_string_pretty(&rep)?)?;
        Ok(())
    })();
    match res {
        Ok(()) => {
            a.set_status(&format!("证据已导出到 {}", dir.display()));
            reveal(&dir);
        }
        Err(e) => error(a.hwnd, &format!("导出失败：{e:#}")),
    }
}
