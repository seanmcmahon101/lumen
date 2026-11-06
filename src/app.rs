//! The overlay: dimmed screen, glowing search bar, results.

use crate::index::{Entry, Index, Kind};
use crate::{calc, win};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Id, Key, Layout, Margin, Modifiers, Pos2,
    Rect, RichText, Sense, Shadow, TextEdit, TextureHandle, TextureOptions, UiBuilder, Vec2, pos2,
    text::{CCursor, CCursorRange, LayoutJob, TextFormat, TextWrapping},
    vec2,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const TEXT: Color32 = Color32::from_rgb(0x16, 0x18, 0x1C);
const SUBTLE: Color32 = Color32::from_rgb(0x63, 0x69, 0x73);
const HINT: Color32 = Color32::from_rgb(0x68, 0x6E, 0x78);
const GHOST: Color32 = Color32::from_rgb(0xB4, 0xB8, 0xBF);
const SELECTED: Color32 = Color32::from_rgb(0xEB, 0xEE, 0xF3);
const HOVER: Color32 = Color32::from_rgb(0xF4, 0xF5, 0xF8);
const DIVIDER: Color32 = Color32::from_rgb(0xEC, 0xED, 0xF0);
const GLYPH_BG: Color32 = Color32::from_rgb(0xF0, 0xF1, 0xF4);

const WIDTH: f32 = 760.0;
const BAR_H: f32 = 80.0;
const FILTER_H: f32 = 44.0;
const TEXT_INSET: f32 = 68.0;
const SEARCH_SIZE: f32 = 26.0;
const ROW_H: f32 = 64.0;
const LIST_PAD: f32 = 6.0;
const FOOTER_H: f32 = 40.0;
const MAX_ROWS: usize = 8;
const RADIUS: u8 = 18;

// Segoe Fluent Icons code points
const G_SEARCH: char = '\u{E721}';
const G_CALC: char = '\u{E8EF}';
const G_GLOBE: char = '\u{E774}';
const G_POWER: char = '\u{E7E8}';
const G_SETTINGS: char = '\u{E713}';
const G_OPEN: char = '\u{E8A7}';

enum Action {
    Open(String),
    Copy(String),
    Quit,
    ToggleStartup,
}

struct Item {
    title: String,
    subtitle: String,
    label: String,
    glyph: Option<char>,
    icon: Option<(String, String)>, // (cache key, shell path)
    action: Action,
    reveal: Option<String>,
    admin: bool,
    copy: Option<String>,
    complete: Option<String>, // what Tab puts in the bar
    history_key: Option<String>,
}

#[derive(Clone, Copy)]
enum Mode {
    Open,
    Reveal,
    Admin,
}

pub struct Lumen {
    query: String,
    last_query: Option<String>,
    last_generation: u64,
    results: Vec<Item>,
    selected: usize,
    selection_by_user: bool,
    index: Arc<Index>,
    history: win::History,
    icons: HashMap<String, Option<TextureHandle>>,
    icon_tx: mpsc::Sender<(String, String)>,
    icon_rx: mpsc::Receiver<(String, Option<egui::ColorImage>)>,
    pending_icons: HashSet<String>,
    seen_focus: bool,
    ghost_shown: bool,
    startup: bool,
    profile: String,
    backdrop: Option<TextureHandle>,
    display_overlay: win::DisplayOverlay,
    update_rx: mpsc::Receiver<crate::update::Update>,
    available_update: Option<crate::update::Update>,
    pinned: bool, // --show: stay open when focus leaves (testing)
    frames: u32,
    search_tx: mpsc::Sender<(String, u64, win::History)>,
    search_rx: mpsc::Receiver<(String, u64, Vec<Entry>, String)>,
    search_key: Option<(String, u64)>,
    found: Vec<Entry>,
    searching: bool,
    search_note: String,
    latest_query: Arc<Mutex<String>>,
}

impl Lumen {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        if let Ok(h) = cc.window_handle()
            && let RawWindowHandle::Win32(w) = h.as_raw()
        {
            win::set_hwnd(w.hwnd.get());
        }
        let ctx = cc.egui_ctx.clone();
        let _ = win::CTX.set(ctx.clone());
        setup_style(&ctx);
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        }

        let index = Arc::new(Index::default());
        let (icon_tx, icon_requests) = mpsc::channel::<(String, String)>();
        let (icon_answers, icon_rx) = mpsc::channel();
        let icon_ctx = ctx.clone();
        std::thread::spawn(move || {
            let initialized =
                unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED).is_ok() };
            while let Ok((key, path)) = icon_requests.recv() {
                let icon = win::load_icon(&path);
                if icon_answers.send((key, icon)).is_err() {
                    break;
                }
                icon_ctx.request_repaint();
            }
            if initialized {
                unsafe {
                    windows::Win32::System::Com::CoUninitialize();
                }
            }
        });
        let repaint = ctx.clone();
        index.rebuild(move || repaint.request_repaint());
        let (search_tx, requests) = mpsc::channel::<(String, u64, win::History)>();
        let (answers, search_rx) = mpsc::channel();
        let latest_query = Arc::new(Mutex::new(String::new()));
        let worker_query = latest_query.clone();
        let (search_index, search_ctx) = (index.clone(), ctx.clone());
        std::thread::spawn(move || {
            while let Ok(mut request) = requests.recv() {
                std::thread::sleep(Duration::from_millis(70));
                while let Ok(newer) = requests.try_recv() {
                    request = newer;
                }
                let (q, generation, history) = request;
                let mut found = search_index.search(&q, MAX_ROWS - 1, |p| history.uses(p));
                let mut note = String::new();
                if let Some(term) = crate::mail::query(&q) {
                    match crate::mail::search(term, || *worker_query.lock().unwrap() != q) {
                        Ok(mail) => {
                            note = if mail.is_empty() {
                                "No indexed Outlook matches · try Outlook online"
                            } else {
                                "Outlook results from this PC"
                            }
                            .into();
                            found.splice(0..0, mail);
                        }
                        Err(_) => note = "Outlook index unavailable · try Outlook online".into(),
                    }
                }
                // What you last opened after typing this goes first, even if it doesn't match.
                if let Some(path) = history.pick(&q) {
                    let learned = match found.iter().position(|e| e.path == path) {
                        Some(i) => Some(found.remove(i)),
                        None => search_index.get(path),
                    };
                    if let Some(e) = learned {
                        found.insert(0, e);
                    }
                }
                if answers.send((q, generation, found, note)).is_err() {
                    break;
                }
                search_ctx.request_repaint();
            }
        });
        // Refresh the index every 15 minutes.
        let (idx, c) = (index.clone(), ctx.clone());
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(15 * 60));
                let c2 = c.clone();
                idx.rebuild(move || c2.request_repaint());
            }
        });

        crate::hook::spawn(win::toggle);

        // `lumen --show "query"` opens straight away (handy for testing).
        let args: Vec<String> = std::env::args().collect();
        let mut query = String::new();
        let pinned = args.iter().any(|a| a == "--show");
        if let Some(i) = args.iter().position(|a| a == "--show") {
            query = args.get(i + 1).cloned().unwrap_or_default();
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(1500));
                win::show();
            });
        }

        Self {
            query,
            last_query: None,
            last_generation: 0,
            results: Vec::new(),
            selected: 0,
            selection_by_user: false,
            index,
            history: win::History::load(),
            icons: HashMap::new(),
            icon_tx,
            icon_rx,
            pending_icons: HashSet::new(),
            seen_focus: false,
            ghost_shown: false,
            startup: win::startup_enabled(),
            profile: std::env::var("USERPROFILE").unwrap_or_default(),
            backdrop: None,
            display_overlay: win::DisplayOverlay::default(),
            update_rx: crate::update::watch(ctx.clone()),
            available_update: None,
            pinned,
            frames: 0,
            search_tx,
            search_rx,
            search_key: None,
            found: Vec::new(),
            searching: false,
            search_note: String::new(),
            latest_query,
        }
    }

    fn query_id() -> Id {
        Id::unique("lumen-query")
    }

    /// (caret is at the end, text is selected)
    fn caret(&self, ctx: &egui::Context) -> (bool, bool) {
        let len = self.query.chars().count();
        TextEdit::load_state(ctx, Self::query_id())
            .and_then(|s| s.cursor.char_range())
            .map_or((true, false), |r| (r.primary.index.0 == len && r.secondary.index.0 == len, r.primary.index != r.secondary.index))
    }

    fn set_cursor(&self, ctx: &egui::Context, select_all: bool) {
        let len = self.query.chars().count();
        let mut state = TextEdit::load_state(ctx, Self::query_id()).unwrap_or_default();
        let range = if select_all { CCursorRange::two(CCursor::new(0), CCursor::new(len)) } else { CCursorRange::one(CCursor::new(len)) };
        state.cursor.set_char_range(Some(range));
        state.store(ctx, Self::query_id());
    }

    fn hide(&mut self) {
        self.display_overlay.close();
        win::hide();
    }

    fn refresh(&mut self) {
        let same_query = self.search_key.as_ref().is_some_and(|(query, _)| query == &self.query);
        let previous_selection = (same_query && self.selection_by_user)
            .then(|| self.results.get(self.selected).map(|item| (item.title.clone(), item.subtitle.clone())))
            .flatten();
        if !same_query {
            self.selection_by_user = false;
        }
        self.last_query = Some(self.query.clone());
        self.last_generation = self.index.generation();
        self.results.clear();
        self.selected = 0;
        let t = self.query.trim().to_string();
        let key = (self.query.clone(), self.last_generation);
        if self.search_key.as_ref() != Some(&key) {
            *self.latest_query.lock().unwrap() = self.query.clone();
            if self.search_key.as_ref().is_none_or(|previous| previous.0 != key.0) {
                self.found.clear();
                self.search_note.clear();
            }
            self.search_key = Some(key.clone());
            self.searching = !t.is_empty();
            let _ = self.search_tx.send((key.0, key.1, self.history.clone()));
        }
        if t.is_empty() {
            return;
        }
        let lower = t.to_lowercase();

        if let Some(v) = calc::eval(&t) {
            let (display, raw) = calc::format(v);
            self.results.push(Item {
                title: display,
                subtitle: format!("{t} ="),
                label: "Calculator".into(),
                glyph: Some(G_CALC),
                icon: None,
                action: Action::Copy(raw.clone()),
                reveal: None,
                admin: false,
                copy: Some(raw.clone()),
                complete: Some(raw),
                history_key: None,
            });
        }

        if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("www.") {
            let url = if lower.starts_with("www.") { format!("https://{t}") } else { t.clone() };
            self.results.push(simple(&format!("Open {t}"), "Website", "Web", G_GLOBE, Action::Open(url)));
        } else if t.len() >= 3 && t.as_bytes()[1] == b':' {
            let mut it = simple(&t, "Open this location", "Path", G_OPEN, Action::Open(t.clone()));
            it.reveal = Some(t.clone());
            it.copy = Some(t.clone());
            self.results.push(it);
        }

        let mut commands = Vec::new();
        if lower.chars().count() >= 3 {
            let startup_title = if self.startup { "Stop starting Lumen at login" } else { "Start Lumen at login" };
            for (title, glyph, action) in [("Quit Lumen", G_POWER, Action::Quit), (startup_title, G_SETTINGS, Action::ToggleStartup)] {
                if title.to_lowercase().split_whitespace().any(|w| w.starts_with(&lower)) || title.to_lowercase().contains(&lower) {
                    commands.push(simple(title, "Lumen", "Command", glyph, action));
                }
            }
        }

        let room = MAX_ROWS.saturating_sub(self.results.len() + commands.len() + 1);
        for e in self.found.iter().take(room).cloned() {
            let item = self.entry_item(e);
            self.results.push(item);
        }
        self.results.extend(commands);

        if let Some(term) = crate::mail::query(&t) {
            let url = format!("https://outlook.office.com/mail/0/search?q={}", url_encode(term));
            self.results.push(simple(
                "Search Outlook online",
                "Microsoft 365 · opens in your browser",
                "Outlook",
                G_GLOBE,
                Action::Open(url),
            ));
        } else {
            let url = format!("https://www.google.com/search?q={}", url_encode(&t));
            self.results.push(simple(&format!("Search the web for \u{201C}{t}\u{201D}"), "Google", "Web", G_SEARCH, Action::Open(url)));
        }
        if let Some((title, subtitle)) = previous_selection {
            self.selected = self.results.iter().position(|item| item.title == title && item.subtitle == subtitle).unwrap_or(0);
        }
    }

    fn entry_item(&self, e: Entry) -> Item {
        let is_app = e.kind == Kind::App;
        let ext = std::path::Path::new(&e.name).extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
        let title = if matches!(ext.as_str(), "lnk" | "url") && e.kind == Kind::File {
            e.name[..e.name.len() - ext.len() - 1].to_string()
        } else {
            e.name.clone()
        };
        let subtitle = if e.kind == Kind::Email {
            e.detail.clone()
        } else if !e.detail.is_empty() {
            format!("{} · {}", e.detail, e.path)
        } else if is_app {
            "Application".to_string()
        } else {
            let parent = std::path::Path::new(&e.path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            match parent.strip_prefix(&self.profile) {
                Some("") => "Home".into(),
                Some(rest) => format!("Home{rest}"),
                None => parent,
            }
        };
        let label = match e.kind {
            Kind::App => "App".into(),
            Kind::Folder => "Folder".into(),
            Kind::Email => "Outlook".into(),
            Kind::File if !ext.is_empty() && ext.len() <= 5 => ext.to_uppercase(),
            Kind::File => "File".into(),
        };
        let key = match e.kind {
            Kind::Folder => "<folder>".to_string(),
            Kind::App => e.path.clone(),
            Kind::Email => "<email>".into(),
            Kind::File if matches!(ext.as_str(), "" | "exe" | "lnk" | "url" | "ico" | "appref-ms") => e.path.clone(),
            Kind::File => format!("ext:{ext}"),
        };
        Item {
            title,
            subtitle,
            label,
            glyph: (e.kind == Kind::Email).then_some('\u{E715}'),
            icon: (e.kind != Kind::Email).then(|| (key, e.path.clone())),
            action: Action::Open(e.path.clone()),
            reveal: (!is_app && e.kind != Kind::Email).then(|| e.path.clone()),
            admin: is_app || ext == "exe",
            copy: (!is_app).then(|| e.path.clone()),
            complete: Some(e.name.clone()),
            history_key: Some(e.path),
        }
    }

    fn completion(&self) -> Option<String> {
        let item = self.results.get(self.selected)?;
        let c = item.complete.as_ref()?;
        let (_, q) = crate::index::split_filter(&self.query);
        if q.is_empty() {
            return None;
        }
        if c.is_char_boundary(q.len()) && c[..q.len()].eq_ignore_ascii_case(q) {
            return (c.len() > q.len()).then(|| c[q.len()..].to_string());
        }
        // A learned pick whose name doesn't start with what was typed ("jd").
        (item.history_key.is_some() && item.history_key.as_deref() == self.history.pick(&self.query)).then(|| format!(" — {c}"))
    }

    fn accept_completion(&mut self, ctx: &egui::Context) {
        if let Some(c) = self.results.get(self.selected).and_then(|i| i.complete.clone()) {
            let (filter, _) = crate::index::split_filter(&self.query);
            self.query = format!("{filter}{c}");
            self.set_cursor(ctx, false);
        }
    }

    fn execute(&mut self, i: usize, mode: Mode) {
        if i >= self.results.len() {
            return;
        }
        self.hide();
        let item = &self.results[i];
        match (mode, &item.reveal, &item.action) {
            (Mode::Reveal, Some(path), _) => win::reveal(path),
            (Mode::Admin, _, Action::Open(target)) if item.admin => {
                win::shell_open(target, "runas", None);
            }
            (_, _, Action::Open(target)) if target.starts_with("lumen-outlook:") => {
                crate::mail::open(target);
            }
            (_, _, Action::Open(target)) => {
                win::shell_open(target, "open", None);
            }
            (_, _, Action::Copy(text)) => win::copy(text),
            (_, _, Action::Quit) => std::process::exit(0),
            (_, _, Action::ToggleStartup) => {
                self.startup = !self.startup;
                win::set_startup(self.startup);
            }
        }
        if let Some(k) = item.history_key.clone() {
            self.history.bump(&k);
            self.history.learn(&self.query, &k);
        }
        self.search_key = None;
        self.last_query = None; // re-rank next time
    }

    fn icon(&mut self, ctx: &egui::Context, key: &str, path: &str, budget: &mut u32) -> Option<TextureHandle> {
        if let Some(t) = self.icons.get(key) {
            return t.clone();
        }
        if self.pending_icons.contains(key) {
            return None;
        }
        if *budget == 0 {
            ctx.request_repaint();
            return None;
        }
        *budget -= 1;
        if self.pending_icons.insert(key.to_string()) {
            let _ = self.icon_tx.send((key.to_string(), path.to_string()));
        }
        None
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let n = self.results.len();
        let (at_end, has_selection) = self.caret(ctx);
        let ghost = self.ghost_shown && at_end && !has_selection;
        let mut run = None;
        let mut complete = false;
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                run = Some(None);
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) && n > 0 {
                self.selection_by_user = true;
                self.selected = (self.selected + 1).min(n - 1);
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                self.selection_by_user = true;
                self.selected = self.selected.saturating_sub(1);
            }
            if i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Enter) {
                run = Some(Some(Mode::Admin));
            } else if i.consume_key(Modifiers::CTRL, Key::Enter) {
                run = Some(Some(Mode::Reveal));
            } else if i.consume_key(Modifiers::NONE, Key::Enter) {
                run = Some(Some(Mode::Open));
            }
            if i.consume_key(Modifiers::NONE, Key::Tab) || (ghost && i.consume_key(Modifiers::NONE, Key::ArrowRight)) {
                complete = true;
            }
            // Ctrl+C with nothing selected in the bar copies the result instead.
            if !has_selection && i.events.iter().any(|e| matches!(e, egui::Event::Copy)) {
                if let Some(text) = self.results.get(self.selected).and_then(|r| r.copy.clone()) {
                    win::copy(&text);
                }
            }
        });
        if complete {
            self.accept_completion(ctx);
        }
        match run {
            Some(None) => self.hide(),
            Some(Some(mode)) if n > 0 => self.execute(self.selected, mode),
            _ => {}
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let screen = ui.max_rect();
        let painter = ui.painter().clone();
        match &self.backdrop {
            Some(tex) => painter.image(
                tex.id(),
                screen,
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::from_gray(win::BACKDROP_BRIGHTNESS),
            ),
            None => painter.rect_filled(screen, 0, Color32::from_gray(24)),
        };

        let available_rows =
            ((screen.height() - 32.0 - BAR_H - FILTER_H - FOOTER_H - LIST_PAD * 2.0 - 1.0) / ROW_H).floor().max(1.0) as usize;
        let rows = self.results.len().min(available_rows);
        // Keep keyboard selection visible on short displays.
        let first_row = self.selected.saturating_sub(rows.saturating_sub(1));
        let w = WIDTH.min(screen.width() - 40.0);
        let list_h = |rows: usize| if rows > 0 { 1.0 + LIST_PAD * 2.0 + rows as f32 * ROW_H + FOOTER_H } else { 48.0 };
        let h = BAR_H + FILTER_H + list_h(rows);
        // Pin the top where a full list would be centred, so the bar stays still as results change.
        let full_h = BAR_H + FILTER_H + list_h(MAX_ROWS.min(available_rows));
        let panel = Rect::from_min_size(pos2(screen.center().x - w / 2.0, screen.center().y - full_h / 2.0), vec2(w, h));

        // Click on the dimmed area closes.
        if ui.interact(screen, Id::unique("dim"), Sense::click()).clicked()
            && !ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| panel.contains(p))
        {
            self.hide();
            return;
        }

        // Glow: stacked white layers, from a wide aura that lifts the backdrop to a hot rim at the edge.
        let r = CornerRadius::same(RADIUS);
        for (blur, spread, alpha) in [(255, 64, 72), (160, 28, 140), (64, 10, 220), (18, 3, 255)] {
            painter.add(Shadow { offset: [0, 0], blur, spread, color: Color32::from_white_alpha(alpha) }.as_shape(panel, r));
        }
        painter.rect_filled(panel, r, Color32::WHITE);

        // Search bar
        let bar = Rect::from_min_size(panel.min, vec2(w, BAR_H));
        painter.text(pos2(bar.left() + 36.0, bar.center().y), Align2::CENTER_CENTER, G_SEARCH, icon_font(22.0), SUBTLE);
        let text_rect = Rect::from_min_max(pos2(bar.left() + TEXT_INSET, bar.top()), pos2(bar.right() - 24.0, bar.bottom()));
        let font = FontId::proportional(SEARCH_SIZE);
        let output = ui
            .scope_builder(UiBuilder::new().max_rect(text_rect).layout(Layout::left_to_right(Align::Center)), |ui| {
                TextEdit::singleline(&mut self.query)
                    .id(Self::query_id())
                    .frame(egui::Frame::NONE)
                    .margin(Margin::ZERO)
                    .font(font.clone())
                    .text_color(TEXT)
                    .hint_text(RichText::new("Find a file, spreadsheet, or email").font(font.clone()).color(HINT))
                    .desired_width(text_rect.width())
                    .lock_focus(true)
                    .show(ui)
            })
            .inner;
        if !output.response.has_focus() {
            output.response.request_focus();
        }

        // Inline autocomplete: grey remainder of the selected result after the caret.
        let (at_end, has_selection) = self.caret(&ctx);
        self.ghost_shown = false;
        if at_end
            && !has_selection
            && let Some(rest) = self.completion()
        {
            let pos = output.galley_pos + vec2(output.galley.size().x, 0.0);
            painter.with_clip_rect(text_rect).text(pos, Align2::LEFT_TOP, rest, font, GHOST);
            self.ghost_shown = true;
        }

        let filters = Rect::from_min_size(pos2(panel.left() + 18.0, bar.bottom()), vec2(w - 36.0, FILTER_H));
        ui.scope_builder(UiBuilder::new().max_rect(filters).layout(Layout::left_to_right(Align::Center)), |ui| {
            for (label, prefix) in [("All", ""), ("Spreadsheets", "excel "), ("Outlook", "mail ")] {
                let active = if prefix.is_empty() {
                    !self.query.starts_with("excel ") && crate::mail::query(&self.query).is_none()
                } else {
                    self.query.starts_with(prefix)
                };
                if ui.selectable_label(active, RichText::new(label).size(16.0)).clicked() {
                    let term =
                        self.query.strip_prefix("excel ").or_else(|| crate::mail::query(&self.query)).unwrap_or(&self.query).to_string();
                    self.query = format!("{prefix}{term}");
                    self.set_cursor(&ctx, false);
                }
            }
            let status = if self.searching && self.found.is_empty() {
                Some("Searching…")
            } else if self.query.trim().is_empty() && self.index.is_building() {
                Some("Indexing…")
            } else {
                None
            };
            if let Some(release) = &self.available_update {
                let response = ui
                    .with_layout(Layout::right_to_left(Align::Center), |ui| ui.link(RichText::new("Update available").size(14.0)))
                    .inner
                    .on_hover_text(format!("View Lumen {} on GitHub", release.version));
                if response.clicked() {
                    let url = release.release_url();
                    self.hide();
                    win::shell_open(&url, "open", None);
                }
            } else if let Some(status) = status {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(status).size(14.0).color(SUBTLE));
                });
            }
        });
        if rows == 0 {
            painter.text(
                pos2(panel.left() + TEXT_INSET, panel.bottom() - 24.0),
                Align2::LEFT_CENTER,
                "Try excel budget, a sheet name, or mail invoice",
                FontId::proportional(15.0),
                SUBTLE,
            );
            return;
        }
        painter.hline(panel.x_range(), bar.bottom() + FILTER_H, (1.0, DIVIDER));

        let mut budget = 3; // icon loads per frame
        let mut clicked = None;
        let mut y = bar.bottom() + FILTER_H + 1.0 + LIST_PAD;
        for i in first_row..first_row + rows {
            let row = Rect::from_min_size(pos2(panel.left() + 8.0, y), vec2(w - 16.0, ROW_H));
            y += ROW_H;
            let resp = ui.interact(row, Id::unique(("row", i)), Sense::click());
            resp.clone().on_hover_text(&self.results[i].subtitle);
            if i == self.selected {
                painter.rect_filled(row, 10, SELECTED);
            } else if resp.hovered() {
                painter.rect_filled(row, 10, HOVER);
            }
            if resp.clicked() {
                clicked = Some(i);
            }

            let icon_rect = Rect::from_center_size(pos2(row.left() + 28.0, row.center().y), Vec2::splat(32.0));
            let (glyph, icon) = (self.results[i].glyph, self.results[i].icon.clone());
            if let Some(g) = glyph {
                painter.rect_filled(icon_rect, 7, GLYPH_BG);
                painter.text(icon_rect.center(), Align2::CENTER_CENTER, g, icon_font(18.0), TEXT);
            } else if let Some((key, path)) = icon
                && let Some(tex) = self.icon(&ctx, &key, &path, &mut budget)
            {
                painter.image(tex.id(), icon_rect, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
            }

            let item = &self.results[i];
            let label =
                painter.text(pos2(row.right() - 14.0, row.center().y), Align2::RIGHT_CENTER, &item.label, FontId::proportional(14.0), HINT);
            let text_x = panel.left() + TEXT_INSET;
            let max_w = label.left() - text_x - 16.0;
            let title = painter.layout_job(single_line(&item.title, 19.0, TEXT, max_w));
            let sub = painter.layout_job(single_line(&item.subtitle, 15.0, SUBTLE, max_w));
            let top = row.center().y - (title.size().y + 3.0 + sub.size().y) / 2.0;
            painter.galley(pos2(text_x, top), title.clone(), TEXT);
            painter.galley(pos2(text_x, top + title.size().y + 3.0), sub, SUBTLE);
        }

        // Key hints
        let mut x = panel.left() + 22.0;
        let fy = panel.bottom() - FOOTER_H / 2.0;
        painter.hline(panel.x_range(), panel.bottom() - FOOTER_H, (1.0, DIVIDER));
        let hints = if !self.search_note.is_empty() {
            vec![("", self.search_note.as_str())]
        } else {
            let selected = self.results.get(self.selected);
            let copy_action = selected.is_some_and(|item| matches!(item.action, Action::Copy(_)));
            let mut hints = vec![("Enter", if copy_action { "Copy answer" } else { "Open" })];
            if selected.is_some_and(|item| item.reveal.is_some()) {
                hints.push(("Ctrl+Enter", "Show in folder"));
            }
            if selected.is_some_and(|item| item.complete.is_some()) {
                hints.push(("Tab", "Complete"));
            }
            hints.push(("Esc", "Close"));
            hints
        };
        for (key, what) in hints {
            let k = painter.text(pos2(x, fy), Align2::LEFT_CENTER, key, FontId::proportional(14.0), SUBTLE);
            let d = painter.text(pos2(k.right() + 5.0, fy), Align2::LEFT_CENTER, what, FontId::proportional(14.0), HINT);
            x = d.right() + 18.0;
        }

        if let Some(i) = clicked {
            self.execute(i, Mode::Open);
        }
    }
}

impl eframe::App for Lumen {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // eframe shows the window after frame 1; put it back to hidden on frame 2.
        self.frames = self.frames.saturating_add(1);
        if self.frames == 1 {
            ctx.request_repaint();
        } else if self.frames == 2 && !win::VISIBLE.load(Ordering::Relaxed) {
            win::hide();
        }
        if !win::VISIBLE.load(Ordering::Relaxed) {
            self.display_overlay.close();
            return;
        }
        if win::JUST_SHOWN.swap(false, Ordering::Acquire) {
            self.backdrop = self.display_overlay.open().map(|img| ctx.load_texture("backdrop", img, TextureOptions::LINEAR));
            if !win::VISIBLE.load(Ordering::Relaxed) {
                self.hide();
                return;
            }
            win::show_native();
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            win::force_foreground();
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.seen_focus = false;
            let had_text = !self.query.is_empty() && self.last_query.is_some();
            self.set_cursor(ctx, had_text);
            ctx.memory_mut(|m| m.request_focus(Self::query_id()));
            self.last_query = None;
            ctx.request_repaint();
        } else {
            // A DPI change can resize the native window after the initial move.
            self.display_overlay.fit();
        }

        while let Ok(update) = self.update_rx.try_recv() {
            self.available_update = Some(update);
        }

        while let Ok((q, generation, found, note)) = self.search_rx.try_recv() {
            if q == self.query && generation <= self.index.generation() {
                self.found = found;
                self.search_note = note;
                self.searching = false;
                self.last_query = None;
            }
        }

        while let Ok((key, image)) = self.icon_rx.try_recv() {
            self.pending_icons.remove(&key);
            let texture = image.map(|img| ctx.load_texture(&key, img, TextureOptions::LINEAR));
            self.icons.insert(key, texture);
        }

        // Clicking another window closes the launcher.
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
        if focused {
            self.seen_focus = true;
        } else if self.seen_focus && !self.pinned {
            self.seen_focus = false;
            self.hide();
            return;
        }

        if self.last_query.as_deref() != Some(self.query.as_str()) || self.last_generation != self.index.generation() {
            self.refresh();
        }
        self.handle_keys(ctx);
        if self.last_query.as_deref() != Some(self.query.as_str()) {
            self.refresh(); // Tab completion changed the text
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if !win::VISIBLE.load(Ordering::Relaxed) {
            return;
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.draw(ui));
        if self.last_query.as_deref() != Some(self.query.as_str()) {
            ui.ctx().request_repaint();
        }
    }
}

fn simple(title: &str, subtitle: &str, label: &str, glyph: char, action: Action) -> Item {
    Item {
        title: title.into(),
        subtitle: subtitle.into(),
        label: label.into(),
        glyph: Some(glyph),
        icon: None,
        action,
        reveal: None,
        admin: false,
        copy: None,
        complete: None,
        history_key: None,
    }
}

fn icon_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("icons".into()))
}

fn single_line(text: &str, size: f32, color: Color32, max_width: f32) -> LayoutJob {
    let mut job = LayoutJob::single_section(text.to_string(), TextFormat::simple(FontId::proportional(size), color));
    job.wrap = TextWrapping::truncate_at_width(max_width.max(10.0));
    job
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn setup_style(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, file: &str| -> bool {
        match std::fs::read(format!(r"C:\Windows\Fonts\{file}")) {
            Ok(bytes) => {
                fonts.font_data.insert(name.into(), Arc::new(FontData::from_owned(bytes)));
                true
            }
            Err(_) => false,
        }
    };
    let segoe = add("segoe", "segoeui.ttf");
    let icons = add("icons", "SegoeIcons.ttf") || add("icons", "segmdl2.ttf");
    // Fallbacks for Chinese, Japanese and Korean names; Segoe UI has no CJK glyphs.
    let fallbacks: Vec<&str> = [("yahei", "msyh.ttc"), ("malgun", "malgun.ttf")]
        .into_iter()
        .filter(|(name, file)| add(name, file))
        .map(|(name, _)| name)
        .collect();
    let proportional = fonts.families.get_mut(&FontFamily::Proportional).unwrap();
    if segoe {
        proportional.insert(0, "segoe".into());
    }
    proportional.extend(fallbacks.iter().map(|name| name.to_string()));
    let mut icon_family = proportional.clone();
    if icons {
        icon_family.insert(0, "icons".into());
    }
    fonts.families.insert(FontFamily::Name("icons".into()), icon_family);
    ctx.set_fonts(fonts);

    let mut visuals = egui::Visuals::light();
    visuals.selection.bg_fill = Color32::from_rgb(0xC9, 0xDA, 0xF8);
    visuals.selection.stroke.color = TEXT;
    visuals.text_cursor.stroke = (2.0, TEXT).into();
    visuals.panel_fill = Color32::TRANSPARENT;
    ctx.set_visuals(visuals);
}
