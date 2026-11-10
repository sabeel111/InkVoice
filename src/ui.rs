//! All drawing. State and behaviour live in main.rs.
use crate::config::{self, Config, Kind};
use crate::player::Unit;
use crate::theme::*;
use crate::{App, View};
use eframe::egui::{
    self, text::CCursor, text::LayoutJob, Align, Align2, Button, FontFamily, FontId, Frame, Key, Layout, Margin,
    Modifiers, ProgressBar, Rect, RichText, Sense, Slider, Stroke, StrokeKind, TextFormat, TextWrapMode, Vec2,
};

const CARD_W: f32 = 164.0;
const COVER_H: f32 = 246.0;

fn primary(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(Button::new(RichText::new(text).color(ON_ACCENT).strong()).fill(ACCENT))
}

fn icon(ui: &mut egui::Ui, glyph: &str, tip: &str) -> egui::Response {
    ui.add(Button::new(RichText::new(glyph).size(16.0)).frame(false)).on_hover_text(tip)
}

#[derive(Clone, Copy)]
enum Ic {
    Play,
    Pause,
    Prev,
    Next,
    Back,
    Fwd,
}

/// Round transport button with a vector icon (font glyphs sit at uneven heights).
fn transport(ui: &mut egui::Ui, ic: Ic, d: f32, primary: bool, enabled: bool, tip: &str) -> egui::Response {
    use egui::{pos2, vec2, Shape};
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(d), if enabled { Sense::click() } else { Sense::hover() });
    let c = rect.center();
    let hovered = enabled && resp.hovered();
    let p = ui.painter();
    if primary {
        p.circle_filled(c, d / 2.0, if !enabled { RAISED } else if hovered { ACCENT_HI } else { ACCENT });
    } else if hovered {
        p.circle_filled(c, d / 2.0, HOVER);
    }
    let col = if primary { if enabled { ON_ACCENT } else { FAINT } } else if enabled { TEXT } else { FAINT };
    let s = d * if primary { 0.26 } else { 0.24 };
    let tri = |a: (f32, f32), b: (f32, f32), e: (f32, f32)| {
        p.add(Shape::convex_polygon(vec![c + vec2(a.0 * s, a.1 * s), c + vec2(b.0 * s, b.1 * s), c + vec2(e.0 * s, e.1 * s)], col, Stroke::NONE));
    };
    let bar = |x: f32, w: f32, h: f32| {
        p.rect_filled(Rect::from_center_size(pos2(c.x + x * s, c.y), vec2(w * s, h * s)), 1.5, col);
    };
    match ic {
        // triangles listed clockwise (screen coordinates) for clean anti-aliasing
        Ic::Play => tri((-0.7, -1.05), (1.1, 0.0), (-0.7, 1.05)),
        Ic::Pause => {
            bar(-0.48, 0.5, 1.9);
            bar(0.48, 0.5, 1.9);
        }
        Ic::Prev => {
            bar(-0.95, 0.32, 1.6);
            tri((-0.7, 0.0), (0.95, -0.8), (0.95, 0.8));
        }
        Ic::Next => {
            bar(0.95, 0.32, 1.6);
            tri((-0.95, -0.8), (0.7, 0.0), (-0.95, 0.8));
        }
        Ic::Back => {
            tri((-1.1, 0.0), (0.0, -0.75), (0.0, 0.75));
            tri((0.0, 0.0), (1.1, -0.75), (1.1, 0.75));
        }
        Ic::Fwd => {
            tri((-1.1, -0.75), (0.0, 0.0), (-1.1, 0.75));
            tri((0.0, -0.75), (1.1, 0.0), (0.0, 0.75));
        }
    }
    let resp = if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
    resp.on_hover_text(tip)
}

fn fmt_speed(s: f32) -> String {
    let t = format!("{s:.2}");
    format!("{}×", t.trim_end_matches('0').trim_end_matches('.'))
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).small().strong().color(DIM));
    ui.add_space(2.0);
}

fn ago(secs: u64) -> String {
    if secs == 0 {
        return "Not opened yet".into();
    }
    let d = crate::book::now_secs().saturating_sub(secs);
    match d {
        0..=59 => "Opened just now".into(),
        60..=3599 => format!("Opened {} min ago", d / 60),
        3600..=86399 => format!("Opened {} h ago", d / 3600),
        86400..=172799 => "Opened yesterday".into(),
        _ => format!("Opened {} days ago", d / 86400),
    }
}

/// Things clicked inside a panel, applied after drawing (keeps borrows simple).
enum Act {
    Open(String),
    Select(String),
    PlayPath(std::path::PathBuf, Unit),
    PlayUnit(Unit),
    Toggle,
    Enqueue(Unit),
    AskRemove(String),
    CancelRemove,
    Remove(String),
    GoChapter(usize),
    GenerateAll,
    Regenerate(usize),
}

impl App {
    fn apply(&mut self, act: Option<Act>) {
        match act {
            Some(Act::Open(id)) => self.open_reader(&id),
            Some(Act::Select(id)) => self.sel = Some(id),
            Some(Act::PlayPath(p, u)) => self.play_path(&p, u, None),
            Some(Act::PlayUnit(u)) => self.play_unit(u, None),
            Some(Act::Toggle) => self.toggle_play(),
            Some(Act::Enqueue(u)) => {
                if self.engine_ready() {
                    self.enqueue(&u);
                }
            }
            Some(Act::AskRemove(id)) => self.confirm_remove = Some(id),
            Some(Act::CancelRemove) => self.confirm_remove = None,
            Some(Act::Remove(id)) => self.remove_book(&id),
            Some(Act::GoChapter(i)) => self.go(i, 0),
            Some(Act::GenerateAll) => self.generate_all(),
            Some(Act::Regenerate(ch)) => self.regenerate_chapter(ch),
            None => {}
        }
    }

    // =============================== input ===============================

    pub(crate) fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.wants_keyboard_input();
        let mut k = [false; 12];
        ctx.input_mut(|i| {
            k[0] = i.consume_key(Modifiers::COMMAND, Key::O);
            k[1] = i.consume_key(Modifiers::COMMAND, Key::Comma);
            k[2] = i.consume_key(Modifiers::COMMAND, Key::Equals) || i.consume_key(Modifiers::COMMAND, Key::Plus);
            k[3] = i.consume_key(Modifiers::COMMAND, Key::Minus);
            if !typing {
                k[4] = i.consume_key(Modifiers::NONE, Key::Space);
                k[5] = i.consume_key(Modifiers::SHIFT, Key::ArrowLeft);
                k[6] = i.consume_key(Modifiers::SHIFT, Key::ArrowRight);
                k[7] = i.consume_key(Modifiers::NONE, Key::ArrowLeft);
                k[8] = i.consume_key(Modifiers::NONE, Key::ArrowRight);
                k[9] = i.consume_key(Modifiers::NONE, Key::Escape);
                k[10] = i.consume_key(Modifiers::NONE, Key::OpenBracket);
                k[11] = i.consume_key(Modifiers::NONE, Key::CloseBracket);
            }
        });
        if k[0] { self.import_dialog(); }
        if k[1] { self.show_settings = !self.show_settings; }
        if k[2] || k[3] {
            self.cfg.font_size = (self.cfg.font_size + if k[2] { 1.0 } else { -1.0 }).clamp(13.0, 36.0);
            self.cfg.save();
        }
        if k[4] { self.toggle_play(); }
        if k[10] || k[11] {
            let speeds = crate::SPEEDS;
            let i = speeds.iter().position(|&s| (s - self.cfg.speed).abs() < 0.001).unwrap_or(2);
            let j = if k[11] { (i + 1).min(speeds.len() - 1) } else { i.saturating_sub(1) };
            self.set_speed(speeds[j]);
            self.notify(format!("Speed {}", fmt_speed(speeds[j])));
        }
        if k[5] { self.seek_by(-15.0); }
        if k[6] { self.seek_by(15.0); }
        if self.view == View::Reader {
            if k[7] { self.prev_page(); }
            if k[8] { self.next_page(); }
        }
        if k[9] {
            if self.show_settings {
                self.show_settings = false;
                self.apply_worker_settings();
                self.cfg.save();
            } else if self.view == View::Reader {
                self.view = View::Library;
            }
        }
    }

    // =============================== frame ===============================

    pub(crate) fn draw(&mut self, ctx: &egui::Context) {
        if self.view == View::Reader && self.open_book().is_none() {
            self.view = View::Library;
        }
        self.top_bar(ctx);
        self.player_bar(ctx);
        self.generation_strip(ctx);
        match self.view {
            View::Library => {
                if self.sel.as_deref().and_then(|id| self.book(id)).is_some() {
                    self.details_panel(ctx);
                }
                egui::CentralPanel::default()
                    .frame(Frame::new().fill(BG).inner_margin(Margin::symmetric(28, 20)))
                    .show(ctx, |ui| self.library_view(ui));
            }
            View::Reader => {
                if self.sidebar {
                    self.contents_panel(ctx);
                }
                egui::CentralPanel::default()
                    .frame(Frame::new().fill(BG).inner_margin(Margin::symmetric(0, 0)))
                    .show(ctx, |ui| self.reader_view(ui));
            }
        }
        self.settings_window(ctx);
        self.toast(ctx);
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top")
            .frame(Frame::side_top_panel(&ctx.style()).inner_margin(Margin::symmetric(16, 8)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.set_height(32.0);
                    match self.view {
                        View::Library => {
                            ui.label(RichText::new("📚  InkVoice").size(17.0).strong().color(ACCENT));
                            ui.add_space(18.0);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.search)
                                    .hint_text("🔍  Search title or author")
                                    .desired_width(280.0)
                                    .margin(Margin::symmetric(8, 5)),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let label = if self.importing { "Importing…" } else { "+  Import book" };
                                if ui.add_enabled_ui(!self.importing, |ui| primary(ui, label)).inner.clicked() {
                                    self.import_dialog();
                                }
                                if icon(ui, "⚙", "Settings (Ctrl+,)").clicked() {
                                    self.show_settings = !self.show_settings;
                                }
                            });
                        }
                        View::Reader => {
                            if icon(ui, "☰", "Show/hide contents").clicked() {
                                self.sidebar = !self.sidebar;
                            }
                            if ui.button("⬅  Library").on_hover_text("Back to library (Esc)").clicked() {
                                self.view = View::Library;
                            }
                            ui.add_space(10.0);
                            if let Some(b) = self.open_book() {
                                let (title, author) = (b.title.clone(), b.author.clone());
                                ui.label(RichText::new(title).strong());
                                if !author.is_empty() {
                                    ui.label(RichText::new(format!("· {author}")).color(DIM));
                                }
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icon(ui, "⚙", "Settings (Ctrl+,)").clicked() {
                                    self.show_settings = !self.show_settings;
                                }
                                ui.add_space(6.0);
                                if ui.button("A+").on_hover_text("Larger text (Ctrl+=)").clicked() {
                                    self.cfg.font_size = (self.cfg.font_size + 1.0).min(36.0);
                                    self.cfg.save();
                                }
                                if ui.button("A−").on_hover_text("Smaller text (Ctrl+-)").clicked() {
                                    self.cfg.font_size = (self.cfg.font_size - 1.0).max(13.0);
                                    self.cfg.save();
                                }
                            });
                        }
                    }
                });
            });
    }

    // =============================== library ===============================

    fn library_view(&mut self, ui: &mut egui::Ui) {
        if self.lib.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.22);
                ui.label(RichText::new("📚").size(64.0).color(FAINT));
                ui.add_space(8.0);
                ui.label(RichText::new("Your library is empty").size(22.0));
                ui.label(RichText::new("Import an EPUB or PDF to read it and turn it into an audiobook.").color(DIM));
                ui.add_space(14.0);
                if primary(ui, "+  Import a book").clicked() {
                    self.import_dialog();
                }
            });
            return;
        }
        let q = self.search.trim().to_lowercase();
        let ids: Vec<String> = self
            .lib
            .iter()
            .filter(|b| q.is_empty() || b.title.to_lowercase().contains(&q) || b.author.to_lowercase().contains(&q))
            .map(|b| b.id.clone())
            .collect();

        if self.installed.is_empty() {
            Frame::new().fill(RAISED).corner_radius(8.0).inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("🎧").size(22.0).color(ACCENT));
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Install a voice to listen to your books").strong());
                        ui.label(RichText::new("Voices run on your own computer. Pick one — you can add others later.").small().color(DIM));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = if self.install.is_some() { "Installing…" } else { "Choose a voice" };
                        if primary(ui, label).clicked() {
                            self.show_settings = true;
                        }
                    });
                });
            });
            ui.add_space(14.0);
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Library").size(24.0).strong());
            ui.label(RichText::new(format!("{} book{}", self.lib.len(), if self.lib.len() == 1 { "" } else { "s" })).color(DIM));
        });
        ui.add_space(14.0);
        if ids.is_empty() {
            ui.label(RichText::new(format!("No books match “{}”", self.search.trim())).color(DIM));
            return;
        }
        let mut act = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(26.0, 26.0);
                for id in &ids {
                    if let Some(a) = self.book_card(ui, id) {
                        act = Some(a);
                    }
                }
            });
            ui.add_space(20.0);
        });
        self.apply(act);
    }

    fn book_card(&mut self, ui: &mut egui::Ui, id: &str) -> Option<Act> {
        let b = self.book(id)?;
        let (title, author, frac) = (b.title.clone(), b.author.clone(), b.read_fraction());
        let audio_n = self.lib_audio.get(id).map_or(0, |v| v.len());
        let selected = self.sel.as_deref() == Some(id);

        let (rect, resp) = ui.allocate_exact_size(Vec2::new(CARD_W, COVER_H + 74.0), Sense::click());
        let cover = Rect::from_min_size(rect.min, Vec2::new(CARD_W, COVER_H));
        // soft shadow
        ui.painter().rect_filled(cover.translate(Vec2::new(0.0, 4.0)), 8.0, egui::Color32::from_black_alpha(90));
        self.paint_cover(ui, cover, id, &title, &author);
        if selected || resp.hovered() {
            let s = Stroke::new(2.0_f32, if selected { ACCENT } else { DIM });
            ui.painter().rect_stroke(cover.expand(3.0), 9.0, s, StrokeKind::Outside);
        }

        // reading progress line
        let bar = Rect::from_min_size(cover.left_bottom() + Vec2::new(0.0, 8.0), Vec2::new(CARD_W, 3.0));
        ui.painter().rect_filled(bar, 2.0, RAISED);
        if frac > 0.0 {
            ui.painter().rect_filled(Rect::from_min_size(bar.min, Vec2::new(CARD_W * frac.max(0.03), 3.0)), 2.0, ACCENT);
        }
        // title (2 lines) + meta
        let mut job = LayoutJob::simple(title.clone(), FontId::proportional(14.0), TEXT, CARD_W);
        job.wrap.max_rows = 2;
        job.wrap.break_anywhere = false;
        let g = ui.fonts(|f| f.layout_job(job));
        let ty = bar.max.y + 7.0;
        let th = g.rect.height();
        ui.painter().galley(egui::pos2(rect.min.x, ty), g, TEXT);
        let meta = match (frac > 0.0, audio_n) {
            (false, 0) => "New".to_string(),
            (true, 0) => format!("{:.0}% read", frac * 100.0),
            (false, n) => format!("🎧 {n}"),
            (true, n) => format!("{:.0}% read  ·  🎧 {n}", frac * 100.0),
        };
        ui.painter().text(
            egui::pos2(rect.min.x, ty + th + 3.0),
            Align2::LEFT_TOP,
            meta,
            FontId::proportional(12.0),
            DIM,
        );

        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Click for details, double-click to open");
        let mut act = None;
        resp.context_menu(|ui| {
            if ui.button("📖  Open").clicked() {
                act = Some(Act::Open(id.to_string()));
                ui.close_menu();
            }
            if ui.button(RichText::new("🗑  Remove…").color(DANGER)).clicked() {
                act = Some(Act::Select(id.to_string()));
                self.confirm_remove = Some(id.to_string());
                ui.close_menu();
            }
        });
        if resp.double_clicked() {
            return Some(Act::Open(id.to_string()));
        }
        if resp.clicked() {
            return Some(Act::Select(id.to_string()));
        }
        act
    }

    /// Cover image cropped to fill `rect`, or a generated cover with the title.
    fn paint_cover(&mut self, ui: &mut egui::Ui, rect: Rect, id: &str, title: &str, author: &str) {
        if let Some(tex) = self.cover(id) {
            let ts = tex.size_vec2();
            let (ta, ra) = (ts.x / ts.y, rect.width() / rect.height());
            let uv = if ta > ra {
                let w = ra / ta;
                Rect::from_min_max(egui::pos2((1.0 - w) / 2.0, 0.0), egui::pos2((1.0 + w) / 2.0, 1.0))
            } else {
                let h = ta / ra;
                Rect::from_min_max(egui::pos2(0.0, (1.0 - h) / 2.0), egui::pos2(1.0, (1.0 + h) / 2.0))
            };
            egui::Image::new(&tex).uv(uv).corner_radius(6.0).paint_at(ui, rect);
            return;
        }
        let p = ui.painter();
        let base = cover_color(id);
        p.rect_filled(rect, 6.0, base);
        p.rect_filled(Rect::from_min_size(rect.min, Vec2::new(10.0, rect.height())), 0.0, base.gamma_multiply(0.7));
        let inner = rect.shrink2(Vec2::new(18.0, 0.0));
        p.line_segment(
            [egui::pos2(inner.min.x, rect.min.y + rect.height() * 0.22), egui::pos2(inner.max.x, rect.min.y + rect.height() * 0.22)],
            Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.6)),
        );
        let size = (rect.width() / 9.0).clamp(14.0, 24.0);
        let mut job = LayoutJob::simple(title.to_string(), FontId::new(size, serif()), TEXT, inner.width());
        job.wrap.max_rows = 5;
        job.halign = Align::Center;
        let g = ui.fonts(|f| f.layout_job(job));
        let top = rect.min.y + rect.height() * 0.30;
        p.galley(egui::pos2(rect.center().x, top), g.clone(), TEXT);
        if !author.is_empty() {
            let mut aj = LayoutJob::simple(author.to_string(), FontId::proportional(size * 0.6), DIM, inner.width());
            aj.wrap.max_rows = 2;
            aj.halign = Align::Center;
            let ag = ui.fonts(|f| f.layout_job(aj));
            p.galley(egui::pos2(rect.center().x, top + g.rect.height() + 12.0), ag, DIM);
        }
    }

    fn details_panel(&mut self, ctx: &egui::Context) {
        let Some(id) = self.sel.clone() else { return };
        let Some(b) = self.book(&id) else { return };
        let (title, author, chapters, pages, frac, opened) =
            (b.title.clone(), b.author.clone(), b.chapters.len(), b.page_count(), b.read_fraction(), b.opened);
        let at = b.chapters.get(b.pos.0).map(|c| format!("{} · page {}", c.title, b.pos.1 + 1)).unwrap_or_default();
        let started = b.pos != (0, 0);
        let mut act = None;
        egui::SidePanel::right("details")
            .exact_width(340.0)
            .frame(Frame::side_top_panel(&ctx.style()).inner_margin(Margin::symmetric(20, 18)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::new(180.0, 270.0), Sense::hover());
                        ui.painter().rect_filled(r.translate(Vec2::new(0.0, 5.0)), 8.0, egui::Color32::from_black_alpha(100));
                        self.paint_cover(ui, r, &id, &title, &author);
                    });
                    ui.add_space(14.0);
                    ui.label(RichText::new(&title).family(serif()).size(21.0).strong());
                    if !author.is_empty() {
                        ui.label(RichText::new(&author).color(DIM));
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("{chapters} chapters · {pages} pages · {}", ago(opened))).small().color(DIM));
                    ui.add_space(8.0);
                    ui.add(ProgressBar::new(frac).desired_height(6.0).fill(ACCENT).corner_radius(3.0));
                    ui.label(RichText::new(if started { format!("{:.0}% · {at}", frac * 100.0) } else { "Not started".into() }).small().color(DIM));
                    ui.add_space(12.0);
                    let label = if started { "📖  Continue reading" } else { "📖  Start reading" };
                    let w = ui.available_width();
                    if ui.add_sized([w, 38.0], Button::new(RichText::new(label).color(ON_ACCENT).strong().size(15.0)).fill(ACCENT)).clicked() {
                        act = Some(Act::Open(id.clone()));
                    }

                    // ---- audio ----
                    let items: Vec<(String, std::path::PathBuf, Unit, String, f32)> = self
                        .lib_audio
                        .get(&id)
                        .map(|v| v.iter().map(|a| (a.profile.clone(), a.path.clone(), a.unit.clone(), a.label.clone(), a.duration)).collect())
                        .unwrap_or_default();
                    let total: f32 = items.iter().map(|i| i.4).sum();
                    section(ui, &format!("AUDIO  ·  {} files · {}", items.len(), fmt_time(total)));
                    if items.is_empty() {
                        ui.label(RichText::new("No audio yet. Open the book and press ⏵ to listen.").color(DIM));
                    }
                    let (now_path, playing) = self
                        .player
                        .as_ref()
                        .map(|p| (p.now.as_ref().map(|n| n.path.clone()), p.is_playing()))
                        .unwrap_or((None, false));
                    let mut last_profile = String::new();
                    for (profile, path, unit, label, dur) in items {
                        if profile != last_profile {
                            ui.add_space(4.0);
                            ui.label(RichText::new(config::profile_label(&profile)).small().color(ACCENT));
                            last_profile = profile;
                        }
                        ui.horizontal(|ui| {
                            let is_now = now_path.as_ref() == Some(&path);
                            let glyph = if is_now && playing { "⏸" } else { "⏵" };
                            let color = if is_now { ACCENT } else { TEXT };
                            if ui.add(Button::new(RichText::new(glyph).color(color)).min_size(Vec2::new(28.0, 24.0))).clicked() {
                                act = Some(if is_now { Act::Toggle } else { Act::PlayPath(path.clone(), unit.clone()) });
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(RichText::new(fmt_time(dur)).small().color(DIM));
                                ui.style_mut().wrap_mode = Some(TextWrapMode::Truncate);
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    ui.label(RichText::new(&label).color(color));
                                });
                            });
                        });
                    }

                    ui.add_space(24.0);
                    ui.separator();
                    if self.confirm_remove.as_deref() == Some(id.as_str()) {
                        ui.label(RichText::new("Remove this book and all of its audio?").color(DANGER));
                        ui.horizontal(|ui| {
                            if ui.add(Button::new(RichText::new("Remove").color(ON_ACCENT)).fill(DANGER)).clicked() {
                                act = Some(Act::Remove(id.clone()));
                            }
                            if ui.button("Cancel").clicked() {
                                act = Some(Act::CancelRemove);
                            }
                        });
                    } else if ui.add(Button::new(RichText::new("🗑  Remove from library").color(DIM)).frame(false)).clicked() {
                        act = Some(Act::AskRemove(id.clone()));
                    }
                });
            });
        self.apply(act);
    }

    // =============================== reader ===============================

    fn contents_panel(&mut self, ctx: &egui::Context) {
        let Some(b) = self.open_book() else { return };
        let chapters: Vec<(String, usize)> = b.chapters.iter().map(|c| (c.title.clone(), c.pages.len())).collect();
        let book_id = b.id.clone();
        let page_mode = self.cfg.page_mode;
        let (now_unit, playing) = self
            .player
            .as_ref()
            .map(|p| (p.now.as_ref().map(|n| n.unit.clone()), p.is_playing()))
            .unwrap_or((None, false));
        let job = self.job.as_ref().map(|j| (j.path.clone(), j.progress));
        let mut ready_total = 0;
        let mut act = None;

        egui::SidePanel::left("contents")
            .default_width(290.0)
            .width_range(220.0..=440.0)
            .frame(Frame::side_top_panel(&ctx.style()).inner_margin(Margin::symmetric(12, 14)))
            .show(ctx, |ui| {
                ui.label(RichText::new(format!("CONTENTS  ·  {} chapters", chapters.len())).small().strong().color(DIM));
                ui.add_space(6.0);
                let footer_h = 70.0;
                egui::ScrollArea::vertical()
                    .max_height(ui.available_height() - footer_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (i, (title, pages)) in chapters.iter().enumerate() {
                            let current = self.ch == i;
                            ui.horizontal(|ui| {
                                ui.allocate_ui_with_layout(Vec2::new(40.0, 24.0), Layout::centered_and_justified(egui::Direction::LeftToRight), |ui| {
                                    if page_mode {
                                        let n = (0..*pages)
                                            .filter(|&p| {
                                                let path = self.book(&book_id).map(|b| self.audio_path(b, i, Some(p)));
                                                path.is_some_and(|p| self.have.contains(&p))
                                            })
                                            .count();
                                        if n == *pages {
                                            ready_total += 1;
                                        }
                                        let (t, c) = if n == *pages { ("✔".to_string(), ACCENT) } else if n > 0 { (format!("{n}/{pages}"), DIM) } else { (String::new(), FAINT) };
                                        ui.label(RichText::new(t).small().color(c));
                                        return;
                                    }
                                    let unit = Unit { book: book_id.clone(), ch: i, pg: None };
                                    let path = self.book(&book_id).map(|b| self.audio_path(b, i, None)).unwrap();
                                    if let Some((jp, prog)) = job.as_ref().filter(|(jp, _)| *jp == path) {
                                        let _ = jp;
                                        ui.label(RichText::new(format!("{:.0}%", prog * 100.0)).small().color(ACCENT)).on_hover_text("Generating…");
                                    } else if self.pending.contains_key(&path) {
                                        ui.label(RichText::new("⏳").color(FAINT)).on_hover_text("Queued");
                                    } else if self.have.contains(&path) {
                                        ready_total += 1;
                                        let is_now = now_unit.as_ref() == Some(&unit);
                                        let glyph = if is_now && playing { "⏸" } else { "⏵" };
                                        let r = ui.add(Button::new(RichText::new(glyph).color(ACCENT)).frame(false));
                                        if r.on_hover_text(if is_now && playing { "Pause" } else { "Play chapter" }).clicked() {
                                            act = Some(if is_now { Act::Toggle } else { Act::PlayUnit(unit) });
                                        }
                                    } else {
                                        let r = ui.add(Button::new(RichText::new("🎧").color(FAINT)).frame(false));
                                        if r.on_hover_text("Generate audio for this chapter").clicked() {
                                            act = Some(Act::Enqueue(unit));
                                        }
                                    }
                                });
                                ui.style_mut().wrap_mode = Some(TextWrapMode::Truncate);
                                let text = RichText::new(title).color(if current { ACCENT } else { TEXT });
                                let r = ui.selectable_label(current, text);
                                if r.clicked() {
                                    act = Some(Act::GoChapter(i));
                                }
                                r.context_menu(|ui| {
                                    if ui.button("⟳  Regenerate audio").on_hover_text("Delete this chapter's audio for the current voice and make it again with the current settings").clicked() {
                                        act = Some(Act::Regenerate(i));
                                        ui.close_menu();
                                    }
                                });
                            });
                        }
                    });
                ui.separator();
                let unit_word = if page_mode { "chapters complete" } else { "chapters ready" };
                ui.label(RichText::new(format!("{ready_total} / {} {unit_word} · {}", chapters.len(), config::profile_label(&self.cfg.profile()))).small().color(DIM));
                let w = ui.available_width();
                if ui.add_sized([w, 30.0], Button::new("🎧  Generate whole book")).on_hover_text("Queue audio for every chapter (or page, in page mode)").clicked() {
                    act = Some(Act::GenerateAll);
                }
            });
        self.apply(act);
    }

    fn reader_view(&mut self, ui: &mut egui::Ui) {
        let Some(b) = self.open_book() else { return };
        let ch = self.ch.min(b.chapters.len() - 1);
        let chapter = &b.chapters[ch];
        let pg = self.pg.min(chapter.pages.len() - 1);
        let page = chapter.pages[pg].clone();
        let ch_title = chapter.title.clone();
        let (n_pages, n_ch) = (chapter.pages.len(), b.chapters.len());
        let page_start = b.page_char_starts(ch)[pg];
        let book_id = b.id.clone();

        // ---- word being spoken, if it is on this page ----
        let page_chars = page.chars().count();
        let mut highlight = None;
        let mut loaded_here = false;
        let mut word_idx = None;
        if let Some(p) = &self.player {
            if let Some(now) = &p.now {
                let u = &now.unit;
                if u.book == book_id && u.ch == ch && (u.pg.is_none() || u.pg == Some(pg)) {
                    loaded_here = true;
                    let off = if u.pg.is_none() { page_start } else { 0 };
                    if let Some(i) = p.current_word() {
                        let w = now.words[i];
                        if w.0 >= off && w.1 <= off + page_chars {
                            highlight = Some((w.0 - off, w.1 - off));
                            word_idx = Some(i);
                        }
                    }
                }
            }
        }
        let audio_here = self.reader_unit().and_then(|u| self.unit_path(&u)).is_some_and(|p| self.is_ready(&p));

        // ---- text layout ----
        let size = self.cfg.font_size;
        let family = if self.cfg.serif { serif() } else { FontFamily::Proportional };
        let font = FontId::new(size, family);
        let base = TextFormat { font_id: font.clone(), color: TEXT, line_height: Some(size * self.cfg.line_spacing), ..Default::default() };
        let hi = TextFormat { color: egui::Color32::from_rgb(255, 216, 150), background: ACCENT_BG, ..base.clone() };
        let col_w = self.cfg.page_width.min(ui.available_width() - 64.0).max(280.0);
        let mut job = LayoutJob::default();
        match highlight {
            Some((s, e)) => {
                let bs = byte_at(&page, s);
                let be = byte_at(&page, e);
                job.append(&page[..bs], 0.0, base.clone());
                job.append(&page[bs..be], 0.0, hi);
                job.append(&page[be..], 0.0, base.clone());
            }
            None => job.append(&page, 0.0, base.clone()),
        }
        job.wrap.max_width = col_w;
        let galley = ui.fonts(|f| f.layout_job(job));

        let mut nav = 0i32;
        let mut click_char = None;
        let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt(("page", ch, pg));
        if self.scroll_top {
            scroll = scroll.vertical_scroll_offset(0.0);
            self.scroll_top = false;
        }
        let follow = self.cfg.follow;
        let last_word = self.last_word;
        let mut new_last_word = last_word;
        scroll.show(ui, |ui| {
            let pad = ((ui.available_width() - col_w) / 2.0).max(24.0);
            ui.add_space(26.0);
            ui.horizontal(|ui| {
                ui.add_space(pad);
                ui.vertical(|ui| {
                    ui.set_width(col_w);
                    if !page.trim_start().starts_with(ch_title.trim()) {
                        ui.label(RichText::new(ch_title.to_uppercase()).small().strong().color(DIM));
                        ui.add_space(14.0);
                    }
                    let resp = ui.add(egui::Label::new(galley.clone()).selectable(false).sense(Sense::click()));
                    if audio_here || loaded_here {
                        let resp = resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
                        if resp.clicked() {
                            if let Some(pos) = resp.interact_pointer_pos() {
                                let c = galley.cursor_from_pos(pos - resp.rect.min);
                                click_char = Some(c.ccursor.index);
                            }
                        }
                    }
                    // keep the spoken word in view
                    if follow && word_idx.is_some() && word_idx != last_word {
                        if let Some((s, _)) = highlight {
                            let r = galley.pos_from_ccursor(CCursor::new(s)).translate(resp.rect.min.to_vec2());
                            ui.scroll_to_rect(r.expand(60.0), None);
                        }
                        new_last_word = word_idx;
                    }

                    ui.add_space(30.0);
                    ui.horizontal(|ui| {
                        let first = ch == 0 && pg == 0;
                        let last = ch + 1 == n_ch && pg + 1 == n_pages;
                        if ui.add_enabled(!first, Button::new("⏴  Previous")).clicked() {
                            nav = -1;
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add_enabled(!last, Button::new("Next  ⏵")).clicked() {
                                nav = 1;
                            }
                            ui.with_layout(Layout::centered_and_justified(egui::Direction::LeftToRight), |ui| {
                                ui.label(RichText::new(format!("Page {} of {}  ·  Chapter {} of {}", pg + 1, n_pages, ch + 1, n_ch)).small().color(DIM));
                            });
                        });
                    });
                    ui.add_space(40.0);
                });
            });
        });
        self.last_word = new_last_word;

        if let Some(c) = click_char {
            // click a word: listen from there
            let unit_char = if self.cfg.page_mode { c } else { page_start + c };
            let loaded_unit = self.player.as_ref().and_then(|p| p.now.as_ref()).map(|n| n.unit.clone());
            match loaded_unit {
                Some(u) if loaded_here => {
                    let off = if u.pg.is_none() { page_start } else { 0 };
                    if let Some(p) = &mut self.player {
                        if let Some(t) = p.time_of_char(off + c) {
                            p.seek(t);
                        }
                    }
                }
                _ => {
                    if let Some(u) = self.reader_unit() {
                        self.play_from_char(u, Some(unit_char));
                    }
                }
            }
        }
        match nav {
            1 => self.next_page(),
            -1 => self.prev_page(),
            _ => {}
        }
    }

    // =============================== bottom ===============================

    fn generation_strip(&mut self, ctx: &egui::Context) {
        if self.pending.is_empty() {
            return;
        }
        let (label, prog, eta) = match &self.job {
            Some(j) => {
                let label = self.pending.get(&j.path).cloned().unwrap_or_default();
                let el = j.started.elapsed().as_secs_f32();
                let eta = (j.progress > 0.03).then(|| el * (1.0 - j.progress) / j.progress);
                (label, j.progress, eta)
            }
            None => ("…".into(), 0.0, None),
        };
        let queued = self.pending.len().saturating_sub(1);
        let mut cancel = false;
        egui::TopBottomPanel::bottom("gen")
            .frame(Frame::new().fill(RAISED).inner_margin(Margin::symmetric(16, 7)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("⏳").color(ACCENT));
                    ui.label(RichText::new(format!("Generating voice · {label}")).strong());
                    ui.add(ProgressBar::new(prog).desired_width(240.0).desired_height(8.0).fill(ACCENT).corner_radius(4.0));
                    let mut info = format!("{:.0}%", prog * 100.0);
                    if let Some(s) = eta {
                        info += &format!(" · about {} left", fmt_time(s.max(1.0)));
                    }
                    if queued > 0 {
                        info += &format!(" · {queued} more queued");
                    }
                    ui.label(RichText::new(info).color(DIM));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(Button::new(RichText::new("✖  Cancel").color(DIM)).frame(false)).on_hover_text("Stop generating and clear the queue").clicked() {
                            cancel = true;
                        }
                    });
                });
            });
        if cancel {
            self.cancel_generation();
            self.notify("Generation cancelled");
        }
    }

    fn player_bar(&mut self, ctx: &egui::Context) {
        let info = self.player.as_ref().and_then(|p| {
            p.now.as_ref().map(|n| (n.label.clone(), p.is_playing(), n.streaming, p.is_buffering()))
        });
        let waiting = self.autoplay.as_ref().map(|a| {
            let prog = self.job.as_ref().filter(|j| &j.path == a).map_or(0.0, |j| j.progress);
            (self.pending.get(a).cloned().unwrap_or_default(), prog)
        });
        let stream_prog = self.feed.as_ref().and_then(|f| self.job.as_ref().filter(|j| j.path == f.out)).map(|j| j.progress);
        let reader_hint = (self.view == View::Reader).then(|| self.reader_unit()).flatten().map(|u| self.unit_label(&u));

        egui::TopBottomPanel::bottom("player")
            .frame(Frame::side_top_panel(&ctx.style()).inner_margin(Margin::symmetric(16, 10)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let has = info.is_some();
                    let playing = info.as_ref().is_some_and(|i| i.1);
                    let can_play = has || reader_hint.is_some();
                    if transport(ui, Ic::Prev, 34.0, false, has, "Previous chapter/page (or restart)").clicked() {
                        self.skip_unit(false);
                    }
                    if transport(ui, Ic::Back, 34.0, false, has, "Back 15 s (Shift+⬅)").clicked() {
                        self.seek_by(-15.0);
                    }
                    let (ic, tip) = if playing { (Ic::Pause, "Pause (Space)") } else { (Ic::Play, "Play (Space)") };
                    if transport(ui, ic, 46.0, true, can_play, tip).clicked() {
                        self.toggle_play();
                    }
                    if transport(ui, Ic::Fwd, 34.0, false, has, "Forward 15 s (Shift+➡)").clicked() {
                        self.seek_by(15.0);
                    }
                    if transport(ui, Ic::Next, 34.0, false, has, "Next chapter/page").clicked() {
                        self.skip_unit(true);
                    }
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.add_space(14.0);

                    let right_w = 575.0;
                    let mid_w = (ui.available_width() - right_w).max(180.0);
                    ui.vertical(|ui| {
                        ui.set_width(mid_w);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        match (&info, &waiting) {
                            (_, Some((label, prog))) => {
                                ui.horizontal(|ui| {
                                    ui.style_mut().wrap_mode = Some(TextWrapMode::Truncate);
                                    ui.label(RichText::new(format!("Preparing audio · {label}")).strong());
                                });
                                ui.add(ProgressBar::new(*prog).desired_width(mid_w).desired_height(6.0).fill(ACCENT).corner_radius(3.0));
                            }
                            (Some((label, _, streaming, buffering)), None) => {
                                ui.horizontal(|ui| {
                                    let status = if *buffering {
                                        Some(RichText::new("Buffering…").small().color(ACCENT))
                                    } else if *streaming {
                                        Some(RichText::new(format!("generating ahead · {:.0}%", stream_prog.unwrap_or(0.0) * 100.0)).small().color(DIM))
                                    } else {
                                        None
                                    };
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if let Some(s) = status {
                                            ui.label(s);
                                        }
                                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                            ui.style_mut().wrap_mode = Some(TextWrapMode::Truncate);
                                            ui.label(RichText::new(label).strong());
                                        });
                                    });
                                });
                                self.seek_row(ui, mid_w);
                            }
                            (None, None) => {
                                let hint = match &reader_hint {
                                    Some(l) => format!("Press ⏵ to listen to {l}"),
                                    None => "Nothing playing".into(),
                                };
                                ui.label(RichText::new(hint).color(DIM));
                                ui.label(RichText::new("Tip: click any word on the page to listen from there").small().color(FAINT));
                            }
                        }
                    });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let voice = config::profile_label(&self.cfg.profile());
                        if ui.button(format!("🎧  {voice}")).on_hover_text("Voice settings").clicked() {
                            self.show_settings = true;
                        }
                        ui.add_space(4.0);
                        let before = self.cfg.page_mode;
                        ui.selectable_value(&mut self.cfg.page_mode, true, "Page");
                        ui.selectable_value(&mut self.cfg.page_mode, false, "Chapter");
                        if before != self.cfg.page_mode {
                            self.cfg.save();
                        }
                        ui.label(RichText::new("Voice by").small().color(DIM));
                        ui.add_space(8.0);

                        let mut pick = None;
                        ui.menu_button(RichText::new(fmt_speed(self.cfg.speed)).strong(), |ui| {
                            ui.label(RichText::new("Playback speed").small().color(DIM));
                            for &s in crate::SPEEDS {
                                if ui.selectable_label((self.cfg.speed - s).abs() < 0.001, fmt_speed(s)).clicked() {
                                    pick = Some(s);
                                    ui.close_menu();
                                }
                            }
                        })
                        .response
                        .on_hover_text("Playback speed ( [ and ] )");
                        if let Some(s) = pick {
                            self.set_speed(s);
                        }
                        ui.add_space(4.0);

                        let mut v = self.cfg.volume;
                        ui.spacing_mut().slider_width = 70.0;
                        if ui.add(Slider::new(&mut v, 0.0..=1.0).show_value(false)).changed() {
                            self.cfg.volume = v;
                            if let Some(p) = &mut self.player {
                                p.set_volume(v);
                            }
                        }
                        ui.label(RichText::new(if v == 0.0 { "🔈" } else { "🔊" }).color(DIM));
                    });
                });
            });
    }

    /// elapsed · seek bar (with generated-so-far region) · total
    fn seek_row(&mut self, ui: &mut egui::Ui, width: f32) {
        let Some(p) = &self.player else { return };
        let (dur, buffered, streaming) = (p.duration().max(0.01), p.buffered(), p.now.as_ref().is_some_and(|n| n.streaming));
        let pos = self.seek_drag.unwrap_or(p.pos()).min(dur);
        let mut seek = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new(fmt_time(pos)).small().color(DIM));
            let total = if streaming { format!("~{}", fmt_time(dur)) } else { fmt_time(dur) };
            let bar_w = (width - 96.0).max(60.0);
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(bar_w, 18.0), Sense::click_and_drag());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let painter = ui.painter();
            let track = Rect::from_center_size(rect.center(), Vec2::new(bar_w, 4.0));
            let x_at = |t: f32| track.left() + track.width() * (t / dur).clamp(0.0, 1.0);
            painter.rect_filled(track, 2.0, RAISED);
            if streaming {
                painter.rect_filled(Rect::from_min_max(track.min, egui::pos2(x_at(buffered), track.max.y)), 2.0, FAINT);
            }
            painter.rect_filled(Rect::from_min_max(track.min, egui::pos2(x_at(pos), track.max.y)), 2.0, ACCENT);
            let active = resp.hovered() || resp.dragged();
            painter.circle_filled(egui::pos2(x_at(pos), track.center().y), if active { 7.0 } else { 5.0 }, ACCENT_HI);
            if let Some(ptr) = resp.interact_pointer_pos() {
                let t = (((ptr.x - track.left()) / track.width()).clamp(0.0, 1.0) * dur).min(buffered);
                if resp.dragged() {
                    self.seek_drag = Some(t);
                }
                if resp.clicked() || resp.drag_stopped() {
                    seek = Some(t);
                }
            } else if resp.drag_stopped() {
                seek = self.seek_drag;
            }
            if let Some(h) = resp.hover_pos() {
                let t = ((h.x - track.left()) / track.width()).clamp(0.0, 1.0) * dur;
                resp.on_hover_text_at_pointer(fmt_time(t));
            }
            ui.label(RichText::new(total).small().color(DIM));
        });
        if let Some(t) = seek {
            self.seek_drag = None;
            if let Some(p) = &mut self.player {
                p.seek(t);
            }
        }
    }

    // =============================== overlays ===============================

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let mut preview = false;
        egui::Window::new(RichText::new("Settings").strong())
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(440.0)
            .max_height(ctx.screen_rect().height() - 120.0)
            .frame(Frame::window(&ctx.style()).inner_margin(Margin::same(20)))
            .show(ctx, |ui| {
              egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                self.engines_section(ui);
                let installed = self.installed.clone();
                let cfg = &mut self.cfg;
                let mut dirty = false;
                section(ui, "VOICE");
                if installed.is_empty() {
                    ui.label(RichText::new("Install a voice engine above to choose a voice.").color(DIM));
                }
                egui::Grid::new("voice").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                    ui.label("Engine");
                    let engines: Vec<String> = cfg.engines.keys().filter(|e| installed.contains(*e)).cloned().collect();
                    egui::ComboBox::from_id_salt("engine").selected_text(&cfg.engine).width(200.0).show_ui(ui, |ui| {
                        for e in engines {
                            dirty |= ui.selectable_value(&mut cfg.engine, e.clone(), e).changed();
                        }
                    });
                    ui.end_row();
                    let voices = Config::voice_list(&cfg.engine);
                    if !voices.is_empty() {
                        ui.label("Voice");
                        let mut v = cfg.voice().unwrap_or(voices[0]).to_string();
                        egui::ComboBox::from_id_salt("voice").selected_text(config::voice_label(&v)).width(200.0).height(320.0).show_ui(ui, |ui| {
                            for name in voices {
                                ui.selectable_value(&mut v, name.to_string(), config::voice_label(name));
                            }
                        });
                        if cfg.voice() != Some(v.as_str()) {
                            cfg.voices.insert(cfg.engine.clone(), v);
                            dirty = true;
                        }
                        ui.end_row();
                    }
                });
                ui.add_space(4.0);
                if ui.button("⏵  Preview voice").clicked() {
                    preview = true;
                }

                section(ui, "PACING");
                egui::Grid::new("pacing").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                    ui.label("Between sentences");
                    dirty |= ui.add(Slider::new(&mut cfg.sentence_pause, 0.1..=1.0).suffix(" s").step_by(0.05)).changed();
                    ui.end_row();
                    ui.label("Between paragraphs");
                    dirty |= ui.add(Slider::new(&mut cfg.para_pause, 0.2..=1.5).suffix(" s").step_by(0.05)).changed();
                    ui.end_row();
                    ui.label("Longest pause");
                    dirty |= ui.add(Slider::new(&mut cfg.max_pause, 0.2..=1.2).suffix(" s").step_by(0.05)).changed();
                    ui.end_row();
                });
                ui.label(RichText::new("Applies to audio generated from now on.").small().color(DIM));

                ui.add_space(8.0);
                egui::CollapsingHeader::new(RichText::new("ADVANCED  ·  for tinkerers").small().strong().color(DIM))
                    .id_salt("advanced")
                    .default_open(std::env::var_os("SHOTS").is_some()) // screenshots only
                    .show(ui, |ui| {
                        let params = Config::params(&cfg.engine);
                        if params.is_empty() {
                            ui.label(RichText::new("This engine has no advanced settings.").color(DIM));
                            return;
                        }
                        egui::Grid::new("adv").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                            for p in params {
                                ui.label(p.label).on_hover_text(p.help);
                                let cur = cfg.adv(p.flag).map(str::to_string);
                                match p.kind {
                                    Kind::Float { min, max, step, default } => {
                                        let mut v: f32 = cur.as_deref().and_then(|c| c.parse().ok()).unwrap_or(default);
                                        let r = ui.add(Slider::new(&mut v, min..=max).step_by(step as f64)).on_hover_text(p.help);
                                        if r.changed() {
                                            let val = ((v - default).abs() > 1e-6).then(|| format!("{v:.3}"));
                                            cfg.set_adv(p.flag, val);
                                            dirty = true;
                                        }
                                    }
                                    Kind::Int { min, max, default } => {
                                        let mut v: i32 = cur.as_deref().and_then(|c| c.parse().ok()).unwrap_or(default);
                                        let r = ui.add(Slider::new(&mut v, min..=max)).on_hover_text(p.help);
                                        if r.changed() {
                                            cfg.set_adv(p.flag, (v != default).then(|| v.to_string()));
                                            dirty = true;
                                        }
                                    }
                                    Kind::Choice { options, default } => {
                                        let idx = cur.as_deref().and_then(|c| options.iter().position(|o| o.0 == c)).unwrap_or(default);
                                        let mut sel = idx;
                                        egui::ComboBox::from_id_salt(p.flag).selected_text(options[sel].1).show_ui(ui, |ui| {
                                            for (i, o) in options.iter().enumerate() {
                                                ui.selectable_value(&mut sel, i, o.1);
                                            }
                                        });
                                        if sel != idx {
                                            cfg.set_adv(p.flag, (sel != default).then(|| options[sel].0.to_string()));
                                            dirty = true;
                                        }
                                    }
                                }
                                ui.end_row();
                            }
                        });
                        ui.add_space(4.0);
                        let changed = cfg.advanced.get(&cfg.engine).is_some_and(|m| !m.is_empty());
                        if ui.add_enabled(changed, Button::new("Reset to defaults")).clicked() {
                            cfg.advanced.remove(&cfg.engine);
                            dirty = true;
                        }
                        ui.label(
                            RichText::new("Hover a setting for what it does. Changes apply to newly generated audio — right-click a chapter → Regenerate audio to hear the difference.")
                                .small()
                                .color(DIM),
                        );
                    });

                section(ui, "READING");
                egui::Grid::new("reading").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                    ui.label("Font");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut cfg.serif, true, RichText::new("Serif").family(serif()));
                        ui.selectable_value(&mut cfg.serif, false, "Sans");
                    });
                    ui.end_row();
                    ui.label("Text size");
                    ui.add(Slider::new(&mut cfg.font_size, 13.0..=36.0).step_by(1.0));
                    ui.end_row();
                    ui.label("Line spacing");
                    ui.add(Slider::new(&mut cfg.line_spacing, 1.2..=2.2).step_by(0.05));
                    ui.end_row();
                    ui.label("Page width");
                    ui.add(Slider::new(&mut cfg.page_width, 480.0..=1100.0).step_by(10.0).suffix(" px"));
                    ui.end_row();
                });

                section(ui, "LISTENING");
                let mut speed = cfg.speed;
                let mut set_speed = false;
                egui::Grid::new("listen").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                    ui.label("Playback speed");
                    set_speed = ui.add(Slider::new(&mut speed, 0.5..=2.5).step_by(0.05).suffix("×")).changed();
                    ui.end_row();
                    ui.label("Start playing after");
                    ui.add(Slider::new(&mut cfg.stream_buffer, 3.0..=60.0).step_by(1.0).suffix(" s ready"))
                        .on_hover_text("When audio is still being generated, playback starts once this much is ready");
                    ui.end_row();
                });
                ui.checkbox(&mut cfg.continuous, "Continue to the next chapter automatically");
                ui.checkbox(&mut cfg.follow, "Follow along: highlight words and turn pages");
                ui.checkbox(&mut cfg.prefetch, "Prepare the next chapter while listening");

                section(ui, "KEYBOARD");
                egui::Grid::new("keys").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                    for (k, what) in [
                        ("Space", "Play / pause"),
                        ("⬅ / ➡", "Previous / next page"),
                        ("Shift + ⬅ / ➡", "Back / forward 15 s"),
                        ("[  /  ]", "Slower / faster"),
                        ("Ctrl + O", "Import a book"),
                        ("Ctrl + = / -", "Text size"),
                        ("Esc", "Back to library / close"),
                    ] {
                        ui.label(RichText::new(k).color(ACCENT).small());
                        ui.label(RichText::new(what).small().color(DIM));
                        ui.end_row();
                    }
                });
                if dirty {
                    self.worker_dirty = true;
                }
                if set_speed {
                    self.set_speed(speed);
                }
              });
            });
        if preview {
            self.preview_voice();
        }
        if !open {
            self.show_settings = false;
            self.apply_worker_settings();
            self.cfg.save();
        }
    }

    /// Install / remove voice engines.
    fn engines_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "VOICE ENGINES");
        if self.installed.is_empty() {
            ui.label(
                RichText::new("The first engine also sets up a private Python for InkVoice (≈ 285 MB, shared by all engines). Nothing is installed system-wide.")
                    .small()
                    .color(DIM),
            );
            ui.add_space(4.0);
        }
        let mut install = None;
        let mut remove = None;
        for s in crate::installer::SPECS {
            Frame::new().fill(RAISED).corner_radius(6.0).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(s.title).strong());
                            let lic = if s.license == "Non-commercial" { DANGER } else { DIM };
                            ui.label(RichText::new(format!("{} · {}", s.size, s.license)).small().color(lic));
                        });
                        ui.label(RichText::new(s.blurb).small().color(DIM));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let busy = self.install.as_ref().map(|i| i.engine.as_str());
                        if busy == Some(s.name) {
                            ui.add(egui::Spinner::new());
                        } else if self.installed.contains(s.name) {
                            if self.confirm_uninstall.as_deref() == Some(s.name) {
                                if ui.add(Button::new(RichText::new("Remove").color(ON_ACCENT)).fill(DANGER)).clicked() {
                                    remove = Some(s.name);
                                }
                            } else {
                                if ui.add(Button::new(RichText::new("Remove").color(DIM)).frame(false)).on_hover_text("Delete this engine's model files").clicked() {
                                    self.confirm_uninstall = Some(s.name.to_string());
                                }
                                ui.label(RichText::new("✔ Installed").color(ACCENT));
                            }
                        } else if ui.add_enabled(busy.is_none(), Button::new(RichText::new("Install").color(ON_ACCENT).strong()).fill(ACCENT)).clicked() {
                            install = Some(s.name);
                        }
                    });
                });
                if self.install.as_ref().is_some_and(|i| i.engine == s.name) {
                    ui.add_space(4.0);
                    ui.label(RichText::new(&self.install_step).small());
                    match self.install_progress {
                        Some(p) => ui.add(ProgressBar::new(p).desired_height(6.0).fill(ACCENT).corner_radius(3.0).show_percentage()),
                        None => ui.add(ProgressBar::new(0.0).desired_height(6.0).animate(true).corner_radius(3.0)),
                    };
                }
            });
            ui.add_space(4.0);
        }
        if let Some(n) = install {
            self.install_engine(n);
        }
        if let Some(n) = remove {
            self.uninstall_engine(n);
        }
    }

    fn toast(&mut self, ctx: &egui::Context) {
        let Some(s) = &self.status else { return };
        let life = if s.error { 10.0 } else { 4.0 };
        let age = s.at.elapsed().as_secs_f32();
        if age > life {
            self.status = None;
            return;
        }
        let fade = ((life - age) / 0.4).clamp(0.0, 1.0);
        let (text, error) = (s.text.clone(), s.error);
        let mut dismiss = false;
        egui::Area::new(egui::Id::new("toast"))
            .anchor(Align2::RIGHT_BOTTOM, [-20.0, -96.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_opacity(fade);
                let r = Frame::popup(&ctx.style())
                    .fill(RAISED)
                    .stroke(Stroke::new(1.0_f32, if error { DANGER } else { BORDER }))
                    .inner_margin(Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_max_width(400.0);
                        ui.label(RichText::new(&text).color(if error { DANGER } else { TEXT }));
                    })
                    .response;
                if r.interact(Sense::click()).clicked() {
                    dismiss = true;
                }
            });
        if dismiss {
            self.status = None;
        }
    }
}

/// Byte index of the `n`-th char.
fn byte_at(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(b, _)| b)
}
