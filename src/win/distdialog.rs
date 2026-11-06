//! "分发给接收人" dialog: images on the left, recipients (grouped) in the middle,
//! output options on the right. Runs the distribution on a worker thread.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::Graphics::Gdi::UpdateWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use blindmark::distribute::{distribute, DistOpts, DistResult, Target};
use blindmark::recipients::Book;

use super::util::*;
use super::{app, fonts};

const ID_LABEL_IMG: u16 = 1721;
const ID_LIST_IMG: u16 = 1701;
const ID_GROUP: u16 = 1702;
const ID_LIST_REC: u16 = 1703;
const ID_OUT: u16 = 1704;
const ID_BROWSE: u16 = 1705;
const ID_PER_DIR: u16 = 1706;
const ID_STAMP: u16 = 1707;
const ID_STATUS: u16 = 1708;
const ID_PROGRESS: u16 = 1709;
const ID_LOG: u16 = 1710;
const ID_START: u16 = 1711;
const ID_CLOSE: u16 = 1712;
const ID_LABEL_OUT: u16 = 1720;

const LVS_EX_CHECKBOXES: u32 = 0x0000_0004;
const LVIS_STATEIMAGEMASK: u32 = 0xf000;
const PBM_SETMARQUEE: u32 = WM_USER + 10;

struct Rec {
    id: String,
    name: String,
    group: String,
    group_id: Option<String>,
}

struct Dist {
    owner: HWND,
    files: Vec<PathBuf>,
    recs: Vec<Rec>,
    rx: Option<Receiver<Result<DistResult, String>>>,
    busy: bool,
}

thread_local! {
    static STATE: std::cell::Cell<*mut Dist> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

fn st<'a>() -> &'a mut Dist {
    STATE.with(|c| unsafe { &mut *c.get() })
}

fn ctl(h: HWND, id: u16) -> HWND {
    unsafe { GetDlgItem(h, id as i32) }
}

fn lv_checked(lv: HWND, i: i32) -> bool {
    ((send(lv, LVM_GETITEMSTATE, i as usize, LVIS_STATEIMAGEMASK as isize) >> 12) as u32) == 2
}

fn lv_set_checked(lv: HWND, i: i32, on: bool) {
    let mut it: LVITEMW = unsafe { std::mem::zeroed() };
    it.stateMask = LVIS_STATEIMAGEMASK;
    it.state = (if on { 2 } else { 1 }) << 12;
    send(lv, LVM_SETITEMSTATE, i as usize, &it as *const _ as isize);
}

fn fill_groups(h: HWND) {
    let Some(a) = app() else { return };
    let g = ctl(h, ID_GROUP);
    cb_clear(g);
    cb_add(g, "全部接收人");
    if let Ok(book) = Book::open(&a.cfg.registry_dir) {
        for grp in &book.groups {
            cb_add(g, &book.group_path(&grp.id));
        }
    }
    cb_set_sel(g, 0);
}

fn fill_recipients(h: HWND) {
    let Some(a) = app() else { return };
    let sel = cb_sel(ctl(h, ID_GROUP));
    let want: Option<String> = if sel <= 0 {
        None
    } else {
        Book::open(&a.cfg.registry_dir).ok().and_then(|b| b.groups.get(sel as usize - 1).map(|g| g.id.clone()))
    };

    let lv = ctl(h, ID_LIST_REC);
    lv_clear(lv);
    let mut rows = Vec::new();
    if let Ok(book) = Book::open(&a.cfg.registry_dir) {
        for r in book.items.iter().filter(|r| !r.archived) {
            let gpath = r.group_id.as_deref().map(|g| book.group_path(g)).unwrap_or_else(|| "未分组".into());
            if let Some(w) = &want {
                if r.group_id.as_deref() != Some(w.as_str()) {
                    continue;
                }
            }
            rows.push(Rec { id: r.id.clone(), name: r.name.clone(), group: gpath, group_id: r.group_id.clone() });
        }
    }
    for (i, r) in rows.iter().enumerate() {
        lv_add(lv, &[r.name.as_str(), r.group.as_str()], i as isize);
        lv_set_checked(lv, i as i32, true);
    }
    st().recs = rows;
}

fn build(h: HWND) {
    let files = st().files.clone();
    let lv = listview(h, ID_LIST_IMG, true);
    lv_column(lv, 0, "文件", sc(210), false);
    for (i, f) in files.iter().enumerate() {
        let name = f.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
        lv_add(lv, &[name.as_str()], i as isize);
    }

    let _ = combo_list(h, ID_GROUP);
    fill_groups(h);

    let rl = listview(h, ID_LIST_REC, true);
    lv_column(rl, 0, "接收人", sc(140), false);
    lv_column(rl, 1, "分组", sc(150), false);
    unsafe { send(rl, LVM_SETEXTENDEDLISTVIEWSTYLE, LVS_EX_CHECKBOXES as usize, LVS_EX_CHECKBOXES as isize) };
    fill_recipients(h);

    create(windows_sys::core::w!("STATIC"), "图片", 0, 0, h, ID_LABEL_IMG);
    create(windows_sys::core::w!("STATIC"), "输出文件夹", 0, 0, h, ID_LABEL_OUT);
    edit(h, ID_OUT, "");
    button(h, ID_BROWSE, "浏览…");
    checkbox(h, ID_PER_DIR, "按接收人分文件夹");
    checkbox(h, ID_STAMP, "为本批次添加可信时间戳");
    set_check(ctl(h, ID_PER_DIR), true);

    if let Some(a) = app() {
        set_check(ctl(h, ID_STAMP), a.cfg.timestamp);
        set_text(ctl(h, ID_OUT), &a.cfg.out_dir);
    }

    progress(h);
    create(windows_sys::core::w!("STATIC"), "勾选接收人后点击“开始分发”。", 0, 0, h, ID_STATUS);
    let style = (WS_VSCROLL as u32) | (ES_MULTILINE as u32) | (ES_READONLY as u32) | (ES_AUTOVSCROLL as u32);
    create(windows_sys::core::w!("EDIT"), "", style, WS_EX_CLIENTEDGE, h, ID_LOG);
    default_button(h, ID_START, "开始分发");
    button(h, ID_CLOSE, "关闭");
}

fn layout(h: HWND) {
    let (w, ht) = client_size(h);
    let m = sc(12);
    let bh = sc(28);
    let top = m + sc(20);
    let col = (w - 3 * m) / 3;
    let mid = m + col + m;
    let right = m + 2 * (col + m);
    let list_h = ht - top - sc(150);

    mv(ctl(h, ID_LABEL_IMG), m, m - sc(2), col, sc(18));
    mv(ctl(h, ID_LIST_IMG), m, top, col, list_h);
    mv(ctl(h, ID_GROUP), mid, m - sc(4), col, sc(220));
    mv(ctl(h, ID_LIST_REC), mid, top, col, list_h);

    mv(ctl(h, ID_LABEL_OUT), right, m - sc(2), col, sc(18));
    mv(ctl(h, ID_OUT), right, m + sc(18), col - sc(66), sc(24));
    mv(ctl(h, ID_BROWSE), right + col - sc(62), m + sc(16), sc(62), bh);
    mv(ctl(h, ID_PER_DIR), right, m + sc(50), col, sc(22));
    mv(ctl(h, ID_STAMP), right, m + sc(76), col, sc(22));
    mv(ctl(h, ID_STATUS), right, m + sc(104), col, sc(20));

    mv(ctl(h, ID_PROGRESS), m, ht - sc(104), w - 2 * m, sc(14));
    mv(ctl(h, ID_LOG), m, ht - sc(84), w - 2 * m - sc(230), sc(72));
    mv(ctl(h, ID_START), w - m - sc(110), ht - m - bh, sc(110), bh);
    mv(ctl(h, ID_CLOSE), w - m - sc(110) - sc(8) - sc(76), ht - m - bh, sc(76), bh);
}

fn start(h: HWND) {
    let Some(a) = app() else { return };
    if st().busy {
        return;
    }
    let Some(key) = a.key.clone() else {
        warn(h, "尚未载入密钥，无法分发。");
        return;
    };
    let lv = ctl(h, ID_LIST_REC);
    let rows = send(lv, LVM_GETITEMCOUNT, 0, 0) as i32;
    let mut targets = Vec::new();
    for i in 0..rows {
        if !lv_checked(lv, i) {
            continue;
        }
        let r = &st().recs[i as usize];
        targets.push(Target {
            id: r.id.clone(),
            name: r.name.clone(),
            group_id: r.group_id.clone(),
            group_name: Some(r.group.clone()),
        });
    }
    if targets.is_empty() {
        warn(h, "请至少勾选一位接收人。");
        return;
    }
    let files = st().files.clone();
    let out = get_text(ctl(h, ID_OUT));
    let root = if out.trim().is_empty() {
        match files.first().and_then(|f| f.parent()) {
            Some(p) => p.to_path_buf(),
            None => return,
        }
    } else {
        PathBuf::from(out.trim())
    };
    let cfg = a.cfg.clone();
    let reg_dir = cfg.registry_dir.clone();
    let per_dir = checked(ctl(h, ID_PER_DIR));
    let stamp = checked(ctl(h, ID_STAMP));
    let batch = format!("b_{}", chrono::Local::now().format("%Y%m%d_%H%M%S"));
    let tsa = if stamp { Some(cfg.tsa_url.clone()) } else { None };

    let (tx, rx) = channel();
    st().rx = Some(rx);
    st().busy = true;
    enable(ctl(h, ID_START), false);
    set_text(ctl(h, ID_STATUS), "正在分发…");
    send(ctl(h, ID_PROGRESS), PBM_SETMARQUEE, 1, 30);

    std::thread::spawn(move || {
        let res = (|| -> Result<DistResult, String> {
            let mut reg = blindmark::registry::Registry::open(&reg_dir).map_err(|e| format!("{e:#}"))?;
            reg.check_key(&key).map_err(|e| format!("{e:#}"))?;
            let opts = DistOpts {
                root,
                suffix: cfg.suffix.clone(),
                jpeg_quality: cfg.jpeg.then_some(cfg.jpeg_quality),
                strength: cfg.strength,
                per_recipient_dir: per_dir,
                batch_id: Some(batch),
                tsa,
            };
            distribute(&files, &targets, &key, &mut reg, &opts).map_err(|e| format!("{e:#}"))
        })();
        let _ = tx.send(res);
    });

    unsafe { SetTimer(h, 1, 100, None) };
}

fn drain(h: HWND) {
    let msgs: Vec<_> = {
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
    for m in msgs {
        match m {
            Ok(res) => {
                let mut text = String::new();
                for c in &res.copies {
                    text.push_str(&format!("{} — 0x{:08x} — {}\r\n", c.recipient, c.id, c.path.display()));
                }
                if let Some(s) = &res.stamp {
                    text.push_str(&if s.ok {
                        format!("\r\n时间戳已获取：{}\r\n", s.dir.display())
                    } else {
                        format!("\r\n时间戳失败：{}\r\n", s.error.as_deref().unwrap_or("未知错误"))
                    });
                }
                set_text(ctl(h, ID_LOG), &text);
                set_text(ctl(h, ID_STATUS), &format!("完成：{} 份", res.copies.len()));
            }
            Err(e) => {
                error(h, &e);
                set_text(ctl(h, ID_STATUS), "分发失败。");
            }
        }
    }
    {
        let s = st();
        s.busy = false;
        s.rx = None;
    }
    unsafe { KillTimer(h, 1) };
    enable(ctl(h, ID_START), true);
}

fn close(h: HWND) {
    unsafe { DestroyWindow(h) };
}

pub unsafe extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    match msg {
        WM_CREATE => {
            build(h);
            layout(h);
            set_font_tree(h, fonts().normal);
            0
        }
        WM_SIZE => {
            layout(h);
            0
        }
        WM_TIMER => {
            if wp == 1 {
                drain(h);
            }
            0
        }
        WM_COMMAND => {
            match (wp & 0xffff) as u16 {
                ID_START => start(h),
                ID_CLOSE => close(h),
                ID_BROWSE => {
                    if let Some(d) = pick_folder(h, "选择输出文件夹") {
                        set_text(ctl(h, ID_OUT), &d.display().to_string());
                    }
                }
                ID_GROUP => {
                    if ((wp >> 16) & 0xffff) as u32 == CBN_SELCHANGE {
                        fill_recipients(h);
                    }
                }
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            close(h);
            0
        }
        WM_DESTROY => {
            let owner = st().owner;
            let raw = STATE.with(|c| c.replace(std::ptr::null_mut()));
            if !raw.is_null() {
                unsafe { drop(Box::from_raw(raw)) };
            }
            unsafe { EnableWindow(owner, 1) };
            0
        }
        _ => unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
}

/// Open the dialog over `owner` with the given images.
pub fn open(owner: HWND, files: Vec<PathBuf>) {
    if files.is_empty() {
        warn(owner, "请先在列表中加入图片。");
        return;
    }
    unsafe {
        let style = WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;
        let ex = WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT;
        let mut r = RECT { left: 0, top: 0, right: sc(820), bottom: sc(560) };
        AdjustWindowRectEx(&mut r, style, 0, ex);
        let (ww, wh) = (r.right - r.left, r.bottom - r.top);
        let mut or = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetWindowRect(owner, &mut or);
        let x = or.left + ((or.right - or.left) - ww) / 2;
        let y = or.top + ((or.bottom - or.top) - wh) / 2;

        let raw = Box::into_raw(Box::new(Dist { owner, files, recs: Vec::new(), rx: None, busy: false }));
        STATE.with(|c| c.set(raw));
        let h = CreateWindowExW(
            ex,
            super::DIST_CLASS,
            w("分发给接收人").as_ptr(),
            style,
            x.max(0),
            y.max(0),
            ww,
            wh,
            owner,
            std::ptr::null_mut(),
            hinst(),
            std::ptr::null(),
        );
        if h.is_null() {
            STATE.with(|c| c.set(std::ptr::null_mut()));
            drop(Box::from_raw(raw));
            return;
        }
        EnableWindow(owner, 0);
        ShowWindow(h, SW_SHOW);
        UpdateWindow(h);
        SetFocus(h);
    }
}
