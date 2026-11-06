//! Guided Wizard97 property sheet: welcome, choose the task, then the steps that belong
//! to that task, then run.
//!
//! Layout follows the MSDN "Creating Wizards" notes: the welcome page is an exterior
//! page (PSP_HIDEHEADER, 317x193 dialog units, left 164 px kept clear for the watermark),
//! every other page is an interior page (317x143) whose header is drawn by the sheet.
//! Controls are created at run time so they follow the same DPI and font handling as the
//! tabbed main window; the templates in assets/blindmark.rc.in stay empty.
#![allow(clippy::too_many_lines)]

use std::cell::Cell;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::{DeleteObject, HBITMAP, LoadBitmapW};
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use blindmark::registry::{load_image, Registry, ThumbIndex};
use blindmark::keys;

use super::config::Settings;
use super::util::*;

// ------------------------------------------------------------------ ids

const PAGE_WELCOME: i32 = 200;
const PAGE_MODE: i32 = 201;
const PAGE_KEY: i32 = 202;
const PAGE_FILES: i32 = 203;
const PAGE_EMBED: i32 = 204;
const PAGE_DETECT: i32 = 205;
const PAGE_RUN: i32 = 206;

const BM_WATERMARK: usize = 301;
const BM_HEADER: usize = 302;

const ID_WELCOME_TITLE: u16 = 1670;
const ID_WELCOME_BODY: u16 = 1671;

const ID_EMBED: u16 = 1601;
const ID_DETECT: u16 = 1602;
const ID_MODE_HINT: u16 = 1603;

const ID_KEY_INFO: u16 = 1610;
const ID_KEY_NEW: u16 = 1611;
const ID_KEY_PICK: u16 = 1612;

const ID_LIST: u16 = 1620;
const ID_ADD_FILES: u16 = 1621;
const ID_ADD_FOLDER: u16 = 1622;
const ID_REMOVE: u16 = 1623;

const ID_STRENGTH: u16 = 1630;
const ID_STRENGTH_LABEL: u16 = 1631;
const ID_FORMAT: u16 = 1632;
const ID_QUALITY: u16 = 1633;
const ID_OUTDIR: u16 = 1634;
const ID_BROWSE: u16 = 1635;
const ID_CHANNEL: u16 = 1636;

const ID_OPT_AFFINE: u16 = 1640;
const ID_OPT_PHOTO: u16 = 1641;
const ID_OPT_CONTENT: u16 = 1642;
const ID_OPT_STRICT: u16 = 1643;

const ID_PROGRESS: u16 = 1660;
const ID_STATUS: u16 = 1661;
const ID_RESULT: u16 = 1662;

const TIMER_RUN: usize = 1;

// ------------------------------------------------------------------ state

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Embed,
    Detect,
}

enum Msg {
    Item { ok: bool, text: String },
    Fatal(String),
    Done,
}

struct State {
    mode: Option<Mode>,
    cfg: Settings,
    key: Option<Vec<u8>>,
    files: Vec<PathBuf>,
    out_dir: PathBuf,
    channel: String,
    running: bool,
    finished: bool,
    total: usize,
    done: usize,
    ok: usize,
    log: String,
    cancel: Arc<AtomicBool>,
    rx: Option<Receiver<Msg>>,
}

thread_local! {
    static STATE: Cell<*mut State> = const { Cell::new(null_mut()) };
}

fn st<'a>() -> &'a mut State {
    STATE.with(|c| unsafe { &mut *c.get() })
}

/// Run the wizard modally. Returns true when the registry was written to.
pub fn open(owner: HWND, cfg: &mut Settings) -> bool {
    let mut boxed = Box::new(State {
        mode: None,
        cfg: cfg.clone(),
        key: None,
        files: Vec::new(),
        out_dir: PathBuf::from(&cfg.out_dir),
        channel: cfg.last_channel.clone(),
        running: false,
        finished: false,
        total: 0,
        done: 0,
        ok: 0,
        log: String::new(),
        cancel: Arc::new(AtomicBool::new(false)),
        rx: None,
    });
    let raw: *mut State = &mut *boxed;
    STATE.with(|c| c.set(raw));

    let mut pages = vec![
        page(PAGE_WELCOME, "", "", true, page_welcome),
        page(PAGE_MODE, "选择任务", "你想给图片加水印，还是检查图片里有没有你的水印？", false, page_mode),
        page(PAGE_KEY, "密钥", "该密钥决定谁能读出你的水印。", false, page_key),
        page(PAGE_FILES, "选择图片", "可以一次加入多张。", false, page_files),
        page(PAGE_EMBED, "加水印设置", "强度、格式与输出位置。", false, page_embed),
        page(PAGE_DETECT, "检测选项", "按需要开启更费时的步骤。", false, page_detect),
        page(PAGE_RUN, "执行", "完成后关闭向导。", false, page_run),
    ];

    let bm_watermark = load_bitmap(BM_WATERMARK);
    let bm_header = load_bitmap(BM_HEADER);

    // No PSH_HEADER / PSH_WATERMARK: the sheet turns the banner bitmap into a tiled brush
    // and drops the header text, so the strip stays blank for the pages to paint.
    let mut hdr: PROPSHEETHEADERW_V2 = unsafe { std::mem::zeroed() };
    hdr.dwSize = std::mem::size_of::<PROPSHEETHEADERW_V2>() as u32;
    hdr.dwFlags = PSH_WIZARD97 | PSH_PROPSHEETPAGE;
    hdr.hwndParent = owner;
    hdr.hInstance = hinst();
    hdr.Anonymous1 = PROPSHEETHEADERW_V2_0 { pszIcon: null() };
    hdr.pszCaption = leak("blindmark 向导");
    hdr.nPages = pages.len() as u32;
    hdr.Anonymous2 = PROPSHEETHEADERW_V2_1 { nStartPage: 0 };
    hdr.Anonymous3 = PROPSHEETHEADERW_V2_2 { ppsp: pages.as_mut_ptr() };
    hdr.pfnCallback = None;
    hdr.Anonymous4 = PROPSHEETHEADERW_V2_3 { hbmWatermark: bm_watermark };
    hdr.hplWatermark = null_mut();
    hdr.Anonymous5 = PROPSHEETHEADERW_V2_4 { hbmHeader: bm_header };

    let ret = unsafe { PropertySheetW(&mut hdr) };

    STATE.with(|c| c.set(null_mut()));
    let done = unsafe { Box::from_raw(raw) };
    unsafe {
        if !bm_watermark.is_null() {
            DeleteObject(bm_watermark as *mut _);
        }
        if !bm_header.is_null() {
            DeleteObject(bm_header as *mut _);
        }
    }

    let accepted = ret == IDOK as isize;
    if accepted {
        cfg.strength = done.cfg.strength;
        cfg.jpeg = done.cfg.jpeg;
        cfg.jpeg_quality = done.cfg.jpeg_quality;
        cfg.out_dir = done.out_dir.display().to_string();
        if done.out_dir.as_os_str().is_empty() {
            cfg.out_dir.clear();
        }
        cfg.last_channel = done.channel;
        cfg.affine = done.cfg.affine;
        cfg.photo = done.cfg.photo;
        cfg.content = done.cfg.content;
        cfg.strict = done.cfg.strict;
        cfg.save();
    }
    accepted && done.ok > 0
}

fn load_bitmap(id: usize) -> HBITMAP {
    unsafe {
        let ddb = LoadBitmapW(hinst(), id as usize as *const u16);
        if !ddb.is_null() {
            return ddb;
        }
        LoadImageW(hinst(), id as usize as *const u16, IMAGE_BITMAP, 0, 0, LR_CREATEDIBSECTION) as HBITMAP
    }
}

fn leak(s: &str) -> windows_sys::core::PCWSTR {
    Box::leak(w(s).into_boxed_slice()).as_ptr()
}

type PageProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> isize;

fn page(id: i32, title: &'static str, subtitle: &'static str, hide_header: bool, proc_: PageProc) -> PROPSHEETPAGEW {
    let mut p: PROPSHEETPAGEW = unsafe { std::mem::zeroed() };
    p.dwSize = std::mem::size_of::<PROPSHEETPAGEW>() as u32;
    p.dwFlags = if hide_header { PSP_HIDEHEADER } else { PSP_USEHEADERTITLE | PSP_USEHEADERSUBTITLE };
    p.hInstance = hinst();
    p.Anonymous1 = PROPSHEETPAGEW_0 { pszTemplate: id as usize as *const u16 };
    p.pfnDlgProc = Some(proc_);
    if !hide_header {
        p.pszHeaderTitle = leak(title);
        p.pszHeaderSubTitle = leak(subtitle);
    }
    p
}

// ------------------------------------------------------------------ helpers

fn ctl(h: HWND, id: u16) -> HWND {
    unsafe { GetDlgItem(h, id as i32) }
}

fn sheet(h: HWND) -> HWND {
    unsafe { GetParent(h) }
}

fn set_buttons(h: HWND, flags: u32) {
    send(sheet(h), PSM_SETWIZBUTTONS, 0, flags as LPARAM);
}

/// Hold the current page: DWL_MSGRESULT = -1 and TRUE, as the SDK documents.
fn block(h: HWND) -> isize {
    unsafe { SetWindowLongPtrW(h, DWLP_MSGRESULT as i32, -1) };
    1
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

fn images_in(dir: &std::path::Path) -> Vec<PathBuf> {
    const EXT: [&str; 8] = ["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif"];
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.is_file()
                        && p.extension()
                            .and_then(|e| e.to_str())
                            .map(|e| EXT.contains(&e.to_ascii_lowercase().as_str()))
                            .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn label_id(parent: HWND, id: u16, text: &str) -> HWND {
    create(windows_sys::core::w!("STATIC"), text, 0, 0, parent, id)
}

fn combo_text(h: HWND) -> String {
    let i = cb_sel(h);
    if i < 0 {
        String::new()
    } else {
        cb_item_text(h, i)
    }
}

fn create_readonly(parent: HWND, id: u16) -> HWND {
    let style = (WS_VSCROLL as u32) | (ES_MULTILINE as u32) | (ES_READONLY as u32) | (ES_AUTOVSCROLL as u32);
    create(windows_sys::core::w!("EDIT"), "", style, WS_EX_CLIENTEDGE, parent, id)
}

// ------------------------------------------------------------------ page 200: welcome

fn build_welcome(h: HWND) {
    label_id(h, ID_WELCOME_TITLE, "欢迎使用 blindmark");
    label_id(
        h,
        ID_WELCOME_BODY,
        "这个向导会分几步带你完成一件事：给图片嵌入可追溯的编号，\
         或者检查一张图里有没有你自己的编号。\n\n\
         所有步骤都可以随时按“上一步”返回修改。\n\n\
         先确认一件事：你手上有密钥文件吗？密钥决定谁能读出图片里的编号，\
         第一次使用时向导会帮你新建一把。",
    );
}

fn layout_welcome(h: HWND) {
    let (w, ht) = client_size(h);
    let left = sc(164) + sc(18);
    let right = sc(18);
    mv(ctl(h, ID_WELCOME_TITLE), left, sc(22), w - left - right, sc(34));
    mv(ctl(h, ID_WELCOME_BODY), left, sc(64), w - left - right, ht - sc(84));
}

unsafe extern "system" fn page_welcome(h: HWND, msg: u32, _wp: WPARAM, lp: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            build_welcome(h);
            set_font_tree(h, super::fonts().normal);
            set_font(ctl(h, ID_WELCOME_TITLE), super::fonts().big);
            layout_welcome(h);
            1
        }
        WM_SIZE => {
            layout_welcome(h);
            1
        }
        WM_NOTIFY => {
            let nm = unsafe { &*(lp as *const NMHDR) };
            if nm.code == PSN_SETACTIVE {
                set_buttons(h, PSWIZB_NEXT);
                return 1;
            }
            0
        }
        _ => 0,
    }
}

// ------------------------------------------------------------------ page 201: task

fn build_mode(h: HWND) {
    radio(h, ID_EMBED, "给图片加水印", true);
    radio(h, ID_DETECT, "检测图片水印", false);
    label_id(h, ID_MODE_HINT, "选好后按“下一步”。之后可以随时用“上一步”回来改。");
}

fn layout_mode(h: HWND) {
    let (w, ht) = client_size(h);
    let m = sc(12);
    mv(ctl(h, ID_EMBED), m, m, w - 2 * m, sc(24));
    mv(ctl(h, ID_DETECT), m, m + sc(30), w - 2 * m, sc(24));
    mv(ctl(h, ID_MODE_HINT), m, ht - m - sc(20), w - 2 * m, sc(20));
}

unsafe extern "system" fn page_mode(h: HWND, msg: u32, _wp: WPARAM, lp: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            build_mode(h);
            set_font_tree(h, super::fonts().normal);
            layout_mode(h);
            1
        }
        WM_SIZE => {
            layout_mode(h);
            1
        }
        WM_NOTIFY => {
            let nm = unsafe { &*(lp as *const NMHDR) };
            match nm.code {
                PSN_SETACTIVE => {
                    if let Some(m) = st().mode {
                        set_check(ctl(h, ID_EMBED), m == Mode::Embed);
                        set_check(ctl(h, ID_DETECT), m == Mode::Detect);
                    }
                    set_buttons(h, PSWIZB_BACK | PSWIZB_NEXT);
                    return 1;
                }
                PSN_WIZNEXT => {
                    if !checked(ctl(h, ID_EMBED)) && !checked(ctl(h, ID_DETECT)) {
                        warn(h, "请先选择要加水印还是检测水印。");
                        return block(h);
                    }
                    st().mode = Some(if checked(ctl(h, ID_EMBED)) { Mode::Embed } else { Mode::Detect });
                    return 0;
                }
                _ => {}
            }
            0
        }
        _ => 0,
    }
}

// ------------------------------------------------------------------ page 202: key

fn build_key(h: HWND) {
    create_readonly(h, ID_KEY_INFO);
    button(h, ID_KEY_NEW, "新建密钥…");
    button(h, ID_KEY_PICK, "选择密钥文件…");
}

fn layout_key(h: HWND) {
    let (w, ht) = client_size(h);
    let m = sc(12);
    mv(ctl(h, ID_KEY_INFO), m, m, w - 2 * m, ht - 2 * m - sc(34));
    let bw = sc(120);
    let by = ht - m - sc(28);
    mv(ctl(h, ID_KEY_NEW), m, by, bw, sc(28));
    mv(ctl(h, ID_KEY_PICK), m + bw + sc(8), by, bw + sc(26), sc(28));
}

fn key_info_text() -> String {
    let s = st();
    match &s.key {
        Some(k) => format!(
            "当前密钥\n{}\n\n指纹 {}\n\n密钥决定谁能读出水印。请把它备份到安全的地方，并且不要分享——丢了密钥就再也检测不出自己的水印。",
            s.cfg.key_path.display(),
            keys::fingerprint(k)
        ),
        None => format!(
            "还没有可用的密钥。\n\n默认位置：{}\n\n点下面的按钮新建一把，或者选择一个已有的密钥文件。",
            s.cfg.key_path.display()
        ),
    }
}

fn load_key_from_settings() -> bool {
    let s = st();
    let path = s.cfg.key_path.clone();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(k) = keys::parse_key_file(&text) {
            s.key = Some(k);
            return true;
        }
    }
    s.key = None;
    false
}

unsafe extern "system" fn page_key(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            build_key(h);
            set_font_tree(h, super::fonts().normal);
            layout_key(h);
            return 1;
        }
        WM_SIZE => {
            layout_key(h);
            return 1;
        }
        WM_COMMAND => {
            match (wp & 0xffff) as u16 {
                ID_KEY_NEW => {
                    let s = st();
                    let path = s.cfg.key_path.clone();
                    if path.exists() {
                        error(
                            h,
                            &format!("{} 已经存在。向导不会覆盖已有密钥，请先把它挪走，或在“设置”里换一个位置。", path.display()),
                        );
                        return 1;
                    }
                    let res = (|| -> anyhow::Result<()> {
                        if let Some(d) = path.parent() {
                            std::fs::create_dir_all(d)?;
                        }
                        let k = keys::generate_key()?;
                        std::fs::write(&path, hex::encode(k) + "\n")?;
                        Ok(())
                    })();
                    match res {
                        Ok(()) => {
                            load_key_from_settings();
                            set_text(ctl(h, ID_KEY_INFO), &key_info_text());
                        }
                        Err(e) => error(h, &format!("新建密钥失败：{e}")),
                    }
                    return 1;
                }
                ID_KEY_PICK => {
                    if let Some(p) = open_files(h, "选择密钥文件", "密钥文件 (*.key)\0*.key\0所有文件 (*.*)\0*.*\0", false).into_iter().next() {
                        match std::fs::read_to_string(&p).map_err(anyhow::Error::from).and_then(|t| keys::parse_key_file(&t)) {
                            Ok(k) => {
                                let s = st();
                                s.key = Some(k);
                                s.cfg.key_path = p;
                                set_text(ctl(h, ID_KEY_INFO), &key_info_text());
                            }
                            Err(e) => error(h, &format!("这不是有效的密钥文件：{e}")),
                        }
                    }
                    return 1;
                }
                _ => {}
            }
            0
        }
        WM_NOTIFY => {
            let nm = unsafe { &*(lp as *const NMHDR) };
            match nm.code {
                PSN_SETACTIVE => {
                    if st().key.is_none() {
                        load_key_from_settings();
                    }
                    set_text(ctl(h, ID_KEY_INFO), &key_info_text());
                    set_buttons(h, PSWIZB_BACK | PSWIZB_NEXT);
                    return 1;
                }
                PSN_WIZNEXT => {
                    if st().key.is_none() {
                        error(h, "还没有可用的密钥。请先新建一把，或者选择一个已有的密钥文件。");
                        return block(h);
                    }
                    return 0;
                }
                _ => {}
            }
            0
        }
        _ => 0,
    }
}

// ------------------------------------------------------------------ page 203: files

fn build_files(h: HWND) {
    let lv = listview(h, ID_LIST, true);
    lv_column(lv, 0, "文件", sc(260), false);
    button(h, ID_ADD_FILES, "添加图片…");
    button(h, ID_ADD_FOLDER, "添加文件夹…");
    button(h, ID_REMOVE, "移除所选");
}

fn layout_files(h: HWND) {
    let (w, ht) = client_size(h);
    let m = sc(12);
    mv(ctl(h, ID_LIST), m, m, w - 2 * m, ht - 2 * m - sc(34));
    let bw = sc(104);
    let by = ht - m - sc(28);
    mv(ctl(h, ID_ADD_FILES), m, by, bw, sc(28));
    mv(ctl(h, ID_ADD_FOLDER), m + bw + sc(8), by, bw + sc(18), sc(28));
    mv(ctl(h, ID_REMOVE), m + 2 * (bw + sc(8)) + sc(12), by, bw, sc(28));
}

fn refresh_files(h: HWND) {
    let lv = ctl(h, ID_LIST);
    lv_clear(lv);
    let rows: Vec<String> = st().files.iter().map(|p| file_name(p)).collect();
    for (i, name) in rows.iter().enumerate() {
        lv_add(lv, &[name.as_str()], i as isize);
    }
}

unsafe extern "system" fn page_files(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            build_files(h);
            set_font_tree(h, super::fonts().normal);
            layout_files(h);
            return 1;
        }
        WM_SIZE => {
            layout_files(h);
            return 1;
        }
        WM_COMMAND => {
            match (wp & 0xffff) as u16 {
                ID_ADD_FILES => {
                    for p in open_files(h, "添加图片", IMAGE_FILTER, true) {
                        st().files.push(p);
                    }
                    refresh_files(h);
                    return 1;
                }
                ID_ADD_FOLDER => {
                    if let Some(d) = pick_folder(h, "添加文件夹中的图片") {
                        let found = images_in(&d);
                        st().files.extend(found);
                        refresh_files(h);
                    }
                    return 1;
                }
                ID_REMOVE => {
                    let sel = lv_selected(ctl(h, ID_LIST));
                    if !sel.is_empty() {
                        let mut files = st().files.clone();
                        for i in sel.into_iter().rev() {
                            if (i as usize) < files.len() {
                                files.remove(i as usize);
                            }
                        }
                        st().files = files;
                        refresh_files(h);
                    }
                    return 1;
                }
                _ => {}
            }
            0
        }
        WM_NOTIFY => {
            let nm = unsafe { &*(lp as *const NMHDR) };
            match nm.code {
                PSN_SETACTIVE => {
                    refresh_files(h);
                    set_buttons(h, PSWIZB_BACK | PSWIZB_NEXT);
                    return 1;
                }
                PSN_WIZNEXT => {
                    if st().files.is_empty() {
                        warn(h, "先加入至少一张图片。");
                        return block(h);
                    }
                    return 0;
                }
                _ => {}
            }
            0
        }
        _ => 0,
    }
}

// ------------------------------------------------------------------ page 204: embed parameters

fn build_embed(h: HWND) {
    label_id(h, 1680, "强度");
    let tb = trackbar(h, ID_STRENGTH, 5, 60);
    tb_set(tb, (st().cfg.strength * 10.0) as i32);
    label_id(h, ID_STRENGTH_LABEL, "");
    label_id(h, 1681, "格式");
    let fmt = combo_list(h, ID_FORMAT);
    cb_add(fmt, "PNG");
    cb_add(fmt, "JPEG");
    cb_set_sel(fmt, 0);
    label_id(h, 1682, "质量");
    let q = combo_list(h, ID_QUALITY);
    for v in (60..=100).step_by(5) {
        cb_add(q, &v.to_string());
    }
    cb_set_sel(q, 4);
    label_id(h, 1683, "输出文件夹");
    edit(h, ID_OUTDIR, "");
    button(h, ID_BROWSE, "浏览…");
    label_id(h, 1684, "渠道备注");
    edit(h, ID_CHANNEL, "");
}

fn layout_embed(h: HWND) {
    let (w, _ht) = client_size(h);
    let m = sc(12);
    let row = sc(28);
    let mut y = m;
    mv(ctl(h, 1680), m, y + sc(4), sc(36), sc(18));
    mv(ctl(h, ID_STRENGTH), m + sc(40), y, w - 2 * m - sc(100), row);
    mv(ctl(h, ID_STRENGTH_LABEL), w - m - sc(52), y + sc(4), sc(52), sc(18));
    y += row;
    mv(ctl(h, 1681), m, y + sc(4), sc(36), sc(18));
    mv(ctl(h, ID_FORMAT), m + sc(40), y, sc(84), sc(200));
    mv(ctl(h, 1682), m + sc(140), y + sc(4), sc(36), sc(18));
    mv(ctl(h, ID_QUALITY), m + sc(178), y, sc(56), sc(200));
    y += row;
    mv(ctl(h, 1683), m, y + sc(4), sc(64), sc(18));
    mv(ctl(h, ID_OUTDIR), m + sc(68), y, w - 2 * m - sc(146), sc(24));
    mv(ctl(h, ID_BROWSE), w - m - sc(70), y, sc(70), sc(26));
    y += row;
    mv(ctl(h, 1684), m, y + sc(4), sc(64), sc(18));
    mv(ctl(h, ID_CHANNEL), m, y + sc(22), w - 2 * m, sc(24));
}

unsafe extern "system" fn page_embed(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            build_embed(h);
            set_font_tree(h, super::fonts().normal);
            layout_embed(h);
            return 1;
        }
        WM_SIZE => {
            layout_embed(h);
            return 1;
        }
        WM_COMMAND => {
            match (wp & 0xffff) as u16 {
                ID_BROWSE => {
                    if let Some(d) = pick_folder(h, "选择输出文件夹") {
                        set_text(ctl(h, ID_OUTDIR), &d.display().to_string());
                    }
                    return 1;
                }
                ID_STRENGTH => {
                    let v = tb_get(ctl(h, ID_STRENGTH));
                    set_text(ctl(h, ID_STRENGTH_LABEL), &format!("{:.1}", v as f32 / 10.0));
                    return 1;
                }
                _ => {}
            }
            0
        }
        WM_NOTIFY => {
            let nm = unsafe { &*(lp as *const NMHDR) };
            match nm.code {
                PSN_SETACTIVE => {
                    if st().mode != Some(Mode::Embed) {
                        return block(h);
                    }
                    set_buttons(h, PSWIZB_BACK | PSWIZB_NEXT);
                    return 1;
                }
                PSN_WIZNEXT => {
                    let s = st();
                    let v = tb_get(ctl(h, ID_STRENGTH));
                    s.cfg.strength = v as f32 / 10.0;
                    s.cfg.jpeg = cb_sel(ctl(h, ID_FORMAT)) == 1;
                    let q = combo_text(ctl(h, ID_QUALITY));
                    s.cfg.jpeg_quality = q.parse().unwrap_or(95);
                    let out = get_text(ctl(h, ID_OUTDIR));
                    s.out_dir = if out.trim().is_empty() { PathBuf::new() } else { PathBuf::from(out.trim()) };
                    s.channel = get_text(ctl(h, ID_CHANNEL));
                    return 0;
                }
                _ => {}
            }
            0
        }
        _ => 0,
    }
}

// ------------------------------------------------------------------ page 205: detect options

fn build_detect(h: HWND) {
    let s = st();
    checkbox(h, ID_OPT_AFFINE, "仿射同步：处理旋转、镜像、拉伸与错切");
    checkbox(h, ID_OPT_PHOTO, "拍屏校正：借助登记表把照片拉正（实验性）");
    checkbox(h, ID_OPT_CONTENT, "画面比对：核对画面是否与登记的图片一致");
    checkbox(h, ID_OPT_STRICT, "严格判定：提高同步阈值，少误报");
    set_check(ctl(h, ID_OPT_AFFINE), s.cfg.affine);
    set_check(ctl(h, ID_OPT_PHOTO), s.cfg.photo);
    set_check(ctl(h, ID_OPT_CONTENT), s.cfg.content);
    set_check(ctl(h, ID_OPT_STRICT), s.cfg.strict);
}

fn layout_detect(h: HWND) {
    let (w, _ht) = client_size(h);
    let m = sc(12);
    let mut y = m;
    for id in [ID_OPT_AFFINE, ID_OPT_PHOTO, ID_OPT_CONTENT, ID_OPT_STRICT] {
        mv(ctl(h, id), m, y, w - 2 * m, sc(22));
        y += sc(30);
    }
}

unsafe extern "system" fn page_detect(h: HWND, msg: u32, _wp: WPARAM, lp: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            build_detect(h);
            set_font_tree(h, super::fonts().normal);
            layout_detect(h);
            return 1;
        }
        WM_SIZE => {
            layout_detect(h);
            return 1;
        }
        WM_NOTIFY => {
            let nm = unsafe { &*(lp as *const NMHDR) };
            match nm.code {
                PSN_SETACTIVE => {
                    if st().mode != Some(Mode::Detect) {
                        return block(h);
                    }
                    set_buttons(h, PSWIZB_BACK | PSWIZB_NEXT);
                    return 1;
                }
                PSN_WIZNEXT => {
                    let s = st();
                    s.cfg.affine = checked(ctl(h, ID_OPT_AFFINE));
                    s.cfg.photo = checked(ctl(h, ID_OPT_PHOTO));
                    s.cfg.content = checked(ctl(h, ID_OPT_CONTENT));
                    s.cfg.strict = checked(ctl(h, ID_OPT_STRICT));
                    return 0;
                }
                _ => {}
            }
            0
        }
        _ => 0,
    }
}

// ------------------------------------------------------------------ page 206: run

fn build_run(h: HWND) {
    progress(h);
    label_id(h, ID_STATUS, "准备中…");
    create_readonly(h, ID_RESULT);
}

fn layout_run(h: HWND) {
    let (w, ht) = client_size(h);
    let m = sc(12);
    mv(ctl(h, ID_PROGRESS), m, m, w - 2 * m, sc(16));
    mv(ctl(h, ID_STATUS), m, m + sc(22), w - 2 * m, sc(18));
    mv(ctl(h, ID_RESULT), m, m + sc(46), w - 2 * m, ht - 2 * m - sc(52));
}

fn start_run(h: HWND) {
    let s = st();
    if s.running {
        return;
    }
    let Some(key) = s.key.clone() else {
        error(h, "先加载密钥。");
        return;
    };
    let files = s.files.clone();
    if files.is_empty() {
        error(h, "先加入图片。");
        return;
    }
    let (tx, rx) = channel();
    s.rx = Some(rx);
    s.running = true;
    s.finished = false;
    s.total = files.len();
    s.done = 0;
    s.ok = 0;
    s.log.clear();
    set_text(ctl(h, ID_RESULT), "");
    set_text(ctl(h, ID_STATUS), "开始处理…");
    pb_set(ctl(h, ID_PROGRESS), 0, files.len());
    send(sheet(h), PSM_SETWIZBUTTONS, 0, PSWIZB_DISABLEDFINISH as LPARAM);

    let cancel = s.cancel.clone();
    cancel.store(false, Ordering::SeqCst);
    let cfg = s.cfg.clone();
    let out_dir = s.out_dir.clone();
    let channel = s.channel.clone();
    let mode = s.mode.unwrap_or(Mode::Embed);
    let suffix = cfg.suffix.clone();
    let reg_dir = cfg.registry_dir.clone();
    let quality = cfg.jpeg_quality;
    let jpeg = cfg.jpeg;
    let strength = cfg.strength;

    std::thread::spawn(move || {
        let mut reg = Registry::open(&reg_dir).ok();
        match mode {
            Mode::Embed => {
                let Some(reg) = reg.as_mut() else {
                    let _ = tx.send(Msg::Fatal("登记表无法打开".into()));
                    return;
                };
                for f in &files {
                    if cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    let out = if out_dir.as_os_str().is_empty() {
                        blindmark::unique_output_path(f.parent().unwrap_or(&std::path::PathBuf::from(".")), f, &suffix, jpeg)
                    } else {
                        blindmark::unique_output_path(&out_dir, f, &suffix, jpeg)
                    };
                    let name = f.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    let ch = if channel.trim().is_empty() { None } else { Some(channel.clone()) };
                    match blindmark::embed_file(f, &out, &key, reg, strength, ch, jpeg.then_some(quality)) {
                        Ok(r) => {
                            let _ = tx.send(Msg::Item {
                                ok: true,
                                text: format!("{name} — 编号 0x{:08x} · PSNR {:.1} dB", r.id, r.psnr),
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(Msg::Item { ok: false, text: format!("{name} — 失败：{e:#}") });
                        }
                    }
                }
            }
            Mode::Detect => {
                let known = reg.as_ref().map(|r| blindmark::wm::KnownIds::new(&key, r.ids()));
                let thumbs = ThumbIndex::new();
                for f in &files {
                    if cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    let name = f.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    match load_image(f) {
                        Ok(img) => {
                            let rep = blindmark::detect_image(&img, &key, reg.as_ref(), known.as_ref(), Some(&thumbs));
                            let _ = tx.send(Msg::Item { ok: rep.det.found, text: describe(&name, &rep) });
                        }
                        Err(e) => {
                            let _ = tx.send(Msg::Item { ok: false, text: format!("{name} — 读不出图片：{e:#}") });
                        }
                    }
                }
            }
        }
        let _ = tx.send(Msg::Done);
    });

    unsafe { SetTimer(h, TIMER_RUN, 100, None) };
}

fn describe(name: &str, rep: &blindmark::Report) -> String {
    if !rep.det.found {
        return format!("{name} — 未检测到水印");
    }
    match rep.det.image_id {
        Some(id) => format!(
            "{name} — 编号 0x{id:08x} · 同步 {:.1} · 画面 {}",
            rep.det.z_sync,
            rep.content_similarity.map(|v| format!("{v:.2}")).unwrap_or_else(|| "—".into())
        ),
        None => format!("{name} — 检测到水印，但无法确定是哪一张（同步 {:.1}）", rep.det.z_sync),
    }
}

fn drain_run(h: HWND) {
    let msgs: Vec<Msg> = {
        let s = st();
        match &s.rx {
            Some(rx) => {
                let mut v = vec![];
                while let Ok(m) = rx.try_recv() {
                    v.push(m);
                }
                v
            }
            None => vec![],
        }
    };
    if msgs.is_empty() {
        return;
    }
    let mut finished = false;
    for m in msgs {
        match m {
            Msg::Item { ok, text } => {
                let s = st();
                s.done += 1;
                if ok {
                    s.ok += 1;
                }
                s.log.push_str(&text);
                s.log.push_str("\r\n");
            }
            Msg::Fatal(e) => {
                st().log.push_str(&format!("错误：{e}\r\n"));
                finished = true;
            }
            Msg::Done => finished = true,
        }
    }
    {
        let s = st();
        let (done, total) = (s.done, s.total);
        set_text(ctl(h, ID_RESULT), &s.log);
        set_text(ctl(h, ID_STATUS), &format!("{done} / {total} 完成"));
        pb_set(ctl(h, ID_PROGRESS), done, total.max(1));
    }
    if finished {
        let s = st();
        s.running = false;
        s.finished = true;
        s.rx = None;
        unsafe { KillTimer(h, TIMER_RUN) };
        set_text(ctl(h, ID_STATUS), &format!("完成：{} / {} 张成功", s.ok, s.total));
        send(sheet(h), PSM_SETWIZBUTTONS, 0, PSWIZB_FINISH as LPARAM);
    }
}

unsafe extern "system" fn page_run(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            build_run(h);
            set_font_tree(h, super::fonts().normal);
            layout_run(h);
            return 1;
        }
        WM_SIZE => {
            layout_run(h);
            return 1;
        }
        WM_TIMER => {
            if wp == TIMER_RUN {
                drain_run(h);
                return 1;
            }
            0
        }
        WM_DESTROY => {
            unsafe { KillTimer(h, TIMER_RUN) };
            0
        }
        WM_NOTIFY => {
            let nm = unsafe { &*(lp as *const NMHDR) };
            match nm.code {
                PSN_SETACTIVE => {
                    if !st().running && !st().finished {
                        start_run(h);
                    }
                    if st().running {
                        send(sheet(h), PSM_SETWIZBUTTONS, 0, PSWIZB_DISABLEDFINISH as LPARAM);
                    } else {
                        send(sheet(h), PSM_SETWIZBUTTONS, 0, PSWIZB_FINISH as LPARAM);
                    }
                    return 1;
                }
                PSN_QUERYCANCEL => {
                    if st().running {
                        if ask(h, "任务还在进行。要中止并关闭向导吗？") {
                            st().cancel.store(true, Ordering::SeqCst);
                            return 0;
                        }
                        return 1;
                    }
                    return 0;
                }
                PSN_WIZBACK => {
                    if st().running {
                        return 1;
                    }
                }
                _ => {}
            }
            0
        }
        _ => 0,
    }
}
