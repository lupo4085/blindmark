#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! blindmark desktop app (egui).

use blindmark::registry::{load_image, thumbnail, Registry, RgbImageBuf};
use blindmark::{keys, wm, Report};
use eframe::egui::{self, Color32, RichText, TextureHandle, TextureOptions};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;

const SIM_OK: f64 = 0.6; // content-similarity threshold ("same picture")
const IMG_EXT: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif"];

// ------------------------------------------------------------------ config

#[derive(Serialize, Deserialize, Clone)]
struct Config {
    key_path: Option<PathBuf>,
    registry_dir: Option<PathBuf>,
    out_dir: Option<PathBuf>,
    strength: f32,
    channel: String,
    jpeg: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config { key_path: None, registry_dir: None, out_dir: None, strength: wm::DEFAULT_STRENGTH, channel: String::new(), jpeg: false }
    }
}

fn config_dir() -> PathBuf {
    std::env::var_os("BLINDMARK_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("blindmark"))
}

fn load_config() -> Config {
    std::fs::read_to_string(config_dir().join("config.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_config(c: &Config) {
    let d = config_dir();
    let _ = std::fs::create_dir_all(&d);
    let _ = std::fs::write(d.join("config.json"), serde_json::to_string_pretty(c).unwrap_or_default());
}

fn open_in_file_manager(p: &Path) {
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(p).spawn();
}

fn is_image(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| IMG_EXT.contains(&e.to_ascii_lowercase().as_str())).unwrap_or(false)
}

// ------------------------------------------------------------------ app state

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Embed,
    Detect,
    Registry,
    Settings,
}

struct EmbedRow {
    input: PathBuf,
    status: Result<(u32, f64, PathBuf), String>,
}

enum EmbedMsg {
    Row(EmbedRow),
    Done,
}

struct DetectJob {
    rx: Receiver<Result<(Report, f64), String>>,
}

struct Preview {
    name: String,
    img: RgbImageBuf,
    tex: TextureHandle,
}

struct App {
    cfg: Config,
    tab: Tab,
    key: Option<Vec<u8>>,
    key_msg: Option<String>,
    reg: Option<Registry>,
    reg_msg: Option<String>,
    // embed
    queue: Vec<PathBuf>,
    rows: Vec<EmbedRow>,
    embed_rx: Option<Receiver<EmbedMsg>>,
    embed_total: usize,
    // detect
    preview: Option<Preview>,
    detect_job: Option<DetectJob>,
    result: Option<Result<(Report, f64), String>>,
    // registry view
    filter: String,
    selected: Option<u32>,
    thumbs: std::collections::HashMap<u32, Option<TextureHandle>>,
    status: Option<(String, Instant)>,
    // test hooks
    test_shot: Option<PathBuf>,
    frames: u64,
}

impl App {
    fn new(cc: &eframe::CreationContext) -> Self {
        let mut style = (*cc.egui_ctx.global_style()).clone();
        use egui::{FontId, TextStyle::*};
        style.text_styles = [
            (Heading, FontId::proportional(22.0)),
            (Body, FontId::proportional(15.0)),
            (Button, FontId::proportional(15.0)),
            (Small, FontId::proportional(12.5)),
            (Monospace, FontId::monospace(14.0)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        cc.egui_ctx.set_global_style(style);

        let mut app = App {
            cfg: load_config(),
            tab: Tab::Embed,
            key: None,
            key_msg: None,
            reg: None,
            reg_msg: None,
            queue: vec![],
            rows: vec![],
            embed_rx: None,
            embed_total: 0,
            preview: None,
            detect_job: None,
            result: None,
            filter: String::new(),
            selected: None,
            thumbs: Default::default(),
            status: None,
            test_shot: std::env::var_os("BLINDMARK_TEST_SHOT").map(PathBuf::from),
            frames: 0,
        };
        app.reload_key();
        app.reload_registry();
        if app.key.is_none() {
            app.tab = Tab::Settings;
        }
        // --- test hooks (used for automated screenshots) ---
        if let Ok(t) = std::env::var("BLINDMARK_TEST_TAB") {
            app.tab = match t.as_str() {
                "detect" => Tab::Detect,
                "registry" => Tab::Registry,
                "settings" => Tab::Settings,
                _ => Tab::Embed,
            };
        }
        if let Ok(files) = std::env::var("BLINDMARK_TEST_OPEN") {
            for f in files.split(';').filter(|s| !s.is_empty()) {
                if app.tab == Tab::Detect {
                    app.open_for_detect(&cc.egui_ctx, Path::new(f));
                } else {
                    app.queue.push(PathBuf::from(f));
                }
            }
        }
        if std::env::var("BLINDMARK_TEST_RUN").is_ok() && app.tab == Tab::Embed {
            app.start_embed(&cc.egui_ctx);
        }
        if let Ok(id) = std::env::var("BLINDMARK_TEST_SELECT") {
            app.selected = u32::from_str_radix(id.trim_start_matches("0x"), 16).ok();
        }
        app
    }

    fn flash(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    fn reload_key(&mut self) {
        self.key = None;
        self.key_msg = None;
        if let Some(p) = &self.cfg.key_path {
            match std::fs::read_to_string(p).map_err(anyhow::Error::from).and_then(|s| keys::parse_key_file(&s)) {
                Ok(k) => self.key = Some(k),
                Err(e) => self.key_msg = Some(format!("无法读取密钥：{e}")),
            }
        }
        self.check_pair();
    }

    fn reload_registry(&mut self) {
        self.reg = None;
        self.reg_msg = None;
        self.thumbs.clear();
        if let Some(d) = &self.cfg.registry_dir {
            match Registry::open(d) {
                Ok(r) => self.reg = Some(r),
                Err(e) => self.reg_msg = Some(format!("无法打开登记表：{e:#}")),
            }
        }
        self.check_pair();
    }

    /// The registry remembers which key it belongs to; refuse to mix them.
    fn check_pair(&mut self) {
        if let (Some(k), Some(r)) = (&self.key, &mut self.reg) {
            if let Err(e) = r.check_key(k) {
                self.reg_msg = Some(format!("密钥与登记表不匹配：{e}"));
            } else if self.reg_msg.as_deref().is_some_and(|m| m.starts_with("密钥与登记表不匹配")) {
                self.reg_msg = None;
            }
        }
    }

    fn ready(&self) -> bool {
        self.key.is_some() && self.reg.is_some() && self.reg_msg.is_none()
    }

    // -------------------------------------------------------------- jobs

    fn start_embed(&mut self, ctx: &egui::Context) {
        let (Some(key), Some(reg_dir)) = (self.key.clone(), self.cfg.registry_dir.clone()) else { return };
        if self.queue.is_empty() || self.embed_rx.is_some() {
            return;
        }
        let out_dir = self.cfg.out_dir.clone().unwrap_or_else(|| {
            self.queue[0].parent().map(|p| p.join("watermarked")).unwrap_or_else(|| PathBuf::from("watermarked"))
        });
        self.cfg.out_dir = Some(out_dir.clone());
        save_config(&self.cfg);
        let files = std::mem::take(&mut self.queue);
        self.embed_total = files.len();
        self.rows.clear();
        let (tx, rx) = channel();
        self.embed_rx = Some(rx);
        let (strength, chan, jpeg) = (self.cfg.strength, self.cfg.channel.clone(), self.cfg.jpeg);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut reg = match Registry::open(&reg_dir) {
                Ok(r) => r,
                Err(e) => {
                    for f in files {
                        let _ = tx.send(EmbedMsg::Row(EmbedRow { input: f, status: Err(format!("{e:#}")) }));
                    }
                    let _ = tx.send(EmbedMsg::Done);
                    return;
                }
            };
            for f in files {
                let stem = f.file_stem().unwrap_or_default().to_string_lossy().to_string();
                let mut out = out_dir.join(format!("{stem}_wm.{}", if jpeg { "jpg" } else { "png" }));
                let mut n = 2;
                while out.exists() {
                    out = out_dir.join(format!("{stem}_wm{n}.{}", if jpeg { "jpg" } else { "png" }));
                    n += 1;
                }
                let ch = (!chan.trim().is_empty()).then(|| chan.clone());
                let status = blindmark::embed_file(&f, &out, &key, &mut reg, strength, ch, jpeg.then_some(95))
                    .map(|r| (r.id, r.psnr, out.clone()))
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(EmbedMsg::Row(EmbedRow { input: f, status }));
                ctx.request_repaint();
            }
            let _ = tx.send(EmbedMsg::Done);
            ctx.request_repaint();
        });
    }

    fn open_for_detect(&mut self, ctx: &egui::Context, p: &Path) {
        match load_image(p) {
            Ok(img) => self.set_detect_image(ctx, p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), img),
            Err(e) => self.flash(format!("无法打开 {}：{e:#}", p.display())),
        }
    }

    fn paste_for_detect(&mut self, ctx: &egui::Context) {
        let got = arboard::Clipboard::new().and_then(|mut c| c.get_image());
        match got {
            Ok(im) => {
                let rgba = image::RgbaImage::from_raw(im.width as u32, im.height as u32, im.bytes.into_owned());
                match rgba {
                    Some(r) => {
                        let img = blindmark::registry::from_dynamic(image::DynamicImage::ImageRgba8(r));
                        self.set_detect_image(ctx, "剪贴板截图".into(), img);
                    }
                    None => self.flash("剪贴板图片格式无法识别"),
                }
            }
            Err(_) => self.flash("剪贴板里没有图片"),
        }
    }

    fn set_detect_image(&mut self, ctx: &egui::Context, name: String, img: RgbImageBuf) {
        let th = thumbnail(&img, 1400);
        let tex = ctx.load_texture("detect-preview", egui::ColorImage::from_rgb([th.w, th.h], &th.data), TextureOptions::LINEAR);
        self.preview = Some(Preview { name, img, tex });
        self.result = None;
        self.start_detect(ctx);
    }

    fn start_detect(&mut self, ctx: &egui::Context) {
        let (Some(key), Some(pv)) = (self.key.clone(), &self.preview) else { return };
        let img = pv.img.clone();
        let reg_dir = self.cfg.registry_dir.clone();
        let (tx, rx) = channel();
        self.detect_job = Some(DetectJob { rx });
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let t = Instant::now();
            let reg = reg_dir.and_then(|d| Registry::open(&d).ok());
            let known = reg.as_ref().map(|r| wm::KnownIds::new(&key, r.ids()));
            let rep = blindmark::detect_image(&img, &key, reg.as_ref(), known.as_ref());
            let _ = tx.send(Ok((rep, t.elapsed().as_secs_f64())));
            ctx.request_repaint();
        });
    }

    fn poll_jobs(&mut self) {
        if let Some(rx) = &self.embed_rx {
            let mut done = false;
            while let Ok(m) = rx.try_recv() {
                match m {
                    EmbedMsg::Row(r) => self.rows.push(r),
                    EmbedMsg::Done => done = true,
                }
            }
            if done {
                self.embed_rx = None;
                let ok = self.rows.iter().filter(|r| r.status.is_ok()).count();
                self.flash(format!("完成：{ok}/{} 张已加水印并登记", self.rows.len()));
                self.reload_registry();
            }
        }
        if let Some(j) = &self.detect_job {
            if let Ok(r) = j.rx.try_recv() {
                self.result = Some(r);
                self.detect_job = None;
            }
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if dropped.is_empty() {
            return;
        }
        match self.tab {
            Tab::Detect => {
                if let Some(p) = dropped.iter().find(|p| is_image(p)) {
                    self.open_for_detect(ctx, &p.clone());
                }
            }
            _ => {
                self.tab = Tab::Embed;
                for p in dropped {
                    if p.is_dir() {
                        if let Ok(rd) = std::fs::read_dir(&p) {
                            let mut v: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| is_image(p)).collect();
                            v.sort();
                            self.queue.extend(v);
                        }
                    } else if is_image(&p) && !self.queue.contains(&p) {
                        self.queue.push(p);
                    }
                }
            }
        }
    }

    fn thumb_tex(&mut self, ctx: &egui::Context, id: u32) -> Option<TextureHandle> {
        if !self.thumbs.contains_key(&id) {
            let t = self.reg.as_ref().and_then(|r| r.get(id).map(|e| r.thumb_path(e))).and_then(|p| load_image(&p).ok()).map(|im| {
                ctx.load_texture(format!("thumb-{id}"), egui::ColorImage::from_rgb([im.w, im.h], &im.data), TextureOptions::LINEAR)
            });
            self.thumbs.insert(id, t);
        }
        self.thumbs.get(&id).cloned().flatten()
    }
}

// ------------------------------------------------------------------ UI helpers

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::group(ui.style()).inner_margin(12.0).corner_radius(8.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical(add).inner
    })
    .inner
}

fn status_frame(ui: &mut egui::Ui, color: Color32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.12))
        .stroke(egui::Stroke::new(1.5, color))
        .corner_radius(8.0)
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
}

fn kv(ui: &mut egui::Ui, k: &str, v: impl Into<String>) {
    ui.label(RichText::new(k).weak());
    ui.label(v.into());
    ui.end_row();
}

fn short_path(p: &Path) -> String {
    let s = p.display().to_string();
    if s.chars().count() > 60 { format!("…{}", s.chars().rev().take(58).collect::<String>().chars().rev().collect::<String>()) } else { s }
}

const GREEN: Color32 = Color32::from_rgb(34, 160, 90);
const AMBER: Color32 = Color32::from_rgb(214, 140, 20);
const RED: Color32 = Color32::from_rgb(200, 60, 60);
const GREY: Color32 = Color32::from_rgb(130, 130, 140);

// ------------------------------------------------------------------ pages

impl App {
    fn page_embed(&mut self, ui: &mut egui::Ui) {
        ui.heading("给图片加水印");
        ui.label(RichText::new("发布前处理：每张图分配一个编号，写进不可见水印，并记录到登记表。原图不会被修改。").weak());
        ui.add_space(4.0);
        let busy = self.embed_rx.is_some();
        card(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new("➕ 添加图片…")).clicked() {
                    if let Some(fs) = rfd::FileDialog::new().add_filter("图片", IMG_EXT).pick_files() {
                        for f in fs {
                            if !self.queue.contains(&f) {
                                self.queue.push(f);
                            }
                        }
                    }
                }
                if ui.add_enabled(!busy, egui::Button::new("📁 添加文件夹…")).clicked() {
                    if let Some(d) = rfd::FileDialog::new().pick_folder() {
                        if let Ok(rd) = std::fs::read_dir(d) {
                            let mut v: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| is_image(p)).collect();
                            v.sort();
                            self.queue.extend(v);
                        }
                    }
                }
                if !self.queue.is_empty() && ui.add_enabled(!busy, egui::Button::new("清空")).clicked() {
                    self.queue.clear();
                }
                ui.label(RichText::new("也可以直接把图片或文件夹拖进窗口").weak());
            });
            if self.queue.is_empty() {
                ui.add_space(6.0);
                ui.vertical_centered(|ui| ui.label(RichText::new("待处理列表为空").weak()));
                ui.add_space(6.0);
            } else {
                egui::ScrollArea::vertical().max_height(150.0).id_salt("queue").show(ui, |ui| {
                    let mut remove = None;
                    for (i, p) in self.queue.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.small_button("✕").clicked() {
                                remove = Some(i);
                            }
                            ui.label(p.file_name().unwrap_or_default().to_string_lossy());
                            ui.label(RichText::new(short_path(p.parent().unwrap_or(Path::new("")))).weak().small());
                        });
                    }
                    if let Some(i) = remove {
                        self.queue.remove(i);
                    }
                });
            }
        });

        card(ui, |ui| {
            egui::Grid::new("embed-opts").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                ui.label("输出文件夹");
                ui.horizontal(|ui| {
                    let txt = self.cfg.out_dir.as_ref().map(|p| short_path(p)).unwrap_or_else(|| "（默认：原图旁边的 watermarked 文件夹）".into());
                    ui.label(txt);
                    if ui.button("选择…").clicked() {
                        if let Some(d) = rfd::FileDialog::new().pick_folder() {
                            self.cfg.out_dir = Some(d);
                            save_config(&self.cfg);
                        }
                    }
                });
                ui.end_row();
                ui.label("渠道备注");
                ui.add(egui::TextEdit::singleline(&mut self.cfg.channel).hint_text("例如：微博 / 发给某客户 / 作品集网站").desired_width(320.0));
                ui.end_row();
                ui.label("水印强度");
                ui.horizontal(|ui| {
                    ui.label(RichText::new("更隐蔽").small().weak());
                    ui.add(egui::Slider::new(&mut self.cfg.strength, 1.5..=4.0).step_by(0.1));
                    ui.label(RichText::new("更抗压缩").small().weak());
                });
                ui.end_row();
                ui.label("输出格式");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.cfg.jpeg, false, "PNG（推荐）");
                    ui.radio_value(&mut self.cfg.jpeg, true, "JPEG 95");
                });
                ui.end_row();
            });
        });

        ui.horizontal(|ui| {
            let can = self.ready() && !self.queue.is_empty() && !busy;
            let label = format!("开始加水印（{} 张）", self.queue.len());
            if ui.add_enabled(can, egui::Button::new(RichText::new(label).strong()).min_size(egui::vec2(200.0, 34.0))).clicked() {
                save_config(&self.cfg);
                let ctx = ui.ctx().clone();
                self.start_embed(&ctx);
            }
            if !self.ready() {
                ui.label(RichText::new("请先在「密钥与设置」里加载密钥和登记表文件夹").color(AMBER));
            }
            if let Some(d) = &self.cfg.out_dir {
                if d.exists() && ui.button("打开输出文件夹").clicked() {
                    open_in_file_manager(d);
                }
            }
        });

        if busy || !self.rows.is_empty() {
            if busy {
                let f = self.rows.len() as f32 / self.embed_total.max(1) as f32;
                ui.add(egui::ProgressBar::new(f).text(format!("{}/{}", self.rows.len(), self.embed_total)).animate(true));
            }
            card(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("rows").show(ui, |ui| {
                    egui::Grid::new("rows-grid").striped(true).num_columns(4).spacing([18.0, 6.0]).show(ui, |ui| {
                        for h in ["文件", "编号", "PSNR", "结果"] {
                            ui.label(RichText::new(h).strong());
                        }
                        ui.end_row();
                        for r in &self.rows {
                            ui.label(r.input.file_name().unwrap_or_default().to_string_lossy());
                            match &r.status {
                                Ok((id, psnr, out)) => {
                                    ui.monospace(format!("0x{id:08x}"));
                                    ui.label(format!("{psnr:.1} dB"));
                                    ui.label(RichText::new(format!("✔ {}", out.file_name().unwrap_or_default().to_string_lossy())).color(GREEN));
                                }
                                Err(e) => {
                                    ui.label("—");
                                    ui.label("—");
                                    ui.label(RichText::new(format!("✕ {e}")).color(RED));
                                }
                            }
                            ui.end_row();
                        }
                    });
                });
            });
        }
    }

    fn page_detect(&mut self, ui: &mut egui::Ui) {
        ui.heading("检测图片");
        ui.label(RichText::new("看到疑似被搬运的图或截图时，用它检查里面有没有你的水印。不需要原图。").weak());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("🖼 打开图片…").clicked() {
                if let Some(f) = rfd::FileDialog::new().add_filter("图片", IMG_EXT).pick_file() {
                    let ctx = ui.ctx().clone();
                    self.open_for_detect(&ctx, &f);
                }
            }
            if ui.button("📋 粘贴截图（Ctrl+V）").clicked() {
                let ctx = ui.ctx().clone();
                self.paste_for_detect(&ctx);
            }
            if self.preview.is_some() && self.detect_job.is_none() && ui.button("重新检测").clicked() {
                let ctx = ui.ctx().clone();
                self.start_detect(&ctx);
            }
            ui.label(RichText::new("也可以把图片拖进窗口").weak());
        });
        if !self.ready() {
            ui.label(RichText::new("请先在「密钥与设置」里加载密钥（登记表能显著提高截图的识别率）").color(AMBER));
        }
        ui.add_space(4.0);

        let avail = ui.available_size();
        ui.horizontal_top(|ui| {
            let left_w = (avail.x * 0.48).max(260.0);
            ui.allocate_ui(egui::vec2(left_w, avail.y), |ui| {
                card(ui, |ui| match &self.preview {
                    Some(pv) => {
                        ui.label(RichText::new(format!("{}  ·  {}×{}", pv.name, pv.img.w, pv.img.h)).weak());
                        let (mw, mh) = (ui.available_width(), (avail.y - 60.0).clamp(240.0, 640.0));
                        let ts = pv.tex.size_vec2();
                        let k = (mw / ts.x).min(mh / ts.y);
                        ui.add(egui::Image::new(&pv.tex).fit_to_exact_size(ts * k));
                    }
                    None => {
                        ui.add_space(80.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("🔍").size(40.0));
                            ui.label(RichText::new("拖入图片、打开文件，或粘贴截图").weak());
                        });
                        ui.add_space(80.0);
                    }
                });
            });
            ui.vertical(|ui| {
                if self.detect_job.is_some() {
                    card(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("正在检测…");
                        });
                    });
                } else if let Some(res) = self.result.clone() {
                    self.result_card(ui, &res);
                }
            });
        });
    }

    fn result_card(&mut self, ui: &mut egui::Ui, res: &Result<(Report, f64), String>) {
        let (rep, secs) = match res {
            Ok(r) => r,
            Err(e) => {
                status_frame(ui, RED, |ui| {
                    ui.label(RichText::new(format!("检测出错：{e}")).color(RED));
                });
                return;
            }
        };
        let d = &rep.det;
        let entry = d.image_id.and_then(|id| self.reg.as_ref().and_then(|r| r.get(id).cloned()));
        let sim = rep.content_similarity;
        let (color, title, sub) = if d.found && sim.is_some_and(|s| s < SIM_OK) {
            (AMBER, "⚠ 编号对上了，但画面和登记的原图不一致", "可能有人把你的水印搬到了另一张图上。请人工核对。")
        } else if d.found {
            (GREEN, "✔ 找到你的水印", "这张图来自你登记过的图片。")
        } else if d.present {
            (AMBER, "⚠ 检测到你的水印，但无法确定是哪一张", "通常是截得太小或压缩太狠。用登记表或更完整的版本再试。")
        } else {
            (GREY, "✕ 未检测到你的水印", "这不能证明图不是你的：水印可能被裁掉、重绘，或强度不足。")
        };
        status_frame(ui, color, |ui| {
            ui.label(RichText::new(title).size(20.0).strong().color(color));
            ui.label(sub);
        });
        ui.add_space(6.0);
        card(ui, |ui| {
            ui.horizontal_top(|ui| {
                if let Some(id) = d.image_id {
                    let ctx = ui.ctx().clone();
                    if let Some(t) = self.thumb_tex(&ctx, id) {
                        ui.add(egui::Image::new(&t).max_size(egui::vec2(150.0, 150.0)));
                    }
                }
                egui::Grid::new("det-grid").num_columns(2).spacing([14.0, 6.0]).show(ui, |ui| {
                    if let Some(h) = &rep.id_hex {
                        kv(ui, "图片编号", h.clone());
                    }
                    if let Some(e) = &entry {
                        kv(ui, "原始文件", e.source.clone());
                        kv(ui, "渠道备注", e.channel.clone().unwrap_or_else(|| "—".into()));
                        kv(ui, "登记时间 (UTC)", e.registered_utc.clone());
                    } else if d.image_id.is_some() {
                        kv(ui, "登记信息", "登记表里没有这个编号");
                    }
                    if let Some(s) = sim {
                        let verdict = if s >= SIM_OK { "一致" } else { "不一致" };
                        kv(ui, "画面一致性", format!("{s:.2}（{verdict}）"));
                    }
                    if let Some(m) = &d.method {
                        kv(ui, "识别方式", if m == "crc" { "直接解码" } else { "登记表比对" });
                    }
                });
            });
            egui::CollapsingHeader::new("技术细节").default_open(false).show(ui, |ui| {
                egui::Grid::new("det-tech").num_columns(2).show(ui, |ui| {
                    kv(ui, "同步分数 z", format!("{:.2}（阈值 {:.1}）", d.z_sync, wm::Z_THRESH));
                    if let Some(z) = d.z_id {
                        kv(ui, "编号分数", format!("{z:.2}"));
                    }
                    if let Some(p) = d.period_px {
                        kv(ui, "平铺周期", format!("{p:.1} px"));
                    }
                    if let Some(o) = rep.content_overlap {
                        kv(ui, "与原图重叠", format!("{:.0}%", o * 100.0));
                    }
                    kv(ui, "用时", format!("{secs:.2} 秒"));
                });
            });
        });
    }

    fn page_registry(&mut self, ui: &mut egui::Ui) {
        ui.heading("登记表");
        let Some(reg) = &self.reg else {
            ui.label(RichText::new("还没有选择登记表文件夹（在「密钥与设置」里设置）").color(AMBER));
            return;
        };
        let n = reg.entries.len();
        let dir = reg.dir.clone();
        ui.horizontal(|ui| {
            ui.label(format!("共 {n} 条 · {}", short_path(&dir)));
            if ui.button("打开文件夹").clicked() {
                open_in_file_manager(&dir);
            }
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("搜索文件名 / 渠道 / 编号").desired_width(240.0));
        });
        let f = self.filter.to_lowercase();
        let mut rows: Vec<(u32, blindmark::registry::Entry)> = reg
            .entries
            .iter()
            .filter_map(|(k, e)| k.parse::<u32>().ok().map(|id| (id, e.clone())))
            .filter(|(id, e)| {
                f.is_empty()
                    || e.source.to_lowercase().contains(&f)
                    || e.channel.as_deref().unwrap_or("").to_lowercase().contains(&f)
                    || format!("{id:08x}").contains(f.trim_start_matches("0x"))
            })
            .collect();
        rows.sort_by(|a, b| b.1.registered_utc.cmp(&a.1.registered_utc));

        let avail = ui.available_size();
        ui.horizontal_top(|ui| {
            ui.allocate_ui(egui::vec2(avail.x * 0.62, avail.y), |ui| {
                card(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("reg-scroll").show(ui, |ui| {
                        egui::Grid::new("reg-grid").striped(true).num_columns(4).spacing([16.0, 6.0]).show(ui, |ui| {
                            for h in ["编号", "原始文件", "渠道", "登记时间 (UTC)"] {
                                ui.label(RichText::new(h).strong());
                            }
                            ui.end_row();
                            for (id, e) in &rows {
                                let sel = self.selected == Some(*id);
                                if ui.selectable_label(sel, RichText::new(format!("0x{id:08x}")).monospace()).clicked() {
                                    self.selected = Some(*id);
                                }
                                ui.label(&e.source);
                                ui.label(e.channel.as_deref().unwrap_or("—"));
                                ui.label(e.registered_utc.replace('T', " ").trim_end_matches('Z'));
                                ui.end_row();
                            }
                        });
                    });
                });
            });
            ui.vertical(|ui| {
                if let Some(id) = self.selected {
                    let e = self.reg.as_ref().and_then(|r| r.get(id).cloned());
                    if let Some(e) = e {
                        card(ui, |ui| {
                            let ctx = ui.ctx().clone();
                            if let Some(t) = self.thumb_tex(&ctx, id) {
                                ui.add(egui::Image::new(&t).max_size(egui::vec2(ui.available_width(), 260.0)));
                            }
                            egui::Grid::new("reg-detail").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                                kv(ui, "编号", format!("0x{id:08x}"));
                                kv(ui, "原始文件", e.source.clone());
                                kv(ui, "尺寸", format!("{}×{}", e.size[1], e.size[0]));
                                kv(ui, "渠道", e.channel.clone().unwrap_or_else(|| "—".into()));
                                kv(ui, "强度", e.strength.map(|s| format!("{s:.1}")).unwrap_or_else(|| "—".into()));
                                kv(ui, "登记时间", e.registered_utc.clone());
                                kv(ui, "原图 SHA-256", format!("{}…", &e.sha256_original[..16]));
                            });
                            if let Some(o) = &e.output {
                                let p = PathBuf::from(o);
                                if p.exists() && ui.button("在文件夹中显示输出图").clicked() {
                                    open_in_file_manager(p.parent().unwrap_or(&p));
                                }
                            }
                        });
                    }
                } else {
                    ui.label(RichText::new("点击左侧编号查看详情").weak());
                }
            });
        });
    }

    fn page_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("密钥与设置");
        card(ui, |ui| {
            ui.label(RichText::new("🔑 私钥").strong());
            ui.label(RichText::new("私钥决定了水印图样。丢失后将无法检测自己的水印；泄露后别人可以伪造或针对性去除。请离线备份两份，不要上传到网盘或代码仓库。").weak());
            ui.horizontal(|ui| {
                match (&self.cfg.key_path, &self.key) {
                    (Some(p), Some(k)) => {
                        ui.label(RichText::new("✔ 已加载").color(GREEN));
                        ui.label(short_path(p));
                        ui.label(RichText::new(format!("指纹 {}", keys::fingerprint(k))).monospace().weak());
                    }
                    _ => {
                        ui.label(RichText::new("未加载密钥").color(AMBER));
                    }
                }
            });
            if let Some(m) = &self.key_msg {
                ui.label(RichText::new(m).color(RED));
            }
            ui.horizontal(|ui| {
                if ui.button("新建密钥…").clicked() {
                    if let Some(p) = rfd::FileDialog::new().set_file_name("blindmark.key").save_file() {
                        if p.exists() {
                            self.flash("该文件已存在，为安全起见不会覆盖");
                        } else {
                            match keys::generate_key().and_then(|k| std::fs::write(&p, hex::encode(k) + "\n").map_err(Into::into)) {
                                Ok(()) => {
                                    if self.cfg.registry_dir.is_none() {
                                        self.cfg.registry_dir = p.parent().map(|d| d.join("blindmark-registry"));
                                    }
                                    self.cfg.key_path = Some(p);
                                    save_config(&self.cfg);
                                    self.reload_key();
                                    self.reload_registry();
                                    self.flash("已生成新密钥，请立即备份");
                                }
                                Err(e) => self.flash(format!("生成失败：{e}")),
                            }
                        }
                    }
                }
                if ui.button("选择已有密钥…").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("blindmark key", &["key", "txt"]).pick_file() {
                        self.cfg.key_path = Some(p);
                        save_config(&self.cfg);
                        self.reload_key();
                    }
                }
            });
        });
        card(ui, |ui| {
            ui.label(RichText::new("📒 登记表文件夹").strong());
            ui.label(RichText::new("保存每个编号对应的原图哈希、缩略图和时间。检测截图时靠它提高识别率，也是你主张所有权的证据之一，请和密钥一起备份。").weak());
            ui.horizontal(|ui| {
                match &self.reg {
                    Some(r) => {
                        ui.label(RichText::new(format!("✔ {} 条记录", r.entries.len())).color(GREEN));
                        ui.label(short_path(&r.dir));
                    }
                    None => {
                        ui.label(RichText::new("未选择").color(AMBER));
                    }
                }
                if ui.button("选择文件夹…").clicked() {
                    if let Some(d) = rfd::FileDialog::new().pick_folder() {
                        self.cfg.registry_dir = Some(d);
                        save_config(&self.cfg);
                        self.reload_registry();
                    }
                }
            });
            if let Some(m) = &self.reg_msg {
                ui.label(RichText::new(m).color(RED));
            }
        });
        card(ui, |ui| {
            ui.label(RichText::new("ℹ 说明").strong());
            ui.label(format!("格式 {} · 与 Python 参考实现完全互通（同一密钥、同一登记表）。", keys::FORMAT));
            ui.label("能抗：压缩、缩放、裁剪、加边框、常见截图。扛不住：旋转/翻转、对着屏幕拍照、AI 重绘、专门针对它的去除。");
            ui.label("证明所有权时，请同时保留原图、登记表（最好做可信时间戳）和检测结果。");
            ui.label(RichText::new(format!("配置文件：{}", config_dir().join("config.json").display())).small().weak());
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frames += 1;
        self.poll_jobs();
        self.handle_drops(&ctx);
        if self.tab == Tab::Detect
            && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)) || matches!(e, egui::Event::Key { key: egui::Key::V, pressed: true, modifiers, .. } if modifiers.command)))
        {
            self.paste_for_detect(&ctx);
        }

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("blindmark").size(20.0).strong());
                ui.label(RichText::new("盲水印").size(16.0).weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match &self.reg {
                        Some(r) if self.reg_msg.is_none() => ui.label(RichText::new(format!("📒 {} 条登记", r.entries.len())).color(GREEN)),
                        _ => ui.label(RichText::new("📒 登记表未就绪").color(AMBER)),
                    };
                    match &self.key {
                        Some(k) => ui.label(RichText::new(format!("🔑 {}", keys::fingerprint(k))).monospace().color(GREEN)),
                        None => ui.label(RichText::new("🔑 未加载密钥").color(AMBER)),
                    };
                });
            });
            ui.add_space(4.0);
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            let msg = self.status.as_ref().filter(|(_, t)| t.elapsed().as_secs() < 8).map(|(m, _)| m.clone());
            ui.horizontal(|ui| {
                ui.label(RichText::new(msg.unwrap_or_else(|| "就绪".into())).small());
            });
        });

        egui::Panel::left("nav").resizable(false).exact_size(150.0).show(ui, |ui| {
            ui.add_space(8.0);
            for (t, label) in [(Tab::Embed, "🖼  加水印"), (Tab::Detect, "🔍  检测"), (Tab::Registry, "📒  登记表"), (Tab::Settings, "🔑  密钥与设置")] {
                let sel = self.tab == t;
                let b = egui::Button::selectable(sel, RichText::new(label).size(16.0)).min_size(egui::vec2(ui.available_width(), 36.0));
                if ui.add(b).clicked() {
                    self.tab = t;
                }
            }
        });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("page").auto_shrink([false, false]).show(ui, |ui| match self.tab {
                Tab::Embed => self.page_embed(ui),
                Tab::Detect => self.page_detect(ui),
                Tab::Registry => self.page_registry(ui),
                Tab::Settings => self.page_settings(ui),
            });
        });

        if let Some(shot) = &self.test_shot {
            let idle = self.embed_rx.is_none() && self.detect_job.is_none();
            if idle && self.frames > 30 {
                let path = shot.clone();
                self.test_shot = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                    let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                    let _ = image::save_buffer(&path, &raw, img.size[0] as u32, img.size[1] as u32, image::ColorType::Rgba8);
                    std::process::exit(0);
                })));
            }
            ctx.request_repaint();
        }
    }
}

fn main() -> eframe::Result {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("blindmark 盲水印")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([900.0, 600.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("blindmark", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
