//! Lumen's palettes and type, made from Windows 11's own materials: Segoe UI
//! Variable, Fluent icons, the system accent color and the light or dark app mode.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak, Shadow, Stroke,
    epaint::text::{Blob, VariationCoords},
    text::{LayoutJob, TextFormat, TextWrapping},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const STRONG: &str = "strong";
pub const DISPLAY: &str = "display";
pub const ICONS: &str = "icons";

/// Light or dark, chosen in /settings. Stored apart from the search settings so
/// that changing it never invalidates the index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Match Windows",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    pub fn load() -> Self {
        crate::storage::load("appearance.json")
    }

    pub fn save(self) {
        crate::storage::save("appearance.json", &self);
    }

    pub fn is_dark(self, windows_is_light: bool) -> bool {
        match self {
            Self::System => !windows_is_light,
            Self::Light => false,
            Self::Dark => true,
        }
    }
}

/// The Windows accent ramp, lightest first: Light 3, 2, 1, the accent, then Dark 1, 2, 3.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Accent(pub [Color32; 7]);

impl Default for Accent {
    /// Windows' default blue.
    fn default() -> Self {
        Self([hex(0x99EBFF), hex(0x4CC2FF), hex(0x0091F8), hex(0x0078D4), hex(0x0067C0), hex(0x003E92), hex(0x001A68)])
    }
}

impl Accent {
    pub fn from_ramp(ramp: [[u8; 3]; 7]) -> Self {
        Self(ramp.map(|[r, g, b]| Color32::from_rgb(r, g, b)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub surface: Color32,
    pub ink: Color32,
    pub ink2: Color32,
    pub ink3: Color32,
    pub ghost: Color32,
    pub line: Color32,
    pub edge: Color32,
    pub hover: Color32,
    pub press: Color32,
    pub selected: Color32,
    pub tile: Color32,
    pub tile_lit: Color32,
    pub key_fill: Color32,
    pub key_edge: Color32,
    pub field: Color32,
    pub segment: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,
    pub danger: Color32,
    /// Backdrop brightness beside the panel and at the screen's edges.
    pub dim_near: f32,
    pub dim_far: f32,
    pub tint: [f32; 3],
    /// (blur, spread, white alpha) for each layer of the panel's light.
    pub glow: [(u8, u8, u8); 3],
}

impl Palette {
    pub fn new(dark: bool, accent: Accent) -> Self {
        let [_, light2, _, _, dark1, _, _] = accent.0;
        if dark {
            let surface = hex(0x212226);
            let ink = hex(0xF3F4F6);
            let white = Color32::WHITE;
            Self {
                dark,
                surface,
                ink,
                ink2: hex(0xADB0B8),
                ink3: hex(0x8E919A),
                ghost: hex(0x676A72),
                line: mix(surface, white, 0.075),
                edge: Color32::from_white_alpha(26),
                hover: mix(surface, white, 0.045),
                press: mix(surface, white, 0.08),
                selected: mix(surface, light2, 0.13),
                tile: mix(surface, white, 0.065),
                tile_lit: mix(surface, light2, 0.2),
                key_fill: mix(surface, white, 0.06),
                key_edge: mix(surface, white, 0.15),
                field: mix(surface, Color32::BLACK, 0.2),
                segment: mix(surface, white, 0.11),
                accent: light2,
                accent_hover: mix(light2, white, 0.12),
                on_accent: hex(0x0B0C0E),
                danger: hex(0xFF99A4),
                dim_near: 0.36,
                dim_far: 0.2,
                tint: [0.92, 0.95, 1.0],
                glow: [(110, 6, 16), (36, 2, 18), (10, 0, 22)],
            }
        } else {
            let surface = Color32::WHITE;
            let ink = hex(0x1A1B1F);
            Self {
                dark,
                surface,
                ink,
                ink2: hex(0x55585F),
                ink3: hex(0x6E717A),
                ghost: hex(0xA6A9B0),
                line: mix(surface, ink, 0.085),
                edge: Color32::from_black_alpha(22),
                hover: mix(surface, ink, 0.04),
                press: mix(surface, ink, 0.075),
                selected: mix(surface, dark1, 0.09),
                tile: mix(surface, ink, 0.05),
                tile_lit: surface,
                key_fill: mix(surface, ink, 0.03),
                key_edge: mix(surface, ink, 0.16),
                field: mix(surface, ink, 0.035),
                segment: surface,
                accent: dark1,
                accent_hover: mix(dark1, Color32::BLACK, 0.12),
                on_accent: Color32::WHITE,
                danger: hex(0xC42B1C),
                dim_near: 0.46,
                dim_far: 0.27,
                tint: [1.0, 1.0, 1.0],
                glow: [(120, 8, 52), (40, 2, 96), (10, 0, 150)],
            }
        }
    }
}

pub const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Mixes two opaque colors; `t` = 1 gives `b`.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let channel = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(channel(a.r(), b.r()), channel(a.g(), b.g()), channel(a.b(), b.b()))
}

/// Segoe UI Variable (Windows 11) with its weight and optical-size axes, or the
/// static Segoe UI faces on Windows 10; Fluent icons; CJK fallbacks.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let read = |file: &str| std::fs::read(format!(r"C:\Windows\Fonts\{file}")).ok();
    if let Some(bytes) = read("SegUIVar.ttf") {
        let blob: Blob = Arc::new(bytes);
        for (name, weight, optical) in [("ui", 400.0, 10.5), ("ui-strong", 600.0, 10.5), ("ui-display", 400.0, 18.0)] {
            let tweak = FontTweak { coords: VariationCoords::new([(b"wght", weight), (b"opsz", optical)]), ..Default::default() };
            fonts.font_data.insert(name.into(), Arc::new(FontData::from_blob(blob.clone(), 0).tweak(tweak)));
        }
    } else {
        for (name, file) in [("ui", "segoeui.ttf"), ("ui-strong", "seguisb.ttf"), ("ui-display", "segoeui.ttf")] {
            if let Some(bytes) = read(file) {
                fonts.font_data.insert(name.into(), Arc::new(FontData::from_owned(bytes)));
            }
        }
    }
    let icons = ["SegoeIcons.ttf", "segmdl2.ttf"].into_iter().find_map(read);
    if let Some(bytes) = icons {
        fonts.font_data.insert("fluent".into(), Arc::new(FontData::from_owned(bytes)));
    }
    // Fallbacks for Chinese, Japanese and Korean names; Segoe UI has no CJK glyphs.
    let mut fallbacks = Vec::new();
    for (name, file) in [("yahei", "msyh.ttc"), ("malgun", "malgun.ttf")] {
        if let Some(bytes) = read(file) {
            fonts.font_data.insert(name.into(), Arc::new(FontData::from_owned(bytes)));
            fallbacks.push(name.to_string());
        }
    }
    let defaults = fonts.families[&FontFamily::Proportional].clone();
    let family = |fonts: &FontDefinitions, first: &str| {
        let mut names: Vec<String> = [first, "ui"].into_iter().filter(|n| fonts.font_data.contains_key(*n)).take(1).map(String::from).collect();
        names.extend(defaults.iter().cloned());
        names.extend(fallbacks.iter().cloned());
        names
    };
    let proportional = family(&fonts, "ui");
    let strong = family(&fonts, "ui-strong");
    let display = family(&fonts, "ui-display");
    let mut icon_family = proportional.clone();
    if fonts.font_data.contains_key("fluent") {
        icon_family.insert(0, "fluent".into());
    }
    fonts.families.insert(FontFamily::Proportional, proportional);
    fonts.families.insert(FontFamily::Name(STRONG.into()), strong);
    fonts.families.insert(FontFamily::Name(DISPLAY.into()), display);
    fonts.families.insert(FontFamily::Name(ICONS.into()), icon_family);
    ctx.set_fonts(fonts);
}

/// Text at `size` points and a variable `weight` (400 regular, 600 semibold).
pub fn text(size: f32, weight: f32, color: Color32) -> TextFormat {
    // The static fallback has no weight axis, so heavy text also picks the semibold face.
    let family = if weight >= 560.0 { FontFamily::Name(STRONG.into()) } else { FontFamily::Proportional };
    TextFormat { font_id: FontId::new(size, family), color, coords: coords(weight, size), ..Default::default() }
}

/// The search field's face: Segoe UI Variable at a display optical size.
pub fn display(size: f32, weight: f32, color: Color32) -> TextFormat {
    TextFormat { font_id: FontId::new(size, FontFamily::Name(DISPLAY.into())), color, coords: coords(weight, size), ..Default::default() }
}

fn coords(weight: f32, size: f32) -> VariationCoords {
    // The optical-size axis is measured in typographic points.
    VariationCoords::new([(b"wght", weight), (b"opsz", (size * 0.75).clamp(8.0, 36.0))])
}

pub fn icon(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ICONS.into()))
}

/// One line of text, cut short with an ellipsis at `max_width`.
pub fn line(text: &str, format: TextFormat, max_width: f32) -> LayoutJob {
    let mut job = LayoutJob::single_section(text.to_owned(), format);
    job.wrap = TextWrapping::truncate_at_width(max_width.max(10.0));
    job
}

/// egui's own widgets (scrollbars, text fields, tooltips) in the same palette.
pub fn visuals(p: &Palette) -> egui::Visuals {
    let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let ink = |alpha: u8| if p.dark { Color32::from_white_alpha(alpha) } else { Color32::from_black_alpha(alpha) };
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.line);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(if p.dark { 110 } else { 40 }) };
    v.popup_shadow = v.window_shadow;
    v.extreme_bg_color = p.field;
    v.text_edit_bg_color = Some(p.field);
    v.faint_bg_color = p.hover;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = mix(p.surface, p.accent, if p.dark { 0.42 } else { 0.24 });
    v.selection.stroke = Stroke::new(1.0, p.ink);
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);
    let widgets = &mut v.widgets;
    widgets.noninteractive.fg_stroke.color = p.ink;
    widgets.noninteractive.bg_stroke.color = p.line;
    widgets.noninteractive.bg_fill = p.surface;
    // Scrollbar handles take their color from these fills.
    for (state, alpha) in [(&mut widgets.inactive, 70), (&mut widgets.hovered, 110), (&mut widgets.active, 140)] {
        state.bg_fill = ink(alpha);
        state.weak_bg_fill = p.tile;
        state.fg_stroke.color = p.ink;
        state.bg_stroke = Stroke::NONE;
        state.corner_radius = CornerRadius::same(6);
    }
    v
}

/// Ease-out cubic.
pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// The selection highlight gliding to a newly selected row.
#[derive(Clone, Copy, Default)]
pub struct Slide {
    from: f32,
    to: f32,
    start: f64,
}

impl Slide {
    const TIME: f64 = 0.09;

    pub fn at(&self, now: f64) -> f32 {
        let t = ((now - self.start) / Self::TIME) as f32;
        self.from + (self.to - self.from) * ease_out(t)
    }

    pub fn running(&self, now: f64) -> bool {
        now - self.start < Self::TIME && self.from != self.to
    }

    /// Glides one step at a time; long jumps and held keys move at once.
    pub fn set(&mut self, target: f32, now: f64, animate: bool, reach: f32) {
        if self.to == target && (animate || !self.running(now)) {
            return;
        }
        let current = self.at(now);
        let glide = animate && !self.running(now) && (target - current).abs() <= reach;
        *self = Slide { from: if glide { current } else { target }, to: target, start: if glide { now } else { f64::NEG_INFINITY } };
    }
}
