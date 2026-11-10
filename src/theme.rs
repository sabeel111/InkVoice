use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle};
use std::sync::Arc;

// Night-reading palette: near-black surfaces, warm off-white text, muted amber accent.
pub const BG: Color32 = Color32::from_rgb(15, 15, 17);
pub const SURFACE: Color32 = Color32::from_rgb(22, 22, 25);
pub const RAISED: Color32 = Color32::from_rgb(30, 30, 34);
pub const HOVER: Color32 = Color32::from_rgb(40, 39, 42);
pub const BORDER: Color32 = Color32::from_rgb(44, 43, 46);
pub const TEXT: Color32 = Color32::from_rgb(214, 209, 196);
pub const DIM: Color32 = Color32::from_rgb(128, 124, 116);
pub const FAINT: Color32 = Color32::from_rgb(84, 82, 78);
pub const ACCENT: Color32 = Color32::from_rgb(222, 166, 88);
pub const ACCENT_HI: Color32 = Color32::from_rgb(240, 190, 118);
pub const ACCENT_BG: Color32 = Color32::from_rgb(74, 56, 30);
pub const ON_ACCENT: Color32 = Color32::from_rgb(24, 18, 10);
pub const DANGER: Color32 = Color32::from_rgb(226, 118, 98);

pub fn serif() -> FontFamily {
    FontFamily::Name("serif".into())
}

fn load(defs: &mut FontDefinitions, name: &str, paths: &[&str]) -> bool {
    for p in paths {
        if let Ok(bytes) = std::fs::read(p) {
            defs.font_data.insert(name.into(), Arc::new(FontData::from_owned(bytes)));
            return true;
        }
    }
    false
}

pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let fallbacks = defs.families[&FontFamily::Proportional].clone();

    // UI: Noto Sans if present (heavier than the default light face; better on dark backgrounds)
    if load(&mut defs, "ui", &["/usr/share/fonts/noto/NotoSans-Regular.ttf", "C:\\Windows\\Fonts\\segoeui.ttf"]) {
        defs.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "ui".into());
    }
    // Reading: a serif face if present, else the UI font
    let mut serif_stack = fallbacks.clone();
    if load(
        &mut defs,
        "serif",
        &[
            "/usr/share/fonts/noto/NotoSerif-Regular.ttf",
            "/usr/share/fonts/liberation/LiberationSerif-Regular.ttf",
            "C:\\Windows\\Fonts\\georgia.ttf",
        ],
    ) {
        serif_stack.insert(0, "serif".into());
    }
    defs.families.insert(serif(), serif_stack);
    ctx.set_fonts(defs);

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Body, FontId::proportional(14.5)),
        (TextStyle::Button, FontId::proportional(14.5)),
        (TextStyle::Heading, FontId::proportional(22.0)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 28.0;
    style.spacing.slider_rail_height = 4.0;

    let mut v = egui::Visuals::dark();
    v.panel_fill = SURFACE;
    v.window_fill = SURFACE;
    v.extreme_bg_color = BG;
    v.faint_bg_color = RAISED;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT_BG;
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.window_corner_radius = 10.0.into();
    v.menu_corner_radius = 8.0.into();
    v.slider_trailing_fill = true;
    for (w, fill) in [
        (&mut v.widgets.noninteractive, SURFACE),
        (&mut v.widgets.inactive, RAISED),
        (&mut v.widgets.hovered, HOVER),
        (&mut v.widgets.active, ACCENT_BG),
        (&mut v.widgets.open, RAISED),
    ] {
        w.corner_radius = 6.0.into();
        w.weak_bg_fill = fill;
        w.bg_fill = fill;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    style.visuals = v;
    ctx.set_style(style);
}

/// Stable muted colour for a generated cover.
pub fn cover_color(seed: &str) -> Color32 {
    let h = seed.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    let hue = (h % 360) as f32 / 360.0;
    let c = egui::ecolor::Hsva::new(hue, 0.35, 0.32, 1.0);
    c.into()
}

pub fn fmt_time(s: f32) -> String {
    let s = s.max(0.0) as u32;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}
