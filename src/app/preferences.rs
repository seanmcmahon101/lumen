//! /settings: theme, the opening shortcut, search sources and folders, laid out
//! like a Windows 11 settings page.

use super::theme::{self, Appearance, Palette};
use super::*;
use crate::settings::Hotkey;
use eframe::egui::{Shape, StrokeKind, epaint::RectShape};

/// The text column inside a row, which itself starts 8 points into the panel.
const LEFT: f32 = TEXT_X - 8.0;

impl Lumen {
    pub(super) fn load_settings_draft(&mut self) {
        self.settings_draft = crate::settings::get();
        self.settings_folders = self.settings_draft.folders.join("\n");
        self.settings_exclusions = self.settings_draft.exclusions.join("\n");
        self.settings_appearance = self.appearance;
        self.settings_error.clear();
    }

    pub(super) fn draw_settings(&mut self, ui: &mut egui::Ui, bounds: Rect, p: &Palette) {
        let painter = ui.painter().clone();
        let keyboard = self.keyboard;
        let footer = Rect::from_min_max(pos2(bounds.left(), bounds.bottom() - FOOTER_H - 12.0), bounds.max);
        let body = Rect::from_min_max(pos2(bounds.left() + 8.0, bounds.top() + 1.0), pos2(bounds.right() - 8.0, footer.top()));
        let scroll = ui.scope_builder(UiBuilder::new().max_rect(body).layout(Layout::top_down(Align::Min)), |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            ui.spacing_mut().scroll = paint::scrollbar();
            egui::ScrollArea::vertical().id_salt("preferences").auto_shrink([false, false]).show(ui, |ui| {
                group(ui, p, "Appearance");
                let themes = Appearance::ALL.map(|a| (a, a.label()));
                choice(ui, p, "Theme", &mut self.settings_appearance, &themes, "theme", keyboard);
                group(ui, p, "Shortcut");
                let hotkeys = Hotkey::ALL.map(|h| (h, h.label()));
                choice(ui, p, "Open Lumen with", &mut self.settings_draft.hotkey, &hotkeys, "hotkey", keyboard);
                group(ui, p, "Search");
                let draft = &mut self.settings_draft;
                toggle(ui, p, "Apps", None, &mut draft.apps, keyboard);
                toggle(ui, p, "Files and folders", None, &mut draft.files, keyboard);
                toggle(ui, p, "Spreadsheets", Some("Workbook names, sheet names and cell text"), &mut draft.excel, keyboard);
                toggle(ui, p, "Outlook", Some("Mail that Windows Search has indexed"), &mut draft.mail, keyboard);
                toggle(ui, p, "Default locations", Some("Your profile, OneDrive and mapped network drives"), &mut draft.profile, keyboard);
                group(ui, p, "Folders");
                let note = "One full path per line, such as C:\\Work. Use a mapped drive for network folders.";
                field(ui, p, "Also search", note, &mut self.settings_folders, "C:\\Work", "folders");
                let note = "Lumen skips these folders and everything inside them.";
                field(ui, p, "Leave out", note, &mut self.settings_exclusions, "C:\\Work\\Archive", "exclusions");
            })
        });
        let scroll = scroll.inner;
        if scroll.content_size.y - scroll.state.clamped_offset().y - scroll.inner_rect.height() > 1.0 {
            let below = Rect::from_min_max(pos2(body.left(), body.bottom() - 28.0), pos2(scroll.inner_rect.right(), body.bottom()));
            paint::fade(&painter, below, p.surface);
        }

        painter.hline(bounds.x_range(), footer.top(), Stroke::new(1.0, p.line));
        let cy = footer.center().y;
        let cancel = paint::button(ui, p, pos2(footer.right() - 20.0, cy), Align2::RIGHT_CENTER, "Cancel", false, keyboard);
        let save = paint::button(ui, p, pos2(cancel.rect.left() - 8.0, cy), Align2::RIGHT_CENTER, "Save", true, keyboard);
        let (note, color) = if self.settings_error.is_empty() {
            let count = thousands(self.index.stats().0);
            let note = if self.index.is_building() { format!("Indexing · {count} items so far") } else { format!("{count} items indexed") };
            (note, p.ink2)
        } else {
            (self.settings_error.clone(), p.danger)
        };
        let note = theme::line(&note, theme::text(12.0, 400.0, color), save.rect.left() - bounds.left() - 44.0);
        paint::text(&painter, pos2(bounds.left() + 20.0, cy), Align2::LEFT_CENTER, note);
        if save.clicked() {
            self.save_settings();
        } else if cancel.clicked() {
            self.navigate(self.return_query.clone());
        }
    }

    fn save_settings(&mut self) {
        let mut draft = self.settings_draft.clone();
        draft.folders = self.settings_folders.lines().map(str::to_string).collect();
        draft.exclusions = self.settings_exclusions.lines().map(str::to_string).collect();
        if let Err(error) = draft.validate() {
            self.settings_error = error.into();
            return;
        }
        if self.settings_appearance != self.appearance {
            self.appearance = self.settings_appearance;
            self.appearance.save();
        }
        if draft == crate::settings::get() {
            // Only the look changed, so there is nothing to index again.
            self.navigate(self.return_query.clone());
            return;
        }
        crate::settings::set(draft.clone());
        self.settings_draft = draft;
        let _ = self.watch_tx.send(());
        self.reindex();
        self.navigate("/status".into());
    }
}

fn group(ui: &mut egui::Ui, p: &Palette, title: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());
    let title = theme::line(title, theme::text(12.0, 600.0, p.ink2), rect.width());
    paint::text(ui.painter(), pos2(rect.left() + LEFT, rect.bottom() - 6.0), Align2::LEFT_BOTTOM, title);
}

fn choice<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    value: &mut T,
    options: &[(T, &str)],
    salt: &str,
    keyboard: bool,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::hover());
    let cy = rect.center().y;
    paint::text(ui.painter(), pos2(rect.left() + LEFT, cy), Align2::LEFT_CENTER, theme::line(title, theme::text(14.0, 450.0, p.ink), 240.0));
    paint::segmented(ui, p, pos2(rect.right() - 12.0, cy), value, options, salt, keyboard);
}

/// A whole-row switch, as in Windows Settings: click anywhere on the row.
fn toggle(ui: &mut egui::Ui, p: &Palette, title: &str, note: Option<&str>, value: &mut bool, keyboard: bool) {
    let height = if note.is_some() { 56.0 } else { 44.0 };
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    if response.clicked() {
        *value = !*value;
    }
    let painter = ui.painter().clone();
    if response.hovered() {
        painter.rect_filled(rect, ROW_RADIUS, p.hover);
    }
    if keyboard && response.has_focus() {
        paint::focus(&painter, p, rect.shrink(2.0), ROW_RADIUS);
    }
    let cy = rect.center().y;
    let width = rect.width() - LEFT - 140.0;
    let title = painter.layout_job(theme::line(title, theme::text(14.0, 450.0, p.ink), width));
    let note = note.map(|note| painter.layout_job(theme::line(note, theme::text(12.5, 400.0, p.ink2), width)));
    let top = cy - (title.size().y + note.as_ref().map_or(0.0, |n| n.size().y)) / 2.0;
    let below = top + title.size().y;
    painter.galley(pos2(rect.left() + LEFT, top), title, p.ink);
    if let Some(note) = note {
        painter.galley(pos2(rect.left() + LEFT, below), note, p.ink2);
    }
    let switch = Rect::from_min_size(pos2(rect.right() - 12.0 - 40.0, cy - 10.0), vec2(40.0, 20.0));
    paint::switch(ui, p, switch, *value, response.id);
    let state = theme::line(if *value { "On" } else { "Off" }, theme::text(13.0, 400.0, p.ink2), 40.0);
    paint::text(&painter, pos2(switch.left() - 12.0, cy), Align2::RIGHT_CENTER, state);
}

/// A titled text area of full paths, one per line.
fn field(ui: &mut egui::Ui, p: &Palette, title: &str, note: &str, text: &mut String, hint: &str, salt: &str) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 44.0), Sense::hover());
    let painter = ui.painter().clone();
    let title = painter.layout_job(theme::line(title, theme::text(14.0, 450.0, p.ink), width - LEFT - 12.0));
    let note = painter.layout_job(theme::line(note, theme::text(12.5, 400.0, p.ink2), width - LEFT - 12.0));
    let below = rect.top() + 4.0 + title.size().y;
    painter.galley(pos2(rect.left() + LEFT, rect.top() + 4.0), title, p.ink);
    painter.galley(pos2(rect.left() + LEFT, below), note, p.ink2);
    let frame = painter.add(Shape::Noop);
    let output = ui
        .horizontal(|ui| {
            ui.add_space(LEFT);
            TextEdit::multiline(text)
                .id_salt(salt)
                .frame(egui::Frame::NONE)
                .margin(Margin::symmetric(10, 8))
                .desired_rows(2)
                .desired_width(width - LEFT - 12.0)
                .font(FontId::proportional(13.0))
                .text_color(p.ink)
                .hint_text(LayoutJob::single_section(hint.into(), theme::text(13.0, 400.0, p.ink3)))
                .show(ui)
        })
        .inner;
    let stroke = if output.response.has_focus() { Stroke::new(1.5, p.accent) } else { Stroke::new(1.0, p.key_edge) };
    painter.set(frame, Shape::Rect(RectShape::new(output.response.rect, 6, p.field, stroke, StrokeKind::Inside)));
    ui.add_space(12.0);
}
