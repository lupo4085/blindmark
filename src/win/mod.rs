//! Native Win32 desktop app: a tabbed main window (Embed / Detect / Registry / Settings)
//! with a menu bar and status bar, plus About and License windows.
#![allow(clippy::too_many_lines)]

mod about;
mod config;
mod controls;
mod detect;
mod embed;
mod regpage;
mod setpage;
mod start;
mod util;
mod wizard;

use std::cell::Cell;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::{DragAcceptFiles, HDROP};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use blindmark::jobs::{EmbedOk, UserError};
use blindmark::registry::{Registry, ThumbIndex};
use blindmark::{keys, Report};
use config::Settings;
use util::*;

pub const WM_WORKER: u32 = WM_APP + 1;

const MAIN_CLASS: windows_sys::core::PCWSTR = windows_sys::core::w!("BlindmarkMain");
const PAGE_CLASS: windows_sys::core::PCWSTR = windows_sys::core::w!("BlindmarkPage");
pub const DLG_CLASS: windows_sys::core::PCWSTR = windows_sys::core::w!("BlindmarkDialog");

/// Menu / accelerator command IDs (100..199). Pages use 1100.., 1200.., 1300.., 1400..; dialogs 1500..
pub mod cmd {
    pub const ADD_FILES: u16 = 101;
    pub const ADD_FOLDER: u16 = 102;
    pub const DETECT_FILE: u16 = 103;
    pub const DETECT_CLIP: u16 = 104;
    pub const EXIT: u16 = 105;
    pub const WIZARD: u16 = 106;
    pub const START: u16 = 107;
    pub const REMOVE: u16 = 110;
    pub const CLEAR: u16 = 111;
    pub const COPY_REPORT: u16 = 112;
    pub const NEW_KEY: u16 = 120;
    pub const PICK_KEY: u16 = 121;
    pub const OPEN_REG: u16 = 122;
    pub const BACKUP_REG: u16 = 123;
    pub const EXPORT_CSV: u16 = 124;
    pub const REFRESH: u16 = 125;
    pub const LICENSE: u16 = 130;
    pub const ABOUT: u16 = 131;
    pub const TAB_EMBED: u16 = 140;
    pub const TAB_DETECT: u16 = 141;
    pub const TAB_REG: u16 = 142;
    pub const TAB_SET: u16 = 143;
}

pub const TAB_EMBED: usize = 0;
pub const TAB_DETECT: usize = 1;
pub const TAB_REG: usize = 2;
pub const TAB_SET: usize = 3;

pub enum WorkerMsg {
    EmbedRunning(usize),
    EmbedItem { row: usize, res: Result<EmbedOk, UserError> },
    EmbedDone { cancelled: bool },
    Detect { token: u64, res: Result<(Report, f64), String> },
}

pub struct App {
    pub hwnd: HWND,
    pub tab: HWND,
    pub status: HWND,
    pub pages: [HWND; 4],
    pub cur: usize,
    pub cfg: Settings,
    pub key: Option<Vec<u8>>,
    pub reg: Option<Registry>,
    pub reg_error: Option<String>,
    pub thumbs: Arc<ThumbIndex>,
    pub tx: Sender<WorkerMsg>,
    pub rx: Receiver<WorkerMsg>,
    pub embed: embed::EmbedPage,
    pub detect: detect::DetectPage,
    pub regp: regpage::RegPage,
    pub sett: setpage::SetPage,
    pub about: Option<about::About>,
    pub license: Option<about::License>,
}

thread_local! {
    static APP: Cell<*mut App> = const { Cell::new(null_mut()) };
    static FONTS: Cell<Option<Fonts>> = const { Cell::new(None) };
}

/// The single app instance (UI thread only).
pub fn app<'a>() -> Option<&'a mut App> {
    APP.with(|a| unsafe { a.get().as_mut() })
}

pub fn fonts() -> Fonts {
    FONTS.with(|f| match f.get() {
        Some(f) => f,
        None => {
            let n = Fonts::new();
            f.set(Some(n));
            n
        }
    })
}

fn reset_fonts() {
    FONTS.with(|f| {
        if let Some(old) = f.take() {
            old.destroy();
        }
        f.set(Some(Fonts::new()));
    });
}

pub fn hwnd_main() -> HWND {
    app().map(|a| a.hwnd).unwrap_or(null_mut())
}

// ------------------------------------------------------------------ entry point

pub fn run() {
    unsafe {
        CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32);
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_WIN95_CLASSES | ICC_TAB_CLASSES | ICC_BAR_CLASSES | ICC_PROGRESS_CLASS | ICC_LISTVIEW_CLASSES | ICC_LINK_CLASS | ICC_STANDARD_CLASSES,
        };
        InitCommonControlsEx(&icc);
    }
    let cfg = Settings::load();
    if cfg.threads > 0 {
        let _ = rayon::ThreadPoolBuilder::new().num_threads(cfg.threads).build_global();
    }
    register_classes();
    controls::register_classes();

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_ACCEPTFILES | WS_EX_CONTROLPARENT,
            MAIN_CLASS,
            w("blindmark 盲水印").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            null_mut(),
            null_mut(),
            hinst(),
            null(),
        )
    };
    if hwnd.is_null() {
        return;
    }
    set_dpi(unsafe { GetDpiForWindow(hwnd) });

    let (tx, rx) = channel();
    let tab = create(WC_TABCONTROLW, "", WS_CLIPSIBLINGS | WS_TABSTOP, 0, hwnd, 0);
    for (i, t) in ["加水印", "检测", "登记表", "设置"].iter().enumerate() {
        let mut s = w(t);
        let mut it: TCITEMW = unsafe { std::mem::zeroed() };
        it.mask = TCIF_TEXT;
        it.pszText = s.as_mut_ptr();
        send(tab, TCM_INSERTITEMW, i, &it as *const _ as LPARAM);
    }
    let status = create(STATUSCLASSNAMEW, "", SBARS_SIZEGRIP, 0, hwnd, 0);
    let mut pages = [null_mut(); 4];
    for p in pages.iter_mut() {
        *p = unsafe {
            CreateWindowExW(WS_EX_CONTROLPARENT, PAGE_CLASS, w("").as_ptr(), WS_CHILD | WS_CLIPSIBLINGS, 0, 0, 10, 10, hwnd, null_mut(), hinst(), null())
        };
    }

    let boxed = Box::new(App {
        hwnd,
        tab,
        status,
        pages,
        cur: 0,
        embed: embed::EmbedPage::create(pages[TAB_EMBED], &cfg),
        detect: detect::DetectPage::create(pages[TAB_DETECT]),
        regp: regpage::RegPage::create(pages[TAB_REG]),
        sett: setpage::SetPage::create(pages[TAB_SET]),
        cfg,
        key: None,
        reg: None,
        reg_error: None,
        thumbs: Arc::new(ThumbIndex::new()),
        tx,
        rx,
        about: None,
        license: None,
    });
    APP.with(|a| a.set(Box::into_raw(boxed)));
    let a = app().unwrap();

    unsafe {
        SetMenu(hwnd, build_menu());
        DragAcceptFiles(hwnd, 1);
        let icon = LoadIconW(hinst(), 1 as _);
        if !icon.is_null() {
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icon as LPARAM);
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, icon as LPARAM);
        }
    }
    set_font_tree(hwnd, fonts().normal);
    a.apply_special_fonts();

    // Window placement.
    unsafe {
        let mut show = SW_SHOWNORMAL;
        match a.cfg.window.filter(|_| a.cfg.remember_window) {
            Some([l, t, r, b, maxd]) if r - l > 200 && b - t > 200 => {
                let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
                wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
                wp.rcNormalPosition = RECT { left: l, top: t, right: r, bottom: b };
                wp.showCmd = SW_HIDE as u32;
                SetWindowPlacement(hwnd, &wp);
                if maxd != 0 {
                    show = SW_SHOWMAXIMIZED;
                }
            }
            _ => {
                SetWindowPos(hwnd, null_mut(), 0, 0, sc(980), sc(680), SWP_NOMOVE | SWP_NOZORDER);
            }
        }
        ShowWindow(hwnd, show);
    }

    a.load_key_and_registry(false);
    let start = if a.cfg.start_tab < 0 { a.cfg.last_tab } else { a.cfg.start_tab };
    a.select_tab(start.clamp(0, 3) as usize);
    a.layout();
    unsafe { UpdateWindow(hwnd) };

    if a.cfg.show_start_wizard {
        a.open_start();
    } else if a.key.is_none() {
        a.set_status("还没有密钥。按 F1 打开“开始使用”完成设置。");
    }

    let accel = build_accelerators();
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            let root = GetAncestor(msg.hwnd, GA_ROOT);
            if root == hwnd && !edit_owns_key(&msg) && TranslateAcceleratorW(hwnd, accel, &msg) != 0 {
                continue;
            }
            if !root.is_null() && IsDialogMessageW(root, &msg) != 0 {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        DestroyAcceleratorTable(accel);
    }
}

/// Keys that a focused text box needs for itself (paste, copy, delete) bypass the accelerators.
fn edit_owns_key(msg: &MSG) -> bool {
    if msg.message != WM_KEYDOWN {
        return false;
    }
    let f = unsafe { GetFocus() };
    if f.is_null() || !class_name(f).eq_ignore_ascii_case("edit") {
        return false;
    }
    let k = msg.wParam as u16;
    k == VK_DELETE || k == b'V' as u16 || k == b'C' as u16
}

fn register_classes() {
    unsafe {
        let icon = LoadIconW(hinst(), 1 as _);
        let mk = |name, proc_: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT, bg: HBRUSH| WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(proc_),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst(),
            hIcon: icon,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: bg,
            lpszMenuName: null(),
            lpszClassName: name,
            hIconSm: icon,
        };
        RegisterClassExW(&mk(MAIN_CLASS, main_proc, (COLOR_BTNFACE + 1) as usize as HBRUSH));
        RegisterClassExW(&mk(PAGE_CLASS, page_proc, (COLOR_WINDOW + 1) as usize as HBRUSH));
        RegisterClassExW(&mk(DLG_CLASS, about::dialog_proc, (COLOR_BTNFACE + 1) as usize as HBRUSH));
    }
}

fn build_menu() -> HMENU {
    unsafe {
        let add = |m: HMENU, id: u16, s: &str| {
            AppendMenuW(m, MF_STRING, id as usize, w(s).as_ptr());
        };
        let sep = |m: HMENU| {
            AppendMenuW(m, MF_SEPARATOR, 0, null());
        };
        let file = CreatePopupMenu();
        add(file, cmd::WIZARD, "使用向导…\tCtrl+W");
        sep(file);
        add(file, cmd::ADD_FILES, "添加图片…\tCtrl+O");
        add(file, cmd::ADD_FOLDER, "添加文件夹…");
        sep(file);
        add(file, cmd::DETECT_FILE, "检测图片…\tCtrl+D");
        add(file, cmd::DETECT_CLIP, "从剪贴板检测\tCtrl+V");
        sep(file);
        add(file, cmd::EXIT, "退出(&X)");
        let edit = CreatePopupMenu();
        add(edit, cmd::REMOVE, "移除所选\tDel");
        add(edit, cmd::CLEAR, "清空列表");
        sep(edit);
        add(edit, cmd::COPY_REPORT, "复制检测报告\tCtrl+Shift+C");
        let view = CreatePopupMenu();
        add(view, cmd::TAB_EMBED, "加水印\tCtrl+1");
        add(view, cmd::TAB_DETECT, "检测\tCtrl+2");
        add(view, cmd::TAB_REG, "登记表\tCtrl+3");
        add(view, cmd::TAB_SET, "设置\tCtrl+4");
        sep(view);
        add(view, cmd::REFRESH, "刷新登记表\tF5");
        let tools = CreatePopupMenu();
        add(tools, cmd::NEW_KEY, "新建密钥…");
        add(tools, cmd::PICK_KEY, "选择密钥文件…");
        sep(tools);
        add(tools, cmd::OPEN_REG, "打开登记表文件夹");
        add(tools, cmd::BACKUP_REG, "备份登记表…");
        add(tools, cmd::EXPORT_CSV, "导出登记表为 CSV…");
        let help = CreatePopupMenu();
        add(help, cmd::START, "开始使用…\tF1");
        sep(help);
        add(help, cmd::LICENSE, "许可证…");
        sep(help);
        add(help, cmd::ABOUT, "关于 blindmark(&A)…");
        let bar = CreateMenu();
        for (m, s) in [(file, "文件(&F)"), (edit, "编辑(&E)"), (view, "视图(&V)"), (tools, "工具(&T)"), (help, "帮助(&H)")] {
            AppendMenuW(bar, MF_POPUP, m as usize, w(s).as_ptr());
        }
        bar
    }
}

fn build_accelerators() -> HACCEL {
    let c = (FVIRTKEY | FCONTROL) as u8;
    let v = FVIRTKEY as u8;
    let mut t = vec![
        ACCEL { fVirt: c, key: b'O' as u16, cmd: cmd::ADD_FILES },
        ACCEL { fVirt: c, key: b'D' as u16, cmd: cmd::DETECT_FILE },
        ACCEL { fVirt: c, key: b'V' as u16, cmd: cmd::DETECT_CLIP },
        ACCEL { fVirt: c, key: b'W' as u16, cmd: cmd::WIZARD },
        ACCEL { fVirt: c | FSHIFT as u8, key: b'C' as u16, cmd: cmd::COPY_REPORT },
        ACCEL { fVirt: v, key: VK_DELETE, cmd: cmd::REMOVE },
        ACCEL { fVirt: v, key: VK_F5, cmd: cmd::REFRESH },
        ACCEL { fVirt: v, key: VK_F1, cmd: cmd::START },
    ];
    for (i, id) in [cmd::TAB_EMBED, cmd::TAB_DETECT, cmd::TAB_REG, cmd::TAB_SET].into_iter().enumerate() {
        t.push(ACCEL { fVirt: c, key: b'1' as u16 + i as u16, cmd: id });
    }
    unsafe { CreateAcceleratorTableW(t.as_ptr(), t.len() as i32) }
}

// ------------------------------------------------------------------ App

impl App {
    fn apply_special_fonts(&self) {
        let f = fonts();
        detect::fonts(self, f);
        regpage::fonts(self, f);
        about::fonts_apply(self, f);
    }

    pub fn layout(&self) {
        let (cw, ch) = client_size(self.hwnd);
        send(self.status, WM_SIZE, 0, 0);
        let mut sr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetWindowRect(self.status, &mut sr) };
        let sh = sr.bottom - sr.top;
        let parts = [cw - sc(470), cw - sc(300), cw - sc(150), -1];
        send(self.status, SB_SETPARTS, 4, parts.as_ptr() as LPARAM);
        let pad = sc(6);
        let (tw, th) = (cw - 2 * pad, ch - sh - 2 * pad);
        mv(self.tab, pad, pad, tw, th);
        let mut r = RECT { left: 0, top: 0, right: tw, bottom: th };
        send(self.tab, TCM_ADJUSTRECT, 0, &mut r as *mut _ as LPARAM);
        let (px, py, pw, ph) = (pad + r.left, pad + r.top, r.right - r.left, r.bottom - r.top);
        for p in self.pages {
            unsafe { SetWindowPos(p, HWND_TOP, px, py, pw, ph, SWP_NOACTIVATE) };
        }
        embed::layout(self, pw, ph);
        detect::layout(self, pw, ph);
        regpage::layout(self, pw, ph);
        setpage::layout(self, pw, ph);
    }

    pub fn select_tab(&mut self, i: usize) {
        self.cur = i;
        send(self.tab, TCM_SETCURSEL, i, 0);
        for (k, p) in self.pages.iter().enumerate() {
            show(*p, k == i);
        }
        self.cfg.last_tab = i as i32;
        if i == TAB_DETECT && self.cfg.auto_clipboard && !self.detect.busy {
            detect::from_clipboard(self, true);
        }
        if i == TAB_SET {
            setpage::load(self);
        }
    }

    pub fn set_status(&self, s: &str) {
        let ws = w(s);
        send(self.status, SB_SETTEXTW, 0, ws.as_ptr() as LPARAM);
    }

    pub fn update_status_parts(&self) {
        let key = match &self.key {
            Some(k) => format!("密钥 {}", keys::fingerprint(k)),
            None => "未加载密钥".into(),
        };
        let reg = match (&self.reg, &self.reg_error) {
            (Some(r), _) => format!("登记表 {} 条", r.entries.len()),
            (None, Some(_)) => "登记表不可用".into(),
            _ => "无登记表".into(),
        };
        for (i, s) in [(1usize, key), (2, reg), (3, format!("v{}", env!("CARGO_PKG_VERSION")))] {
            let ws = w(&s);
            send(self.status, SB_SETTEXTW, i, ws.as_ptr() as LPARAM);
        }
    }

    /// (Re)load the key file and the registry named in the settings.
    pub fn load_key_and_registry(&mut self, first_run: bool) {
        self.key = None;
        let kp = self.cfg.key_path.clone();
        if kp.as_os_str().is_empty() {
            // Key unloaded on purpose: nothing to read.
        } else if kp.exists() {
            match std::fs::read_to_string(&kp).map_err(anyhow::Error::from).and_then(|t| keys::parse_key_file(&t)) {
                Ok(k) => self.key = Some(k),
                Err(e) => error(self.hwnd, &format!("无法读取密钥文件：\n{}\n\n{e}", kp.display())),
            }
        } else if first_run
            && ask(
                self.hwnd,
                &format!(
                    "还没有密钥。\n\n现在新建一把吗？它将保存到：\n{}\n\n密钥决定了谁能读出水印——请务必备份，并且不要分享给任何人。",
                    kp.display()
                ),
            )
        {
            self.create_key_at(&kp);
        }
        self.reload_registry();
        if let Some(e) = &self.reg_error {
            warn(self.hwnd, &format!("登记表无法使用：\n{e}\n\n可以在“设置”里换一个登记表文件夹，或者选择与它匹配的密钥。"));
        }
        self.set_status(if self.key.is_some() { "就绪" } else { "未加载密钥——请在“设置”里新建或选择密钥" });
    }

    pub fn reload_registry(&mut self) {
        self.reg = None;
        self.reg_error = None;
        self.thumbs = Arc::new(ThumbIndex::new());
        if self.cfg.registry_dir.as_os_str().is_empty() {
            // Registry unloaded on purpose: keep it detached until the user picks a folder.
            embed::refresh_channels(self);
            regpage::refresh(self);
            self.update_status_parts();
            return;
        }
        match Registry::open(&self.cfg.registry_dir) {
            Ok(mut r) => {
                if let Some(k) = &self.key {
                    if let Err(e) = r.check_key(k) {
                        self.reg_error = Some(format!("{e}"));
                    } else {
                        self.reg = Some(r);
                    }
                } else {
                    self.reg = Some(r);
                }
            }
            Err(e) => self.reg_error = Some(format!("{e}")),
        }
        embed::refresh_channels(self);
        regpage::refresh(self);
        self.update_status_parts();
    }

    pub fn create_key_at(&mut self, path: &std::path::Path) -> bool {
        if path.exists() {
            error(self.hwnd, &format!("{} 已经存在。为避免覆盖已有密钥，请换一个文件名。", path.display()));
            return false;
        }
        let res = (|| -> anyhow::Result<String> {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d)?;
            }
            let k = keys::generate_key()?;
            std::fs::write(path, hex::encode(k) + "\n")?;
            Ok(keys::fingerprint(&k))
        })();
        match res {
            Ok(fp) => {
                self.cfg.key_path = path.to_path_buf();
                self.cfg.save();
                info(
                    self.hwnd,
                    &format!("已新建密钥（指纹 {fp}）：\n{}\n\n请立刻把这个文件备份到安全的地方。丢失密钥后将无法再读出水印。", path.display()),
                );
                true
            }
            Err(e) => {
                error(self.hwnd, &format!("新建密钥失败：{e}"));
                false
            }
        }
    }

    pub fn new_key(&mut self) {
        let def = self.cfg.key_path.with_file_name("blindmark-new.key");
        let name = def.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        if let Some(p) = save_file(self.hwnd, "新建密钥", "密钥文件 (*.key)\0*.key\0", &name, "key") {
            if !(self.reg.as_ref().is_some_and(|r| !r.entries.is_empty())
                && !ask(self.hwnd, "当前登记表是用旧密钥建立的，新密钥不能和它一起使用。\n\n新建后需要在“设置”里为新密钥选择一个新的（空的）登记表文件夹。继续吗？"))
            {
                // Overwrite prompt was shown by the dialog, but never replace a key file.
                if p.exists() {
                    error(self.hwnd, "不会覆盖已有的密钥文件，请换一个文件名。");
                    return;
                }
                if self.create_key_at(&p) {
                    self.load_key_and_registry(false);
                    setpage::load(self);
                }
            }
        }
    }

    pub fn pick_key(&mut self) {
        if let Some(p) = open_files(self.hwnd, "选择密钥文件", "密钥文件 (*.key)\0*.key\0所有文件 (*.*)\0*.*\0", false).into_iter().next() {
            let ok = std::fs::read_to_string(&p).map_err(anyhow::Error::from).and_then(|t| keys::parse_key_file(&t));
            match ok {
                Ok(_) => {
                    self.cfg.key_path = p;
                    self.cfg.save();
                    self.load_key_and_registry(false);
                    setpage::load(self);
                }
                Err(e) => error(self.hwnd, &format!("这不是有效的密钥文件：{e}")),
            }
        }
    }

    /// Detach the key from the app. The key file itself is left untouched.
    pub fn unload_key(&mut self) {
        if self.cfg.key_path.as_os_str().is_empty() && self.key.is_none() {
            info(self.hwnd, "当前没有已载入的密钥。");
            return;
        }
        if !ask(
            self.hwnd,
            "要从本程序卸载当前密钥吗？\n\n密钥文件不会被删除，仍保留在原位置。卸载后登记表也将不可用，直至重新载入同一密钥。",
        ) {
            return;
        }
        self.key = None;
        self.cfg.key_path = PathBuf::new();
        self.cfg.save();
        self.reload_registry();
        self.set_status("密钥已卸载（文件保留在原处）");
        setpage::load(self);
    }

    /// Detach the registry folder from the app. Nothing on disk is removed.
    pub fn unload_registry(&mut self) {
        if self.cfg.registry_dir.as_os_str().is_empty() {
            info(self.hwnd, "当前没有已载入的登记表。");
            return;
        }
        if !ask(
            self.hwnd,
            "要从本程序卸载当前登记表吗？\n\n登记表文件夹不会被删除，仍保留在原位置。卸载后需重新指定文件夹才能继续登记。",
        ) {
            return;
        }
        self.reg = None;
        self.cfg.registry_dir = PathBuf::new();
        self.cfg.save();
        self.reload_registry();
        self.set_status("登记表已卸载（文件夹保留在原处）");
        setpage::load(self);
    }

    pub fn backup_registry(&mut self) {
        let Some(r) = &self.reg else {
            warn(self.hwnd, "登记表当前不可用。");
            return;
        };
        if let Some(d) = pick_folder(self.hwnd, "选择备份保存位置") {
            match r.backup_to(&d) {
                Ok(t) => {
                    info(self.hwnd, &format!("已备份到：\n{}", t.display()));
                    reveal(&t);
                }
                Err(e) => error(self.hwnd, &format!("备份失败：{e}")),
            }
        }
    }

    /// Run the guided wizard, then reload the key and registry it may have changed.
    pub fn open_wizard(&mut self) {
        let changed = wizard::open(self.hwnd, &mut self.cfg);
        self.load_key_and_registry(false);
        self.set_status(if changed { "向导完成，登记表已更新" } else { "向导已结束" });
    }

    /// Run the new setup wizard (W-1) and apply the small piece of state it returns.
    pub fn open_start(&mut self) {
        let ctx = start::WizardContext {
            cfg: self.cfg.clone(),
            key: self.key.clone(),
            embed_files: embed::files(self),
        };
        let result = start::run(self.hwnd, ctx);
        if result.show_on_start != self.cfg.show_start_wizard {
            self.cfg.show_start_wizard = result.show_on_start;
            self.cfg.save();
        }
        self.load_key_and_registry(false);
        match result.outcome {
            start::WizardOutcome::Closed => {}
            // TODO(W-2/W-3): 接上 SettingsChanged / ShowTab / ShowInRegistry
            _ => {}
        }
    }

    pub fn export_csv(&mut self) {
        let Some(r) = &self.reg else {
            warn(self.hwnd, "登记表当前不可用。");
            return;
        };
        let name = format!("blindmark-registry-{}.csv", chrono::Local::now().format("%Y%m%d"));
        if let Some(p) = save_file(self.hwnd, "导出登记表", "CSV 文件 (*.csv)\0*.csv\0", &name, "csv") {
            match r.export_csv(&p) {
                Ok(()) => self.set_status(&format!("已导出 {} 条到 {}", r.entries.len(), p.display())),
                Err(e) => error(self.hwnd, &format!("导出失败：{e}")),
            }
        }
    }

    fn on_command(&mut self, id: u16, code: u32, ctl: HWND) {
        match id {
            cmd::ADD_FILES => {
                self.select_tab(TAB_EMBED);
                embed::add_files_dialog(self);
            }
            cmd::ADD_FOLDER => {
                self.select_tab(TAB_EMBED);
                embed::add_folder_dialog(self);
            }
            cmd::DETECT_FILE => {
                self.select_tab(TAB_DETECT);
                detect::open_dialog(self);
            }
            cmd::DETECT_CLIP => {
                self.select_tab(TAB_DETECT);
                detect::from_clipboard(self, false);
            }
            cmd::EXIT => unsafe {
                PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
            },
            cmd::WIZARD => self.open_wizard(),
            cmd::START => self.open_start(),
            cmd::REMOVE => {
                if self.cur == TAB_EMBED {
                    embed::remove_selected(self);
                }
            }
            cmd::CLEAR => embed::clear(self),
            cmd::COPY_REPORT => detect::copy_report(self),
            cmd::NEW_KEY => self.new_key(),
            cmd::PICK_KEY => self.pick_key(),
            cmd::OPEN_REG => {
                let _ = std::fs::create_dir_all(&self.cfg.registry_dir);
                shell_open(&self.cfg.registry_dir.display().to_string());
            }
            cmd::BACKUP_REG => self.backup_registry(),
            cmd::EXPORT_CSV => self.export_csv(),
            cmd::REFRESH => {
                self.reload_registry();
                self.set_status("已刷新登记表");
            }
            cmd::LICENSE => about::open_license(self, self.hwnd),
            cmd::ABOUT => about::open_about(self),
            cmd::TAB_EMBED..=cmd::TAB_SET => self.select_tab((id - cmd::TAB_EMBED) as usize),
            1100..=1199 => embed::command(self, id, code, ctl),
            1200..=1299 => detect::command(self, id, code, ctl),
            1300..=1399 => regpage::command(self, id, code, ctl),
            1400..=1499 => setpage::command(self, id, code, ctl),
            _ => {}
        }
    }

    fn on_notify(&mut self, nm: &NMHDR) -> LRESULT {
        if nm.hwndFrom == self.tab && nm.code == TCN_SELCHANGE {
            let i = send(self.tab, TCM_GETCURSEL, 0, 0);
            if i >= 0 {
                self.select_tab(i as usize);
            }
            return 0;
        }
        let id = nm.idFrom as u16;
        match id {
            1100..=1199 => embed::notify(self, nm),
            1200..=1299 => detect::notify(self, nm),
            1300..=1399 => regpage::notify(self, nm),
            _ => 0,
        }
    }

    fn on_drop(&mut self, files: Vec<PathBuf>) {
        if files.is_empty() {
            return;
        }
        if self.cur == TAB_DETECT {
            detect::open_path(self, &files[0]);
        } else {
            self.select_tab(TAB_EMBED);
            embed::add_paths(self, &files);
        }
    }

    fn drain_worker(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                WorkerMsg::EmbedRunning(row) => embed::on_running(self, row),
                WorkerMsg::EmbedItem { row, res } => embed::on_item(self, row, res),
                WorkerMsg::EmbedDone { cancelled } => embed::on_done(self, cancelled),
                WorkerMsg::Detect { token, res } => detect::on_result(self, token, res),
            }
        }
    }

    fn on_dpi_changed(&mut self, dpi: u32, suggested: &RECT) {
        set_dpi(dpi);
        reset_fonts();
        set_font_tree(self.hwnd, fonts().normal);
        self.apply_special_fonts();
        unsafe {
            SetWindowPos(
                self.hwnd,
                null_mut(),
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        embed::on_dpi(self);
        self.layout();
    }

    fn save_window(&mut self) {
        unsafe {
            let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
            wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
            if GetWindowPlacement(self.hwnd, &mut wp) != 0 {
                let r = wp.rcNormalPosition;
                self.cfg.window = Some([r.left, r.top, r.right, r.bottom, (wp.showCmd == SW_SHOWMAXIMIZED as u32) as i32]);
            }
        }
        self.cfg.save();
    }
}

/// Post a worker message and wake the UI thread. `hwnd` is the main window as an integer.
pub fn post(tx: &Sender<WorkerMsg>, hwnd: isize, m: WorkerMsg) {
    let _ = tx.send(m);
    unsafe { PostMessageW(hwnd as HWND, WM_WORKER, 0, 0) };
}

// ------------------------------------------------------------------ window procedures

unsafe extern "system" fn main_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let Some(a) = app() else { return DefWindowProcW(h, msg, wp, lp) };
        if a.hwnd != h {
            return DefWindowProcW(h, msg, wp, lp);
        }
        match msg {
            WM_SIZE => {
                a.layout();
                0
            }
            WM_GETMINMAXINFO => {
                let mm = &mut *(lp as *mut MINMAXINFO);
                mm.ptMinTrackSize.x = sc(860);
                mm.ptMinTrackSize.y = sc(600);
                0
            }
            WM_COMMAND => {
                a.on_command((wp & 0xffff) as u16, ((wp >> 16) & 0xffff) as u32, lp as HWND);
                0
            }
            WM_NOTIFY => a.on_notify(&*(lp as *const NMHDR)),
            WM_DROPFILES => {
                let files = dropped_files(wp as HDROP);
                a.on_drop(files);
                0
            }
            WM_WORKER => {
                a.drain_worker();
                0
            }
            WM_DPICHANGED => {
                a.on_dpi_changed((wp & 0xffff) as u32, &*(lp as *const RECT));
                0
            }
            WM_CLOSE => {
                if a.embed.cancel.is_some() && !ask(h, "正在加水印。确定要退出吗？已完成的图片会保留。") {
                    return 0;
                }
                if let Some(c) = &a.embed.cancel {
                    c.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                a.save_window();
                DestroyWindow(h);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

unsafe extern "system" fn page_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND | WM_NOTIFY | WM_DROPFILES => {
                // Forward to the main window so all routing lives in one place.
                let m = hwnd_main();
                if m.is_null() {
                    return 0;
                }
                SendMessageW(m, msg, wp, lp)
            }
            WM_HSCROLL => {
                if let Some(a) = app() {
                    let ctl = lp as HWND;
                    embed::on_scroll(a, ctl);
                    setpage::on_scroll(a, ctl);
                }
                0
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
                let dc = wp as HDC;
                SetBkColor(dc, GetSysColor(COLOR_WINDOW));
                if let Some(a) = app() {
                    if regpage::is_hint(a, lp as HWND) || setpage::is_hint(a, lp as HWND) || embed::is_hint(a, lp as HWND) {
                        SetTextColor(dc, rgb(100, 100, 100));
                    }
                }
                GetSysColorBrush(COLOR_WINDOW) as LRESULT
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}
