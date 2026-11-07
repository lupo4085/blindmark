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

use blindmark::jobs::{self, RegistryStatus};
use blindmark::keys;

use super::config::{app_dir, Settings};
use super::util::*;

// ------------------------------------------------------------------ ids

const PAGE_WELCOME: i32 = 210;
const PAGE_TASK: i32 = 211;
const PAGE_PLACEHOLDER: i32 = 219;
const PAGE_SETUP_KEY: i32 = 220;
const PAGE_SETUP_REGISTRY: i32 = 221;
const PAGE_SETUP_DONE: i32 = 222;

const BM_WATERMARK: usize = 301;

const ID_WELCOME_BODY: u16 = 1720;
const ID_SHOW_ON_START: u16 = 1721;

const ID_TASK_RADIO: [u16; 4] = [1731, 1732, 1733, 1734];
const ID_TASK_DESC: [u16; 4] = [1741, 1742, 1743, 1744];

const ID_PLACEHOLDER_BODY: u16 = 1750;

const ID_S1_NEW: u16 = 1810;
const ID_S1_PATH: u16 = 1811;
const ID_S1_CHANGE: u16 = 1812;
const ID_S1_EXISTING: u16 = 1813;
const ID_S1_BROWSE: u16 = 1814;
const ID_S1_EXISTING_PATH: u16 = 1815;
const ID_S1_FINGERPRINT: u16 = 1816;
const ID_S1_HINT: u16 = 1817;
const ID_S1_ERROR: u16 = 1818;

const ID_S2_BODY: u16 = 1820;
const ID_S2_LABEL: u16 = 1821;
const ID_S2_DIR: u16 = 1822;
const ID_S2_BROWSE: u16 = 1823;
const ID_S2_STATUS: u16 = 1824;
const ID_S2_NOTE: u16 = 1825;
const ID_S2_ERROR: u16 = 1826;

const ID_S3_BODY: u16 = 1830;
const ID_S3_BACKUP_KEY: u16 = 1831;
const ID_S3_BACKUP_REG: u16 = 1832;
const ID_S3_HINT: u16 = 1833;

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

const S1_TITLE: &str = "创建或选择你的密钥";
const S1_SUBTITLE: &str = "只有用这把密钥，才能读出你加的水印";
const S1_HINT: &str = "丢了密钥，就再也读不出已加的水印。设置完成后请立即备份。";

const S2_TITLE: &str = "选择登记表的保存位置";
const S2_SUBTITLE: &str = "登记表记录每个编号对应哪张原图、发给了谁";
const S2_BODY: &str = "它和密钥一样重要，建议放在会定期备份的位置。";

const S3_TITLE: &str = "设置完成";
const S3_SUBTITLE: &str = "现在备份你的密钥";
const S3_BODY: &str = "把密钥文件复制到 U 盘或密码管理器里。请不要发给任何人。";
const S3_HINT: &str = "以后可以在主窗口的“设置”选项卡里随时备份。";

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

#[derive(Clone)]
enum KeyChoice {
    New(PathBuf),
    Existing(Option<PathBuf>),
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
    key_choice: KeyChoice,
    registry_dir: PathBuf,
    new_key: Option<Vec<u8>>,
    prefix_setup: bool,
    created_key_path: Option<PathBuf>,
    new_path: PathBuf,
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
    let new_path = if ctx.cfg.key_path.as_os_str().is_empty() { app_dir().join("blindmark.key") } else { ctx.cfg.key_path.clone() };
    let registry_dir = if ctx.cfg.registry_dir.as_os_str().is_empty() { app_dir().join("registry") } else { ctx.cfg.registry_dir.clone() };
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
        key_choice: KeyChoice::New(new_path.clone()),
        registry_dir,
        new_key: None,
        prefix_setup: false,
        created_key_path: None,
        new_path,
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
        page(PAGE_SETUP_KEY, page_s1, raw as isize),
        page(PAGE_SETUP_REGISTRY, page_s2, raw as isize),
        page(PAGE_SETUP_DONE, page_s3, raw as isize),
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

/// Move to `target`, remembering the current page for "上一步".
fn advance_to(h: HWND, target: i32) -> isize {
    if let Some(s) = st(h) {
        s.history.push(s.cur);
    }
    unsafe { SetWindowLongPtrW(h, DWLP_MSGRESULT, target as isize); };
    1
}

/// Kept as a named constant so the only literal `-1` result remains `go_back`'s "no page" case.
const NO_RESULT: isize = -1;

/// Keep the page active (`DWLP_MSGRESULT = -1`).
fn block_page(h: HWND) -> isize {
    unsafe { SetWindowLongPtrW(h, DWLP_MSGRESULT, NO_RESULT); };
    1
}

fn step_y(s: &St) -> i32 {
    match s.task {
        Task::Embed => if s.prefix_setup { 5 } else { 3 },
        Task::Check => if s.prefix_setup { 4 } else { 2 },
        _ => 2,
    }
}

fn step_text(x: i32, y: i32, base: &str) -> String {
    format!("第 {x} 步，共 {y} 步 · {base}")
}

/// A small TaskDialog with custom buttons. Returns the pressed button ID (0 = closed).
fn task_dialog(owner: HWND, text: &str, buttons: &[(i32, &str)]) -> i32 {
    unsafe {
        let title = w("BlindMark 向导");
        let main = w(text);
        let texts: Vec<Vec<u16>> = buttons.iter().map(|(_, s)| w(s)).collect();
        let items: Vec<TASKDIALOG_BUTTON> = buttons
            .iter()
            .zip(&texts)
            .map(|((id, _), s)| TASKDIALOG_BUTTON { nButtonID: *id, pszButtonText: s.as_ptr() })
            .collect();
        let mut cfg: TASKDIALOGCONFIG = std::mem::zeroed();
        cfg.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
        cfg.hwndParent = owner;
        cfg.dwFlags = TDF_ALLOW_DIALOG_CANCELLATION | TDF_POSITION_RELATIVE_TO_WINDOW;
        cfg.pszWindowTitle = title.as_ptr();
        cfg.Anonymous1 = TASKDIALOGCONFIG_0 { pszMainIcon: null() };
        cfg.pszMainInstruction = main.as_ptr();
        cfg.cButtons = items.len() as u32;
        cfg.pButtons = items.as_ptr();
        cfg.nDefaultButton = buttons.first().map(|(id, _)| *id).unwrap_or(0);
        let mut pressed = 0i32;
        TaskDialogIndirect(&cfg, &mut pressed, null_mut(), null_mut());
        pressed
    }
}

/// Red for the S1 hint/error labels and the S2 mismatch/error labels.
unsafe fn on_ctlcolor_red(h: HWND, hdc: HDC, child: HWND) -> isize {
    SetBkMode(hdc, OPAQUE as i32);
    if high_contrast() {
        SetTextColor(hdc, GetSysColor(COLOR_WINDOWTEXT));
        return GetSysColorBrush(COLOR_WINDOW) as isize;
    }
    let mut red = child == ctl(h, ID_S1_HINT) || child == ctl(h, ID_S1_ERROR) || child == ctl(h, ID_S2_ERROR);
    if child == ctl(h, ID_S2_STATUS) && get_text(child).starts_with("这个登记表属于另一把密钥") {
        red = true;
    }
    SetTextColor(hdc, if red { rgb(176, 0, 32) } else { rgb(32, 36, 40) });
    st(h).map(|s| s.white as isize).unwrap_or(0)
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
                        s.prefix_setup = matches!(chosen, Task::Embed | Task::Check) && s.ctx.key.is_none();
                        s.history.push(s.cur);
                    }
                    let target = if chosen == Task::Setup { PAGE_SETUP_KEY } else { PAGE_PLACEHOLDER };
                    unsafe { SetWindowLongPtrW(h, DWLP_MSGRESULT, target as isize) };
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

// ------------------------------------------------------------------ page 220: S1 key

fn build_s1(h: HWND) {
    radio(h, ID_S1_NEW, "新建一把密钥（推荐）", true);
    label_id(h, ID_S1_PATH, "");
    button(h, ID_S1_CHANGE, "更改位置…");
    radio(h, ID_S1_EXISTING, "使用已有的密钥文件", false);
    button(h, ID_S1_BROWSE, "浏览…");
    label_id(h, ID_S1_EXISTING_PATH, "");
    label_id(h, ID_S1_FINGERPRINT, "");
    label_id(h, ID_S1_HINT, S1_HINT);
    label_id(h, ID_S1_ERROR, "");
    refresh_s1(h);
}

fn refresh_s1(h: HWND) {
    let Some(s) = st(h) else { return };
    let new_path = s.new_path.clone();
    let existing = match &s.key_choice {
        KeyChoice::Existing(p) => p.clone(),
        KeyChoice::New(_) => None,
    };
    set_check(ctl(h, ID_S1_NEW), existing.is_none());
    set_check(ctl(h, ID_S1_EXISTING), existing.is_some());
    set_text(ctl(h, ID_S1_PATH), &format!("保存位置：{}", new_path.display()));
    match &existing {
        Some(p) => {
            set_text(ctl(h, ID_S1_EXISTING_PATH), &p.display().to_string());
            match jobs::read_key_file(p) {
                Ok(k) => set_text(ctl(h, ID_S1_FINGERPRINT), &format!("密钥指纹：{}（用来核对是不是同一把密钥）", keys::fingerprint(&k))),
                Err(_) => set_text(ctl(h, ID_S1_FINGERPRINT), ""),
            }
        }
        None => {
            set_text(ctl(h, ID_S1_EXISTING_PATH), "");
            set_text(ctl(h, ID_S1_FINGERPRINT), "");
        }
    }
}

fn layout_s1(h: HWND) {
    let (w, ht) = client_size(h);
    let x = content_x(w);
    let cw = content_w(h);
    let mut y = 72;
    mv(ctl(h, ID_S1_NEW), x, y, cw, sc(22));
    y += sc(24);
    mv(ctl(h, ID_S1_PATH), x + sc(20), y, cw - sc(120), sc(20));
    mv(ctl(h, ID_S1_CHANGE), x + cw - sc(100), y - sc(2), sc(100), sc(26));
    y += sc(30);
    mv(ctl(h, ID_S1_EXISTING), x, y, cw, sc(22));
    y += sc(24);
    mv(ctl(h, ID_S1_BROWSE), x + sc(20), y, sc(90), sc(26));
    y += sc(30);
    mv(ctl(h, ID_S1_EXISTING_PATH), x + sc(20), y, cw - sc(20), sc(20));
    y += sc(22);
    mv(ctl(h, ID_S1_FINGERPRINT), x + sc(20), y, cw - sc(20), sc(20));
    let err_y = ht - sc(16) - sc(20);
    mv(ctl(h, ID_S1_ERROR), x, err_y, cw, sc(20));
    let hint_y = (err_y - sc(6) - sc(34)).max(y + sc(22));
    mv(ctl(h, ID_S1_HINT), x, hint_y, cw, sc(34));
}

unsafe extern "system" fn page_s1(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    guard(h, || page_s1_impl(h, msg, wp, lp))
}

unsafe fn page_s1_impl(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            init_state(h, lp);
            build_s1(h);
            set_font_tree(h, super::fonts().normal);
            layout_s1(h);
            schedule_relayout(h);
            1
        }
        WM_SIZE => {
            layout_s1(h);
            1
        }
        WM_ERASEBKGND => {
            let y = st(h).map(|s| step_y(s)).unwrap_or(2);
            let sub = step_text(1, y, S1_SUBTITLE);
            paint_banner(h, wp as HDC, S1_TITLE, &sub);
            1
        }
        WM_CTLCOLORSTATIC => on_ctlcolor_red(h, wp as HDC, lp as HWND),
        WM_CTLCOLORBTN => on_ctlcolor(h, wp as HDC, lp as HWND, 0),
        WM_COMMAND => {
            if ((wp >> 16) & 0xffff) as u32 != BN_CLICKED {
                return 0;
            }
            match (wp & 0xffff) as u16 {
                ID_S1_NEW => {
                    if let Some(s) = st(h) {
                        let p = s.new_path.clone();
                        s.key_choice = KeyChoice::New(p);
                    }
                    set_text(ctl(h, ID_S1_ERROR), "");
                    refresh_s1(h);
                    1
                }
                ID_S1_EXISTING => {
                    if let Some(s) = st(h) {
                        let existing = match &s.key_choice {
                            KeyChoice::Existing(p) => p.clone(),
                            KeyChoice::New(_) => None,
                        };
                        s.key_choice = KeyChoice::Existing(existing);
                    }
                    refresh_s1(h);
                    1
                }
                ID_S1_CHANGE => {
                    if let Some(p) = save_file(h, "选择密钥保存位置", "密钥文件 (*.key)\0*.key\0所有文件 (*.*)\0*.*\0", "blindmark.key", "key") {
                        if let Some(s) = st(h) {
                            s.new_path = p.clone();
                            s.key_choice = KeyChoice::New(p);
                        }
                        set_text(ctl(h, ID_S1_ERROR), "");
                        refresh_s1(h);
                    }
                    1
                }
                ID_S1_BROWSE => {
                    if let Some(p) = open_files(h, "选择密钥文件", "密钥文件 (*.key)\0*.key\0所有文件 (*.*)\0*.*\0", false).into_iter().next() {
                        if let Some(s) = st(h) {
                            s.key_choice = KeyChoice::Existing(Some(p.clone()));
                        }
                        match jobs::read_key_file(&p) {
                            Ok(_) => set_text(ctl(h, ID_S1_ERROR), ""),
                            Err(e) => set_text(ctl(h, ID_S1_ERROR), &e.message),
                        }
                        refresh_s1(h);
                    }
                    1
                }
                _ => 0,
            }
        }
        WM_NOTIFY => {
            let nm = unsafe { &*(lp as *const NMHDR) };
            match nm.code {
                PSN_SETACTIVE => {
                    if let Some(s) = st(h) {
                        s.cur = PAGE_SETUP_KEY;
                    }
                    refresh_s1(h);
                    set_buttons(h, PSWIZB_BACK | PSWIZB_NEXT);
                    0
                }
                PSN_WIZNEXT => {
                    let choice = st(h).map(|s| s.key_choice.clone()).unwrap_or(KeyChoice::Existing(None));
                    match choice {
                        KeyChoice::New(path) => {
                            if path.exists() {
                                match jobs::read_key_file(&path) {
                                    Ok(k) => {
                                        let fp = keys::fingerprint(&k);
                                        let question = format!("这里已经有一把密钥（密钥指纹 {fp}）。要直接使用它吗？");
                                        let pressed = task_dialog(h, &question, &[(1, "使用这把密钥"), (2, "换个位置")]);
                                        if pressed == 1 {
                                            if let Some(s) = st(h) {
                                                s.key_choice = KeyChoice::Existing(Some(path));
                                            }
                                            refresh_s1(h);
                                            return advance_to(h, PAGE_SETUP_REGISTRY);
                                        }
                                        return block_page(h);
                                    }
                                    Err(e) => {
                                        set_text(ctl(h, ID_S1_ERROR), &e.message);
                                        return block_page(h);
                                    }
                                }
                            }
                            advance_to(h, PAGE_SETUP_REGISTRY)
                        }
                        KeyChoice::Existing(Some(path)) => match jobs::read_key_file(&path) {
                            Ok(_) => advance_to(h, PAGE_SETUP_REGISTRY),
                            Err(e) => {
                                set_text(ctl(h, ID_S1_ERROR), &e.message);
                                block_page(h)
                            }
                        },
                        KeyChoice::Existing(None) => {
                            set_text(ctl(h, ID_S1_ERROR), "请先新建一把密钥，或选择已有的密钥文件。");
                            block_page(h)
                        }
                    }
                }
                _ => 0,
            }
        }
        WM_TIMER if wp == 0x7A => {
            unsafe { KillTimer(h, 0x7A) };
            layout_s1(h);
            1
        }
        _ => 0,
    }
}

// ------------------------------------------------------------------ page 221: S2 registry

fn build_s2(h: HWND) {
    let dir = st(h).map(|s| s.registry_dir.display().to_string()).unwrap_or_default();
    label_id(h, ID_S2_BODY, S2_BODY);
    label_id(h, ID_S2_LABEL, "位置");
    edit(h, ID_S2_DIR, &dir);
    button(h, ID_S2_BROWSE, "浏览…");
    label_id(h, ID_S2_STATUS, "");
    label_id(h, ID_S2_NOTE, "");
    label_id(h, ID_S2_ERROR, "");
    refresh_s2(h);
}

fn refresh_s2(h: HWND) {
    let Some(s) = st(h) else { return };
    let dir = PathBuf::from(get_text(ctl(h, ID_S2_DIR)).trim());
    if dir.as_os_str().is_empty() {
        set_text(ctl(h, ID_S2_STATUS), "");
        return;
    }
    let key = match &s.key_choice {
        KeyChoice::Existing(Some(p)) => jobs::read_key_file(p).ok(),
        _ => None,
    };
    match jobs::inspect_registry(&dir, key.as_deref()) {
        Ok(RegistryStatus::New) => set_text(ctl(h, ID_S2_STATUS), "这是一个新的登记表。"),
        Ok(RegistryStatus::Matches { entries }) => {
            let text = if key.is_some() { format!("已有登记表，共 {entries} 条记录，与所选密钥匹配。") } else { format!("已有登记表，共 {entries} 条记录。") };
            set_text(ctl(h, ID_S2_STATUS), &text);
        }
        Ok(RegistryStatus::Mismatch { fingerprint }) => set_text(
            ctl(h, ID_S2_STATUS),
            &format!("这个登记表属于另一把密钥（密钥指纹 {fingerprint}），不能和所选密钥一起使用。"),
        ),
        Err(e) => set_text(ctl(h, ID_S2_STATUS), &e.message),
    }
}

fn set_submit_text(h: HWND) {
    let button = if st(h).map(|s| s.task == Task::Setup).unwrap_or(true) { "保存设置" } else { "保存并继续" };
    let text = w(button);
    send(sheet(h), PSM_SETNEXTTEXT, 0, text.as_ptr() as isize);
    set_text(ctl(h, ID_S2_NOTE), &format!("点击“{button}”后会保存设置。"));
}

fn layout_s2(h: HWND) {
    let (w, ht) = client_size(h);
    let x = content_x(w);
    let cw = content_w(h);
    mv(ctl(h, ID_S2_BODY), x, 74, cw, sc(20));
    mv(ctl(h, ID_S2_LABEL), x, 104, sc(40), sc(20));
    mv(ctl(h, ID_S2_DIR), x + sc(44), 100, cw - sc(140), sc(24));
    mv(ctl(h, ID_S2_BROWSE), x + cw - sc(90), 99, sc(90), sc(26));
    mv(ctl(h, ID_S2_STATUS), x, 132, cw, sc(20));
    let err_y = ht - sc(16) - sc(20);
    mv(ctl(h, ID_S2_ERROR), x, err_y, cw, sc(20));
    mv(ctl(h, ID_S2_NOTE), x, err_y - sc(6) - sc(20), cw, sc(20));
}

unsafe extern "system" fn page_s2(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    guard(h, || page_s2_impl(h, msg, wp, lp))
}

unsafe fn page_s2_impl(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            init_state(h, lp);
            build_s2(h);
            set_font_tree(h, super::fonts().normal);
            layout_s2(h);
            schedule_relayout(h);
            1
        }
        WM_SIZE => {
            layout_s2(h);
            1
        }
        WM_ERASEBKGND => {
            let y = st(h).map(|s| step_y(s)).unwrap_or(2);
            let sub = step_text(2, y, S2_SUBTITLE);
            paint_banner(h, wp as HDC, S2_TITLE, &sub);
            1
        }
        WM_CTLCOLORSTATIC => on_ctlcolor_red(h, wp as HDC, lp as HWND),
        WM_CTLCOLORBTN => on_ctlcolor(h, wp as HDC, lp as HWND, 0),
        WM_COMMAND => {
            let id = (wp & 0xffff) as u16;
            let code = ((wp >> 16) & 0xffff) as u32;
            if id == ID_S2_BROWSE && code == BN_CLICKED {
                if let Some(d) = pick_folder(h, "选择登记表文件夹") {
                    set_text(ctl(h, ID_S2_DIR), &d.display().to_string());
                    refresh_s2(h);
                }
                return 1;
            }
            if id == ID_S2_DIR && code == EN_CHANGE {
                unsafe { SetTimer(h, 0x7B, 300, None) };
                return 1;
            }
            0
        }
        WM_NOTIFY => {
            let nm = unsafe { &*(lp as *const NMHDR) };
            match nm.code {
                PSN_SETACTIVE => {
                    if let Some(s) = st(h) {
                        s.cur = PAGE_SETUP_REGISTRY;
                    }
                    refresh_s2(h);
                    set_submit_text(h);
                    set_buttons(h, PSWIZB_BACK | PSWIZB_NEXT);
                    0
                }
                PSN_KILLACTIVE => {
                    let text = w("下一步(&N) >");
                    send(sheet(h), PSM_SETNEXTTEXT, 0, text.as_ptr() as isize);
                    0
                }
                PSN_WIZNEXT => {
                    let dir = PathBuf::from(get_text(ctl(h, ID_S2_DIR)).trim());
                    if dir.as_os_str().is_empty() {
                        set_text(ctl(h, ID_S2_ERROR), "请选择登记表的保存位置。");
                        return block_page(h);
                    }
                    let choice = st(h).map(|s| s.key_choice.clone()).unwrap_or(KeyChoice::Existing(None));
                    let (key, key_path) = match choice {
                        KeyChoice::New(path) => match jobs::create_key_file(&path) {
                            Ok(k) => {
                                if let Some(s) = st(h) {
                                    s.new_key = Some(k.clone());
                                    s.created_key_path = Some(path.clone());
                                }
                                (k, path)
                            }
                            Err(e) => {
                                set_text(ctl(h, ID_S2_ERROR), &e.message);
                                return block_page(h);
                            }
                        },
                        KeyChoice::Existing(Some(path)) => match jobs::read_key_file(&path) {
                            Ok(k) => (k, path),
                            Err(e) => {
                                set_text(ctl(h, ID_S2_ERROR), &e.message);
                                return block_page(h);
                            }
                        },
                        KeyChoice::Existing(None) => {
                            set_text(ctl(h, ID_S2_ERROR), "请先新建一把密钥，或选择已有的密钥文件。");
                            return block_page(h);
                        }
                    };
                    match jobs::inspect_registry(&dir, Some(&key)) {
                        Ok(RegistryStatus::Mismatch { fingerprint }) => {
                            set_text(
                                ctl(h, ID_S2_ERROR),
                                &format!("这个登记表属于另一把密钥（密钥指纹 {fingerprint}），不能和所选密钥一起使用。"),
                            );
                            return block_page(h);
                        }
                        Ok(_) => {}
                        Err(e) => {
                            set_text(ctl(h, ID_S2_ERROR), &e.message);
                            return block_page(h);
                        }
                    }
                    if let Err(e) = jobs::open_registry_for_write(&dir, &key) {
                        set_text(ctl(h, ID_S2_ERROR), &e.message);
                        return block_page(h);
                    }
                    let target = if st(h).map(|s| s.task == Task::Setup).unwrap_or(true) { PAGE_SETUP_DONE } else { PAGE_PLACEHOLDER };
                    if let Some(s) = st(h) {
                        s.ctx.key = Some(key);
                        s.ctx.cfg.key_path = key_path.clone();
                        s.ctx.cfg.registry_dir = dir.clone();
                        s.registry_dir = dir.clone();
                        s.outcome = WizardOutcome::SettingsChanged { key_path, registry_dir: dir };
                    }
                    advance_to(h, target)
                }
                _ => 0,
            }
        }
        WM_TIMER if wp == 0x7B => {
            unsafe { KillTimer(h, 0x7B) };
            refresh_s2(h);
            1
        }
        WM_TIMER if wp == 0x7A => {
            unsafe { KillTimer(h, 0x7A) };
            layout_s2(h);
            1
        }
        _ => 0,
    }
}

// ------------------------------------------------------------------ page 222: S3 done

fn build_s3(h: HWND) {
    label_id(h, ID_S3_BODY, S3_BODY);
    button(h, ID_S3_BACKUP_KEY, "备份密钥…");
    button(h, ID_S3_BACKUP_REG, "备份登记表…");
    label_id(h, ID_S3_HINT, S3_HINT);
}

fn layout_s3(h: HWND) {
    let (w, _ht) = client_size(h);
    let x = content_x(w);
    let cw = content_w(h);
    mv(ctl(h, ID_S3_BODY), x, 84, cw, sc(44));
    mv(ctl(h, ID_S3_BACKUP_KEY), x, 140, sc(150), sc(30));
    mv(ctl(h, ID_S3_BACKUP_REG), x + sc(160), 140, sc(160), sc(30));
    mv(ctl(h, ID_S3_HINT), x, 184, cw, sc(20));
}

unsafe extern "system" fn page_s3(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    guard(h, || page_s3_impl(h, msg, wp, lp))
}

unsafe fn page_s3_impl(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            init_state(h, lp);
            build_s3(h);
            set_font_tree(h, super::fonts().normal);
            layout_s3(h);
            schedule_relayout(h);
            1
        }
        WM_SIZE => {
            layout_s3(h);
            1
        }
        WM_ERASEBKGND => {
            paint_banner(h, wp as HDC, S3_TITLE, S3_SUBTITLE);
            1
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => on_ctlcolor(h, wp as HDC, lp as HWND, 0),
        WM_COMMAND => {
            if ((wp >> 16) & 0xffff) as u32 != BN_CLICKED {
                return 0;
            }
            match (wp & 0xffff) as u16 {
                ID_S3_BACKUP_KEY => {
                    let key_path = st(h).map(|s| s.ctx.cfg.key_path.clone()).unwrap_or_default();
                    let name = format!("blindmark-key-backup-{}.key", chrono::Local::now().format("%Y%m%d"));
                    if let Some(p) = save_file(h, "备份密钥", "密钥文件 (*.key)\0*.key\0所有文件 (*.*)\0*.*\0", &name, "key") {
                        match std::fs::copy(&key_path, &p) {
                            Ok(_) => info(h, &format!("密钥已备份到 {}", p.display())),
                            Err(e) => error(h, &format!("备份失败：{e:#}")),
                        }
                    }
                    1
                }
                ID_S3_BACKUP_REG => {
                    let reg_dir = st(h).map(|s| s.registry_dir.clone()).unwrap_or_default();
                    if let Some(d) = pick_folder(h, "选择备份保存位置") {
                        let dest = d.join(format!("blindmark-registry-backup-{}", chrono::Local::now().format("%Y%m%d")));
                        match jobs::copy_dir(&reg_dir, &dest) {
                            Ok(n) => info(h, &format!("登记表已备份到 {}（{n} 个文件）。", dest.display())),
                            Err(e) => error(h, &e.message),
                        }
                    }
                    1
                }
                _ => 0,
            }
        }
        WM_NOTIFY => {
            let nm = unsafe { &*(lp as *const NMHDR) };
            match nm.code {
                PSN_SETACTIVE => {
                    if let Some(s) = st(h) {
                        s.cur = PAGE_SETUP_DONE;
                    }
                    set_buttons(h, PSWIZB_FINISH);
                    0
                }
                PSN_WIZBACK => block_page(h),
                PSN_WIZFINISH => 0,
                _ => 0,
            }
        }
        WM_TIMER if wp == 0x7A => {
            unsafe { KillTimer(h, 0x7A) };
            layout_s3(h);
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
