//! Painted pieces of the launcher: the lit backdrop, the panel, keys and controls.

use super::theme::{self, Palette};
use super::{G_RETURN, RADIUS, ROW_RADIUS};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontFamily, FontId, Id, Mesh, Painter, Pos2, Rect, Response, Sense, Shadow, Shape, Stroke,
    StrokeKind, TextFormat, TextureId, Ui, Vec2, epaint::Vertex, pos2, text::LayoutJob, vec2,
};

/// A slim scrollbar handle in space of its own, so it never covers a row's label.
pub fn scrollbar() -> egui::style::ScrollStyle {
    egui::style::ScrollStyle {
        bar_width: 6.0,
        bar_inner_margin: 4.0,
        bar_outer_margin: 0.0,
        handle_min_length: 28.0,
        foreground_color: false,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.0,
        dormant_handle_opacity: 0.6,
        active_handle_opacity: 0.8,
        interact_handle_opacity: 1.0,
        ..egui::style::ScrollStyle::solid()
    }
}

/// Lays out `job` and paints it with its `anchor` point at `pos`.
pub fn text(painter: &Painter, pos: Pos2, anchor: Align2, job: LayoutJob) -> Rect {
    let galley = painter.layout_job(job);
    let rect = anchor.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, Color32::PLACEHOLDER);
    rect
}

/// The desktop behind Lumen, dimmed like a room lit only by the panel: brightest
/// beside it and falling off towards the edges. `open` runs from 0 to 1 as it dims.
pub fn backdrop(painter: &Painter, texture: TextureId, screen: Rect, panel: Rect, p: &Palette, open: f32) {
    const COLS: u32 = 48;
    const ROWS: u32 = 28;
    let mut mesh = Mesh::with_texture(texture);
    for row in 0..=ROWS {
        for col in 0..=COLS {
            let (u, v) = (col as f32 / COLS as f32, row as f32 / ROWS as f32);
            let pos = pos2(screen.left() + u * screen.width(), screen.top() + v * screen.height());
            let dx = (panel.left() - pos.x).max(pos.x - panel.right()).max(0.0);
            let dy = (panel.top() - pos.y).max(pos.y - panel.bottom()).max(0.0);
            let spill = (-dx.hypot(dy) / 320.0).exp();
            let level = p.dim_far + (p.dim_near - p.dim_far) * spill;
            let channel = |tint: f32| {
                let lit = level * tint;
                (255.0 * (1.0 + (lit - 1.0) * open)).round().clamp(0.0, 255.0) as u8
            };
            let color = Color32::from_rgb(channel(p.tint[0]), channel(p.tint[1]), channel(p.tint[2]));
            mesh.vertices.push(Vertex { pos, uv: pos2(u, v), color });
        }
    }
    let stride = COLS + 1;
    for row in 0..ROWS {
        for col in 0..COLS {
            let i = row * stride + col;
            mesh.add_triangle(i, i + 1, i + stride);
            mesh.add_triangle(i + 1, i + stride + 1, i + stride);
        }
    }
    painter.add(Shape::mesh(mesh));
}

/// Fades the content under `rect` into `color`, towards the bottom: there is more below.
pub fn fade(painter: &Painter, rect: Rect, color: Color32) {
    let mut mesh = Mesh::default();
    for (pos, color) in [(rect.left_top(), Color32::TRANSPARENT), (rect.right_top(), Color32::TRANSPARENT), (rect.left_bottom(), color), (rect.right_bottom(), color)] {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(Shape::mesh(mesh));
}

/// The panel and the light it gives off.
pub fn panel(painter: &Painter, rect: Rect, p: &Palette, open: f32) {
    let radius = CornerRadius::same(RADIUS);
    for (blur, spread, alpha) in p.glow {
        let color = Color32::from_white_alpha((alpha as f32 * open).round() as u8);
        painter.add(Shadow { offset: [0, 0], blur, spread, color }.as_shape(rect, radius));
    }
    painter.rect(rect, radius, p.surface, Stroke::new(1.0, p.edge), StrokeKind::Outside);
    if p.dark {
        // Light catching the top edge.
        let inset = RADIUS as f32;
        painter.hline(rect.left() + inset..=rect.right() - inset, rect.top() + 0.5, Stroke::new(1.0, Color32::from_white_alpha(20)));
    }
}

/// The selected row, lit with the accent and marked with Windows' selection pill.
pub fn selection(painter: &Painter, p: &Palette, row: Rect) {
    painter.rect_filled(row, ROW_RADIUS, p.selected);
    let pill = Rect::from_center_size(pos2(row.left() + 3.5, row.center().y), vec2(3.0, 18.0));
    painter.rect_filled(pill, 1.5, p.accent);
}

/// A key such as "Esc" or "Ctrl K"; "Enter" is drawn as its return arrow.
pub fn keycap(painter: &Painter, p: &Palette, pos: Pos2, anchor: Align2, key: &str) -> Rect {
    let job = if key == "Enter" {
        LayoutJob::single_section(G_RETURN.to_string(), TextFormat::simple(theme::icon(11.0), p.ink2))
    } else {
        theme::line(key, theme::text(11.0, 600.0, p.ink2), f32::INFINITY)
    };
    let galley = painter.layout_job(job);
    let rect = anchor.anchor_size(pos, vec2((galley.size().x + 12.0).max(22.0), 20.0));
    painter.rect(rect, 5, p.key_fill, Stroke::new(1.0, p.key_edge), StrokeKind::Inside);
    painter.galley(rect.center() - galley.size() / 2.0, galley, p.ink2);
    rect
}

/// A Fluent glyph on a soft tile, used where a result has no shell icon.
pub fn tile(painter: &Painter, p: &Palette, center: Pos2, glyph: char, lit: bool) {
    let rect = Rect::from_center_size(center, Vec2::splat(32.0));
    painter.rect_filled(rect, 8, if lit { p.tile_lit } else { p.tile });
    painter.text(center, Align2::CENTER_CENTER, glyph, theme::icon(16.0), if lit { p.accent } else { p.ink2 });
}

/// The search bar's leading glyph, which shows how Lumen reads the query.
pub fn mode(painter: &Painter, p: &Palette, center: Pos2, glyph: Option<char>, active: bool) {
    let color = if active { p.accent } else { p.ink2 };
    match glyph {
        Some(glyph) => {
            painter.text(center, Align2::CENTER_CENTER, glyph, theme::icon(20.0), color);
        }
        // The command list: a slash on a key.
        None => {
            let key = Rect::from_center_size(center, Vec2::splat(22.0));
            painter.rect_stroke(key, 6, Stroke::new(1.5, color), StrokeKind::Inside);
            painter.text(center + vec2(0.5, 0.0), Align2::CENTER_CENTER, "/", FontId::new(13.0, FontFamily::Proportional), color);
        }
    }
}

/// A Fluent toggle switch in `rect` (40 × 20).
pub fn switch(ui: &Ui, p: &Palette, rect: Rect, on: bool, id: Id) {
    let t = ui.ctx().animate_bool_with_time(id, on, 0.12);
    let painter = ui.painter();
    let track = theme::mix(p.field, p.accent, t);
    let stroke = Stroke::new(1.0, theme::mix(p.ink2, p.accent, t));
    painter.rect(rect, CornerRadius::same(10), track, stroke, StrokeKind::Inside);
    let x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
    painter.circle_filled(pos2(x, rect.center().y), 5.5 + t * 0.5, theme::mix(p.ink2, p.on_accent, t));
}

/// A 2-point focus outline, drawn only while the keyboard is driving.
pub fn focus(painter: &Painter, p: &Palette, rect: Rect, radius: u8) {
    painter.rect_stroke(rect.expand(2.0), radius + 2, Stroke::new(2.0, p.ink), StrokeKind::Outside);
}

/// A row of choices, one selected, right-aligned at `right_center`.
pub fn segmented<T: Copy + PartialEq>(
    ui: &Ui,
    p: &Palette,
    right_center: Pos2,
    value: &mut T,
    options: &[(T, &str)],
    salt: &str,
    keyboard: bool,
) -> Rect {
    let painter = ui.painter().clone();
    let galleys: Vec<_> = options
        .iter()
        .map(|(option, label)| {
            let weight = if *option == *value { 600.0 } else { 500.0 };
            painter.layout_job(theme::line(label, theme::text(12.5, weight, Color32::PLACEHOLDER), f32::INFINITY))
        })
        .collect();
    let widths: Vec<f32> = galleys.iter().map(|g| g.size().x + 24.0).collect();
    let outer = Align2::RIGHT_CENTER.anchor_size(right_center, vec2(widths.iter().sum::<f32>() + 4.0, 32.0));
    painter.rect(outer, 8, p.field, Stroke::new(1.0, p.line), StrokeKind::Inside);
    let mut x = outer.left() + 2.0;
    for (index, ((option, _), galley)) in options.iter().zip(galleys).enumerate() {
        let rect = Rect::from_min_size(pos2(x, outer.top() + 2.0), vec2(widths[index], 28.0));
        let response = ui.interact(rect, Id::unique((salt, index)), Sense::click());
        let active = *option == *value;
        if active {
            painter.rect(rect, 6, p.segment, Stroke::new(1.0, p.line), StrokeKind::Inside);
        } else if response.hovered() {
            painter.rect_filled(rect, 6, p.hover);
        }
        let color = if active || response.hovered() { p.ink } else { p.ink2 };
        painter.galley(rect.center() - galley.size() / 2.0, galley, color);
        if keyboard && response.has_focus() {
            focus(&painter, p, rect, 6);
        }
        if response.clicked() {
            *value = *option;
        }
        x += widths[index];
    }
    outer
}

/// A push button; the primary one is filled with the accent.
pub fn button(ui: &Ui, p: &Palette, pos: Pos2, anchor: Align2, label: &str, primary: bool, keyboard: bool) -> Response {
    let painter = ui.painter().clone();
    let weight = if primary { 600.0 } else { 500.0 };
    let galley = painter.layout_job(theme::line(label, theme::text(13.0, weight, Color32::PLACEHOLDER), f32::INFINITY));
    let rect = anchor.anchor_size(pos, vec2((galley.size().x + 32.0).max(88.0), 32.0));
    let response = ui.interact(rect, Id::unique(("lumen-button", label)), Sense::click());
    let pressed = response.is_pointer_button_down_on();
    let (fill, stroke, color) = if primary {
        (if response.hovered() || pressed { p.accent_hover } else { p.accent }, Stroke::NONE, p.on_accent)
    } else {
        (if pressed { p.press } else if response.hovered() { p.hover } else { p.surface }, Stroke::new(1.0, p.key_edge), p.ink)
    };
    painter.rect(rect, 6, fill, stroke, StrokeKind::Inside);
    painter.galley(rect.center() - galley.size() / 2.0, galley, color);
    if keyboard && response.has_focus() {
        focus(&painter, p, rect, 6);
    }
    response
}
