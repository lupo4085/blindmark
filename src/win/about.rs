//! 关于 (About) and 许可证 (License) windows. Both are modal: the owner is disabled while open.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use windows_sys::Win32::System::SystemServices::{SS_ETCHEDHORZ, SS_ICON, SS_REALSIZECONTROL};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};

use super::util::*;
use super::{app, fonts, App, DLG_CLASS};
use blindmark::keys;

const ID_OK: u16 = IDOK as u16;
const ID_CANCEL: u16 = IDCANCEL as u16;
const ID_LICENSE: u16 = 1501;
const ID_COPY: u16 = 1502;
const ID_SAVE: u16 = 1503;
const ID_LINK: u16 = 1504;
const ID_TABS: u16 = 1505;

const MIT: &str = include_str!("../../LICENSE-MIT");
const APACHE: &str = include_str!("../../LICENSE-APACHE");
const THIRD_PARTY: &str = include_str!(concat!(env!("OUT_DIR"), "/third_party.txt"));

pub struct About {
    hwnd: HWND,
    title: HWND,
    owner: HWND,
}

pub struct License {
    hwnd: HWND,
    owner: HWND,
    tabs: HWND,
    edit: HWND,
    cur: usize,
}

fn license_text(i: usize) -> String {
    let head = match i {
        0 => "blindmark 可以按 MIT 许可证使用（也可以选择 Apache-2.0，见下一个选项卡）。\n\n",
        1 => "blindmark 可以按 Apache License 2.0 使用（也可以选择 MIT，见上一个选项卡）。\n\n",
        _ => "",
    };
    crlf(&(head.to_string() + [MIT, APACHE, THIRD_PARTY][i.min(2)]))
}

/// Create a captioned popup with the given client size, centred on the owner, and disable the owner.
fn make_dialog(owner: HWND, title: &str, cw: i32, ch: i32) -> HWND {
    unsafe {
        let style = WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;
        let ex = WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT;
        let mut r = RECT { left: 0, top: 0, right: sc(cw), bottom: sc(ch) };
        AdjustWindowRectEx(&mut r, style, 0, ex);
        let (ww, wh) = (r.right - r.left, r.bottom - r.top);
        let mut or = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetWindowRect(owner, &mut or);
        let x = or.left + ((or.right - or.left) - ww) / 2;
        let y = or.top + ((or.bottom - or.top) - wh) / 2;
        let h = CreateWindowExW(ex, DLG_CLASS, w(title).as_ptr(), style, x.max(0), y.max(0), ww, wh, owner, null_mut(), hinst(), null());
        EnableWindow(owner, 0);
        h
    }
}

fn finish_dialog(h: HWND) {
    set_font_tree(h, fonts().normal);
    unsafe {
        ShowWindow(h, SW_SHOW);
        UpdateWindow(h);
    }
}

pub fn open_about(a: &mut App) {
    if a.about.is_some() {
        return;
    }
    let h = make_dialog(a.hwnd, "关于 blindmark", 460, 450);
    let m = sc(20);
    let icon = create(windows_sys::core::w!("STATIC"), "", SS_ICON | SS_REALSIZECONTROL, 0, h, 0);
    unsafe {
        let hi = LoadImageW(hinst(), 1 as _, IMAGE_ICON, sc(64), sc(64), 0);
        if !hi.is_null() {
            SendMessageW(icon, STM_SETICON, hi as usize, 0);
        }
    }
    mv(icon, m, m, sc(64), sc(64));
    let tx = m + sc(80);
    let tw = sc(460) - tx - m;
    let title = label(h, "blindmark 盲水印");
    mv(title, tx, m, tw, sc(28));
    let ver = label(h, &format!("版本 {} · 水印格式 {}", env!("CARGO_PKG_VERSION"), keys::FORMAT));
    mv(ver, tx, m + sc(32), tw, sc(20));
    let build = label(h, &format!("构建于 {} · {}", env!("BM_BUILD_DATE"), env!("BM_GIT_HASH")));
    mv(build, tx, m + sc(52), tw, sc(20));
    let desc = label(
        h,
        "给自己的图片加上看不见的水印。截图、缩放、裁剪、旋转、翻转之后仍能读出，用来证明所有权，并通过每份拷贝独有的编号追踪泄露来源。",
    );
    mv(desc, m, m + sc(88), sc(420), sc(60));
    let sep = create(windows_sys::core::w!("STATIC"), "", SS_ETCHEDHORZ, 0, h, 0);
    mv(sep, m, m + sc(156), sc(420), sc(2));
    let key = a.key.as_ref().map(|k| keys::fingerprint(k)).unwrap_or_else(|| "未加载".into());
    let reg = a.reg.as_ref().map(|r| format!("{} 条", r.entries.len())).unwrap_or_else(|| "不可用".into());
    let mut y = m + sc(170);
    for (k, v) in [
        ("作者", "Lupo".to_string()),
        ("许可证", "MIT 或 Apache-2.0（双许可，任选其一）".to_string()),
        ("当前密钥", format!("指纹 {key}")),
        ("登记表", reg),
    ] {
        let l = label(h, k);
        mv(l, m, y, sc(80), sc(20));
        let v = label(h, &v);
        mv(v, m + sc(84), y, sc(336), sc(20));
        y += sc(24);
    }
    let copy = label(h, "© 2026 Lupo");
    mv(copy, m, y + sc(10), sc(420), sc(20));
    let link = create(
        WC_LINK,
        "本软件按“原样”提供，不附带任何明示或暗示的担保。<a id=\"license\">查看许可证全文</a>",
        WS_TABSTOP,
        0,
        h,
        ID_LINK,
    );
    mv(link, m, y + sc(34), sc(420), sc(40));
    let lic = button(h, ID_LICENSE, "许可证…");
    let ok = default_button(h, ID_OK, "确定");
    mv(lic, sc(460) - m - sc(90) - sc(8) - sc(96), sc(450) - m - sc(28), sc(96), sc(28));
    mv(ok, sc(460) - m - sc(90), sc(450) - m - sc(28), sc(90), sc(28));
    a.about = Some(About { hwnd: h, title, owner: a.hwnd });
    finish_dialog(h);
    set_font(title, fonts().big);
    unsafe { SetFocus(ok) };
}

pub fn open_license(a: &mut App, owner: HWND) {
    if a.license.is_some() {
        return;
    }
    let h = make_dialog(owner, "许可证", 600, 520);
    let m = sc(12);
    let tabs = create(WC_TABCONTROLW, "", WS_TABSTOP | WS_CLIPSIBLINGS, 0, h, ID_TABS);
    for (i, t) in ["MIT", "Apache-2.0", "第三方组件"].iter().enumerate() {
        let mut s = w(t);
        let mut it: TCITEMW = unsafe { std::mem::zeroed() };
        it.mask = TCIF_TEXT;
        it.pszText = s.as_mut_ptr();
        send(tabs, TCM_INSERTITEMW, i, &it as *const _ as LPARAM);
    }
    let (cw, ch) = (sc(600), sc(520));
    let bh = sc(28);
    mv(tabs, m, m, cw - 2 * m, ch - 3 * m - bh);
    let edit = create(
        windows_sys::core::w!("EDIT"),
        "",
        WS_TABSTOP | WS_VSCROLL | WS_CLIPSIBLINGS | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32,
        WS_EX_CLIENTEDGE,
        h,
        0,
    );
    let mut r = RECT { left: 0, top: 0, right: cw - 2 * m, bottom: ch - 3 * m - bh };
    send(tabs, TCM_ADJUSTRECT, 0, &mut r as *mut _ as LPARAM);
    unsafe { SetWindowPos(edit, HWND_TOP, m + r.left, m + r.top, r.right - r.left, r.bottom - r.top, 0) };
    send(edit, EM_SETLIMITTEXT, 0, 0);
    let copy = button(h, ID_COPY, "复制全文");
    let save = button(h, ID_SAVE, "另存为…");
    let ok = default_button(h, ID_OK, "确定");
    let by = ch - m - bh;
    mv(copy, m, by, sc(96), bh);
    mv(save, m + sc(104), by, sc(96), bh);
    mv(ok, cw - m - sc(90), by, sc(90), bh);
    a.license = Some(License { hwnd: h, owner, tabs, edit, cur: 0 });
    finish_dialog(h);
    set_font(edit, fonts().mono);
    set_text(edit, &license_text(0));
    unsafe { SetFocus(ok) };
}

pub fn fonts_apply(a: &App, f: Fonts) {
    if let Some(ab) = &a.about {
        set_font_tree(ab.hwnd, f.normal);
        set_font(ab.title, f.big);
    }
    if let Some(l) = &a.license {
        set_font_tree(l.hwnd, f.normal);
        set_font(l.edit, f.mono);
    }
}

fn close(a: &mut App, h: HWND) {
    // The License window may be owned by About: close it first.
    if let Some(lh) = a.license.as_ref().filter(|l| l.owner == h).map(|l| l.hwnd) {
        close(a, lh);
    }
    let owner = if a.about.as_ref().is_some_and(|x| x.hwnd == h) {
        a.about.take().map(|x| x.owner)
    } else if a.license.as_ref().is_some_and(|x| x.hwnd == h) {
        a.license.take().map(|x| x.owner)
    } else {
        None
    };
    unsafe {
        if let Some(o) = owner {
            EnableWindow(o, 1);
            SetForegroundWindow(o);
        }
        DestroyWindow(h);
    }
}

pub unsafe extern "system" fn dialog_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let Some(a) = app() else { return DefWindowProcW(h, msg, wp, lp) };
        let is_about = a.about.as_ref().is_some_and(|x| x.hwnd == h);
        let is_license = a.license.as_ref().is_some_and(|x| x.hwnd == h);
        if !is_about && !is_license {
            return DefWindowProcW(h, msg, wp, lp);
        }
        match msg {
            WM_COMMAND => {
                let id = (wp & 0xffff) as u16;
                match id {
                    ID_OK | ID_CANCEL => close(a, h),
                    ID_LICENSE => open_license(a, h),
                    ID_COPY => {
                        if let Some(l) = &a.license {
                            if set_clipboard_text(h, &license_text(l.cur)) {
                                a.set_status("许可证文本已复制");
                            }
                        }
                    }
                    ID_SAVE => {
                        if let Some(cur) = a.license.as_ref().map(|l| l.cur) {
                            let name = ["LICENSE-MIT.txt", "LICENSE-APACHE.txt", "THIRD-PARTY-LICENSES.txt"][cur.min(2)];
                            if let Some(p) = save_file(h, "保存许可证", "文本文件 (*.txt)\0*.txt\0", name, "txt") {
                                if let Err(e) = std::fs::write(&p, license_text(cur)) {
                                    error(h, &format!("保存失败：{e}"));
                                }
                            }
                        }
                    }
                    _ => {}
                }
                0
            }
            WM_NOTIFY => {
                let nm = &*(lp as *const NMHDR);
                if nm.idFrom as u16 == ID_LINK && (nm.code == NM_CLICK || nm.code == NM_RETURN) {
                    open_license(a, h);
                } else if nm.idFrom as u16 == ID_TABS && nm.code == TCN_SELCHANGE {
                    if let Some(l) = a.license.as_mut() {
                        l.cur = send(l.tabs, TCM_GETCURSEL, 0, 0).max(0) as usize;
                        set_text(l.edit, &license_text(l.cur));
                    }
                }
                0
            }
            WM_CTLCOLORSTATIC => {
                if a.license.as_ref().is_some_and(|l| l.edit == lp as HWND) {
                    SetBkColor(wp as HDC, GetSysColor(COLOR_WINDOW));
                    return GetSysColorBrush(COLOR_WINDOW) as LRESULT;
                }
                DefWindowProcW(h, msg, wp, lp)
            }
            WM_CLOSE => {
                close(a, h);
                0
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}
