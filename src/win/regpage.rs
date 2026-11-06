//! 登记表 page: searchable, sortable list of every issued image ID with a detail pane.

use std::path::PathBuf;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{BN_CLICKED, CBN_SELCHANGE, EN_CHANGE};

use super::controls::{image_box, image_hint, image_set};
use super::util::*;
use super::App;
use blindmark::registry::{load_image, Entry};

const SEARCH: u16 = 1301;
const FILTER: u16 = 1302;
const REFRESH: u16 = 1303;
const CSV: u16 = 1304;
const LIST: u16 = 1305;
const CHAN_EDIT: u16 = 1306;
const CHAN_SAVE: u16 = 1307;
const REVEAL: u16 = 1308;
const COPY_HASH: u16 = 1309;

pub struct RegPage {
    lbl_search: HWND,
    search: HWND,
    filter: HWND,
    refresh_btn: HWND,
    csv: HWND,
    count: HWND,
    list: HWND,
    grp: HWND,
    thumb: HWND,
    info: HWND,
    lbl_chan: HWND,
    chan_edit: HWND,
    chan_save: HWND,
    reveal_btn: HWND,
    copy_hash: HWND,
    sort_col: i32,
    sort_asc: bool,
    selected: Option<u32>,
}

impl RegPage {
    pub fn create(p: HWND) -> RegPage {
        let lbl_search = label(p, "搜索");
        let search = edit(p, SEARCH, "");
        cue(search, "编号、文件名或渠道");
        let filter = combo_list(p, FILTER);
        let refresh_btn = button(p, REFRESH, "刷新");
        let csv = button(p, CSV, "导出 CSV…");
        let count = label(p, "");
        let list = listview(p, LIST, true);
        for (i, (t, wd, r)) in
            [("编号", 100, false), ("原始文件", 200, false), ("渠道", 120, false), ("登记时间", 130, false), ("尺寸", 100, false), ("强度", 50, true)]
                .iter()
                .enumerate()
        {
            lv_column(list, i as i32, t, *wd, *r);
        }
        let grp = group(p, "详细信息");
        let thumb = image_box(p, "选择一条记录");
        let info = edit_readonly_multi(p, 0);
        let lbl_chan = label(p, "渠道");
        let chan_edit = edit(p, CHAN_EDIT, "");
        cue(chan_edit, "这份拷贝发给了谁");
        let chan_save = button(p, CHAN_SAVE, "保存");
        let reveal_btn = button(p, REVEAL, "打开输出位置");
        let copy_hash = button(p, COPY_HASH, "复制哈希");
        let r = RegPage {
            lbl_search,
            search,
            filter,
            refresh_btn,
            csv,
            count,
            list,
            grp,
            thumb,
            info,
            lbl_chan,
            chan_edit,
            chan_save,
            reveal_btn,
            copy_hash,
            sort_col: 3,
            sort_asc: false,
            selected: None,
        };
        r.enable_detail(false);
        r
    }

    fn enable_detail(&self, on: bool) {
        for h in [self.chan_edit, self.chan_save, self.reveal_btn, self.copy_hash] {
            enable(h, on);
        }
    }
}

pub fn fonts(_a: &App, _f: Fonts) {}

pub fn is_hint(a: &App, h: HWND) -> bool {
    h == a.regp.count
}

pub fn layout(a: &App, w: i32, h: i32) {
    let r = &a.regp;
    let m = sc(10);
    let bh = sc(28);
    mv(r.lbl_search, m, m + sc(5), sc(36), sc(20));
    mv(r.search, m + sc(38), m + sc(2), sc(220), sc(24));
    mv(r.filter, m + sc(266), m + sc(1), sc(160), sc(300));
    mv(r.refresh_btn, m + sc(434), m, sc(64), bh);
    mv(r.csv, m + sc(504), m, sc(96), bh);
    mv(r.count, m + sc(610), m + sc(5), (w - m - sc(610) - m).max(0), sc(20));

    let detail_w = sc(300).max(w / 3);
    let top = m + bh + sc(8);
    let lw = w - 2 * m - detail_w - sc(10);
    mv(r.list, m, top, lw, h - top - m);
    let gx = m + lw + sc(10);
    mv(r.grp, gx, top - sc(6), detail_w, h - top - m + sc(6));
    let ix = gx + sc(12);
    let iw = detail_w - sc(24);
    let th = (iw * 3 / 4).min(sc(220));
    mv(r.thumb, ix, top + sc(16), iw, th);
    let y_btn = h - m - sc(12) - bh;
    let y_chan = y_btn - sc(8) - sc(24);
    let info_top = top + sc(16) + th + sc(8);
    mv(r.info, ix, info_top, iw, y_chan - sc(8) - info_top);
    mv(r.lbl_chan, ix, y_chan + sc(4), sc(36), sc(20));
    mv(r.chan_save, ix + iw - sc(60), y_chan - sc(2), sc(60), bh);
    mv(r.chan_edit, ix + sc(38), y_chan, iw - sc(38) - sc(66), sc(24));
    let half = (iw - sc(8)) / 2;
    mv(r.reveal_btn, ix, y_btn, half, bh);
    mv(r.copy_hash, ix + half + sc(8), y_btn, half, bh);
}

/// Rebuild the channel filter and the list from the current registry.
pub fn refresh(a: &mut App) {
    let keep = cb_item_text(a.regp.filter, cb_sel(a.regp.filter).max(0));
    cb_clear(a.regp.filter);
    cb_add(a.regp.filter, "全部渠道");
    cb_add(a.regp.filter, "（未填写渠道）");
    let mut sel = 0;
    if let Some(reg) = &a.reg {
        for (i, c) in reg.channels().iter().enumerate() {
            cb_add(a.regp.filter, c);
            if *c == keep {
                sel = i as i32 + 2;
            }
        }
    }
    if keep == "（未填写渠道）" {
        sel = 1;
    }
    cb_set_sel(a.regp.filter, sel);
    fill(a);
}

fn fill(a: &mut App) {
    let r = &a.regp;
    lv_clear(r.list);
    let Some(reg) = &a.reg else {
        set_text(r.count, if a.reg_error.is_some() { "登记表不可用（密钥不匹配或文件损坏）" } else { "没有登记表" });
        show_detail(a, None);
        return;
    };
    let q = get_text(r.search).trim().to_lowercase();
    let fsel = cb_sel(r.filter);
    let fname = cb_item_text(r.filter, fsel);
    let mut rows: Vec<(u32, &Entry)> = reg
        .entries
        .iter()
        .filter_map(|(k, e)| k.parse::<u32>().ok().map(|id| (id, e)))
        .filter(|(_, e)| match fsel {
            ..=0 => true,
            1 => e.channel.is_none(),
            _ => e.channel.as_deref() == Some(fname.as_str()),
        })
        .filter(|(id, e)| {
            q.is_empty()
                || format!("0x{id:08x}").contains(&q)
                || e.source.to_lowercase().contains(&q)
                || e.channel.as_deref().unwrap_or("").to_lowercase().contains(&q)
        })
        .collect();
    let (col, asc) = (r.sort_col, r.sort_asc);
    rows.sort_by(|x, y| {
        let o = match col {
            0 => x.0.cmp(&y.0),
            1 => x.1.source.to_lowercase().cmp(&y.1.source.to_lowercase()),
            2 => x.1.channel.cmp(&y.1.channel),
            4 => (x.1.size[0] * x.1.size[1]).cmp(&(y.1.size[0] * y.1.size[1])),
            5 => x.1.strength.partial_cmp(&y.1.strength).unwrap_or(std::cmp::Ordering::Equal),
            _ => x.1.registered_utc.cmp(&y.1.registered_utc),
        };
        if asc { o } else { o.reverse() }
    });
    let total = reg.entries.len();
    let shown = rows.len();
    let mut keep_row = None;
    for (i, (id, e)) in rows.iter().enumerate() {
        lv_add(
            r.list,
            &[
                &format!("0x{id:08x}"),
                &e.source,
                e.channel.as_deref().unwrap_or(""),
                &local_time(&e.registered_utc),
                &format!("{} × {}", e.size[1], e.size[0]),
                &e.strength.map(|s| format!("{s:.1}")).unwrap_or_default(),
            ],
            *id as isize,
        );
        if Some(*id) == r.selected {
            keep_row = Some(i as i32);
        }
    }
    set_text(r.count, &if shown == total { format!("共 {total} 条") } else { format!("显示 {shown} / {total} 条") });
    match keep_row {
        Some(row) => lv_select(a.regp.list, row),
        None => show_detail(a, None),
    }
}

pub fn select_id(a: &mut App, id: u32) {
    set_text(a.regp.search, "");
    cb_set_sel(a.regp.filter, 0);
    a.regp.selected = Some(id);
    fill(a);
}

fn show_detail(a: &mut App, id: Option<u32>) {
    a.regp.selected = id;
    let entry = id.and_then(|i| a.reg.as_ref().and_then(|r| r.get(i).map(|e| (i, e.clone(), r.thumb_path(e)))));
    let r = &a.regp;
    match entry {
        Some((id, e, thumb)) => {
            match load_image(&thumb) {
                Ok(t) => image_set(r.thumb, Some(&t)),
                Err(_) => {
                    image_set(r.thumb, None);
                    image_hint(r.thumb, "缩略图缺失");
                }
            }
            let mut s = format!("编号：0x{id:08x}\n原始文件：{}\n", e.source);
            s += &format!("渠道：{}\n", e.channel.as_deref().unwrap_or("（未填写）"));
            s += &format!("登记时间：{}\n", local_time(&e.registered_utc));
            s += &format!("尺寸：{} × {}\n", e.size[1], e.size[0]);
            if let Some(st) = e.strength {
                s += &format!("强度：{st:.1}\n");
            }
            if let Some(t) = e.tile_px {
                s += &format!("水印周期：{t} px\n");
            }
            s += &format!("输出文件：{}\n", e.output.as_deref().unwrap_or("（未记录）"));
            s += &format!("原图像素 SHA-256：\n{}\n加水印后像素 SHA-256：\n{}", e.sha256_original, e.sha256_marked);
            set_text(r.info, &crlf(&s));
            set_text(r.chan_edit, e.channel.as_deref().unwrap_or(""));
            r.enable_detail(true);
            enable(r.reveal_btn, e.output.is_some());
        }
        None => {
            image_set(r.thumb, None);
            image_hint(r.thumb, "选择一条记录");
            set_text(r.info, "");
            set_text(r.chan_edit, "");
            r.enable_detail(false);
        }
    }
}

pub fn command(a: &mut App, id: u16, code: u32, _ctl: HWND) {
    match (id, code) {
        (SEARCH, EN_CHANGE) => fill(a),
        (FILTER, CBN_SELCHANGE) => fill(a),
        (REFRESH, BN_CLICKED) => a.reload_registry(),
        (CSV, BN_CLICKED) => a.export_csv(),
        (CHAN_SAVE, BN_CLICKED) => save_channel(a),
        (REVEAL, BN_CLICKED) => {
            let out = a.regp.selected.and_then(|i| a.reg.as_ref().and_then(|r| r.get(i)).and_then(|e| e.output.clone()));
            if let Some(o) = out {
                let p = PathBuf::from(o);
                if p.exists() {
                    reveal(&p);
                } else {
                    warn(a.hwnd, &format!("输出文件已不在原位置：\n{}", p.display()));
                }
            }
        }
        (COPY_HASH, BN_CLICKED) => {
            if let Some((id, e)) = a.regp.selected.and_then(|i| a.reg.as_ref().and_then(|r| r.get(i).map(|e| (i, e)))) {
                let t = format!(
                    "blindmark 0x{id:08x}\n原图像素 SHA-256：{}\n加水印后像素 SHA-256：{}\n",
                    e.sha256_original, e.sha256_marked
                );
                if set_clipboard_text(a.hwnd, &t) {
                    a.set_status("哈希已复制");
                }
            }
        }
        _ => {}
    }
}

fn save_channel(a: &mut App) {
    if a.embed.cancel.is_some() {
        warn(a.hwnd, "正在加水印，完成后再修改登记表。");
        return;
    }
    let Some(id) = a.regp.selected else { return };
    let text = get_text(a.regp.chan_edit);
    let res = match a.reg.as_mut() {
        Some(r) => r.set_channel(id, Some(text)),
        None => return,
    };
    match res {
        Ok(()) => {
            a.set_status(&format!("已更新 0x{id:08x} 的渠道"));
            super::embed::refresh_channels(a);
            refresh(a);
        }
        Err(e) => error(a.hwnd, &format!("保存失败：{e}")),
    }
}

pub fn notify(a: &mut App, nm: &NMHDR) -> LRESULT {
    if nm.idFrom as u16 != LIST {
        return 0;
    }
    match nm.code {
        LVN_ITEMCHANGED => {
            let lv = unsafe { &*(nm as *const NMHDR as *const NMLISTVIEW) };
            if lv.uChanged & LVIF_STATE != 0 && lv.uNewState & LVIS_SELECTED != 0 && lv.uOldState & LVIS_SELECTED == 0 {
                show_detail(a, Some(lv.lParam as u32));
            }
        }
        LVN_COLUMNCLICK => {
            let lv = unsafe { &*(nm as *const NMHDR as *const NMLISTVIEW) };
            if a.regp.sort_col == lv.iSubItem {
                a.regp.sort_asc = !a.regp.sort_asc;
            } else {
                a.regp.sort_col = lv.iSubItem;
                a.regp.sort_asc = lv.iSubItem != 3;
            }
            fill(a);
        }
        NM_DBLCLK => {
            let out = a.regp.selected.and_then(|i| a.reg.as_ref().and_then(|r| r.get(i)).and_then(|e| e.output.clone()));
            if let Some(o) = out {
                reveal(&PathBuf::from(o));
            }
        }
        _ => {}
    }
    0
}
