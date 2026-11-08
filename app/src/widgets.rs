use crate::theme::{bold, Palette};
use eframe::egui::{
    self, epaint::{Mesh, Vertex}, Align2, Color32, CornerRadius, FontId, Frame, Margin, Pos2, Rect, Response, RichText,
    Sense, Shape, Stroke, Ui, Vec2,
};

pub fn heading(text: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(text).font(FontId::new(size, bold())).color(color)
}
pub fn icon(glyph: &str, size: f32, color: Color32) -> RichText {
    RichText::new(glyph).size(size).color(color)
}

/// Fixed-size card. `w`/`h` are outer sizes.
pub fn card(ui: &mut Ui, p: &Palette, w: f32, h: Option<f32>, add: impl FnOnce(&mut Ui)) {
    let m = 16.0;
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.hair))
        .corner_radius(CornerRadius::same(18))
        .inner_margin(Margin::same(m as i8))
        .show(ui, |ui| {
            ui.set_width(w - 2.0 * m - 2.0);
            if let Some(h) = h { ui.set_min_height(h - 2.0 * m - 2.0); ui.set_max_height(h - 2.0 * m - 2.0); }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                add(ui);
            });
        });
}

pub fn card_title(ui: &mut Ui, p: &Palette, glyph: &str, title: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(icon(glyph, 16.0, p.accent));
        ui.label(RichText::new(title).font(FontId::new(13.0, bold())).color(p.muted));
    });
}

pub fn chip(ui: &mut Ui, p: &Palette, glyph: &str, text: &str, color: Color32) {
    let paint = ui.painter();
    let t = paint.layout_no_wrap(text.to_string(), FontId::new(12.0, bold()), p.text);
    let i = paint.layout_no_wrap(glyph.to_string(), FontId::new(13.0, egui::FontFamily::Proportional), color);
    let iw = if glyph.is_empty() { 0.0 } else { i.size().x + 6.0 };
    let size = Vec2::new(t.size().x + iw + 20.0, 26.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(99), mix_alpha(color, 0.16));
    let mut x = rect.min.x + 10.0;
    if !glyph.is_empty() {
        ui.painter().galley(Pos2::new(x, rect.center().y - i.size().y / 2.0), i.clone(), color);
        x += iw;
    }
    ui.painter().galley(Pos2::new(x, rect.center().y - t.size().y / 2.0), t, p.text);
}

/// Left/right aligned pair on one fixed-height row (keeps card heights stable).
pub fn split(ui: &mut Ui, h: f32, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)), left);
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::right_to_left(egui::Align::Center)), right);
}

pub fn mix_alpha(c: Color32, a: f32) -> Color32 { c.gamma_multiply(a) }

/// Rounded progress meter.
pub fn meter(ui: &mut Ui, p: &Palette, frac: f32, color: Color32, h: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
    let r = CornerRadius::same((h / 2.0) as u8);
    ui.painter().rect_filled(rect, r, p.line);
    let f = frac.clamp(0.0, 1.0);
    if f > 0.0 {
        let fill = Rect::from_min_size(rect.min, Vec2::new((rect.width() * f).max(h), h));
        ui.painter().rect_filled(fill, r, color);
    }
}

/// Circular gauge with a centred value.
pub fn ring(ui: &mut Ui, p: &Palette, frac: f32, size: f32, color: Color32, big: &str, small: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let c = rect.center();
    let w = (size * 0.11).max(6.0);
    let r = size / 2.0 - w / 2.0;
    let pt = |a: f32| Pos2::new(c.x + r * a.cos(), c.y + r * a.sin());
    let paint = ui.painter();
    paint.circle_stroke(c, r, Stroke::new(w, p.line));
    let f = frac.clamp(0.0, 1.0);
    if f > 0.002 {
        let start = -std::f32::consts::FRAC_PI_2;
        let n = ((f * 72.0) as usize).max(2);
        let pts: Vec<Pos2> = (0..=n).map(|i| pt(start + f * std::f32::consts::TAU * i as f32 / n as f32)).collect();
        paint.add(Shape::line(pts.clone(), Stroke::new(w, color)));
        for e in [pts[0], pts[n]] { paint.circle_filled(e, w / 2.0, color); }
    }
    paint.text(Pos2::new(c.x, c.y - if small.is_empty() { 0.0 } else { size * 0.06 }), Align2::CENTER_CENTER, big, FontId::new(size * 0.26, bold()), p.text);
    if !small.is_empty() {
        paint.text(Pos2::new(c.x, c.y + size * 0.19), Align2::CENTER_CENTER, small, FontId::new(size * 0.12, egui::FontFamily::Proportional), p.muted);
    }
}

pub struct Series<'a> { pub name: &'a str, pub data: Vec<f32>, pub color: Color32 }

/// Time-series area/line chart; newest sample on the right edge. `window` = number of slots.
pub fn chart(ui: &mut Ui, p: &Palette, id: &str, series: &[Series], window: usize, max: f32, height: f32, fmt: &dyn Fn(f32) -> String) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let paint = ui.painter_at(rect.expand(2.0));
    let plot = Rect::from_min_max(Pos2::new(rect.min.x, rect.min.y + 6.0), Pos2::new(rect.max.x, rect.max.y - 2.0));
    for i in 0..3 {
        let y = plot.max.y - plot.height() * i as f32 / 2.0;
        paint.line_segment([Pos2::new(plot.min.x, y), Pos2::new(plot.max.x, y)], Stroke::new(1.0, p.hair));
    }
    paint.text(Pos2::new(plot.min.x + 2.0, plot.min.y + 8.0), Align2::LEFT_CENTER, fmt(max), FontId::new(10.5, egui::FontFamily::Proportional), p.faint);
    if series.iter().all(|s| s.data.len() < 2) {
        paint.text(plot.center(), Align2::CENTER_CENTER, "Collecting data…", FontId::new(12.5, egui::FontFamily::Proportional), p.faint);
    }
    let dx = plot.width() / (window.saturating_sub(1)).max(1) as f32;
    let xy = |n: usize, i: usize, v: f32| Pos2::new(plot.max.x - (n - 1 - i) as f32 * dx, plot.max.y - plot.height() * (v / max.max(1e-6)).clamp(0.0, 1.0));
    for (si, s) in series.iter().enumerate() {
        let n = s.data.len();
        if n < 2 { continue; }
        let pts: Vec<Pos2> = s.data.iter().enumerate().map(|(i, v)| xy(n, i, *v)).collect();
        if si == 0 {
            let mut mesh = Mesh::default();
            for (i, q) in pts.iter().enumerate() {
                mesh.vertices.push(Vertex { pos: *q, uv: egui::epaint::WHITE_UV, color: s.color.gamma_multiply(0.30) });
                mesh.vertices.push(Vertex { pos: Pos2::new(q.x, plot.max.y), uv: egui::epaint::WHITE_UV, color: s.color.gamma_multiply(0.0) });
                if i > 0 {
                    let b = (i * 2) as u32;
                    mesh.indices.extend_from_slice(&[b - 2, b - 1, b, b - 1, b + 1, b]);
                }
            }
            paint.add(Shape::mesh(mesh));
        }
        paint.add(Shape::line(pts, Stroke::new(2.0, s.color)));
    }
    if let Some(hp) = resp.hover_pos().filter(|h| plot.contains(*h)) {
        let n = series.iter().map(|s| s.data.len()).max().unwrap_or(0);
        if n >= 2 {
            let idx = (n as f32 - 1.0 - ((plot.max.x - hp.x) / dx).round()).clamp(0.0, n as f32 - 1.0) as usize;
            let x = xy(n, idx, 0.0).x;
            paint.line_segment([Pos2::new(x, plot.min.y), Pos2::new(x, plot.max.y)], Stroke::new(1.0, p.line));
            let mut lines = vec![];
            for s in series {
                let off = n - s.data.len();
                if idx >= off {
                    let v = s.data[idx - off];
                    let q = xy(n, idx, 0.0).x;
                    let pos = Pos2::new(q, plot.max.y - plot.height() * (v / max.max(1e-6)).clamp(0.0, 1.0));
                    paint.circle_filled(pos, 5.0, p.card);
                    paint.circle_filled(pos, 3.5, s.color);
                    lines.push(format!("{}  {}", s.name, fmt(v)));
                }
            }
            let _ = id;
            let layer = egui::LayerId::new(egui::Order::Tooltip, ui.id().with(("chart-tip", id)));
            let tp = ui.ctx().layer_painter(layer);
            let galleys: Vec<_> = lines.iter().map(|l| tp.layout_no_wrap(l.clone(), FontId::new(12.0, bold()), p.text)).collect();
            let w = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max) + 20.0;
            let h = galleys.len() as f32 * 18.0 + 10.0;
            let mut tl = Pos2::new(x + 12.0, plot.min.y + 4.0);
            if tl.x + w > rect.max.x { tl.x = x - 12.0 - w; }
            tp.rect(Rect::from_min_size(tl, Vec2::new(w, h)), CornerRadius::same(8), p.raised, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
            for (i, g) in galleys.into_iter().enumerate() { tp.galley(Pos2::new(tl.x + 10.0, tl.y + 5.0 + i as f32 * 18.0), g, p.text); }
        }
    }
}

/// Pill-shaped toggle/action button. Returns the response.
pub fn pill_button(ui: &mut Ui, p: &Palette, glyph: &str, text: &str, selected: bool) -> Response {
    let h = if crate::theme::MOBILE { 40.0 } else { 34.0 };
    let font = FontId::new(13.5, bold());
    let g = ui.painter().layout_no_wrap(text.to_string(), font.clone(), p.text);
    let ig = ui.painter().layout_no_wrap(glyph.to_string(), FontId::new(16.0, egui::FontFamily::Proportional), p.text);
    let gap = if glyph.is_empty() || text.is_empty() { 0.0 } else { 6.0 };
    let (iw, tw) = (if glyph.is_empty() { 0.0 } else { ig.size().x }, if text.is_empty() { 0.0 } else { g.size().x });
    let w = (iw + gap + tw + 28.0).max(h);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let (bg, fg) = if selected { (p.accent, p.on_accent) } else if resp.hovered() { (crate::theme::mix(p.raised, p.accent, 0.16), p.text) } else { (p.raised, p.text) };
    ui.painter().rect_filled(rect, CornerRadius::same(99), if resp.is_pointer_button_down_on() { crate::theme::mix(bg, p.accent, 0.3) } else { bg });
    let mut x = rect.center().x - (iw + gap + tw) / 2.0;
    if !glyph.is_empty() {
        ui.painter().text(Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, glyph, FontId::new(16.0, egui::FontFamily::Proportional), fg);
        x += iw + gap;
    }
    if !text.is_empty() { ui.painter().text(Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, text, font, fg); }
    resp
}

/// Large primary button (accent filled).
pub fn primary_button(ui: &mut Ui, p: &Palette, text: &str, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), if crate::theme::MOBILE { 52.0 } else { 46.0 }), Sense::click());
    let bg = if !enabled { p.line } else if resp.is_pointer_button_down_on() { crate::theme::mix(p.accent, p.page, 0.25) } else if resp.hovered() { crate::theme::mix(p.accent, p.text, 0.1) } else { p.accent };
    ui.painter().rect_filled(rect, CornerRadius::same(14), bg);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, FontId::new(15.0, bold()), if enabled { p.on_accent } else { p.faint });
    resp
}

/// Status dot + label (never colour alone).
pub fn status(ui: &mut Ui, p: &Palette, color: Color32, glyph: &str, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(icon(glyph, 14.0, color));
        ui.label(RichText::new(text).font(FontId::new(13.0, bold())).color(p.text));
    });
}

/// Label on the left, switch on the right.
pub fn toggle_row(ui: &mut Ui, p: &Palette, label: &str, value: &mut bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::click());
    let mut changed = false;
    if resp.clicked() { *value = !*value; changed = true; }
    ui.painter().text(Pos2::new(rect.min.x, rect.center().y), Align2::LEFT_CENTER, label, FontId::new(14.0, egui::FontFamily::Proportional), p.text);
    let sw = Rect::from_center_size(Pos2::new(rect.max.x - 25.0, rect.center().y), Vec2::new(50.0, 28.0));
    ui.painter().rect_filled(sw, CornerRadius::same(99), if *value { p.accent } else { p.line });
    let knob_x = if *value { sw.max.x - 14.0 } else { sw.min.x + 14.0 };
    ui.painter().circle_filled(Pos2::new(knob_x, sw.center().y), 10.0, if *value { p.on_accent } else { p.card });
    changed
}

/// Label + value on one line, thin slider underneath. Returns true when changed.
pub fn slider_row(ui: &mut Ui, p: &Palette, label: &str, value_text: &str, v: &mut f32, min: f32, max: f32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::click_and_drag());
    ui.painter().text(Pos2::new(rect.min.x, rect.min.y + 10.0), Align2::LEFT_CENTER, label, FontId::new(14.0, egui::FontFamily::Proportional), p.text);
    ui.painter().text(Pos2::new(rect.max.x, rect.min.y + 10.0), Align2::RIGHT_CENTER, value_text, FontId::new(13.5, bold()), p.muted);
    let track = Rect::from_min_max(Pos2::new(rect.min.x + 10.0, rect.min.y + 34.0), Pos2::new(rect.max.x - 10.0, rect.min.y + 40.0));
    let mut changed = false;
    if let (true, Some(pos)) = (resp.dragged() || resp.clicked(), resp.interact_pointer_pos()) {
        let t = ((pos.x - track.min.x) / track.width()).clamp(0.0, 1.0);
        let nv = min + t * (max - min);
        if (nv - *v).abs() > f32::EPSILON { *v = nv; changed = true; }
    }
    let t = ((*v - min) / (max - min)).clamp(0.0, 1.0);
    let x = track.min.x + track.width() * t;
    ui.painter().rect_filled(track, CornerRadius::same(3), p.line);
    ui.painter().rect_filled(Rect::from_min_max(track.min, Pos2::new(x, track.max.y)), CornerRadius::same(3), p.accent);
    ui.painter().circle_filled(Pos2::new(x, track.center().y), if resp.dragged() { 11.0 } else { 9.0 }, p.accent);
    ui.painter().circle_filled(Pos2::new(x, track.center().y), 4.0, p.on_accent);
    changed
}

/// Monospace inline code chip, used in help text.
pub fn code(text: &str, p: &Palette) -> RichText {
    RichText::new(text).font(FontId::new(13.0, egui::FontFamily::Monospace)).color(p.text).background_color(p.raised)
}
