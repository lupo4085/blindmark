//! New setup wizard (W-1): welcome page, task page and a temporary placeholder page.
//!
//! This is the replacement for `wizard.rs`; the old file is kept untouched until W-4.
//! State ownership: one `Box<St>` handed out through `PROPSHEETPAGEW.lParam`, stored into
//! `GWLP_USERDATA` by each page, and reclaimed exactly once after `PropertySheetW`.
#![allow(clippy::too_many_lines)]
#![allow(unsafe_op_in_unsafe_fn)]

use std::path::PathBuf;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::config::Settings;
use super::util::*;

// ------------------------------------------------------------------ ids

const PAGE_WELCOME: i32 = 210;
const PAGE_TASK: i32 = 211;
const PAGE_PLACEHOLDER: i32 = 219;

const BM_WATERMARK: usize = 301;

const ID_WELCOME_BODY: u16 = 1720;
const ID_SHOW_ON_START: u16 = 1721;

const ID_TASK_RADIO: [u16; 4] = [1731, 1732, 1733, 1734];
const ID_TASK_DESC: [u16; 4] = [1741, 1742, 1743, 1744];

const ID_PLACEHOLDER_BODY: u16 = 1750;

/// `SetWindowLongPtrW` takes an `i32` index; windows-sys exposes `DWLP_MSGRESULT` as `u32`.
const DWLP_MSGRESULT: i32 = windows_sys::Win32::UI::WindowsAndMessaging::DWLP_MSGRESULT as i32;

/// Copy from `ui-copy.md` section 2 ("向导内部意外（panic）").
const PANIC_COPY: &str = "向导遇到了意外问题，已经关闭。你的图片、密钥和登记表都没有受影响。";

// ------------------------------------------------------------------ copy

const WELCOME_TITLE: &str = "欢迎使用 BlindMark";
const WELCOME_SUBTITLE: &str = "给图片加上看不见的水印，证明图片是你的";
const WELCOME_BODY: &str = "BlindMark 会在图片里加入肉眼看不见的编号。图片被转发、截图或压缩后，通常仍然能读出这个编号，用来证明图片是你的，并查出是从哪一份副本流出去的。\n\n这个向导会带你完成：\n· 首次设置：创建密钥、选择登记表的位置\n· 给图片加水印\n· 分发给多人：每人一份独立编号的副本\n· 检查可疑图片\n\n所有处理都在你的电脑上完成，图片不会上传。";
const SHOW_ON_START: &str = "启动时显示此向导";

const TASK_TITLE: &str = "选择要做的事";
const TASK_SUBTITLE: &str = "以后随时可以按 F1 回到这里。";
const TASK_TITLES: [&str; 4] = ["完成首次设置", "给图片加水印", "分发给多人", "检查可疑图片"];
const TASK_TITLE_EXISTING: &str = "更换密钥或登记表";
const TASK_DESCS: [&str; 4] = [
    "创建密钥，并选择登记表的保存位置。",
    "为每张图片生成一份带编号的副本，编号会记进登记表。",
    "给每位接收人各生成一份编号不同的副本，泄露后能查出是谁。",
    "看图片里有没有你的水印，以及它是哪一份副本。",
];
const TASK_DESC_EXISTING: &str = "改用另一把密钥，或换一个登记表文件夹。";

const PLACEHOLDER_TITLE: &str = "这一步还在制作中";

const PLACEHOLDER_SETUP: &str = "“完成首次设置”的向导流程会在后续版本中提供。现在可以先在主窗口里完成这件事：“设置”选项卡。";
const PLACEHOLDER_EMBED: &str = "“给图片加水印”的向导流程会在后续版本中提供。现在可以先在主窗口里完成这件事：“加水印”选项卡。";
const PLACEHOLDER_DISTRIBUTE: &str = "“分发给多人”的向导流程会在后续版本中提供。现在可以先在主窗口里完成这件事：“加水印”选项卡里的“分发给多人…”。";
const PLACEHOLDER_CHECK: &str = "“检查可疑图片”的向导流程会在后续版本中提供。现在可以先在主窗口里完成这件事：“检测”选项卡。";

// ------------------------------------------------------------------ state

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Task {
    Setup,
    Embed,
    Distribute,
    Check,
}

pub enum WizardOutcome {
    Closed,
    SettingsChanged { key_path: PathBuf, registry_dir: PathBuf },
    ShowTab(usize),
    ShowInRegistry(Vec<u32>),
}

pub struct WizardContext {
    pub cfg: Settings,
    pub key: Option<Vec<u8>>,
    pub embed_files: Vec<PathBuf>,
}

pub struct WizardResult {
    pub outcome: WizardOutcome,
    pub show_on_start: bool,
}

struct St {
    ctx: WizardContext,
    task: Task,
    history: Vec<i32>,
    cur: i32,
    show_on_start: bool,
    outcome: WizardOutcome,
    bmp: HBITMAP,
    white: HBRUSH,
    panicked: bool,
}

/// Read the state pointer stored in `GWLP_USERDATA` by the page's `WM_INITDIALOG`.
fn st(h: HWND) -> Option<&'static mut St> {
    let p = unsafe { GetWindowLongPtrW(h, GWLP_USERDATA) } as *mut St;
    if p.is_null() {
        None
    } else {
        unsafe { Some(&mut *p) }
    }
}

// ------------------------------------------------------------------ entry point

pub fn run(owner: HWND, ctx: WizardContext) -> WizardResult {
    let show_on_start = ctx.cfg.show_start_wizard;
    let raw = Box::into_raw(Box::new(St {
        ctx,
        task: Task::Setup,
        history: Vec::new(),
        cur: 0,
        show_on_start,
        outcome: WizardOutcome::Closed,
        bmp: null_mut(),
        white: null_mut(),
        panicked: false,
    }));

    let bmp = load_bitmap(BM_WATERMARK);
    let white = unsafe { CreateSolidBrush(rgb(255, 255, 255)) };
    unsafe {
        (*raw).bmp = bmp;
        (*raw).white = white;
    }

    let mut pages = vec![
        page(PAGE_WELCOME, page_welcome, raw as isize),
        page(PAGE_TASK, page_task, raw as isize),
        page(PAGE_PLACEHOLDER, page_placeholder, raw as isize),
    ];

    let mut hdr: PROPSHEETHEADERW_V2 = unsafe { std::mem::zeroed() };
    hdr.dwSize = std::mem::size_of::<PROPSHEETHEADERW_V2>() as u32;
    hdr.dwFlags = PSH_WIZARD_LITE | PSH_PROPSHEETPAGE;
    hdr.hwndParent = owner;
    hdr.hInstance = hinst();
    hdr.Anonymous1 = PROPSHEETHEADERW_V2_0 { pszIcon: null() };
    hdr.pszCaption = leak("BlindMark 向导");
    hdr.nPages = pages.len() as u32;
    hdr.Anonymous2 = PROPSHEETHEADERW_V2_1 { nStartPage: 0 };
    hdr.Anonymous3 = PROPSHEETHEADERW_V2_2 { ppsp: pages.as_mut_ptr() };
    hdr.pfnCallback = None;
    hdr.Anonymous4 = PROPSHEETHEADERW_V2_3 { hbmWatermark: null_mut() };
    hdr.hplWatermark = null_mut();
    hdr.Anonymous5 = PROPSHEETHEADERW_V2_4 { hbmHeader: null_mut() };

    unsafe { PropertySheetW(&mut hdr) };

    let st = unsafe { Box::from_raw(raw) };
    unsafe {
        if !st.bmp.is_null() {
            DeleteObject(st.bmp as _);
        }
        if !st.white.is_null() {
            DeleteObject(st.white as _);
        }
    }
    WizardResult { outcome: st.outcome, show_on_start: st.show_on_start }
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

fn page(id: i32, proc_: PageProc, state: isize) -> PROPSHEETPAGEW {
    let mut p: PROPSHEETPAGEW = unsafe { std::mem::zeroed() };
    p.dwSize = std::mem::size_of::<PROPSHEETPAGEW>() as u32;
    p.dwFlags = 0;
    p.hInstance = hinst();
    p.Anonymous1 = PROPSHEETPAGEW_0 { pszTemplate: id as usize as *const u16 };
    p.pfnDlgProc = Some(proc_);
    p.lParam = state;
    p
}

unsafe fn init_state(h: HWND, lp: LPARAM) {
    let p = lp as *const PROPSHEETPAGEW;
    if !p.is_null() {
        let state = (*p).lParam;
        if state != 0 {
            SetWindowLongPtrW(h, GWLP_USERDATA, state);
        }
    }
}

// ------------------------------------------------------------------ panic guard

fn guard<F: FnOnce() -> isize>(h: HWND, f: F) -> isize {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(e) => {
            if let Some(s) = st(h) {
                if s.panicked {
                    return 0;
                }
                s.panicked = true;
            }
            if let Some(m) = e.downcast_ref::<&str>() {
                eprintln!("start.rs panic: {m}");
            } else if let Some(m) = e.downcast_ref::<String>() {
                eprintln!("start.rs panic: {m}");
            } else {
                eprintln!("start.rs panic");
            }
            unsafe {
                let text = w(PANIC_COPY);
                let caption = w("BlindMark 向导");
                MessageBoxW(h, text.as_ptr(), caption.as_ptr(), MB_OK | MB_ICONWARNING);
                let sheet = sheet(h);
                if !sheet.is_null() {
                    PostMessageW(sheet, PSM_PRESSBUTTON, PSBTN_CANCEL as WPARAM, 0);
                }
            }
            0
        }
    }
}

// ------------------------------------------------------------------ painting

/// Content area to the right of the painted watermark strip.
const CONTENT_X: i32 = 196;

fn content_x(_w: i32) -> i32 {
    CONTENT_X
}

fn content_w(h: HWND) -> i32 {
    let (pw, _) = client_size(h);
    (pw - CONTENT_X - 24).max(180)
}

fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}

unsafe fn fill(hdc: HDC, rc: &RECT, colour: u32) {
    let br = CreateSolidBrush(colour);
    FillRect(hdc, rc, br);
    DeleteObject(br as _);
}

/// `HIGHCONTRASTW` from `Win32::UI::Accessibility`; that windows-sys feature is not enabled,
/// so the layout is declared here (it is a plain C struct).
#[repr(C)]
struct HighContrast {
    cb_size: u32,
    flags: u32,
    default_scheme: *mut u16,
}

/// `HCF_HIGHCONTRASTON`.
const HCF_HIGH_CONTRAST_ON: u32 = 1;

fn high_contrast() -> bool {
    unsafe {
        let mut hc = HighContrast { cb_size: std::mem::size_of::<HighContrast>() as u32, flags: 0, default_scheme: std::ptr::null_mut() };
        if SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cb_size, &mut hc as *mut _ as *mut _, 0) == 0 {
            return false;
        }
        (hc.flags & HCF_HIGH_CONTRAST_ON) != 0
    }
}

/// Paint the watermark strip plus the header title/subtitle of a page.
unsafe fn paint_banner(h: HWND, hdc: HDC, title: &str, subtitle: &str) {
    let (cw, ht) = client_size(h);
    let all = RECT { left: 0, top: 0, right: cw, bottom: ht };
    if high_contrast() {
        fill(hdc, &all, GetSysColor(COLOR_WINDOW));
        if !title.is_empty() {
            let f = super::fonts();
            let x = content_x(cw);
            let text_w = (cw - x - 12).max(120);
            SetBkMode(hdc, TRANSPARENT as i32);
            SelectObject(hdc, f.big as _);
            SetTextColor(hdc, GetSysColor(COLOR_WINDOWTEXT));
            let t = w(title);
            let mut r = RECT { left: x, top: 8, right: x + text_w, bottom: 40 };
            DrawTextW(hdc, t.as_ptr(), -1, &mut r, DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
            SelectObject(hdc, f.normal as _);
            let t = w(subtitle);
            let mut r = RECT { left: x, top: 38, right: x + text_w, bottom: 66 };
            DrawTextW(hdc, t.as_ptr(), -1, &mut r, DT_LEFT | DT_WORDBREAK | DT_NOPREFIX);
        }
        return;
    }

    fill(hdc, &all, rgb(255, 255, 255));

    let bmp = st(h).map(|s| s.bmp).unwrap_or(null_mut());
    if !bmp.is_null() {
        let strip = content_x(cw) - 8;
        let mem = CreateCompatibleDC(hdc);
        let old = SelectObject(mem, bmp as _);
        StretchBlt(hdc, 0, 0, strip, ht, mem, 0, 0, 164, 314, SRCCOPY);
        SelectObject(mem, old);
        DeleteDC(mem);
    }

    if !title.is_empty() {
        let f = super::fonts();
        let x = content_x(cw);
        SetBkMode(hdc, TRANSPARENT as i32);
        SelectObject(hdc, f.big as _);
        SetTextColor(hdc, rgb(28, 32, 36));
        let text_w = (cw - x - 12).max(120);
        let t = w(title);
        let mut r = RECT { left: x, top: 8, right: x + text_w, bottom: 40 };
        DrawTextW(hdc, t.as_ptr(), -1, &mut r, DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
        SelectObject(hdc, f.normal as _);
        SetTextColor(hdc, rgb(96, 104, 112));
        let t = w(subtitle);
        let mut r = RECT { left: x, top: 38, right: x + text_w, bottom: 66 };
        DrawTextW(hdc, t.as_ptr(), -1, &mut r, DT_LEFT | DT_WORDBREAK | DT_NOPREFIX);
        let line = RECT { left: x, top: 68, right: cw - 12, bottom: 69 };
        fill(hdc, &line, rgb(226, 230, 234));
    }
}

/// Opaque background for labels and checkboxes so glyphs are painted on a clean surface.
unsafe fn on_ctlcolor(h: HWND, hdc: HDC, child: HWND, warn_id: u16) -> isize {
    SetBkMode(hdc, OPAQUE as i32);
    if high_contrast() {
        SetTextColor(hdc, GetSysColor(COLOR_WINDOWTEXT));
        return GetSysColorBrush(COLOR_WINDOW) as isize;
    }
    if warn_id != 0 && child == ctl(h, warn_id) {
        SetTextColor(hdc, rgb(176, 0, 32));
    } else {
        SetTextColor(hdc, rgb(32, 36, 40));
    }
    st(h).map(|s| s.white as isize).unwrap_or(0)
}

/// Task-page description labels are grey; everything else uses the normal colour.
unsafe fn on_ctlcolor_desc(h: HWND, hdc: HDC, child: HWND) -> isize {
    if !ID_TASK_DESC.iter().any(|&id| ctl(h, id) == child) {
        return on_ctlcolor(h, hdc, child, 0);
    }
    SetBkMode(hdc, OPAQUE as i32);
    if high_contrast() {
        SetTextColor(hdc, GetSysColor(COLOR_WINDOWTEXT));
        return GetSysColorBrush(COLOR_WINDOW) as isize;
    }
    SetTextColor(hdc, rgb(96, 104, 112));
    st(h).map(|s| s.white as isize).unwrap_or(0)
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

fn schedule_relayout(h: HWND) {
    unsafe { SetTimer(h, 0x7A, 120, None) };
}

/// Shared "上一步": pop the visited-page stack. `-1` means there is nothing to go back to.
fn go_back(h: HWND) -> isize {
    let popped = st(h).and_then(|s| s.history.pop());
    match popped {
        Some(target) => unsafe { SetWindowLongPtrW(h, DWLP_MSGRESULT, target as isize); },
        None => unsafe { SetWindowLongPtrW(h, DWLP_MSGRESULT, -1); },
    }
    1
}

fn label_id(parent: HWND, id: u16, text: &str) -> HWND {
    create(windows_sys::core::w!("STATIC"), text, 0, 0, parent, id)
}

/// Height needed by a word-wrapped STATIC, measured with the normal font.
fn measure_text(h: HWND, text: &str, width: i32) -> i32 {
    unsafe {
        let dc = GetDC(h);
        let f = super::fonts();
        let old = SelectObject(dc, f.normal as _);
        let mut r = RECT { left: 0, top: 0, right: width.max(20), bottom: 0 };
        let t = w(text);
        DrawTextW(dc, t.as_ptr(), -1, &mut r, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX);
        SelectObject(dc, old);
        ReleaseDC(h, dc);
        (r.bottom - r.top).max(sc(18))
    }
}

fn task_title(i: usize, key_present: bool) -> &'static str {
    if i == 0 && key_present {
        TASK_TITLE_EXISTING
    } else {
        TASK_TITLES[i]
    }
}

fn task_desc(i: usize, key_present: bool) -> &'static str {
    if i == 0 && key_present {
        TASK_DESC_EXISTING
    } else {
        TASK_DESCS[i]
    }
}

fn placeholder_body(task: Task) -> &'static str {
    match task {
        Task::Setup => PLACEHOLDER_SETUP,
        Task::Embed => PLACEHOLDER_EMBED,
        Task::Distribute => PLACEHOLDER_DISTRIBUTE,
        Task::Check => PLACEHOLDER_CHECK,
    }
}

// ------------------------------------------------------------------ page 210: welcome

fn build_welcome(h: HWND) {
    label_id(h, ID_WELCOME_BODY, WELCOME_BODY);
    checkbox(h, ID_SHOW_ON_START, SHOW_ON_START);
    if let Some(s) = st(h) {
        set_check(ctl(h, ID_SHOW_ON_START), s.show_on_start);
    }
}

fn layout_welcome(h: HWND) {
    let (w, ht) = client_size(h);
    let x = content_x(w);
    let cw = content_w(h);
    let check_h = sc(22);
    let check_y = ht - sc(14) - check_h;
    mv(ctl(h, ID_SHOW_ON_START), x, check_y, cw, check_h);
    let body_y = 76;
    let body_h = (check_y - sc(6) - body_y).max(sc(60));
    mv(ctl(h, ID_WELCOME_BODY), x, body_y, cw, body_h);
}

unsafe extern "system" fn page_welcome(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    guard(h, || page_welcome_impl(h, msg, wp, lp))
}

unsafe fn page_welcome_impl(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            init_state(h, lp);
            build_welcome(h);
            set_font_tree(h, super::fonts().normal);
            layout_welcome(h);
            schedule_relayout(h);
            1
        }
        WM_SIZE => {
            layout_welcome(h);
            1
        }
        WM_ERASEBKGND => {
            paint_banner(h, wp as HDC, WELCOME_TITLE, WELCOME_SUBTITLE);
            1
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => on_ctlcolor(h, wp as HDC, lp as HWND, 0),
        WM_COMMAND => {
            if (wp & 0xffff) as u16 == ID_SHOW_ON_START && ((wp >> 16) & 0xffff) as u32 == BN_CLICKED {
                if let Some(s) = st(h) {
                    s.show_on_start = checked(ctl(h, ID_SHOW_ON_START));
                }
                return 1;
            }
            0
        }
        WM_NOTIFY => {
            let nm = unsafe { &*(lp as *const NMHDR) };
            match nm.code {
                PSN_SETACTIVE => {
                    if let Some(s) = st(h) {
                        s.cur = PAGE_WELCOME;
                    }
                    set_buttons(h, PSWIZB_NEXT);
                    0
                }
                PSN_WIZNEXT => {
                    if let Some(s) = st(h) {
                        s.history.push(s.cur);
                    }
                    unsafe { SetWindowLongPtrW(h, DWLP_MSGRESULT, PAGE_TASK as isize) };
                    1
                }
                _ => 0,
            }
        }
        WM_TIMER if wp == 0x7A => {
            unsafe { KillTimer(h, 0x7A) };
            layout_welcome(h);
            1
        }
        _ => 0,
    }
}

// ------------------------------------------------------------------ page 211: task

fn build_task(h: HWND) {
    let key_present = st(h).map(|s| s.ctx.key.is_some()).unwrap_or(false);
    for i in 0..4 {
        radio(h, ID_TASK_RADIO[i], task_title(i, key_present), i == 0);
        label_id(h, ID_TASK_DESC[i], task_desc(i, key_present));
    }
    let default = if key_present { ID_TASK_RADIO[1] } else { ID_TASK_RADIO[0] };
    set_check(ctl(h, default), true);
}

fn layout_task(h: HWND) {
    let (w, ht) = client_size(h);
    let x = content_x(w);
    let cw = content_w(h);
    let key_present = st(h).map(|s| s.ctx.key.is_some()).unwrap_or(false);
    let desc_w = cw.max(sc(80));
    let heights: Vec<i32> = (0..4).map(|i| measure_text(h, task_desc(i, key_present), desc_w)).collect();
    let total: i32 = 4 * sc(22) + 4 * sc(6) + heights.iter().sum::<i32>();
    let bottom = ht - sc(16);
    let mut y = if 78 + total <= bottom { 78 } else { (bottom - total).max(74) };
    for i in 0..4 {
        mv(ctl(h, ID_TASK_RADIO[i]), x, y, cw, sc(22));
        mv(ctl(h, ID_TASK_DESC[i]), x, y + sc(20), desc_w, heights[i]);
        y += sc(22) + heights[i] + sc(6);
    }
}

unsafe extern "system" fn page_task(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    guard(h, || page_task_impl(h, msg, wp, lp))
}

unsafe fn page_task_impl(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            init_state(h, lp);
            build_task(h);
            set_font_tree(h, super::fonts().normal);
            for id in ID_TASK_RADIO {
                set_font(ctl(h, id), super::fonts().bold);
            }
            layout_task(h);
            schedule_relayout(h);
            1
        }
        WM_SIZE => {
            layout_task(h);
            1
        }
        WM_ERASEBKGND => {
            paint_banner(h, wp as HDC, TASK_TITLE, TASK_SUBTITLE);
            1
        }
        WM_CTLCOLORSTATIC => on_ctlcolor_desc(h, wp as HDC, lp as HWND),
        WM_CTLCOLORBTN => on_ctlcolor(h, wp as HDC, lp as HWND, 0),
        WM_NOTIFY => {
            let nm = unsafe { &*(lp as *const NMHDR) };
            match nm.code {
                PSN_SETACTIVE => {
                    if let Some(s) = st(h) {
                        s.cur = PAGE_TASK;
                        if !ID_TASK_RADIO.iter().any(|&id| checked(ctl(h, id))) {
                            let default = if s.ctx.key.is_some() { ID_TASK_RADIO[1] } else { ID_TASK_RADIO[0] };
                            set_check(ctl(h, default), true);
                        }
                    }
                    set_buttons(h, PSWIZB_BACK | PSWIZB_NEXT);
                    0
                }
                PSN_WIZNEXT => {
                    let tasks = [Task::Setup, Task::Embed, Task::Distribute, Task::Check];
                    let mut chosen = Task::Setup;
                    for (i, &id) in ID_TASK_RADIO.iter().enumerate() {
                        if checked(ctl(h, id)) {
                            chosen = tasks[i];
                        }
                    }
                    if let Some(s) = st(h) {
                        s.task = chosen;
                        s.history.push(s.cur);
                    }
                    unsafe { SetWindowLongPtrW(h, DWLP_MSGRESULT, PAGE_PLACEHOLDER as isize) };
                    1
                }
                PSN_WIZBACK => go_back(h),
                _ => 0,
            }
        }
        WM_TIMER if wp == 0x7A => {
            unsafe { KillTimer(h, 0x7A) };
            layout_task(h);
            1
        }
        _ => 0,
    }
}

// ------------------------------------------------------------------ page 219: placeholder

fn build_placeholder(h: HWND) {
    let task = st(h).map(|s| s.task).unwrap_or(Task::Setup);
    label_id(h, ID_PLACEHOLDER_BODY, placeholder_body(task));
}

fn layout_placeholder(h: HWND) {
    let (w, ht) = client_size(h);
    let x = content_x(w);
    let cw = content_w(h);
    mv(ctl(h, ID_PLACEHOLDER_BODY), x, 84, cw, (ht - 84 - sc(16)).max(sc(60)));
}

unsafe extern "system" fn page_placeholder(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    guard(h, || page_placeholder_impl(h, msg, wp, lp))
}

unsafe fn page_placeholder_impl(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            init_state(h, lp);
            build_placeholder(h);
            set_font_tree(h, super::fonts().normal);
            layout_placeholder(h);
            schedule_relayout(h);
            1
        }
        WM_SIZE => {
            layout_placeholder(h);
            1
        }
        WM_ERASEBKGND => {
            paint_banner(h, wp as HDC, PLACEHOLDER_TITLE, "");
            1
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => on_ctlcolor(h, wp as HDC, lp as HWND, 0),
        WM_NOTIFY => {
            let nm = unsafe { &*(lp as *const NMHDR) };
            match nm.code {
                PSN_SETACTIVE => {
                    if let Some(s) = st(h) {
                        s.cur = PAGE_PLACEHOLDER;
                    }
                    set_buttons(h, PSWIZB_BACK | PSWIZB_FINISH);
                    0
                }
                PSN_WIZBACK => go_back(h),
                PSN_WIZFINISH => {
                    if let Some(s) = st(h) {
                        s.outcome = WizardOutcome::Closed;
                    }
                    0
                }
                _ => 0,
            }
        }
        WM_TIMER if wp == 0x7A => {
            unsafe { KillTimer(h, 0x7A) };
            layout_placeholder(h);
            1
        }
        _ => 0,
    }
}
