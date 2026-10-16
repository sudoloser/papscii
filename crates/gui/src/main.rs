//! Papscii GUI (eframe + egui_component).
//!
//! Screens: gallery (empty-state skeletons + scrim, populated cards with a
//! "Use" action) and a full-screen tabbed settings view (Appearance, About).
//! All user text comes from `papscii-i18n`.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use eframe::egui;
use egui::{
    Align, Align2, Color32, CornerRadius, FontFamily, FontId, Layout, Rect, RichText, Sense,
    Stroke, StrokeKind, Vec2,
};
use egui_component::components::button;
use egui_component::theme::{ButtonSize, Color, Colors, CornerRadius as Radii, SetColor, Theme};
use egui_phosphor::regular as icon;

use papscii_core::{
    ipc::{request_at, Request, Response},
    paths, Grid, Reader,
};
use papscii_encode::Settings as EncodeSettings;
use papscii_i18n::Localizer;

const FONT_SANS: &[u8] = include_bytes!("../../../assets/fonts/NotoSans-Regular.ttf");
const FONT_SANS_BOLD: &[u8] = include_bytes!("../../../assets/fonts/NotoSans-Bold.ttf");
const FONT_MONO: &[u8] = include_bytes!("../../../assets/fonts/NotoSansMono-Regular.ttf");

/// Card content size (inside the frame margins): identical for every card so
/// the wrapped grid stays aligned.
const CARD_W: f32 = 300.0;
const CARD_PREVIEW_H: f32 = 150.0;
const CARD_BODY_H: f32 = 90.0;
/// Outer footprint of one card cell (frame margins included). Every card is
/// allocated exactly this box, so rows stay uniform no matter what.
const CARD_CELL_W: f32 = CARD_W + 20.0;
const CARD_CELL_H: f32 = 280.0;
const CARD_GAP: f32 = 12.0;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 720.0])
            .with_min_inner_size([860.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Papscii",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

// --- theme ------------------------------------------------------------------

fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

fn ramp(levels: [[u8; 3]; 10]) -> Color {
    let c = |i: usize| rgb(levels[i][0], levels[i][1], levels[i][2]);
    Color {
        l0: c(0),
        l1: c(1),
        l2: c(2),
        l3: c(3),
        l4: c(4),
        l5: c(5),
        l6: c(6),
        l7: c(7),
        l8: c(8),
        l9: c(9),
    }
}

fn neutral_ramp() -> Color {
    ramp([
        [26, 27, 30],
        [32, 34, 37],
        [43, 45, 49],
        [49, 51, 56],
        [56, 58, 64],
        [64, 66, 73],
        [78, 80, 88],
        [92, 94, 102],
        [109, 111, 120],
        [128, 131, 141],
    ])
}

fn accent_ramp() -> Color {
    ramp([
        [238, 242, 255],
        [224, 231, 255],
        [199, 210, 254],
        [165, 180, 252],
        [129, 140, 248],
        [99, 102, 241],
        [79, 70, 229],
        [67, 56, 202],
        [55, 48, 163],
        [49, 46, 129],
    ])
}

fn make_theme(dark: bool) -> Theme {
    let neutral = neutral_ramp();
    let accent = accent_ramp();
    let (bg, fg) = if dark {
        (
            SetColor {
                base: rgb(30, 31, 34),
                subtle: rgb(43, 45, 49),
                accent: rgb(49, 51, 56),
            },
            SetColor {
                base: rgb(242, 243, 245),
                subtle: rgb(181, 186, 193),
                accent: rgb(219, 222, 225),
            },
        )
    } else {
        (
            SetColor {
                base: rgb(250, 250, 252),
                subtle: rgb(238, 239, 243),
                accent: rgb(226, 228, 234),
            },
            SetColor {
                base: rgb(24, 25, 28),
                subtle: rgb(82, 84, 92),
                accent: rgb(48, 50, 56),
            },
        )
    };
    let _ = (ButtonSize::Md, neutral);
    Theme {
        colors: Colors {
            bg,
            fg,
            primary: accent,
            gray: neutral,
            red: neutral,
            green: neutral,
            blue: neutral,
            yellow: neutral,
            orange: neutral,
        },
        corner_radius: Radii {
            xxs: 3.0,
            xs: 5.0,
            sm: 6.0,
            md: 8.0,
            lg: 10.0,
            xl: 14.0,
        },
    }
}

// --- model ------------------------------------------------------------------

#[derive(Clone)]
struct Wallpaper {
    name: String,
    path: PathBuf,
    cover: Grid,
    charset: Vec<u8>,
    frames: u32,
    fps: u16,
    text_size: u16,
    transparent: bool,
    _color: bool,
}

impl Wallpaper {
    fn load(path: PathBuf) -> Option<Self> {
        let data = std::fs::read(&path).ok()?;
        let reader = Reader::parse(data).ok()?;
        let h = reader.header();
        Some(Wallpaper {
            name: path.file_stem()?.to_string_lossy().into_owned(),
            path,
            cover: h.cover.clone(),
            charset: h.charset.clone(),
            frames: h.frame_count,
            fps: h.fps,
            text_size: h.text_size,
            transparent: h.transparent,
            _color: h.color_mode.is_color(),
        })
    }

    /// ASCII preview of the cover grid.
    fn preview(&self) -> String {
        let cols = self.cover.cols as usize;
        let mut s = String::with_capacity(self.cover.rows as usize * (cols + 1));
        for r in 0..self.cover.rows as usize {
            for c in 0..cols {
                let idx = self.cover.cells[r * cols + c] as usize;
                s.push(*self.charset.get(idx).unwrap_or(&b' ') as char);
            }
            s.push('\n');
        }
        s
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Appearance,
    System,
    About,
}

#[derive(PartialEq, Clone, Copy)]
enum View {
    Gallery,
    Settings(Tab),
}

#[derive(Clone, Copy)]
enum PickTarget {
    Video,
    Import,
}

struct CreateForm {
    video: String,
    name: String,
    fps: u16,
    density: u16,
    color: bool,
    text_size: u16,
    transparent: bool,
    brightness: f32,
}

impl CreateForm {
    fn fresh() -> Self {
        CreateForm {
            video: String::new(),
            name: String::new(),
            fps: 24,
            density: 80,
            color: false,
            text_size: 0,
            transparent: false,
            brightness: 1.0,
        }
    }
}

#[derive(Default, Clone)]
struct EditState {
    path: PathBuf,
    name: String,
    fps: u16,
    brightness: f32,
    text_size: u16,
    transparent: bool,
}

struct App {
    loc: Localizer,
    theme: Theme,
    dark: bool,
    view: View,
    wallpapers: Vec<Wallpaper>,
    current: Option<String>,
    daemon_running: bool,
    last_poll: f64,

    service_available: bool,
    autostart: Option<bool>,
    last_service_poll: f64,

    show_create: bool,
    create: CreateForm,
    show_import: bool,
    import_path: String,
    import_name: String,
    show_edit: bool,
    edit: EditState,

    focus_exempt: Vec<String>,
    known_apps: Vec<String>,
    new_app: String,
    last_apps_poll: f64,

    busy: Option<Receiver<Result<PathBuf, String>>>,
    pending_pick: Option<Receiver<(PickTarget, Option<PathBuf>)>>,
    status: Option<String>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        let app = App {
            loc: Localizer::english(),
            theme: make_theme(true),
            dark: true,
            view: View::Gallery,
            wallpapers: Vec::new(),
            current: None,
            daemon_running: false,
            last_poll: -10.0,
            service_available: service::available(),
            autostart: None,
            last_service_poll: -10.0,
            show_create: false,
            create: CreateForm::fresh(),
            show_import: false,
            import_path: String::new(),
            import_name: String::new(),
            show_edit: false,
            edit: EditState::default(),
            focus_exempt: papscii_core::config::focus_exempt_apps(),
            known_apps: Vec::new(),
            new_app: String::new(),
            last_apps_poll: -10.0,
            busy: None,
            pending_pick: None,
            status: None,
        };
        let mut app = app;
        app.refresh_library();
        app
    }

    fn refresh_library(&mut self) {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(paths::library_dir()) {
            let mut paths: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().map(|x| x == "pscii").unwrap_or(false))
                .collect();
            paths.sort();
            for p in paths {
                if let Some(w) = Wallpaper::load(p) {
                    out.push(w);
                }
            }
        }
        self.wallpapers = out;
    }

    fn poll_daemon(&mut self) {
        let socket = paths::socket_path();
        match request_at(&socket, &Request::Query) {
            Ok(Response::State { wallpaper, .. }) => {
                self.daemon_running = true;
                self.current = wallpaper;
            }
            _ => {
                self.daemon_running = false;
                self.current = None;
            }
        }
    }

    fn start_daemon(&mut self) {
        if self.service_available {
            service::start();
        } else {
            spawn_daemon();
        }
        self.last_poll = -10.0;
    }

    fn stop_daemon(&mut self) {
        if self.service_available {
            service::stop();
            self.autostart = Some(service::is_enabled());
        }
        let socket = paths::socket_path();
        let _ = request_at(&socket, &Request::Stop);
        self.daemon_running = false;
        self.current = None;
    }

    fn use_wallpaper(&mut self, path: &str) {
        let socket = paths::socket_path();
        match request_at(
            &socket,
            &Request::SetWallpaper {
                path: path.to_string(),
            },
        ) {
            Ok(Response::Ok) => {
                self.current = Some(path.to_string());
                self.status = None;
            }
            Ok(Response::Error(e)) => self.status = Some(e),
            _ => {
                self.status = Some(format!(
                    "daemon not running (try `papscii daemon`); {}",
                    socket.display()
                ))
            }
        }
    }

    fn delete_wallpaper(&mut self, path: &str) {
        match std::fs::remove_file(path) {
            Ok(_) => {
                if self.current.as_deref() == Some(path) {
                    self.current = None;
                }
                self.refresh_library();
            }
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    fn start_encode(&mut self) {
        let video = PathBuf::from(self.create.video.trim());
        if !video.exists() {
            self.status = Some(format!("no such video: {}", video.display()));
            return;
        }
        let name = if self.create.name.trim().is_empty() {
            video
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            self.create.name.trim().to_string()
        };
        let settings = EncodeSettings {
            fps: self.create.fps,
            density: self.create.density,
            color: self.create.color,
            text_size: self.create.text_size,
            transparent: self.create.transparent,
            brightness: self.create.brightness,
            ..EncodeSettings::default()
        };
        let out = paths::library_dir().join(format!("{name}.pscii"));
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = paths::ensure_dirs();
            let res = papscii_encode::encode_to_file(&video, &out, &settings)
                .map(|_| out)
                .map_err(|e| e.to_string());
            let _ = tx.send(res);
        });
        self.busy = Some(rx);
        self.show_create = false;
        self.create = CreateForm::fresh();
        self.status = Some(self.loc.text("encode.status.working"));
    }

    fn start_import(&mut self) {
        let src = PathBuf::from(self.import_path.trim());
        if !src.exists() {
            self.status = Some(format!("no such file: {}", src.display()));
            return;
        }
        let name = if self.import_name.trim().is_empty() {
            src.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            self.import_name.trim().to_string()
        };
        let _ = paths::ensure_dirs();
        let dest = paths::library_dir().join(format!("{name}.pscii"));
        match std::fs::copy(&src, &dest) {
            Ok(_) => {
                self.refresh_library();
                self.show_import = false;
                self.status = Some(format!("imported {}", dest.display()));
            }
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    fn start_edit(&mut self) {
        let path = self.edit.path.clone();
        let options = papscii_encode::AdjustOptions {
            brightness: Some(self.edit.brightness),
            fps: Some(self.edit.fps),
            text_size: Some(self.edit.text_size),
            transparent: Some(self.edit.transparent),
        };
        let is_current = self.current.as_deref() == Some(path.to_string_lossy().as_ref());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let res = papscii_encode::adjust_file(&path, &path, &options)
                .map(|_| {
                    if is_current {
                        let _ = request_at(&paths::socket_path(), &Request::Reload);
                    }
                    path
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(res);
        });
        self.busy = Some(rx);
        self.show_edit = false;
        self.status = Some(self.loc.text("edit.status.working"));
    }

    fn poll_busy(&mut self) {
        if let Some(rx) = &self.busy {
            match rx.try_recv() {
                Ok(Ok(path)) => {
                    self.refresh_library();
                    self.status = Some(format!("created {}", path.display()));
                    self.busy = None;
                }
                Ok(Err(e)) => {
                    self.status = Some(e);
                    self.busy = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => self.busy = None,
            }
        }
    }

    fn pick(&mut self, target: PickTarget) {
        let (tx, rx) = mpsc::channel();
        let (title, filter, exts): (String, String, &'static [&'static str]) = match target {
            PickTarget::Video => (
                self.loc.text("dialog.pick.video"),
                self.loc.text("dialog.filter.video"),
                &["mp4", "mkv", "webm", "mov", "avi", "m4v", "gif"],
            ),
            PickTarget::Import => (
                self.loc.text("dialog.pick.pscii"),
                self.loc.text("dialog.filter.pscii"),
                &["pscii"],
            ),
        };
        std::thread::spawn(move || {
            let picked = rfd::FileDialog::new()
                .set_title(title)
                .add_filter(filter, exts)
                .pick_file();
            let _ = tx.send((target, picked));
        });
        self.pending_pick = Some(rx);
    }

    fn poll_pick(&mut self) {
        let result = self.pending_pick.as_ref().map(|rx| rx.try_recv());
        match result {
            Some(Ok((target, picked))) => {
                if let Some(path) = picked {
                    let stem = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let text = path.to_string_lossy().into_owned();
                    match target {
                        PickTarget::Video => {
                            if self.create.name.trim().is_empty() {
                                self.create.name = stem;
                            }
                            self.create.video = text;
                        }
                        PickTarget::Import => {
                            if self.import_name.trim().is_empty() {
                                self.import_name = stem;
                            }
                            self.import_path = text;
                        }
                    }
                }
                self.pending_pick = None;
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => self.pending_pick = None,
            _ => {}
        }
    }

    fn solid(&self) -> button::Style {
        button::Style::new_solid_md(&self.theme)
    }

    fn surface(&self) -> button::Style {
        button::Style::new_surface_md(&self.theme)
    }

    /// Union of running app classes and the exempt list, for the picker.
    fn refresh_known_apps(&mut self) {
        let mut apps = self.focus_exempt.clone();
        if let Some((s1, _)) = papscii_core::config::hyprland_sockets() {
            apps.extend(papscii_core::config::client_classes(&s1));
        }
        apps.sort();
        apps.dedup();
        self.known_apps = apps;
    }

    /// Persist the exempt list and nudge the running daemon to reload it.
    fn save_focus_exempt(&mut self) {
        self.focus_exempt.sort();
        self.focus_exempt.dedup();
        if let Err(e) = papscii_core::config::set_focus_exempt_apps(&self.focus_exempt) {
            self.status = Some(e.to_string());
            return;
        }
        let socket = paths::socket_path();
        let _ = request_at(&socket, &Request::ReloadSettings);
        self.refresh_known_apps();
    }
}

fn browse_button(ui: &mut egui::Ui, loc: &Localizer, key: &str) -> bool {
    ui.add(egui::Button::new(RichText::new(icon::FOLDER_OPEN).size(18.0)).frame(false))
        .on_hover_text(loc.text(key))
        .clicked()
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "noto".into(),
        std::sync::Arc::new(egui::FontData::from_static(FONT_SANS)),
    );
    fonts.font_data.insert(
        "noto-bold".into(),
        std::sync::Arc::new(egui::FontData::from_static(FONT_SANS_BOLD)),
    );
    fonts.font_data.insert(
        "noto-mono".into(),
        std::sync::Arc::new(egui::FontData::from_static(FONT_MONO)),
    );

    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "noto".into());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "noto-mono".into());
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Poll the daemon roughly once a second.
        let now = ctx.input(|i| i.time);
        if now - self.last_poll > 1.0 {
            self.last_poll = now;
            self.poll_daemon();
        }
        if self.service_available
            && matches!(self.view, View::Settings(Tab::System))
            && now - self.last_service_poll > 3.0
        {
            self.last_service_poll = now;
            self.autostart = Some(service::is_enabled());
        }
        if matches!(self.view, View::Settings(Tab::System)) && now - self.last_apps_poll > 3.0 {
            self.last_apps_poll = now;
            self.refresh_known_apps();
        }
        self.poll_busy();
        self.poll_pick();

        apply_visuals(&ctx, &self.theme, self.dark);

        match self.view {
            View::Gallery => self.gallery(ui),
            View::Settings(_) => self.settings(ui),
        }

        if self.show_create {
            self.window_create(&ctx);
        }
        if self.show_import {
            self.window_import(&ctx);
        }
        if self.show_edit {
            self.window_edit(&ctx);
        }
    }
}

fn apply_visuals(ctx: &egui::Context, theme: &Theme, dark: bool) {
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = theme.colors.bg.base;
    v.window_fill = theme.colors.bg.subtle;
    v.extreme_bg_color = theme.colors.bg.subtle;
    v.override_text_color = Some(theme.colors.fg.base);
    v.widgets.noninteractive.corner_radius = CornerRadius::same(6);
    ctx.set_visuals(v);
}

impl App {
    fn gallery(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("topbar")
            .frame(
                egui::Frame::default()
                    .fill(self.theme.colors.bg.base)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.loc.text("app.name")).size(20.0).strong());
                    ui.label(
                        RichText::new(self.loc.text("app.tagline"))
                            .color(self.theme.colors.fg.subtle),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let running = self.daemon_running;
                        let (glyph, key) = if running {
                            (icon::STOP, "topbar.stop")
                        } else {
                            (icon::PLAY, "topbar.start")
                        };
                        let toggle =
                            ui.add(egui::Button::new(RichText::new(glyph).size(20.0)).frame(false));
                        if toggle.on_hover_text(self.loc.text(key)).clicked() {
                            if running {
                                self.stop_daemon();
                            } else {
                                self.start_daemon();
                            }
                        }
                        ui.add_space(6.0);
                        let gear = ui.add(
                            egui::Button::new(RichText::new(icon::GEAR).size(20.0)).frame(false),
                        );
                        if gear
                            .on_hover_text(self.loc.text("topbar.settings"))
                            .clicked()
                        {
                            self.view = View::Settings(Tab::Appearance);
                        }
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(self.theme.colors.bg.base)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show_inside(ui, |ui| {
                if let Some(status) = &self.status {
                    ui.horizontal(|ui| {
                        if self.busy.is_some() {
                            ui.spinner();
                        }
                        ui.label(RichText::new(status).color(self.theme.colors.fg.subtle));
                    });
                    ui.add_space(8.0);
                }
                if self.wallpapers.is_empty() {
                    self.empty_state(ui);
                } else {
                    self.card_grid(ui);
                }
            });
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(48.0);
            let w = ui.available_width().min(940.0);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 220.0), Sense::hover());
            let painter = ui.painter();
            let card = Vec2::new(280.0, 170.0);
            let gap = 22.0;
            let total = card.x * 3.0 + gap * 2.0;
            let mut x = rect.center().x - total / 2.0;

            for _ in 0..3 {
                let r = Rect::from_min_size(egui::pos2(x, rect.top()), card);
                painter.rect_filled(r, CornerRadius::same(10), self.theme.colors.bg.subtle);
                painter.rect_stroke(
                    r,
                    CornerRadius::same(10),
                    Stroke::new(1.5f32, self.theme.colors.bg.accent),
                    StrokeKind::Inside,
                );
                x += card.x + gap;
            }
            // Scrim above the cards, below the text.
            painter.rect_filled(
                Rect::from_min_max(
                    egui::pos2(rect.left(), rect.top() - 12.0),
                    egui::pos2(rect.right(), rect.bottom() + 12.0),
                ),
                CornerRadius::same(12),
                Color32::from_black_alpha(150),
            );
            painter.text(
                egui::pos2(rect.center().x, rect.center().y),
                Align2::CENTER_CENTER,
                self.loc.text("gallery.empty.title"),
                FontId::proportional(30.0),
                self.theme.colors.fg.base,
            );

            ui.add_space(28.0);
            ui.horizontal(|ui| {
                let gap = 0.0f32.min(0.0);
                let _ = gap;
                let avail = ui.available_width();
                let row = 360.0;
                ui.add_space((avail - row).max(0.0) / 2.0);
                if button::render(
                    ui,
                    &format!("{}  {}", icon::PLUS, self.loc.text("gallery.empty.create")),
                    self.solid(),
                )
                .clicked()
                {
                    self.show_create = true;
                }
                ui.add_space(12.0);
                if button::render(
                    ui,
                    &format!(
                        "{}  {}",
                        icon::DOWNLOAD_SIMPLE,
                        self.loc.text("gallery.empty.import")
                    ),
                    self.surface(),
                )
                .clicked()
                {
                    self.show_import = true;
                }
            });
        });
    }

    fn card_grid(&mut self, ui: &mut egui::Ui) {
        let mut use_path: Option<String> = None;
        let mut delete_path: Option<String> = None;
        let mut edit_target: Option<Wallpaper> = None;
        let mut new_clicked = false;
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Manual rows (top-aligned) instead of horizontal_wrapped: every
            // cell gets an identical fixed box, so cards can never stagger.
            let cols = (((ui.available_width() + CARD_GAP) / (CARD_CELL_W + CARD_GAP)).floor()
                as usize)
                .max(1);
            let total = self.wallpapers.len() + 1; // +1 for the plus card
            let mut start = 0;
            while start < total {
                ui.horizontal_top(|ui| {
                    for j in start..(start + cols).min(total) {
                        ui.allocate_ui_with_layout(
                            Vec2::new(CARD_CELL_W, CARD_CELL_H),
                            Layout::top_down(Align::Min),
                            |ui| {
                                if let Some(w) = self.wallpapers.get(j) {
                                    self.card(
                                        ui,
                                        w,
                                        &mut use_path,
                                        &mut delete_path,
                                        &mut edit_target,
                                    );
                                } else {
                                    self.plus_card(ui, &mut new_clicked);
                                }
                            },
                        );
                        ui.add_space(CARD_GAP);
                    }
                });
                ui.add_space(CARD_GAP);
                start += cols;
            }
        });
        if new_clicked {
            self.show_create = true;
        }
        if let Some(w) = edit_target {
            self.edit = EditState {
                path: w.path.clone(),
                name: w.name.clone(),
                fps: w.fps,
                brightness: 1.0,
                text_size: w.text_size,
                transparent: w.transparent,
            };
            self.show_edit = true;
        }
        if let Some(p) = use_path {
            self.use_wallpaper(&p);
        }
        if let Some(p) = delete_path {
            self.delete_wallpaper(&p);
        }
    }

    fn plus_card(&self, ui: &mut egui::Ui, clicked: &mut bool) {
        egui::Frame::default()
            .fill(self.theme.colors.bg.subtle)
            .corner_radius(CornerRadius::same(10))
            .stroke(Stroke::new(1.0f32, self.theme.colors.bg.accent))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(CARD_W);
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(CARD_W, CARD_PREVIEW_H), Sense::click());
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(6), Color32::BLACK);
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    icon::PLUS,
                    FontId::proportional(40.0),
                    Color32::from_gray(150),
                );
                if resp.clicked() {
                    *clicked = true;
                }

                ui.add_space(6.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(CARD_W, CARD_BODY_H),
                    Layout::top_down(Align::LEFT),
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(self.loc.text("gallery.card.new"))
                                    .strong()
                                    .size(16.0),
                            )
                            .truncate(),
                        );
                        ui.label(
                            RichText::new(self.loc.text("app.tagline"))
                                .small()
                                .color(self.theme.colors.fg.subtle),
                        );
                        ui.add_space(2.0);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if button::render(
                                ui,
                                &self.loc.text("gallery.card.create"),
                                self.solid(),
                            )
                            .clicked()
                            {
                                *clicked = true;
                            }
                        });
                    },
                );
            });
        ui.add_space(4.0);
    }

    fn card(
        &self,
        ui: &mut egui::Ui,
        w: &Wallpaper,
        use_path: &mut Option<String>,
        delete_path: &mut Option<String>,
        edit_target: &mut Option<Wallpaper>,
    ) {
        let in_use = self.current.as_deref() == Some(w.path.to_string_lossy().as_ref());
        egui::Frame::default()
            .fill(self.theme.colors.bg.subtle)
            .corner_radius(CornerRadius::same(10))
            .stroke(Stroke::new(1.0f32, self.theme.colors.bg.accent))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(CARD_W);
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(CARD_W, CARD_PREVIEW_H), Sense::hover());
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(6), Color32::BLACK);
                let cols = w.cover.cols.max(1) as f32;
                let rows = w.cover.rows.max(1) as f32;
                let size = (rect.width() / cols)
                    .min(rect.height() / (rows * 1.2))
                    .clamp(2.5, 12.0);
                let galley = ui.painter().layout_no_wrap(
                    w.preview(),
                    FontId::monospace(size),
                    Color32::from_gray(210),
                );
                ui.painter()
                    .galley(rect.min, galley, Color32::from_gray(210));

                ui.add_space(6.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(CARD_W, CARD_BODY_H),
                    Layout::top_down(Align::LEFT),
                    |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(&w.name).strong().size(16.0)).truncate(),
                        );
                        let count = w.frames.to_string();
                        let frames = self.loc.format(
                            "gallery.card.frames",
                            Some(&papscii_i18n::args([("count", count.as_str())])),
                        );
                        ui.label(
                            RichText::new(frames)
                                .small()
                                .color(self.theme.colors.fg.subtle),
                        );
                        ui.add_space(2.0);

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let label = if in_use {
                                self.loc.text("gallery.card.in-use")
                            } else {
                                self.loc.text("gallery.card.use")
                            };
                            let style = if in_use { self.surface() } else { self.solid() };
                            if button::render(ui, &label, style).clicked() && !in_use {
                                *use_path = Some(w.path.to_string_lossy().into_owned());
                            }
                            ui.add_space(6.0);
                            if button::render(
                                ui,
                                &self.loc.text("gallery.card.edit"),
                                self.surface(),
                            )
                            .clicked()
                            {
                                *edit_target = Some(w.clone());
                            }
                            ui.add_space(6.0);
                            if button::render(
                                ui,
                                &self.loc.text("gallery.card.delete"),
                                self.surface(),
                            )
                            .clicked()
                            {
                                *delete_path = Some(w.path.to_string_lossy().into_owned());
                            }
                        });
                    },
                );
            });
        ui.add_space(4.0);
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        let active = match self.view {
            View::Settings(t) => t,
            _ => Tab::Appearance,
        };

        egui::Panel::top("settings_top")
            .frame(
                egui::Frame::default()
                    .fill(self.theme.colors.bg.base)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new(icon::ARROW_LEFT).size(20.0))
                                .frame(false),
                        )
                        .clicked()
                    {
                        self.view = View::Gallery;
                    }
                    ui.label(
                        RichText::new(self.loc.text("settings.title"))
                            .size(20.0)
                            .strong(),
                    );
                });
            });

        egui::Panel::left("settings_rail")
            .resizable(false)
            .frame(
                egui::Frame::default()
                    .fill(self.theme.colors.bg.base)
                    .inner_margin(egui::Margin::same(12)),
            )
            .show_inside(ui, |ui| {
                if self.tab_button(
                    ui,
                    Tab::Appearance,
                    icon::PALETTE,
                    "settings.tab.appearance",
                ) {
                    self.view = View::Settings(Tab::Appearance);
                }
                if self.tab_button(ui, Tab::System, icon::POWER, "settings.tab.system") {
                    self.view = View::Settings(Tab::System);
                }
                if self.tab_button(ui, Tab::About, icon::INFO, "settings.tab.about") {
                    self.view = View::Settings(Tab::About);
                }
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(self.theme.colors.bg.base)
                    .inner_margin(egui::Margin::same(20)),
            )
            .show_inside(ui, |ui| match active {
                Tab::Appearance => self.tab_appearance(ui),
                Tab::System => self.tab_system(ui),
                Tab::About => self.tab_about(ui),
            });
    }

    fn tab_button(&self, ui: &mut egui::Ui, tab: Tab, glyph: &str, key: &str) -> bool {
        let active = matches!(self.view, View::Settings(t) if t == tab);
        let style = if active {
            self.solid()
        } else {
            button::Style::new_ghost_md(&self.theme)
        };
        let resp = button::render(ui, &format!("{glyph}   {}", self.loc.text(key)), style);
        ui.add_space(4.0);
        resp.clicked()
    }

    fn tab_appearance(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.loc.text("settings.appearance.theme"));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let dark_label = self.loc.text("settings.appearance.color-mode.dark");
            let light_label = self.loc.text("settings.appearance.color-mode.light");
            let dark_style = if self.dark {
                self.solid()
            } else {
                self.surface()
            };
            let light_style = if self.dark {
                self.surface()
            } else {
                self.solid()
            };
            if button::render(ui, &dark_label, dark_style).clicked() {
                self.dark = true;
                self.theme = make_theme(true);
            }
            if button::render(ui, &light_label, light_style).clicked() {
                self.dark = false;
                self.theme = make_theme(false);
            }
        });
    }

    fn tab_system(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.loc.text("settings.tab.system"));
        ui.add_space(12.0);

        ui.label(RichText::new(self.loc.text("settings.system.status")).strong());
        let state = if self.daemon_running {
            self.loc.text("settings.system.running")
        } else {
            self.loc.text("settings.system.stopped")
        };
        ui.label(state);
        ui.add_space(18.0);

        self.focus_section(ui);
        ui.add_space(18.0);

        if !self.service_available {
            ui.label(
                RichText::new(self.loc.text("settings.system.unavailable"))
                    .color(self.theme.colors.fg.subtle),
            );
            ui.add_space(12.0);
            ui.label(
                RichText::new(self.loc.text("settings.system.hint"))
                    .color(self.theme.colors.fg.subtle),
            );
            return;
        }

        ui.label(RichText::new(self.loc.text("settings.system.autostart")).strong());
        let enabled = self.autostart.unwrap_or(false);
        let state_key = if enabled {
            "settings.system.enabled"
        } else {
            "settings.system.disabled"
        };
        ui.label(self.loc.text(state_key));
        ui.add_space(8.0);
        let action_key = if enabled {
            "settings.system.disable"
        } else {
            "settings.system.enable"
        };
        if button::render(ui, &self.loc.text(action_key), self.solid()).clicked() {
            if enabled {
                service::disable();
            } else {
                service::enable();
            }
            self.autostart = Some(!enabled);
        }

        ui.add_space(18.0);
        ui.label(
            RichText::new(self.loc.text("settings.system.hint")).color(self.theme.colors.fg.subtle),
        );
    }

    fn focus_section(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new(self.loc.text("settings.system.focus"))
                .strong()
                .size(16.0),
        );
        ui.label(
            RichText::new(self.loc.text("settings.system.focus-hint"))
                .small()
                .color(self.theme.colors.fg.subtle),
        );
        ui.add_space(8.0);

        let apps: Vec<String> = self.known_apps.clone();
        let mut changed = false;
        if apps.is_empty() {
            ui.label(
                RichText::new(self.loc.text("settings.system.focus-empty"))
                    .color(self.theme.colors.fg.subtle),
            );
        }
        for app in &apps {
            let mut on = self.focus_exempt.iter().any(|e| e == app);
            if ui.checkbox(&mut on, app).changed() {
                changed = true;
                if on {
                    if !self.focus_exempt.iter().any(|e| e == app) {
                        self.focus_exempt.push(app.clone());
                    }
                } else {
                    self.focus_exempt.retain(|e| e != app);
                }
            }
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_app)
                    .desired_width(200.0)
                    .hint_text(self.loc.text("settings.system.focus-add-hint")),
            );
            if button::render(
                ui,
                &self.loc.text("settings.system.focus-add"),
                self.surface(),
            )
            .clicked()
                && !self.new_app.trim().is_empty()
            {
                let app = self.new_app.trim().to_string();
                if !self.focus_exempt.iter().any(|e| e == &app) {
                    self.focus_exempt.push(app);
                }
                self.new_app.clear();
                changed = true;
            }
        });

        if changed {
            self.save_focus_exempt();
        }
    }

    fn tab_about(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.loc.text("app.name"));
        ui.label(RichText::new(self.loc.text("app.tagline")).color(self.theme.colors.fg.subtle));
        ui.add_space(12.0);
        let version = self.loc.format(
            "about.version",
            Some(&papscii_i18n::args([(
                "version",
                env!("CARGO_PKG_VERSION"),
            )])),
        );
        ui.label(version);
        ui.add_space(12.0);
        ui.label(RichText::new(self.loc.text("about.developer")).strong());
        ui.label("sudoloser");
        ui.add_space(8.0);
        ui.label(RichText::new(self.loc.text("about.github")).strong());
        ui.hyperlink_to(
            "github.com/sudoloser/papscii",
            "https://github.com/sudoloser/papscii",
        );
        ui.add_space(8.0);
        ui.label(RichText::new(self.loc.text("about.license")).strong());
        ui.label("MIT");
    }

    fn window_create(&mut self, ctx: &egui::Context) {
        let mut open = true;
        egui::Window::new(self.loc.text("encode.title"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                egui::Grid::new("create_form")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(self.loc.text("encode.field.name"));
                        ui.text_edit_singleline(&mut self.create.name);
                        ui.end_row();

                        ui.label(self.loc.text("encode.field.video"));
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.create.video)
                                    .desired_width(240.0),
                            );
                            if browse_button(ui, &self.loc, "encode.field.browse")
                                && self.pending_pick.is_none()
                            {
                                self.pick(PickTarget::Video);
                            }
                        });
                        ui.end_row();

                        ui.label(self.loc.text("encode.field.fps"));
                        ui.add(egui::DragValue::new(&mut self.create.fps).range(1..=120));
                        ui.end_row();

                        ui.label(self.loc.text("encode.field.density"));
                        ui.add(egui::DragValue::new(&mut self.create.density).range(16..=400));
                        ui.end_row();

                        ui.label(self.loc.text("encode.field.text-size"));
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.create.text_size).range(0..=200));
                            ui.label(
                                RichText::new(self.loc.text("encode.field.text-size-hint"))
                                    .small()
                                    .color(self.theme.colors.fg.subtle),
                            );
                        });
                        ui.end_row();

                        ui.label(self.loc.text("encode.field.brightness"));
                        ui.add(
                            egui::Slider::new(&mut self.create.brightness, 0.2..=3.0)
                                .text("x")
                                .step_by(0.05),
                        );
                        ui.end_row();

                        ui.label(self.loc.text("encode.field.options"));
                        ui.horizontal(|ui| {
                            ui.checkbox(
                                &mut self.create.color,
                                self.loc.text("encode.field.color"),
                            );
                            ui.checkbox(
                                &mut self.create.transparent,
                                self.loc.text("encode.field.transparent"),
                            );
                        });
                        ui.end_row();
                    });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let mut start = false;
                    if button::render(ui, &self.loc.text("encode.action.start"), self.solid())
                        .clicked()
                    {
                        start = true;
                    }
                    if button::render(ui, &self.loc.text("encode.action.cancel"), self.surface())
                        .clicked()
                    {
                        self.show_create = false;
                    }
                    if start {
                        self.start_encode();
                    }
                });
            });
        if !open {
            self.show_create = false;
        }
    }

    fn window_import(&mut self, ctx: &egui::Context) {
        let mut open = true;
        egui::Window::new(self.loc.text("gallery.empty.import"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                egui::Grid::new("import_form")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(self.loc.text("encode.field.file"));
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.import_path)
                                    .desired_width(240.0),
                            );
                            if browse_button(ui, &self.loc, "encode.field.browse")
                                && self.pending_pick.is_none()
                            {
                                self.pick(PickTarget::Import);
                            }
                        });
                        ui.end_row();
                        ui.label(self.loc.text("encode.field.name"));
                        ui.text_edit_singleline(&mut self.import_name);
                        ui.end_row();
                    });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let mut go = false;
                    if button::render(ui, &self.loc.text("encode.action.start"), self.solid())
                        .clicked()
                    {
                        go = true;
                    }
                    if button::render(ui, &self.loc.text("encode.action.cancel"), self.surface())
                        .clicked()
                    {
                        self.show_import = false;
                    }
                    if go {
                        self.start_import();
                    }
                });
            });
        if !open {
            self.show_import = false;
        }
    }

    fn window_edit(&mut self, ctx: &egui::Context) {
        let mut open = true;
        egui::Window::new(self.loc.text("edit.title"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                egui::Grid::new("edit_form")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(self.loc.text("encode.field.name"));
                        ui.label(RichText::new(&self.edit.name).strong());
                        ui.end_row();

                        ui.label(self.loc.text("edit.field.brightness"));
                        ui.add(
                            egui::Slider::new(&mut self.edit.brightness, 0.2..=3.0)
                                .text("x")
                                .step_by(0.05),
                        );
                        ui.end_row();

                        ui.label(self.loc.text("edit.field.fps"));
                        ui.add(egui::DragValue::new(&mut self.edit.fps).range(1..=120));
                        ui.end_row();

                        ui.label(self.loc.text("edit.field.text-size"));
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.edit.text_size).range(0..=200));
                            ui.label(
                                RichText::new(self.loc.text("encode.field.text-size-hint"))
                                    .small()
                                    .color(self.theme.colors.fg.subtle),
                            );
                        });
                        ui.end_row();

                        ui.label(self.loc.text("encode.field.options"));
                        ui.checkbox(
                            &mut self.edit.transparent,
                            self.loc.text("edit.field.transparent"),
                        );
                        ui.end_row();
                    });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let mut start = false;
                    if button::render(ui, &self.loc.text("edit.action.save"), self.solid())
                        .clicked()
                    {
                        start = true;
                    }
                    if button::render(ui, &self.loc.text("edit.action.cancel"), self.surface())
                        .clicked()
                    {
                        self.show_edit = false;
                    }
                    if start {
                        self.start_edit();
                    }
                });
            });
        if !open {
            self.show_edit = false;
        }
    }
}

// --- daemon / systemd user service ------------------------------------------

/// Launch the daemon as a detached child, preferring the sibling binary.
fn spawn_daemon() {
    let path = std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(|e| e.parent())
        .map(|d| d.join("papscii-daemon"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("papscii-daemon"));
    let _ = std::process::Command::new(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

mod service {
    use std::process::Command;

    const UNIT: &str = "papscii-daemon.service";

    fn systemctl(args: &[&str]) -> Option<String> {
        let out = Command::new("systemctl")
            .arg("--user")
            .args(args)
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub fn available() -> bool {
        Command::new("systemctl")
            .args(["--user", "cat", UNIT])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn is_enabled() -> bool {
        systemctl(&["is-enabled", UNIT]).as_deref() == Some("enabled")
    }

    fn run(action: &str) {
        let _ = Command::new("systemctl")
            .args(["--user", action, UNIT])
            .status();
    }

    pub fn start() {
        run("start");
    }

    pub fn stop() {
        run("stop");
    }

    pub fn enable() {
        run("enable");
    }

    pub fn disable() {
        run("disable");
    }
}
