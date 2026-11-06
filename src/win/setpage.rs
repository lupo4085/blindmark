//! 设置 page: key, registry, embedding defaults, detection options and general behaviour.

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{BN_CLICKED, ES_NUMBER, SetWindowLongW, GetWindowLongW, GWL_STYLE};

use super::config::Settings;
use super::util::*;
use super::App;
use blindmark::keys;

const NEW_KEY: u16 = 1401;
const PICK_KEY: u16 = 1402;
const BACKUP_KEY: u16 = 1403;
const REG_DIR: u16 = 1404;
const REG_OPEN: u16 = 1405;
const REG_BACKUP: u16 = 1406;
const REG_CSV: u16 = 1407;
const STRENGTH: u16 = 1408;
const PNG: u16 = 1409;
const JPG: u16 = 1410;
const QUALITY: u16 = 1411;
const OUT: u16 = 1412;
const OUT_BROWSE: u16 = 1413;
const SUFFIX: u16 = 1414;
const AFFINE: u16 = 1415;
const PHOTO: u16 = 1416;
const CONTENT: u16 = 1417;
const STD: u16 = 1418;
const STRICT: u16 = 1419;
const THREADS: u16 = 1420;
const START_TAB: u16 = 1421;
const REMEMBER: u16 = 1422;
const AUTOCLIP: u16 = 1423;
const DEFAULTS: u16 = 1424;
const APPLY: u16 = 1425;
const STAMP: u16 = 1428;
const TSA: u16 = 1429;

const THREAD_CHOICES: [usize; 7] = [0, 1, 2, 4, 8, 12, 16];

pub struct SetPage {
    groups: [HWND; 5],
    // key
    key_path: HWND,
    key_fp: HWND,
    key_btns: [HWND; 3],
    // registry
    reg_path: HWND,
    reg_info: HWND,
    reg_btns: [HWND; 4],
    stamp: HWND,
    tsa_label: HWND,
    tsa: HWND,
    // defaults
    l_strength: HWND,
    strength: HWND,
    strength_val: HWND,
    l_fmt: HWND,
    png: HWND,
    jpg: HWND,
    l_quality: HWND,
    quality: HWND,
    l_out: HWND,
    out: HWND,
    out_browse: HWND,
    l_suffix: HWND,
    suffix: HWND,
    meta_note: HWND,
    // detection
    affine: HWND,
    photo: HWND,
    content: HWND,
    l_std: HWND,
    std: HWND,
    strict: HWND,
    l_threads: HWND,
    threads: HWND,
    threads_note: HWND,
    // general
    l_start: HWND,
    start_tab: HWND,
    remember: HWND,
    autoclip: HWND,
    // bottom
    defaults: HWND,
    apply: HWND,
}

impl SetPage {
    pub fn create(p: HWND) -> SetPage {
        let groups = [group(p, "密钥"), group(p, "登记表"), group(p, "加水印默认值"), group(p, "检测"), group(p, "常规")];
        let key_path = label(p, "");
        let key_fp = label(p, "");
        let key_btns = [button(p, NEW_KEY, "新建密钥…"), button(p, PICK_KEY, "选择密钥…"), button(p, BACKUP_KEY, "备份密钥…")];
        let reg_path = label(p, "");
        let reg_info = label(p, "");
        let reg_btns = [
            button(p, REG_DIR, "更改文件夹…"),
            button(p, REG_OPEN, "打开"),
            button(p, REG_BACKUP, "备份…"),
            button(p, REG_CSV, "导出 CSV…"),
        ];
        let stamp = checkbox(p, STAMP, "登记时添加可信时间戳（RFC 3161）");
        let tsa_label = label(p, "时间戳服务");
        let tsa = edit(p, TSA, "");
        let l_strength = label(p, "强度");
        let strength = trackbar(p, STRENGTH, 15, 40);
        let strength_val = label(p, "");
        let l_fmt = label(p, "格式");
        let png = radio(p, PNG, "PNG（无损）", true);
        let jpg = radio(p, JPG, "JPEG", false);
        let l_quality = label(p, "JPEG 质量");
        let quality = edit(p, QUALITY, "");
        unsafe {
            let st = GetWindowLongW(quality, GWL_STYLE);
            SetWindowLongW(quality, GWL_STYLE, st | ES_NUMBER);
        }
        let l_out = label(p, "输出文件夹");
        let out = edit(p, OUT, "");
        cue(out, "留空 = 保存在原图旁边");
        let out_browse = button(p, OUT_BROWSE, "浏览…");
        let l_suffix = label(p, "文件名后缀");
        let suffix = edit(p, SUFFIX, "");
        let meta_note = label(p, "输出文件不包含 EXIF、GPS 等元数据。");
        let affine = checkbox(p, AFFINE, "抗旋转、翻转、拉伸（仿射同步，较慢）");
        let photo = checkbox(p, PHOTO, "拍屏透视校正（实验性，需要登记表）");
        let content = checkbox(p, CONTENT, "与登记的原图比对画面");
        let l_std = label(p, "判定标准");
        let std = radio(p, STD, "标准（z ≥ 6.5）", true);
        let strict = radio(p, STRICT, "严格（z ≥ 7.5，误报更少）", false);
        let l_threads = label(p, "计算线程");
        let threads = combo_list(p, THREADS);
        for t in THREAD_CHOICES {
            cb_add(threads, &if t == 0 { "自动（全部核心）".to_string() } else { format!("{t}") });
        }
        let threads_note = label(p, "重启后生效");
        let l_start = label(p, "启动时打开");
        let start_tab = combo_list(p, START_TAB);
        for t in ["加水印", "检测", "登记表", "设置", "上次使用的页面"] {
            cb_add(start_tab, t);
        }
        let remember = checkbox(p, REMEMBER, "记住窗口大小和位置");
        let autoclip = checkbox(p, AUTOCLIP, "切换到“检测”时自动读取剪贴板里的图片");
        let defaults = button(p, DEFAULTS, "恢复默认值");
        let apply = default_button(p, APPLY, "应用");
        SetPage {
            groups,
            key_path,
            key_fp,
            key_btns,
            reg_path,
            reg_info,
            reg_btns,
            stamp,
            tsa_label,
            tsa,
            l_strength,
            strength,
            strength_val,
            l_fmt,
            png,
            jpg,
            l_quality,
            quality,
            l_out,
            out,
            out_browse,
            l_suffix,
            suffix,
            meta_note,
            affine,
            photo,
            content,
            l_std,
            std,
            strict,
            l_threads,
            threads,
            threads_note,
            l_start,
            start_tab,
            remember,
            autoclip,
            defaults,
            apply,
        }
    }

    fn show_strength(&self) {
        set_text(self.strength_val, &format!("{:.1}", tb_get(self.strength) as f32 / 10.0));
    }
}

pub fn is_hint(a: &App, h: HWND) -> bool {
    let s = &a.sett;
    h == s.key_fp || h == s.reg_info || h == s.meta_note || h == s.threads_note
}

pub fn layout(a: &App, w: i32, _h: i32) {
    let s = &a.sett;
    let m = sc(10);
    let bh = sc(28);
    let gap = sc(10);
    let col_w = (w - 2 * m - gap) / 2;
    let lx = m;
    let rx = m + col_w + gap;
    let pad = sc(14);
    let row = |y: i32| y;

    // --- key (left)
    let mut y = m;
    let gh = sc(112);
    mv(s.groups[0], lx, y, col_w, gh);
    mv(s.key_path, lx + pad, row(y + sc(24)), col_w - 2 * pad, sc(20));
    mv(s.key_fp, lx + pad, row(y + sc(44)), col_w - 2 * pad, sc(20));
    let mut bx = lx + pad;
    for b in s.key_btns {
        mv(b, bx, y + sc(72), sc(100), bh);
        bx += sc(106);
    }
    y += gh + gap;
    // --- registry (left)
    let rgh = sc(176);
    mv(s.groups[1], lx, y, col_w, rgh);
    mv(s.reg_path, lx + pad, y + sc(24), col_w - 2 * pad, sc(20));
    mv(s.reg_info, lx + pad, y + sc(44), col_w - 2 * pad, sc(20));
    let mut bx = lx + pad;
    for (b, bw) in s.reg_btns.iter().zip([104, 60, 70, 92]) {
        mv(*b, bx, y + sc(72), sc(bw), bh);
        bx += sc(bw) + sc(6);
    }
    mv(s.stamp, lx + pad, y + sc(108), col_w - 2 * pad, sc(22));
    mv(s.tsa_label, lx + pad, y + sc(136), sc(64), sc(20));
    mv(s.tsa, lx + pad + sc(70), y + sc(132), col_w - 2 * pad - sc(70), sc(24));
    y += rgh + gap;
    // --- defaults (left)
    let dh = sc(196);
    mv(s.groups[2], lx, y, col_w, dh);
    let fx = lx + pad + sc(80);
    let fw = col_w - 2 * pad - sc(80);
    let mut r = y + sc(24);
    mv(s.l_strength, lx + pad, r + sc(4), sc(78), sc(20));
    mv(s.strength, fx - sc(4), r - sc(2), sc(180), sc(30));
    mv(s.strength_val, fx + sc(182), r + sc(4), sc(50), sc(20));
    r += sc(32);
    mv(s.l_fmt, lx + pad, r + sc(4), sc(78), sc(20));
    mv(s.png, fx, r + sc(2), sc(110), sc(22));
    mv(s.jpg, fx + sc(114), r + sc(2), sc(64), sc(22));
    mv(s.l_quality, fx + sc(186), r + sc(4), sc(70), sc(20));
    mv(s.quality, fx + sc(258), r, sc(48), sc(24));
    r += sc(32);
    mv(s.l_out, lx + pad, r + sc(4), sc(78), sc(20));
    mv(s.out, fx, r, fw - sc(76), sc(24));
    mv(s.out_browse, fx + fw - sc(70), r - sc(2), sc(70), bh);
    r += sc(32);
    mv(s.l_suffix, lx + pad, r + sc(4), sc(78), sc(20));
    mv(s.suffix, fx, r, sc(100), sc(24));
    r += sc(32);
    mv(s.meta_note, lx + pad, r + sc(2), col_w - 2 * pad, sc(20));

    // --- detection (right)
    let mut y = m;
    let dgh = sc(190);
    mv(s.groups[3], rx, y, col_w, dgh);
    let mut r = y + sc(24);
    for c in [s.affine, s.photo, s.content] {
        mv(c, rx + pad, r, col_w - 2 * pad, sc(22));
        r += sc(26);
    }
    r += sc(6);
    mv(s.l_std, rx + pad, r + sc(3), sc(70), sc(20));
    mv(s.std, rx + pad + sc(72), r + sc(1), sc(130), sc(22));
    mv(s.strict, rx + pad + sc(206), r + sc(1), col_w - 2 * pad - sc(206), sc(22));
    r += sc(32);
    mv(s.l_threads, rx + pad, r + sc(4), sc(70), sc(20));
    mv(s.threads, rx + pad + sc(72), r, sc(150), sc(300));
    mv(s.threads_note, rx + pad + sc(230), r + sc(4), sc(100), sc(20));
    y += dgh + gap;
    // --- general (right)
    let ggh = sc(120);
    mv(s.groups[4], rx, y, col_w, ggh);
    let mut r = y + sc(24);
    mv(s.l_start, rx + pad, r + sc(4), sc(78), sc(20));
    mv(s.start_tab, rx + pad + sc(80), r, sc(160), sc(300));
    r += sc(32);
    mv(s.remember, rx + pad, r, col_w - 2 * pad, sc(22));
    r += sc(26);
    mv(s.autoclip, rx + pad, r, col_w - 2 * pad, sc(22));
    y += ggh + gap;
    // --- buttons (right, under general)
    mv(s.apply, rx + col_w - sc(90), y, sc(90), bh);
    mv(s.defaults, rx + col_w - sc(90) - sc(8) - sc(104), y, sc(104), bh);
}

fn controls_from(a: &App, c: &Settings) {
    let s = &a.sett;
    tb_set(s.strength, (c.strength * 10.0).round() as i32);
    s.show_strength();
    set_check(s.png, !c.jpeg);
    set_check(s.jpg, c.jpeg);
    set_text(s.quality, &c.jpeg_quality.to_string());
    set_text(s.out, &c.out_dir);
    set_text(s.suffix, &c.suffix);
    set_check(s.affine, c.affine);
    set_check(s.photo, c.photo);
    set_check(s.content, c.content);
    set_check(s.std, !c.strict);
    set_check(s.strict, c.strict);
    cb_set_sel(s.threads, THREAD_CHOICES.iter().position(|&t| t == c.threads).unwrap_or(0) as i32);
    cb_set_sel(s.start_tab, if c.start_tab < 0 { 4 } else { c.start_tab.min(3) });
    set_check(s.remember, c.remember_window);
    set_check(s.autoclip, c.auto_clipboard);
    set_check(s.stamp, c.timestamp);
    set_text(s.tsa, &c.tsa_url);
}

/// Refresh every control from the saved settings and the loaded key / registry.
pub fn load(a: &mut App) {
    let s = &a.sett;
    set_text(s.key_path, &format!("文件：{}", a.cfg.key_path.display()));
    set_text(
        s.key_fp,
        &match &a.key {
            Some(k) => format!("指纹 {} · 只有持有这把密钥的人能读出水印，请备份并保密", keys::fingerprint(k)),
            None if a.cfg.key_path.exists() => "密钥文件无效".into(),
            None => "尚未创建——点“新建密钥…”".into(),
        },
    );
    set_text(s.reg_path, &format!("文件夹：{}", a.cfg.registry_dir.display()));
    set_text(
        s.reg_info,
        &match (&a.reg, &a.reg_error) {
            (Some(r), _) => format!("{} 条记录 · 绑定密钥 {}", r.entries.len(), r.meta.as_ref().map(|m| m.key_fingerprint.as_str()).unwrap_or("—")),
            (None, Some(e)) => format!("不可用：{e}"),
            _ => "不可用".into(),
        },
    );
    enable(s.key_btns[2], a.key.is_some());
    let cfg = a.cfg.clone();
    controls_from(a, &cfg);
}

fn read_controls(a: &App) -> Result<Settings, String> {
    let s = &a.sett;
    let mut c = a.cfg.clone();
    c.strength = tb_get(s.strength) as f32 / 10.0;
    c.jpeg = checked(s.jpg);
    let q: u32 = get_text(s.quality).trim().parse().map_err(|_| "JPEG 质量请填 50–100 之间的数字。".to_string())?;
    if !(50..=100).contains(&q) {
        return Err("JPEG 质量请填 50–100 之间的数字。".into());
    }
    c.jpeg_quality = q as u8;
    c.out_dir = get_text(s.out).trim().to_string();
    let suffix = get_text(s.suffix).trim().to_string();
    if suffix.chars().any(|ch| r#"\/:*?"<>|"#.contains(ch)) {
        return Err("文件名后缀里不能有 \\ / : * ? \" < > | 这些字符。".into());
    }
    c.suffix = suffix;
    c.affine = checked(s.affine);
    c.photo = checked(s.photo);
    c.content = checked(s.content);
    c.strict = checked(s.strict);
    c.threads = THREAD_CHOICES[cb_sel(s.threads).clamp(0, THREAD_CHOICES.len() as i32 - 1) as usize];
    c.start_tab = match cb_sel(s.start_tab) {
        4 => -1,
        i => i.clamp(0, 3),
    };
    c.remember_window = checked(s.remember);
    c.auto_clipboard = checked(s.autoclip);
    c.timestamp = checked(s.stamp);
    let tsa = get_text(s.tsa).trim().to_string();
    if !tsa.is_empty() && !tsa.starts_with("http://") && !tsa.starts_with("https://") {
        return Err("时间戳服务地址需要以 http:// 或 https:// 开头。".into());
    }
    c.tsa_url = if tsa.is_empty() { blindmark::timestamp::DEFAULT_TSA.to_string() } else { tsa };
    Ok(c)
}

fn apply(a: &mut App) {
    match read_controls(a) {
        Ok(c) => {
            let threads_changed = c.threads != a.cfg.threads;
            a.cfg = c;
            a.cfg.save();
            // New defaults also become the current choices on the Embed page.
            super::embed::apply_defaults(a);
            a.set_status(if threads_changed { "设置已保存（线程数重启后生效）" } else { "设置已保存" });
        }
        Err(e) => warn(a.hwnd, &e),
    }
}

pub fn command(a: &mut App, id: u16, code: u32, _ctl: HWND) {
    if code != BN_CLICKED {
        return;
    }
    match id {
        NEW_KEY => a.new_key(),
        PICK_KEY => a.pick_key(),
        BACKUP_KEY => {
            let name = format!("blindmark-key-backup-{}.key", chrono::Local::now().format("%Y%m%d"));
            if let Some(p) = save_file(a.hwnd, "备份密钥", "密钥文件 (*.key)\0*.key\0", &name, "key") {
                match std::fs::copy(&a.cfg.key_path, &p) {
                    Ok(_) => info(a.hwnd, &format!("密钥已备份到：\n{}\n\n请把它放在只有你能访问的地方（例如加密的 U 盘或密码管理器）。", p.display())),
                    Err(e) => error(a.hwnd, &format!("备份失败：{e}")),
                }
            }
        }
        REG_DIR => {
            if let Some(d) = pick_folder(a.hwnd, "选择登记表文件夹（新文件夹会自动创建登记表）") {
                a.cfg.registry_dir = d;
                a.cfg.save();
                a.reload_registry();
                if let Some(e) = a.reg_error.clone() {
                    warn(a.hwnd, &format!("这个文件夹里的登记表不能和当前密钥一起使用：\n{e}"));
                }
                load(a);
            }
        }
        REG_OPEN => {
            let _ = std::fs::create_dir_all(&a.cfg.registry_dir);
            shell_open(&a.cfg.registry_dir.display().to_string());
        }
        REG_BACKUP => a.backup_registry(),
        REG_CSV => a.export_csv(),
        OUT_BROWSE => {
            if let Some(d) = pick_folder(a.hwnd, "选择默认输出文件夹") {
                set_text(a.sett.out, &d.display().to_string());
            }
        }
        DEFAULTS => {
            let mut d = Settings::default();
            d.key_path = a.cfg.key_path.clone();
            d.registry_dir = a.cfg.registry_dir.clone();
            controls_from(a, &d);
            a.set_status("已恢复默认值——点“应用”保存");
        }
        APPLY => apply(a),
        _ => {}
    }
}

pub fn on_scroll(a: &mut App, ctl: HWND) {
    if ctl == a.sett.strength {
        a.sett.show_strength();
    }
}
