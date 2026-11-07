//! 加水印 page: a file list, output settings and a background batch job.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::WindowsAndMessaging::BN_CLICKED;

use super::config::Settings;
use super::util::*;
use super::{post, App, WorkerMsg};
use blindmark::jobs::{run_embed, EmbedEvent, EmbedJob, EmbedOk, UserError};

const ADD: u16 = 1101;
const FOLDER: u16 = 1102;
const REMOVE: u16 = 1103;
const CLEAR: u16 = 1104;
const LIST: u16 = 1105;
const OUT: u16 = 1106;
const BROWSE: u16 = 1107;
const CHANNEL: u16 = 1108;
const STRENGTH: u16 = 1109;
const PNG: u16 = 1110;
const JPG: u16 = 1111;
const OPEN_AFTER: u16 = 1112;
const START: u16 = 1113;
const CANCEL: u16 = 1114;
const SAME_DIR: u16 = 1115;

pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif"];

#[derive(Clone, Copy, PartialEq)]
enum State {
    Pending,
    Running,
    Done,
    Failed,
}

struct Item {
    path: PathBuf,
    dims: Option<(u32, u32)>,
    bytes: u64,
    state: State,
    status: String,
}

pub struct EmbedPage {
    btns: [HWND; 4],
    hint: HWND,
    list: HWND,
    grp: HWND,
    labels: [HWND; 4],
    out: HWND,
    browse: HWND,
    same_dir: HWND,
    channel: HWND,
    strength: HWND,
    strength_val: HWND,
    png: HWND,
    jpg: HWND,
    open_after: HWND,
    progress: HWND,
    start: HWND,
    cancel_btn: HWND,
    items: Vec<Item>,
    pub cancel: Option<Arc<AtomicBool>>,
    run_ok: usize,
    run_fail: usize,
    run_dirs: Vec<PathBuf>,
    run_total: usize,
    run_done: usize,
}

impl EmbedPage {
    pub fn create(p: HWND, cfg: &Settings) -> EmbedPage {
        let btns = [
            button(p, ADD, "添加图片…"),
            button(p, FOLDER, "添加文件夹…"),
            button(p, REMOVE, "移除所选"),
            button(p, CLEAR, "清空"),
        ];
        let hint = label(p, "提示：可以把图片或文件夹直接拖进窗口");
        let list = listview(p, LIST, false);
        for (i, (t, wd, r)) in [("文件名", 220, false), ("尺寸", 100, false), ("大小", 80, true), ("位置", 260, false), ("状态", 240, false)]
            .iter()
            .enumerate()
        {
            lv_column(list, i as i32, t, *wd, *r);
        }
        let grp = group(p, "输出设置");
        let labels = [label(p, "输出文件夹"), label(p, "渠道"), label(p, "强度"), label(p, "格式")];
        let out = edit(p, OUT, &cfg.out_dir);
        cue(out, "留空 = 保存在原图旁边");
        let browse = button(p, BROWSE, "浏览…");
        let same_dir = button(p, SAME_DIR, "原图旁边");
        let channel = combo_edit(p, CHANNEL);
        let strength = trackbar(p, STRENGTH, 15, 40);
        tb_set(strength, (cfg.strength * 10.0).round() as i32);
        let strength_val = label(p, "");
        let png = radio(p, PNG, "PNG（无损，推荐）", true);
        let jpg = radio(p, JPG, &format!("JPEG（质量 {}）", cfg.jpeg_quality), false);
        set_check(if cfg.jpeg { jpg } else { png }, true);
        let open_after = checkbox(p, OPEN_AFTER, "完成后打开输出文件夹");
        set_check(open_after, cfg.open_folder_after);
        let progress = progress(p);
        let start = default_button(p, START, "开始加水印");
        let cancel_btn = button(p, CANCEL, "取消");
        enable(cancel_btn, false);
        let pg = EmbedPage {
            btns,
            hint,
            list,
            grp,
            labels,
            out,
            browse,
            same_dir,
            channel,
            strength,
            strength_val,
            png,
            jpg,
            open_after,
            progress,
            start,
            cancel_btn,
            items: vec![],
            cancel: None,
            run_ok: 0,
            run_fail: 0,
            run_dirs: vec![],
            run_total: 0,
            run_done: 0,
        };
        pg.show_strength();
        set_text(pg.channel, &cfg.last_channel);
        pg
    }

    fn show_strength(&self) {
        set_text(self.strength_val, &format!("{:.1}", tb_get(self.strength) as f32 / 10.0));
    }
}

pub fn is_hint(a: &App, h: HWND) -> bool {
    h == a.embed.hint
}

pub fn layout(a: &App, w: i32, h: i32) {
    let e = &a.embed;
    let m = sc(10);
    let bh = sc(28);
    let mut x = m;
    for (b, bw) in e.btns.iter().zip([96, 104, 88, 64]) {
        mv(*b, x, m, sc(bw), bh);
        x += sc(bw) + sc(6);
    }
    mv(e.hint, x + sc(8), m + sc(6), w - x - m - sc(8), sc(20));
    let bottom_row = sc(40);
    let grp_h = sc(122);
    let list_top = m + bh + sc(8);
    let grp_y = h - m - bottom_row - grp_h;
    mv(e.list, m, list_top, w - 2 * m, grp_y - list_top - sc(8));

    mv(e.grp, m, grp_y, w - 2 * m, grp_h);
    let lx = m + sc(14);
    let fx = lx + sc(76);
    let right = w - m - sc(14);
    let ch = sc(24);
    // row 1: output folder
    let r1 = grp_y + sc(24);
    mv(e.labels[0], lx, r1 + sc(4), sc(72), sc(20));
    mv(e.same_dir, right - sc(86), r1 - sc(1), sc(86), bh);
    mv(e.browse, right - sc(86) - sc(6) - sc(80), r1 - sc(1), sc(80), bh);
    mv(e.out, fx, r1, right - sc(86) - sc(12) - sc(80) - fx, ch);
    // row 2: channel + strength
    let r2 = r1 + sc(32);
    mv(e.labels[1], lx, r2 + sc(4), sc(72), sc(20));
    mv(e.channel, fx, r2, sc(230), sc(200));
    let sx = fx + sc(230) + sc(24);
    mv(e.labels[2], sx, r2 + sc(4), sc(40), sc(20));
    mv(e.strength, sx + sc(40), r2 - sc(2), sc(180), sc(30));
    mv(e.strength_val, sx + sc(224), r2 + sc(4), sc(60), sc(20));
    // row 3: format + open after
    let r3 = r2 + sc(32);
    mv(e.labels[3], lx, r3 + sc(4), sc(72), sc(20));
    mv(e.png, fx, r3 + sc(2), sc(150), sc(22));
    mv(e.jpg, fx + sc(156), r3 + sc(2), sc(150), sc(22));
    mv(e.open_after, sx + sc(40), r3 + sc(2), sc(220), sc(22));

    let by = h - m - bh;
    mv(e.cancel_btn, w - m - sc(80), by, sc(80), bh);
    mv(e.start, w - m - sc(80) - sc(8) - sc(120), by, sc(120), bh);
    mv(e.progress, m, by + sc(6), w - 2 * m - sc(216), sc(16));
}

pub fn on_dpi(a: &mut App) {
    for (i, wd) in [220, 100, 80, 260, 240].iter().enumerate() {
        lv_set_col_width(a.embed.list, i as i32, sc(*wd));
    }
}

pub fn refresh_channels(a: &mut App) {
    let e = &a.embed;
    let keep = get_text(e.channel);
    cb_clear(e.channel);
    if let Some(r) = &a.reg {
        for c in r.channels().iter().take(50) {
            cb_add(e.channel, c);
        }
    }
    set_text(e.channel, &keep);
}

fn busy(a: &App) -> bool {
    a.embed.cancel.is_some()
}

pub fn command(a: &mut App, id: u16, code: u32, _ctl: HWND) {
    if code != BN_CLICKED {
        return; // edit / combobox notifications
    }
    match id {
        ADD => add_files_dialog(a),
        FOLDER => add_folder_dialog(a),
        REMOVE => remove_selected(a),
        CLEAR => clear(a),
        BROWSE => {
            if let Some(d) = pick_folder(a.hwnd, "选择输出文件夹") {
                set_text(a.embed.out, &d.display().to_string());
            }
        }
        SAME_DIR => set_text(a.embed.out, ""),
        START => start(a),
        CANCEL => {
            if let Some(c) = &a.embed.cancel {
                c.store(true, Ordering::Relaxed);
                enable(a.embed.cancel_btn, false);
                a.set_status("正在取消…当前这张完成后停止");
            }
        }
        _ => {}
    }
}

pub fn notify(a: &mut App, nm: &NMHDR) -> LRESULT {
    if nm.idFrom as u16 == LIST && nm.code == NM_DBLCLK {
        if let Some(&row) = lv_selected(a.embed.list).first() {
            if let Some(it) = a.embed.items.get(row as usize) {
                reveal(&it.path);
            }
        }
    }
    0
}

pub fn on_scroll(a: &mut App, ctl: HWND) {
    if ctl == a.embed.strength {
        a.embed.show_strength();
    }
}

pub fn add_files_dialog(a: &mut App) {
    let files = open_files(a.hwnd, "选择要加水印的图片", IMAGE_FILTER, true);
    add_paths(a, &files);
}

pub fn add_folder_dialog(a: &mut App) {
    if let Some(d) = pick_folder(a.hwnd, "选择包含图片的文件夹（不含子文件夹）") {
        add_paths(a, &[d]);
    }
}

fn is_image(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

pub fn add_paths(a: &mut App, paths: &[PathBuf]) {
    if busy(a) {
        a.set_status("正在加水印，完成后再添加");
        return;
    }
    let mut files = vec![];
    for p in paths {
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(p) {
                let mut v: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|q| q.is_file() && is_image(q)).collect();
                v.sort();
                files.extend(v);
            }
        } else if is_image(p) {
            files.push(p.clone());
        }
    }
    let mut added = 0;
    for f in files {
        if a.embed.items.iter().any(|i| i.path == f) {
            continue;
        }
        let dims = image::image_dimensions(&f).ok();
        let bytes = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
        a.embed.items.push(Item { path: f, dims, bytes, state: State::Pending, status: "等待".into() });
        added += 1;
    }
    rebuild(a);
    a.set_status(&format!("已添加 {added} 张，共 {} 张", a.embed.items.len()));
}

fn rebuild(a: &App) {
    let e = &a.embed;
    lv_clear(e.list);
    for (i, it) in e.items.iter().enumerate() {
        let name = it.path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let dims = it.dims.map(|(w, h)| format!("{w} × {h}")).unwrap_or_else(|| "?".into());
        let dir = it.path.parent().map(|p| p.display().to_string()).unwrap_or_default();
        lv_add(e.list, &[&name, &dims, &human_size(it.bytes), &dir, &it.status], i as isize);
    }
}

/// Current file list, read-only (used by the setup wizard).
pub fn files(a: &App) -> Vec<PathBuf> {
    a.embed.items.iter().map(|i| i.path.clone()).collect()
}

fn set_row_status(a: &mut App, row: usize, s: String) {
    if let Some(it) = a.embed.items.get_mut(row) {
        it.status = s;
        lv_set(a.embed.list, row as i32, 4, &it.status);
    }
}

pub fn remove_selected(a: &mut App) {
    if busy(a) {
        return;
    }
    let sel = lv_selected(a.embed.list);
    for r in sel.into_iter().rev() {
        if (r as usize) < a.embed.items.len() {
            a.embed.items.remove(r as usize);
        }
    }
    rebuild(a);
}

pub fn clear(a: &mut App) {
    if busy(a) {
        return;
    }
    a.embed.items.clear();
    rebuild(a);
    pb_set(a.embed.progress, 0, 1);
}

fn set_busy(a: &App, on: bool) {
    let e = &a.embed;
    for b in e.btns {
        enable(b, !on);
    }
    for h in [e.out, e.browse, e.same_dir, e.channel, e.strength, e.png, e.jpg, e.start] {
        enable(h, !on);
    }
    enable(e.cancel_btn, on);
}

fn start(a: &mut App) {
    if busy(a) {
        return;
    }
    let Some(key) = a.key.clone() else {
        warn(a.hwnd, "还没有加载密钥。请先在“设置”里新建或选择密钥。");
        return;
    };
    if let Some(err) = &a.reg_error {
        warn(a.hwnd, &format!("登记表不可用，无法加水印：\n{err}"));
        return;
    }
    let jobs: Vec<(usize, PathBuf)> =
        a.embed.items.iter().enumerate().filter(|(_, i)| i.state != State::Done).map(|(k, i)| (k, i.path.clone())).collect();
    if jobs.is_empty() {
        info(a.hwnd, if a.embed.items.is_empty() { "列表是空的。先添加图片。" } else { "列表里的图片都已经加过水印了。" });
        return;
    }
    let out_text = get_text(a.embed.out).trim().to_string();
    let out_dir = if out_text.is_empty() { None } else { Some(PathBuf::from(&out_text)) };
    if let Some(d) = &out_dir {
        if let Err(e) = std::fs::create_dir_all(d) {
            error(a.hwnd, &format!("无法创建输出文件夹：\n{}\n\n{e}", d.display()));
            return;
        }
    }
    let channel = get_text(a.embed.channel).trim().to_string();
    let strength = tb_get(a.embed.strength) as f32 / 10.0;
    let jpeg = checked(a.embed.jpg);
    let quality = a.cfg.jpeg_quality;
    let suffix = a.cfg.suffix.clone();
    a.cfg.last_channel = channel.clone();
    a.cfg.open_folder_after = checked(a.embed.open_after);
    a.cfg.save();

    for (row, _) in &jobs {
        a.embed.items[*row].state = State::Pending;
        set_row_status(a, *row, "等待".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    a.embed.cancel = Some(cancel.clone());
    a.embed.run_ok = 0;
    a.embed.run_fail = 0;
    a.embed.run_dirs.clear();
    a.embed.run_total = jobs.len();
    a.embed.run_done = 0;
    pb_set(a.embed.progress, 0, jobs.len());
    set_busy(a, true);
    a.set_status(&format!("正在加水印 0 / {}", jobs.len()));

    let tx = a.tx.clone();
    let hw = a.hwnd as isize;
    let regdir = a.cfg.registry_dir.clone();
    let chan = if channel.is_empty() { None } else { Some(channel) };
    let rows: Vec<usize> = jobs.iter().map(|(row, _)| *row).collect();
    let files: Vec<PathBuf> = jobs.iter().map(|(_, path)| path.clone()).collect();
    std::thread::spawn(move || {
        let job = EmbedJob {
            files,
            out_dir,
            suffix,
            strength,
            jpeg_quality: jpeg.then_some(quality),
            channel: chan,
        };
        let cancelled = match run_embed(&job, &key, &regdir, cancel.as_ref(), &mut |ev| match ev {
            EmbedEvent::Running { index } => {
                if let Some(&row) = rows.get(index) {
                    post(&tx, hw, WorkerMsg::EmbedRunning(row));
                }
            }
            EmbedEvent::Item { index, result } => {
                if let Some(&row) = rows.get(index) {
                    post(&tx, hw, WorkerMsg::EmbedItem { row, res: result });
                }
            }
        }) {
            Ok(summary) => summary.cancelled,
            Err(e) => {
                for &row in &rows {
                    post(&tx, hw, WorkerMsg::EmbedItem { row, res: Err(e.clone()) });
                }
                false
            }
        };
        post(&tx, hw, WorkerMsg::EmbedDone { cancelled });
    });
}

pub fn on_running(a: &mut App, row: usize) {
    if let Some(it) = a.embed.items.get_mut(row) {
        it.state = State::Running;
    }
    set_row_status(a, row, "处理中…".into());
    lv_select(a.embed.list, row as i32);
}

pub fn on_item(a: &mut App, row: usize, res: Result<EmbedOk, UserError>) {
    a.embed.run_done += 1;
    match res {
        Ok(ok) => {
            let EmbedOk { id, psnr, output: out } = ok;
            a.embed.run_ok += 1;
            if let Some(d) = out.parent() {
                if !a.embed.run_dirs.iter().any(|x| x == d) {
                    a.embed.run_dirs.push(d.to_path_buf());
                }
            }
            if let Some(it) = a.embed.items.get_mut(row) {
                it.state = State::Done;
            }
            let name = out.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            set_row_status(a, row, format!("✓ 0x{id:08x} · PSNR {psnr:.1} dB → {name}"));
        }
        Err(e) => {
            eprintln!("{}", e.detail);
            a.embed.run_fail += 1;
            if let Some(it) = a.embed.items.get_mut(row) {
                it.state = State::Failed;
            }
            set_row_status(a, row, format!("✗ {}", e.message));
        }
    }
    pb_set(a.embed.progress, a.embed.run_done, a.embed.run_total);
    a.set_status(&format!("正在加水印 {} / {}", a.embed.run_done, a.embed.run_total));
}

pub fn on_done(a: &mut App, cancelled: bool) {
    a.embed.cancel = None;
    set_busy(a, false);
    for i in 0..a.embed.items.len() {
        if a.embed.items[i].state == State::Running || (cancelled && a.embed.items[i].state == State::Pending) {
            a.embed.items[i].state = State::Pending;
            set_row_status(a, i, if cancelled { "已取消".into() } else { "等待".into() });
        }
    }
    let (ok, fail) = (a.embed.run_ok, a.embed.run_fail);
    let mut s = format!("完成：成功 {ok} 张");
    if fail > 0 {
        s += &format!("，失败 {fail} 张");
    }
    if cancelled {
        s += "（已取消）";
    }
    a.set_status(&s);
    a.reload_registry();
    if ok > 0 && checked(a.embed.open_after) {
        if let Some(d) = a.embed.run_dirs.first() {
            shell_open(&d.display().to_string());
        }
    }
    if fail > 0 {
        warn(a.hwnd, &format!("{fail} 张图片没有加上水印，原因见列表中的“状态”列。"));
    }
}

/// Copy the saved defaults (strength, format, output folder) into the page controls.
pub fn apply_defaults(a: &mut App) {
    if busy(a) {
        return;
    }
    let e = &a.embed;
    tb_set(e.strength, (a.cfg.strength * 10.0).round() as i32);
    e.show_strength();
    set_check(e.png, !a.cfg.jpeg);
    set_check(e.jpg, a.cfg.jpeg);
    set_text(e.jpg, &format!("JPEG（质量 {}）", a.cfg.jpeg_quality));
    set_text(e.out, &a.cfg.out_dir);
}
