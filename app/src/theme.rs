//! Palette derivation (same rules as the Hearth/Quickshell palette) and egui styling.
use eframe::egui::{
    self, epaint::Shadow, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Vec2,
};
use eframe::egui::ecolor::Hsva;
use proto::ThemeColors;
use std::sync::Arc;

pub const MOBILE: bool = cfg!(any(target_os = "android", feature = "mobile-preview"));

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Palette {
    pub dark: bool,
    pub page: Color32,
    pub card: Color32,
    pub bar: Color32,
    pub raised: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub line: Color32,
    pub hair: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub tint: Color32,
    pub series2: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub bad: Color32,
}

fn rgb(c: [u8; 3]) -> Color32 { Color32::from_rgb(c[0], c[1], c[2]) }

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

fn lum(c: Color32) -> f32 {
    let f = |v: u8| { let v = v as f32 / 255.0; if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) } };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}

/// Qt-style QColor::darker / lighter on the HSV value channel.
fn darker(c: Color32, f: f32) -> Color32 {
    let mut h = Hsva::from_srgba_unmultiplied([c.r(), c.g(), c.b(), 255]);
    h.v /= f;
    let [r, g, b, _] = h.to_srgba_unmultiplied();
    Color32::from_rgb(r, g, b)
}
fn lighter(c: Color32, f: f32) -> Color32 {
    let mut h = Hsva::from_srgba_unmultiplied([c.r(), c.g(), c.b(), 255]);
    h.v *= f;
    if h.v > 1.0 { h.s = (h.s - (h.v - 1.0)).max(0.0); h.v = 1.0; }
    let [r, g, b, _] = h.to_srgba_unmultiplied();
    Color32::from_rgb(r, g, b)
}

impl Palette {
    pub fn from_system(dark: bool) -> Self {
        let (bg, fg) = if dark { ([0x1b, 0x1e, 0x26], [0xe6, 0xe9, 0xef]) } else { ([0xf1, 0xf3, 0xf7], [0x1c, 0x22, 0x30]) };
        Self::derive(&ThemeColors { bg, fg, accent: None, source: "Default".into() })
    }

    pub fn derive(t: &ThemeColors) -> Self {
        let (bg, fg) = (rgb(t.bg), rgb(t.fg));
        let dark = lum(bg) < 0.4;
        let (page, card, bar, raised) = if dark {
            (darker(bg, 1.35), lighter(bg, 1.28), bg, lighter(bg, 1.55))
        } else {
            let card = mix(bg, Color32::WHITE, 0.65);
            (darker(bg, 1.05), card, card, mix(card, fg, 0.06))
        };
        let mut accent = t.accent.map(rgb).unwrap_or(if dark { Color32::from_rgb(0x5a, 0xa9, 0xff) } else { Color32::from_rgb(0x1f, 0x6f, 0xeb) });
        // Keep the accent legible on cards.
        for _ in 0..4 {
            let contrast = (lum(accent).max(lum(card)) + 0.05) / (lum(accent).min(lum(card)) + 0.05);
            if contrast >= 3.0 { break; }
            accent = if dark { lighter(accent, 1.2) } else { darker(accent, 1.25) };
        }
        let on_accent = if lum(accent) > 0.35 { Color32::from_rgb(0x14, 0x16, 0x1a) } else { Color32::from_rgb(0xfa, 0xfa, 0xfc) };
        let mut h = Hsva::from_srgba_unmultiplied([accent.r(), accent.g(), accent.b(), 255]);
        h.h = (h.h + 0.5).fract();
        h.s = h.s.clamp(0.45, 0.8);
        h.v = if dark { h.v.max(0.8) } else { h.v.min(0.7) };
        let [r, g, b, _] = h.to_srgba_unmultiplied();
        let (ok, warn, bad) = if dark {
            (Color32::from_rgb(0x57, 0xc7, 0x85), Color32::from_rgb(0xf0, 0xb4, 0x29), Color32::from_rgb(0xf2, 0x6d, 0x6d))
        } else {
            (Color32::from_rgb(0x1a, 0x8a, 0x4a), Color32::from_rgb(0xa8, 0x6a, 0x00), Color32::from_rgb(0xc6, 0x28, 0x28))
        };
        Self {
            dark, page, card, bar, raised, text: fg,
            muted: mix(card, fg, 0.62), faint: mix(card, fg, 0.38), line: mix(card, fg, 0.14), hair: mix(card, fg, 0.07),
            accent, on_accent, tint: mix(card, accent, 0.18), series2: Color32::from_rgb(r, g, b), ok, warn, bad,
        }
    }
}

pub fn bold() -> FontFamily { FontFamily::Name("bold".into()) }

pub fn install_fonts(ctx: &egui::Context) {
    let mut f = FontDefinitions::default();
    let add = |f: &mut FontDefinitions, n: &str, b: &'static [u8]| { f.font_data.insert(n.into(), Arc::new(FontData::from_static(b))); };
    add(&mut f, "Montserrat", include_bytes!("../assets/Montserrat-Regular.ttf"));
    add(&mut f, "MontserratSemi", include_bytes!("../assets/Montserrat-SemiBold.ttf"));
    add(&mut f, "Phosphor", include_bytes!("../assets/Phosphor.ttf"));
    let prop = f.families.entry(FontFamily::Proportional).or_default();
    prop.insert(0, "Montserrat".into());
    prop.push("Phosphor".into());
    let mut b = vec!["MontserratSemi".to_string(), "Montserrat".to_string()];
    b.extend(f.families[&FontFamily::Proportional].iter().skip(1).cloned());
    f.families.insert(bold(), b);
    ctx.set_fonts(f);
}

pub fn apply(ctx: &egui::Context, p: &Palette) {
    let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let r = CornerRadius::same(12);
    v.panel_fill = p.page;
    v.window_fill = p.card;
    v.extreme_bg_color = p.page;
    v.faint_bg_color = p.raised;
    v.window_corner_radius = CornerRadius::same(18);
    v.menu_corner_radius = r;
    v.window_stroke = Stroke::new(1.0, p.hair);
    v.window_shadow = Shadow { offset: [0, 6], blur: 24, spread: 0, color: Color32::from_black_alpha(90) };
    v.popup_shadow = v.window_shadow;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.override_text_color = None;
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.card;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.hair);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;
    for (s, fill) in [(&mut w.inactive, p.raised), (&mut w.hovered, mix(p.raised, p.accent, 0.16)), (&mut w.active, mix(p.raised, p.accent, 0.3)), (&mut w.open, mix(p.raised, p.accent, 0.16))] {
        s.bg_fill = fill;
        s.weak_bg_fill = fill;
        s.corner_radius = r;
        s.fg_stroke = Stroke::new(1.0, p.text);
        s.expansion = 0.0;
    }
    w.inactive.bg_stroke = Stroke::NONE;
    w.hovered.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.5));
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    ctx.set_visuals(v);

    ctx.all_styles_mut(|s| {
        let k = if MOBILE { 1.08 } else { 1.0 };
        s.spacing.item_spacing = Vec2::new(10.0, 10.0);
        s.spacing.button_padding = Vec2::new(16.0, if MOBILE { 12.0 } else { 9.0 });
        s.spacing.interact_size.y = if MOBILE { 46.0 } else { 38.0 };
        s.spacing.slider_width = 200.0;
        s.spacing.scroll.bar_width = 6.0;
        s.text_styles = [
            (TextStyle::Heading, FontId::new(24.0 * k, bold())),
            (TextStyle::Body, FontId::new(14.5 * k, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.5 * k, bold())),
            (TextStyle::Small, FontId::new(12.0 * k, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.5 * k, FontFamily::Monospace)),
        ].into();
    });
}
