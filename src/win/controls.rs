//! Two small owner-drawn controls: an image box (scaled-to-fit preview with a hint text)
//! and a coloured result banner.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::util::*;
use blindmark::registry::{thumbnail, RgbImageBuf};

const IMG_CLASS: windows_sys::core::PCWSTR = windows_sys::core::w!("BlindmarkImageBox");
const BANNER_CLASS: windows_sys::core::PCWSTR = windows_sys::core::w!("BlindmarkBanner");

pub fn register_classes() {
    unsafe {
        for (name, proc_) in [
            (IMG_CLASS, img_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT),
            (BANNER_CLASS, banner_proc),
        ] {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(proc_),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinst(),
                hIcon: null_mut(),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                hbrBackground: null_mut(),
                lpszMenuName: null(),
                lpszClassName: name,
                hIconSm: null_mut(),
            };
            RegisterClassExW(&wc);
        }
    }
}

// ------------------------------------------------------------------ image box

#[derive(Default)]
struct ImgData {
    bgra: Vec<u8>,
    w: i32,
    h: i32,
    hint: String,
}

pub fn image_box(parent: HWND, hint: &str) -> HWND {
    let h = create(IMG_CLASS, "", 0, 0, parent, 0);
    let d = Box::new(ImgData { hint: hint.to_string(), ..Default::default() });
    unsafe { SetWindowLongPtrW(h, GWLP_USERDATA, Box::into_raw(d) as isize) };
    h
}

fn img_data<'a>(h: HWND) -> Option<&'a mut ImgData> {
    unsafe { (GetWindowLongPtrW(h, GWLP_USERDATA) as *mut ImgData).as_mut() }
}

/// Show an image (downscaled for display) or clear it.
pub fn image_set(h: HWND, img: Option<&RgbImageBuf>) {
    let Some(d) = img_data(h) else { return };
    match img {
        Some(im) => {
            let t = thumbnail(im, 1600);
            let mut bgra = vec![0u8; t.w * t.h * 4];
            for i in 0..t.w * t.h {
                bgra[i * 4] = t.data[i * 3 + 2];
                bgra[i * 4 + 1] = t.data[i * 3 + 1];
                bgra[i * 4 + 2] = t.data[i * 3];
                bgra[i * 4 + 3] = 255;
            }
            d.bgra = bgra;
            d.w = t.w as i32;
            d.h = t.h as i32;
        }
        None => {
            d.bgra.clear();
            d.w = 0;
            d.h = 0;
        }
    }
    invalidate(h);
}

pub fn image_hint(h: HWND, hint: &str) {
    if let Some(d) = img_data(h) {
        d.hint = hint.to_string();
        invalidate(h);
    }
}

unsafe extern "system" fn img_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let dc = BeginPaint(h, &mut ps);
                let (cw, ch) = client_size(h);
                let rc = RECT { left: 0, top: 0, right: cw, bottom: ch };
                let bg = CreateSolidBrush(rgb(250, 250, 250));
                FillRect(dc, &rc, bg);
                DeleteObject(bg as _);
                let border = CreateSolidBrush(rgb(200, 200, 200));
                FrameRect(dc, &rc, border);
                DeleteObject(border as _);
                if let Some(d) = img_data(h) {
                    if d.w > 0 && d.h > 0 {
                        let pad = sc(6);
                        let (aw, ah) = ((cw - 2 * pad).max(1) as f64, (ch - 2 * pad).max(1) as f64);
                        let s = (aw / d.w as f64).min(ah / d.h as f64).min(4.0);
                        let (dw, dh) = ((d.w as f64 * s).round() as i32, (d.h as f64 * s).round() as i32);
                        let (x, y) = ((cw - dw) / 2, (ch - dh) / 2);
                        let mut bi: BITMAPINFO = std::mem::zeroed();
                        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                        bi.bmiHeader.biWidth = d.w;
                        bi.bmiHeader.biHeight = -d.h;
                        bi.bmiHeader.biPlanes = 1;
                        bi.bmiHeader.biBitCount = 32;
                        bi.bmiHeader.biCompression = BI_RGB;
                        SetStretchBltMode(dc, HALFTONE);
                        SetBrushOrgEx(dc, 0, 0, null_mut());
                        StretchDIBits(dc, x, y, dw, dh, 0, 0, d.w, d.h, d.bgra.as_ptr() as _, &bi, DIB_RGB_COLORS, SRCCOPY);
                    } else if !d.hint.is_empty() {
                        let font = super::fonts().normal;
                        let old = SelectObject(dc, font as _);
                        SetBkMode(dc, TRANSPARENT as _);
                        SetTextColor(dc, rgb(120, 120, 120));
                        let mut t: Vec<u16> = d.hint.encode_utf16().collect();
                        let mut r = rc;
                        let mut calc = RECT { left: 0, top: 0, right: cw - sc(20), bottom: 0 };
                        DrawTextW(dc, t.as_mut_ptr(), t.len() as i32, &mut calc, DT_CENTER | DT_WORDBREAK | DT_CALCRECT);
                        let th = calc.bottom - calc.top;
                        r.left += sc(10);
                        r.right -= sc(10);
                        r.top = (ch - th) / 2;
                        r.bottom = r.top + th;
                        DrawTextW(dc, t.as_mut_ptr(), t.len() as i32, &mut r, DT_CENTER | DT_WORDBREAK);
                        SelectObject(dc, old);
                    }
                }
                EndPaint(h, &ps);
                0
            }
            WM_NCDESTROY => {
                let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut ImgData;
                if !p.is_null() {
                    drop(Box::from_raw(p));
                    SetWindowLongPtrW(h, GWLP_USERDATA, 0);
                }
                DefWindowProcW(h, msg, wp, lp)
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

// ------------------------------------------------------------------ banner

#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    Neutral,
    Busy,
    Good,
    Warn,
    Bad,
}

impl Tone {
    fn colors(self) -> (COLORREF, COLORREF) {
        match self {
            Tone::Neutral => (rgb(243, 243, 243), rgb(150, 150, 150)),
            Tone::Busy => (rgb(232, 241, 251), rgb(0, 120, 215)),
            Tone::Good => (rgb(230, 244, 234), rgb(46, 125, 50)),
            Tone::Warn => (rgb(255, 244, 229), rgb(178, 106, 0)),
            Tone::Bad => (rgb(253, 236, 234), rgb(198, 40, 40)),
        }
    }
}

struct BannerData {
    title: String,
    sub: String,
    tone: Tone,
}

pub fn banner(parent: HWND) -> HWND {
    let h = create(BANNER_CLASS, "", 0, 0, parent, 0);
    let d = Box::new(BannerData { title: String::new(), sub: String::new(), tone: Tone::Neutral });
    unsafe { SetWindowLongPtrW(h, GWLP_USERDATA, Box::into_raw(d) as isize) };
    h
}

pub fn banner_set(h: HWND, tone: Tone, title: &str, sub: &str) {
    let p = unsafe { GetWindowLongPtrW(h, GWLP_USERDATA) } as *mut BannerData;
    if let Some(d) = unsafe { p.as_mut() } {
        d.title = title.to_string();
        d.sub = sub.to_string();
        d.tone = tone;
        invalidate(h);
    }
}

unsafe extern "system" fn banner_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let dc = BeginPaint(h, &mut ps);
                let (cw, ch) = client_size(h);
                let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *const BannerData;
                if let Some(d) = p.as_ref() {
                    let (bgc, fg) = d.tone.colors();
                    let rc = RECT { left: 0, top: 0, right: cw, bottom: ch };
                    let bg = CreateSolidBrush(bgc);
                    FillRect(dc, &rc, bg);
                    DeleteObject(bg as _);
                    let fr = CreateSolidBrush(fg);
                    FrameRect(dc, &rc, fr);
                    let stripe = RECT { left: 0, top: 0, right: sc(5), bottom: ch };
                    FillRect(dc, &stripe, fr);
                    DeleteObject(fr as _);
                    SetBkMode(dc, TRANSPARENT as _);
                    let f = super::fonts();
                    let old = SelectObject(dc, f.big as _);
                    SetTextColor(dc, fg);
                    let mut t: Vec<u16> = d.title.encode_utf16().collect();
                    let mut r = RECT { left: sc(16), top: sc(8), right: cw - sc(10), bottom: sc(8) + sc(24) };
                    DrawTextW(dc, t.as_mut_ptr(), t.len() as i32, &mut r, DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
                    SelectObject(dc, f.normal as _);
                    SetTextColor(dc, rgb(60, 60, 60));
                    let mut s: Vec<u16> = d.sub.encode_utf16().collect();
                    let mut r2 = RECT { left: sc(16), top: sc(34), right: cw - sc(10), bottom: ch - sc(4) };
                    DrawTextW(dc, s.as_mut_ptr(), s.len() as i32, &mut r2, DT_LEFT | DT_WORDBREAK | DT_END_ELLIPSIS | DT_NOPREFIX);
                    SelectObject(dc, old);
                }
                EndPaint(h, &ps);
                0
            }
            WM_NCDESTROY => {
                let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut BannerData;
                if !p.is_null() {
                    drop(Box::from_raw(p));
                    SetWindowLongPtrW(h, GWLP_USERDATA, 0);
                }
                DefWindowProcW(h, msg, wp, lp)
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}
