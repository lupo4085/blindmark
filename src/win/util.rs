//! Thin helpers over raw Win32 (windows-sys): strings, control creation, common dialogs,
//! clipboard, list views, fonts and DPI scaling.
#![allow(clippy::missing_safety_doc)]

use std::cell::Cell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::System::Ole::{CF_DIB, CF_HDROP, CF_UNICODETEXT};
use windows_sys::Win32::UI::Controls::Dialogs::*;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::Win32::System::SystemServices::{SS_LEFT, SS_NOPREFIX};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows_sys::core::{BOOL, PCWSTR};

/// Not exported by windows-sys (WM_USER + 0).
const TBM_GETPOS: u32 = WM_USER;

use blindmark::registry::RgbImageBuf;

// ------------------------------------------------------------------ strings

/// UTF-16, NUL-terminated.
pub fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Windows edit controls want CRLF line ends.
pub fn crlf(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\n', "\r\n")
}

pub fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

pub fn hinst() -> HINSTANCE {
    unsafe { GetModuleHandleW(null()) }
}

// ------------------------------------------------------------------ DPI

thread_local! {
    static DPI: Cell<u32> = const { Cell::new(96) };
}

pub fn set_dpi(dpi: u32) {
    DPI.with(|d| d.set(dpi.max(96)));
}

pub fn dpi() -> u32 {
    DPI.with(|d| d.get())
}

/// Scale a 96-DPI length to the current DPI.
pub fn sc(v: i32) -> i32 {
    (v as i64 * dpi() as i64 / 96) as i32
}

// ------------------------------------------------------------------ fonts

#[derive(Clone, Copy)]
pub struct Fonts {
    pub normal: HFONT,
    pub bold: HFONT,
    pub big: HFONT,
    pub mono: HFONT,
}

unsafe fn make_font(pt: i32, weight: i32, face: &str) -> HFONT {
    unsafe {
        let h = -(pt * dpi() as i32 / 72);
        CreateFontW(
            h, 0, 0, 0, weight, 0, 0, 0,
            DEFAULT_CHARSET as _, OUT_DEFAULT_PRECIS as _, CLIP_DEFAULT_PRECIS as _, CLEARTYPE_QUALITY as _,
            (DEFAULT_PITCH as u32 | FF_DONTCARE as u32) as _,
            w(face).as_ptr(),
        )
    }
}

impl Fonts {
    pub fn new() -> Fonts {
        unsafe {
            Fonts {
                normal: make_font(9, FW_NORMAL as i32, "Microsoft YaHei UI"),
                bold: make_font(9, FW_BOLD as i32, "Microsoft YaHei UI"),
                big: make_font(12, FW_BOLD as i32, "Microsoft YaHei UI"),
                mono: make_font(9, FW_NORMAL as i32, "Consolas"),
            }
        }
    }
    pub fn destroy(&self) {
        unsafe {
            for f in [self.normal, self.bold, self.big, self.mono] {
                DeleteObject(f as _);
            }
        }
    }
}

unsafe extern "system" fn set_font_cb(h: HWND, lp: LPARAM) -> BOOL {
    unsafe { SendMessageW(h, WM_SETFONT, lp as WPARAM, 1) };
    1
}

/// Apply a font to a window and all its descendants.
pub fn set_font_tree(h: HWND, font: HFONT) {
    unsafe {
        SendMessageW(h, WM_SETFONT, font as WPARAM, 1);
        EnumChildWindows(h, Some(set_font_cb), font as LPARAM);
    }
}

pub fn set_font(h: HWND, font: HFONT) {
    unsafe { SendMessageW(h, WM_SETFONT, font as WPARAM, 1) };
}

// ------------------------------------------------------------------ windows & controls

pub fn send(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { SendMessageW(h, msg, wp, lp) }
}

pub fn create(class: PCWSTR, text: &str, style: u32, ex: u32, parent: HWND, id: u16) -> HWND {
    unsafe {
        CreateWindowExW(
            ex,
            class,
            w(text).as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            0, 0, 10, 10,
            parent,
            id as usize as HMENU,
            hinst(),
            null(),
        )
    }
}

pub fn button(parent: HWND, id: u16, text: &str) -> HWND {
    create(windows_sys::core::w!("BUTTON"), text, WS_TABSTOP | BS_PUSHBUTTON as u32, 0, parent, id)
}
pub fn default_button(parent: HWND, id: u16, text: &str) -> HWND {
    create(windows_sys::core::w!("BUTTON"), text, WS_TABSTOP | BS_DEFPUSHBUTTON as u32, 0, parent, id)
}
pub fn checkbox(parent: HWND, id: u16, text: &str) -> HWND {
    create(windows_sys::core::w!("BUTTON"), text, WS_TABSTOP | BS_AUTOCHECKBOX as u32, 0, parent, id)
}
/// `first` starts a new radio group.
pub fn radio(parent: HWND, id: u16, text: &str, first: bool) -> HWND {
    let g = if first { WS_GROUP | WS_TABSTOP } else { 0 };
    create(windows_sys::core::w!("BUTTON"), text, g | BS_AUTORADIOBUTTON as u32, 0, parent, id)
}
pub fn group(parent: HWND, text: &str) -> HWND {
    create(windows_sys::core::w!("BUTTON"), text, BS_GROUPBOX as u32, 0, parent, 0)
}
pub fn label(parent: HWND, text: &str) -> HWND {
    create(windows_sys::core::w!("STATIC"), text, SS_LEFT | SS_NOPREFIX, 0, parent, 0)
}
pub fn edit(parent: HWND, id: u16, text: &str) -> HWND {
    create(windows_sys::core::w!("EDIT"), text, WS_TABSTOP | ES_AUTOHSCROLL as u32, WS_EX_CLIENTEDGE, parent, id)
}
pub fn edit_readonly_multi(parent: HWND, id: u16) -> HWND {
    create(
        windows_sys::core::w!("EDIT"),
        "",
        WS_TABSTOP | WS_VSCROLL | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32,
        WS_EX_CLIENTEDGE,
        parent,
        id,
    )
}
pub fn combo_edit(parent: HWND, id: u16) -> HWND {
    create(windows_sys::core::w!("COMBOBOX"), "", WS_TABSTOP | WS_VSCROLL | (CBS_DROPDOWN | CBS_AUTOHSCROLL) as u32, 0, parent, id)
}
pub fn combo_list(parent: HWND, id: u16) -> HWND {
    create(windows_sys::core::w!("COMBOBOX"), "", WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32, 0, parent, id)
}
pub fn trackbar(parent: HWND, id: u16, min: i32, max: i32) -> HWND {
    let h = create(TRACKBAR_CLASSW, "", WS_TABSTOP | TBS_HORZ | TBS_AUTOTICKS, 0, parent, id);
    send(h, TBM_SETRANGE, 1, ((max as u32) << 16 | (min as u32 & 0xffff)) as LPARAM);
    send(h, TBM_SETTICFREQ, 5, 0);
    send(h, TBM_SETPAGESIZE, 0, 5);
    h
}
pub fn tb_get(h: HWND) -> i32 {
    send(h, TBM_GETPOS, 0, 0) as i32
}
pub fn tb_set(h: HWND, v: i32) {
    send(h, TBM_SETPOS, 1, v as LPARAM);
}
pub fn progress(parent: HWND) -> HWND {
    create(PROGRESS_CLASSW, "", 0, 0, parent, 0)
}
pub fn pb_set(h: HWND, pos: usize, max: usize) {
    send(h, PBM_SETRANGE32, 0, max.max(1) as LPARAM);
    send(h, PBM_SETPOS, pos, 0);
}

pub fn mv(h: HWND, x: i32, y: i32, cx: i32, cy: i32) {
    unsafe { MoveWindow(h, x, y, cx.max(0), cy.max(0), 1) };
}

pub fn show(h: HWND, on: bool) {
    unsafe { ShowWindow(h, if on { SW_SHOW } else { SW_HIDE }) };
}

pub fn enable(h: HWND, on: bool) {
    unsafe { EnableWindow(h, on as BOOL) };
}

pub fn get_text(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h);
        let mut b = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, b.as_mut_ptr(), b.len() as i32);
        String::from_utf16_lossy(&b[..got.max(0) as usize])
    }
}

pub fn set_text(h: HWND, s: &str) {
    unsafe { SetWindowTextW(h, w(s).as_ptr()) };
}

pub fn checked(h: HWND) -> bool {
    send(h, BM_GETCHECK, 0, 0) == BST_CHECKED as isize
}
pub fn set_check(h: HWND, on: bool) {
    send(h, BM_SETCHECK, if on { BST_CHECKED as usize } else { BST_UNCHECKED as usize }, 0);
}

pub fn cue(h: HWND, s: &str) {
    let ws = w(s);
    send(h, EM_SETCUEBANNER, 1, ws.as_ptr() as LPARAM);
}

pub fn cb_clear(h: HWND) {
    send(h, CB_RESETCONTENT, 0, 0);
}
pub fn cb_add(h: HWND, s: &str) {
    let ws = w(s);
    send(h, CB_ADDSTRING, 0, ws.as_ptr() as LPARAM);
}
pub fn cb_sel(h: HWND) -> i32 {
    send(h, CB_GETCURSEL, 0, 0) as i32
}
pub fn cb_set_sel(h: HWND, i: i32) {
    send(h, CB_SETCURSEL, i as usize, 0);
}
pub fn cb_item_text(h: HWND, i: i32) -> String {
    let n = send(h, CB_GETLBTEXTLEN, i as usize, 0);
    if n < 0 {
        return String::new();
    }
    let mut b = vec![0u16; n as usize + 1];
    send(h, CB_GETLBTEXT, i as usize, b.as_mut_ptr() as LPARAM);
    from_wide(&b)
}

pub fn invalidate(h: HWND) {
    unsafe { InvalidateRect(h, null(), 1) };
}

pub fn client_size(h: HWND) -> (i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetClientRect(h, &mut r) };
    (r.right - r.left, r.bottom - r.top)
}

pub fn msgbox(owner: HWND, text: &str, flags: u32) -> i32 {
    unsafe { MessageBoxW(owner, w(text).as_ptr(), w("blindmark").as_ptr(), flags) }
}
pub fn info(owner: HWND, text: &str) {
    msgbox(owner, text, MB_OK | MB_ICONINFORMATION);
}
pub fn warn(owner: HWND, text: &str) {
    msgbox(owner, text, MB_OK | MB_ICONWARNING);
}
pub fn error(owner: HWND, text: &str) {
    msgbox(owner, text, MB_OK | MB_ICONERROR);
}
pub fn ask(owner: HWND, text: &str) -> bool {
    msgbox(owner, text, MB_YESNO | MB_ICONQUESTION) == IDYES
}

pub fn class_name(h: HWND) -> String {
    let mut b = [0u16; 64];
    let n = unsafe { GetClassNameW(h, b.as_mut_ptr(), 64) };
    String::from_utf16_lossy(&b[..n.max(0) as usize])
}

// ------------------------------------------------------------------ shell

pub fn shell_open(target: &str) {
    unsafe {
        ShellExecuteW(null_mut(), w("open").as_ptr(), w(target).as_ptr(), null(), null(), SW_SHOWNORMAL);
    }
}

/// Open Explorer with the file selected.
pub fn reveal(path: &Path) {
    unsafe {
        if path.exists() {
            let arg = format!("/select,\"{}\"", path.display());
            ShellExecuteW(null_mut(), w("open").as_ptr(), w("explorer.exe").as_ptr(), w(&arg).as_ptr(), null(), SW_SHOWNORMAL);
        } else if let Some(p) = path.parent().filter(|p| p.exists()) {
            shell_open(&p.display().to_string());
        }
    }
}

/// Files from a WM_DROPFILES handle (the handle is released).
pub fn dropped_files(hdrop: HDROP) -> Vec<PathBuf> {
    unsafe {
        let n = DragQueryFileW(hdrop, u32::MAX, null_mut(), 0);
        let mut out = vec![];
        for i in 0..n {
            let len = DragQueryFileW(hdrop, i, null_mut(), 0);
            let mut b = vec![0u16; len as usize + 1];
            DragQueryFileW(hdrop, i, b.as_mut_ptr(), b.len() as u32);
            out.push(PathBuf::from(from_wide(&b)));
        }
        DragFinish(hdrop);
        out
    }
}

// ------------------------------------------------------------------ common dialogs

pub const IMAGE_FILTER: &str = "图片 (*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.tif;*.tiff;*.gif)\0*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.tif;*.tiff;*.gif\0所有文件 (*.*)\0*.*\0";

fn filter_w(filter: &str) -> Vec<u16> {
    let mut v: Vec<u16> = filter.encode_utf16().collect();
    v.push(0);
    v.push(0);
    v
}

/// Open-file dialog. `multi` allows several files.
pub fn open_files(owner: HWND, title: &str, filter: &str, multi: bool) -> Vec<PathBuf> {
    unsafe {
        let mut buf = vec![0u16; 65536];
        let f = filter_w(filter);
        let t = w(title);
        let mut ofn: OPENFILENAMEW = std::mem::zeroed();
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = owner;
        ofn.lpstrFilter = f.as_ptr();
        ofn.lpstrFile = buf.as_mut_ptr();
        ofn.nMaxFile = buf.len() as u32;
        ofn.lpstrTitle = t.as_ptr();
        ofn.Flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY | if multi { OFN_ALLOWMULTISELECT } else { 0 };
        if GetOpenFileNameW(&mut ofn) == 0 {
            return vec![];
        }
        // Multi-select: "dir\0name1\0name2\0\0"; single: "full\0\0".
        let mut parts = vec![];
        let mut start = 0;
        for i in 0..buf.len() {
            if buf[i] == 0 {
                if i == start {
                    break;
                }
                parts.push(String::from_utf16_lossy(&buf[start..i]));
                start = i + 1;
            }
        }
        if parts.len() <= 1 {
            parts.into_iter().map(PathBuf::from).collect()
        } else {
            let dir = PathBuf::from(&parts[0]);
            parts[1..].iter().map(|n| dir.join(n)).collect()
        }
    }
}

pub fn save_file(owner: HWND, title: &str, filter: &str, default_name: &str, def_ext: &str) -> Option<PathBuf> {
    unsafe {
        let mut buf = vec![0u16; 4096];
        for (i, c) in default_name.encode_utf16().take(4000).enumerate() {
            buf[i] = c;
        }
        let f = filter_w(filter);
        let t = w(title);
        let e = w(def_ext);
        let mut ofn: OPENFILENAMEW = std::mem::zeroed();
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = owner;
        ofn.lpstrFilter = f.as_ptr();
        ofn.lpstrFile = buf.as_mut_ptr();
        ofn.nMaxFile = buf.len() as u32;
        ofn.lpstrTitle = t.as_ptr();
        ofn.lpstrDefExt = e.as_ptr();
        ofn.Flags = OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY;
        if GetSaveFileNameW(&mut ofn) == 0 {
            return None;
        }
        Some(PathBuf::from(from_wide(&buf)))
    }
}

pub fn pick_folder(owner: HWND, title: &str) -> Option<PathBuf> {
    unsafe {
        let mut name = [0u16; 260];
        let t = w(title);
        let bi = BROWSEINFOW {
            hwndOwner: owner,
            pidlRoot: null_mut(),
            pszDisplayName: name.as_mut_ptr(),
            lpszTitle: t.as_ptr(),
            ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
            lpfn: None,
            lParam: 0,
            iImage: 0,
        };
        let pidl = SHBrowseForFolderW(&bi);
        if pidl.is_null() {
            return None;
        }
        let mut path = [0u16; 1024];
        let ok = SHGetPathFromIDListW(pidl, path.as_mut_ptr());
        windows_sys::Win32::System::Com::CoTaskMemFree(pidl as *const c_void);
        if ok == 0 {
            return None;
        }
        Some(PathBuf::from(from_wide(&path)))
    }
}

// ------------------------------------------------------------------ clipboard

pub fn set_clipboard_text(owner: HWND, s: &str) -> bool {
    unsafe {
        if OpenClipboard(owner) == 0 {
            return false;
        }
        EmptyClipboard();
        let ws: Vec<u16> = crlf(s).encode_utf16().chain(Some(0)).collect();
        let bytes = ws.len() * 2;
        let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        let mut ok = false;
        if !hmem.is_null() {
            let p = GlobalLock(hmem) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(ws.as_ptr(), p, ws.len());
                GlobalUnlock(hmem);
                ok = !SetClipboardData(CF_UNICODETEXT as u32, hmem as HANDLE).is_null();
            }
            if !ok {
                GlobalFree(hmem);
            }
        }
        CloseClipboard();
        ok
    }
}

pub enum ClipImage {
    Image(RgbImageBuf),
    File(PathBuf),
}

/// An image (CF_DIB) or the first copied file (CF_HDROP) on the clipboard.
pub fn clipboard_image(owner: HWND) -> Option<ClipImage> {
    unsafe {
        let has_dib = IsClipboardFormatAvailable(CF_DIB as u32) != 0;
        let has_drop = IsClipboardFormatAvailable(CF_HDROP as u32) != 0;
        if !has_dib && !has_drop {
            return None;
        }
        if OpenClipboard(owner) == 0 {
            return None;
        }
        let mut out = None;
        if has_dib {
            let h = GetClipboardData(CF_DIB as u32);
            if !h.is_null() {
                let size = GlobalSize(h as HGLOBAL);
                let p = GlobalLock(h as HGLOBAL) as *const u8;
                if !p.is_null() {
                    let bytes = std::slice::from_raw_parts(p, size);
                    out = parse_dib(bytes).map(ClipImage::Image);
                    GlobalUnlock(h as HGLOBAL);
                }
            }
        } else {
            let h = GetClipboardData(CF_HDROP as u32);
            if !h.is_null() {
                let hdrop = h as HDROP;
                let len = DragQueryFileW(hdrop, 0, null_mut(), 0);
                if len > 0 {
                    let mut b = vec![0u16; len as usize + 1];
                    DragQueryFileW(hdrop, 0, b.as_mut_ptr(), b.len() as u32);
                    out = Some(ClipImage::File(PathBuf::from(from_wide(&b))));
                }
            }
        }
        CloseClipboard();
        out
    }
}

/// Packed DIB (BITMAPINFOHEADER + pixels), 24 or 32 bpp, uncompressed or BI_BITFIELDS.
pub fn parse_dib(b: &[u8]) -> Option<RgbImageBuf> {
    let rd32 = |o: usize| -> Option<u32> { b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])) };
    let rd16 = |o: usize| -> Option<u16> { b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]])) };
    let hsize = rd32(0)? as usize;
    let width = rd32(4)? as i32;
    let height = rd32(8)? as i32;
    let bpp = rd16(14)? as usize;
    let comp = rd32(16)?;
    let clr_used = rd32(32).unwrap_or(0) as usize;
    if width <= 0 || height == 0 || !(bpp == 24 || bpp == 32) || !(comp == 0 || comp == 3) {
        return None;
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    let masks = if comp == 3 && hsize == 40 { 12 } else { 0 };
    let off = hsize + masks + clr_used * 4;
    let stride = (w * bpp).div_ceil(32) * 4;
    if b.len() < off + stride * h {
        return None;
    }
    let bottom_up = height > 0;
    let px = bpp / 8;
    let mut data = vec![0u8; w * h * 3];
    for y in 0..h {
        let sy = if bottom_up { h - 1 - y } else { y };
        let row = &b[off + sy * stride..off + sy * stride + w * px];
        for x in 0..w {
            let s = &row[x * px..x * px + 3];
            let d = &mut data[(y * w + x) * 3..(y * w + x) * 3 + 3];
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
        }
    }
    Some(RgbImageBuf { w, h, data, alpha: None })
}

// ------------------------------------------------------------------ list view

pub fn listview(parent: HWND, id: u16, single: bool) -> HWND {
    let mut style = WS_TABSTOP | LVS_REPORT | LVS_SHOWSELALWAYS;
    if single {
        style |= LVS_SINGLESEL;
    }
    let h = create(WC_LISTVIEWW, "", style, WS_EX_CLIENTEDGE, parent, id);
    let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP;
    send(h, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as LPARAM);
    unsafe { SetWindowTheme(h, w("Explorer").as_ptr(), null()) };
    h
}

pub fn lv_column(h: HWND, i: i32, text: &str, width: i32, right: bool) {
    let mut t = w(text);
    let mut c: LVCOLUMNW = unsafe { std::mem::zeroed() };
    c.mask = LVCF_TEXT | LVCF_WIDTH | LVCF_FMT;
    c.fmt = if right { LVCFMT_RIGHT } else { LVCFMT_LEFT };
    c.cx = sc(width);
    c.pszText = t.as_mut_ptr();
    send(h, LVM_INSERTCOLUMNW, i as usize, &c as *const _ as LPARAM);
}

pub fn lv_set_col_width(h: HWND, i: i32, width: i32) {
    send(h, LVM_SETCOLUMNWIDTH, i as usize, width as LPARAM);
}

/// Append a row; returns its index.
pub fn lv_add(h: HWND, cols: &[&str], param: isize) -> i32 {
    let n = send(h, LVM_GETITEMCOUNT, 0, 0) as i32;
    let mut t = w(cols.first().copied().unwrap_or(""));
    let mut it: LVITEMW = unsafe { std::mem::zeroed() };
    it.mask = LVIF_TEXT | LVIF_PARAM;
    it.iItem = n;
    it.pszText = t.as_mut_ptr();
    it.lParam = param;
    let row = send(h, LVM_INSERTITEMW, 0, &it as *const _ as LPARAM) as i32;
    for (c, s) in cols.iter().enumerate().skip(1) {
        lv_set(h, row, c as i32, s);
    }
    row
}

pub fn lv_set(h: HWND, row: i32, col: i32, text: &str) {
    let mut t = w(text);
    let mut it: LVITEMW = unsafe { std::mem::zeroed() };
    it.iSubItem = col;
    it.pszText = t.as_mut_ptr();
    send(h, LVM_SETITEMTEXTW, row as usize, &it as *const _ as LPARAM);
}

pub fn lv_clear(h: HWND) {
    send(h, LVM_DELETEALLITEMS, 0, 0);
}



pub fn lv_selected(h: HWND) -> Vec<i32> {
    let mut out = vec![];
    let mut i = -1isize;
    loop {
        i = send(h, LVM_GETNEXTITEM, i as usize, LVNI_SELECTED as LPARAM);
        if i < 0 {
            break;
        }
        out.push(i as i32);
    }
    out
}


pub fn lv_select(h: HWND, row: i32) {
    let mut it: LVITEMW = unsafe { std::mem::zeroed() };
    it.stateMask = LVIS_SELECTED | LVIS_FOCUSED;
    it.state = 0;
    send(h, LVM_SETITEMSTATE, usize::MAX, &it as *const _ as LPARAM); // clear all
    it.state = LVIS_SELECTED | LVIS_FOCUSED;
    send(h, LVM_SETITEMSTATE, row as usize, &it as *const _ as LPARAM);
    send(h, LVM_ENSUREVISIBLE, row as usize, 0);
}

// ------------------------------------------------------------------ formatting

pub fn human_size(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.0} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

/// RFC 3339 UTC -> local "YYYY-MM-DD HH:MM".
pub fn local_time(utc: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(utc)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|_| utc.to_string())
}

pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}
