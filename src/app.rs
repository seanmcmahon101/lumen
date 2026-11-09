//! The overlay: a dimmed desktop, the lit search panel, and its results.

use crate::index::{Entry, Index, Kind};
use crate::{calc, win};
use eframe::egui::{
    self, Align, Align2, Color32, FontFamily, FontId, Id, Key, Layout, Margin, Modifiers, Pos2, Rect, Sense, Stroke, TextEdit,
    TextureHandle, TextureOptions, UiBuilder, Vec2, pos2,
    text::{CCursor, CCursorRange, LayoutJob, TextWrapping},
    vec2,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use theme::{Appearance, Palette, Slide};

mod commands;
mod paint;
mod preferences;
mod theme;

const WIDTH: f32 = 760.0;
const BAR_H: f32 = 72.0;
const FILTER_H: f32 = 40.0;
/// The icon column's centre and the text column's start, from the panel's left edge.
/// The search glyph, result icons, query, tabs and titles all hang off these two lines.
const GLYPH_X: f32 = 36.0;
const TEXT_X: f32 = 64.0;
const SEARCH_SIZE: f32 = 24.0;
const ROW_H: f32 = 56.0;
const HEADER_H: f32 = 30.0;
const LIST_PAD: f32 = 6.0;
const FOOTER_H: f32 = 40.0;
const EMPTY_H: f32 = 52.0;
const MAX_VISIBLE_ROWS: usize = 8;
const RADIUS: u8 = 16;
const ROW_RADIUS: u8 = 8;
/// How long the room takes to dim when Lumen opens.
const OPEN_TIME: f64 = 0.2;

// Segoe Fluent Icons code points
const G_SEARCH: char = '\u{E721}';
const G_CALC: char = '\u{E8EF}';
const G_GLOBE: char = '\u{E774}';
const G_POWER: char = '\u{E7E8}';
const G_SETTINGS: char = '\u{E713}';
const G_OPEN: char = '\u{E8A7}';
const G_MAIL: char = '\u{E715}';
const G_SHEET: char = '\u{E80A}';
const G_FOLDER: char = '\u{E8B7}';
const G_FILE: char = '\u{E8A5}';
const G_APP: char = '\u{ECAA}';
const G_PIN: char = '\u{E718}';
const G_UNPIN: char = '\u{E77A}';
const G_INFO: char = '\u{E946}';
const G_HELP: char = '\u{E897}';
const G_RETURN: char = '\u{E751}';
const G_CALENDAR: char = '\u{E787}';
const G_CONVERT: char = '\u{E8AB}';
const G_HISTORY: char = '\u{E81C}';
const G_LINK: char = '\u{E71B}';
const G_WINDOWS: char = '\u{E7C4}';
const G_MORE: char = '\u{E712}';
const G_PREVIEW: char = '\u{E7B3}';
const G_REFRESH: char = '\u{E72C}';
const G_STATUS: char = '\u{E9D9}';
const G_SPEED: char = '\u{EC4A}';
const G_KEYBOARD: char = '\u{E765}';
const G_REVEAL: char = '\u{E8DA}';
const G_COPY: char = '\u{E8C8}';
const G_ADMIN: char = '\u{E7EF}';
const G_TERMINAL: char = '\u{E756}';
const G_LIST: char = '\u{E8FD}';
const G_DOWNLOAD: char = '\u{E896}';
const G_CHEVRON: char = '\u{E76C}';

#[derive(Clone)]
enum Action {
    Open(String),
    Copy(String),
    Quit,
    ToggleStartup,
    System(&'static crate::system::Command),
    Query(String),
    Switch(crate::desktop::Window),
    Reveal(String),
    Admin(String),
    Terminal(String),
    Pin(crate::library::Saved),
    Unpin(String),
    BeginAlias(crate::library::Saved),
    SetAlias(String, crate::library::Saved),
    RemoveAlias(String),
    Preview(crate::library::Saved, String),
    AddFolder(String),
    PickFolder,
    Reindex,
    Noop,
}

impl Action {
    /// What a second Enter will do, for commands that ask first.
    fn confirm(&self) -> Option<&'static str> {
        match self {
            Action::System(command) => command.confirm,
            _ => None,
        }
    }

    /// What Enter does, shown on the selected row.
    fn verb(&self) -> Option<&'static str> {
        Some(match self {
            Action::Open(target) if target.starts_with("https://www.google.com/search") || target.contains("/mail/0/search") => "Search",
            Action::Open(_) | Action::Terminal(_) => "Open",
            Action::Copy(_) => "Copy",
            Action::Quit => "Quit",
            Action::ToggleStartup => "Change",
            Action::System(command) if command.note == "Windows" => "Run",
            Action::System(_) => "Open",
            Action::Query(query) if query.starts_with('/') => "Open",
            Action::Query(_) => "Try",
            Action::Switch(_) => "Switch",
            Action::Reveal(_) => "Reveal",
            Action::Admin(_) => "Run",
            Action::Pin(_) => "Pin",
            Action::Unpin(_) => "Unpin",
            Action::BeginAlias(_) => "Choose",
            Action::SetAlias(..) => "Save",
            Action::RemoveAlias(_) => "Remove",
            Action::Preview(..) => "Preview",
            Action::AddFolder(_) => "Add",
            Action::PickFolder => "Choose",
            Action::Reindex => "Refresh",
            Action::Noop => return None,
        })
    }
}

#[derive(Clone)]
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
    saved: Option<crate::library::Saved>,
    header: Option<&'static str>, // a group title drawn above this row
    answer: bool,                 // a calculation, shown large
}

impl Item {
    fn height(&self) -> f32 {
        ROW_H + if self.header.is_some() { HEADER_H } else { 0.0 }
    }

    /// Stands in for the shell icon until it loads.
    fn placeholder(&self) -> char {
        match self.saved.as_ref().map(|saved| saved.kind) {
            Some(Kind::Folder) => G_FOLDER,
            Some(Kind::App) => G_APP,
            Some(Kind::Email) => G_MAIL,
            _ => G_FILE,
        }
    }
}

/// Row positions in a list whose group headers make some rows taller.
struct Rows {
    offsets: Vec<f32>,
}

impl Rows {
    fn new(items: &[Item]) -> Self {
        let mut offsets = Vec::with_capacity(items.len() + 1);
        let mut y = 0.0;
        offsets.push(y);
        for item in items {
            y += item.height();
            offsets.push(y);
        }
        Self { offsets }
    }

    fn total(&self) -> f32 {
        self.offsets.last().copied().unwrap_or(0.0)
    }

    fn top(&self, i: usize) -> f32 {
        self.offsets[i]
    }

    fn bottom(&self, i: usize) -> f32 {
        self.offsets[i + 1]
    }

    /// The row at content height `y`.
    fn at(&self, y: f32) -> usize {
        self.offsets.partition_point(|&top| top <= y).saturating_sub(1).min(self.offsets.len().saturating_sub(2))
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Open,
    Reveal,
    Admin,
}

pub struct Lumen {
    query: String,
    ui_query: String,
    last_query: Option<String>,
    last_generation: u64,
    results: Vec<Item>,
    selected: usize,
    selection_by_user: bool,
    scroll_offset: f32,
    scroll_to_selected: bool,
    suppress_scroll_inertia: bool,
    index: Arc<Index>,
    history: win::History,
    icons: HashMap<String, Option<TextureHandle>>,
    icon_tx: mpsc::Sender<(String, String)>,
    icon_rx: mpsc::Receiver<(String, Option<egui::ColorImage>)>,
    pending_icons: HashSet<String>,
    seen_focus: bool,
    ghost_shown: bool,
    show_at_launch: bool,
    profile: String,
    backdrop: Option<TextureHandle>,
    display_overlay: win::DisplayOverlay,
    update_rx: mpsc::Receiver<crate::update::Update>,
    update_opened: mpsc::Sender<()>,
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
    pending: Option<(String, Mode)>, // Enter pressed before this query's results arrived
    armed: Option<(String, String)>, // (query, title): a command waiting for a second Enter
    library: crate::library::Library,
    command_target: Option<Item>,
    return_query: String,
    watch_tx: mpsc::Sender<()>,
    preview_tx: mpsc::Sender<crate::preview::Request>,
    preview_rx: mpsc::Receiver<crate::preview::Response>,
    folder_tx: mpsc::Sender<String>,
    folder_rx: mpsc::Receiver<String>, // folders chosen in the /index picker
    preview_key: Option<crate::preview::Request>,
    preview_lines: Option<Result<Vec<crate::preview::Line>, String>>,
    preview_terms: String,
    window_tx: mpsc::Sender<()>,
    window_rx: mpsc::Receiver<Vec<crate::desktop::Window>>,
    windows: Vec<crate::desktop::Window>,
    windows_loading: bool,
    settings_draft: crate::settings::Settings,
    settings_folders: String,
    settings_exclusions: String,
    settings_error: String,
    settings_appearance: Appearance,
    perf_rows: Vec<(String, String)>,
    appearance: Appearance,
    windows_light: bool,
    accent: theme::Accent,
    visuals_for: Option<Palette>,
    motion: bool,
    opened_at: Option<f64>,
    slide: Slide,
    snap_slide: bool,
    keyboard: bool, // the keyboard, not the pointer, moved focus last: show focus outlines
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
        theme::install_fonts(&ctx);
        // Lumen chooses light or dark itself; keep egui from switching styles under it.
        ctx.set_theme(egui::Theme::Light);
        let appearance = Appearance::load();
        let windows_light = win::apps_use_light_theme();
        let accent = win::accent_ramp().map(theme::Accent::from_ramp).unwrap_or_default();
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        }

        let index = Arc::new(Index::default());
        let settings = crate::settings::get();
        crate::hook::set_hotkey(settings.hotkey);
        let repaint = ctx.clone();
        let (preview_tx, preview_rx) = crate::preview::spawn(move || repaint.request_repaint());
        let (folder_tx, folder_rx) = mpsc::channel();
        let (window_tx, window_requests) = mpsc::channel();
        let (window_answers, window_rx) = mpsc::channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            while window_requests.recv().is_ok() {
                while window_requests.try_recv().is_ok() {}
                if window_answers.send(crate::desktop::windows()).is_err() { break; }
                repaint.request_repaint();
            }
        });
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
        index.start(move || repaint.request_repaint());
        let repaint = ctx.clone();
        let watch_tx = crate::watch::spawn(index.clone(), move || repaint.request_repaint());
        let (shared_index, repaint, shared_watch) = (index.clone(), ctx.clone(), watch_tx.clone());
        std::thread::spawn(move || {
            if crate::settings::add_shared_folders() {
                let _ = shared_watch.send(());
                shared_index.rebuild(move || repaint.request_repaint());
            }
        });
        let (search_tx, requests) = mpsc::channel::<(String, u64, win::History)>();
        let (answers, search_rx) = mpsc::channel();
        let latest_query = Arc::new(Mutex::new(String::new()));
        let worker_query = latest_query.clone();
        let (search_index, search_ctx) = (index.clone(), ctx.clone());
        std::thread::spawn(move || {
            // Start the search pool now, not on the first keystroke.
            rayon::broadcast(|_| ());
            while let Ok(mut request) = requests.recv() {
                while let Ok(newer) = requests.try_recv() {
                    request = newer;
                }
                // Only explicit mail queries are debounced; local search starts at once.
                if crate::mail::query(crate::command::search_query(&request.0)).is_some() {
                    std::thread::sleep(Duration::from_millis(70));
                    while let Ok(newer) = requests.try_recv() {
                        request = newer;
                    }
                }
                let (q, generation, history) = request;
                let term = crate::command::search_query(&q);
                let started = std::time::Instant::now();
                let mut found = search_index.search(term, usize::MAX, |p| history.uses(p));
                if !term.is_empty() { crate::metrics::searched(started.elapsed().as_secs_f64() * 1000.); }
                let mut note = String::new();
                if let Some(term) = crate::mail::query(term) && crate::settings::get().mail {
                    match crate::mail::search(term, || *worker_query.lock().unwrap() != q) {
                        Ok(mail) => {
                            note = if mail.is_empty() {
                                "No indexed Outlook matches · try Outlook online"
                            } else if mail.len() >= crate::mail::MAX_RESULTS {
                                "First 1,000 Outlook matches · narrow your search for more"
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

        if !std::env::args().any(|a| a == "--isolated") {
            crate::hook::spawn(win::toggle);
            crate::tray::spawn();
            win::migrate_startup();
        }

        // `lumen --show "query"` opens straight away (handy for testing).
        let args: Vec<String> = std::env::args().collect();
        let mut query = String::new();
        let pinned = args.iter().any(|a| a == "--show");
        // Started from the Start menu or the installer, not at login: show the search bar.
        let show_at_launch = !pinned && !args.iter().any(|a| a == "--background");
        let (update_rx, update_opened) = crate::update::watch(ctx.clone());
        if let Some(i) = args.iter().position(|a| a == "--show") {
            query = args.get(i + 1).cloned().unwrap_or_default();
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(1500));
                win::show();
            });
        }

        Self {
            ui_query: String::new(),
            query,
            last_query: None,
            last_generation: 0,
            results: Vec::new(),
            selected: 0,
            selection_by_user: false,
            scroll_offset: 0.0,
            scroll_to_selected: true,
            suppress_scroll_inertia: false,
            index,
            history: win::History::load(),
            icons: HashMap::new(),
            icon_tx,
            icon_rx,
            pending_icons: HashSet::new(),
            seen_focus: false,
            ghost_shown: false,
            show_at_launch,
            profile: std::env::var("USERPROFILE").unwrap_or_default(),
            backdrop: None,
            display_overlay: win::DisplayOverlay::default(),
            update_rx,
            update_opened,
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
            pending: None,
            armed: None,
            library: crate::library::Library::load(),
            command_target: None,
            return_query: String::new(),
            watch_tx,
            preview_tx,
            preview_rx,
            folder_tx,
            folder_rx,
            preview_key: None,
            preview_lines: None,
            preview_terms: String::new(),
            window_tx,
            window_rx,
            windows: Vec::new(),
            windows_loading: false,
            settings_draft: settings,
            settings_folders: String::new(),
            settings_exclusions: String::new(),
            settings_error: String::new(),
            settings_appearance: appearance,
            perf_rows: Vec::new(),
            appearance,
            windows_light,
            accent,
            visuals_for: None,
            motion: win::animations_enabled(),
            opened_at: None,
            slide: Slide::default(),
            snap_slide: true,
            keyboard: false,
        }
    }

    fn query_id() -> Id {
        Id::unique("lumen-query")
    }

    /// Settings preview their theme before it is saved.
    fn palette(&self) -> Palette {
        let appearance = if is_settings(&self.query) { self.settings_appearance } else { self.appearance };
        Palette::new(appearance.is_dark(self.windows_light), self.accent)
    }

    /// How far the room has dimmed since Lumen opened, eased, from 0 to 1.
    fn opening(&self, now: f64) -> f32 {
        match self.opened_at {
            Some(start) if self.motion => theme::ease_out(((now - start) / OPEN_TIME) as f32),
            _ => 1.0,
        }
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

    /// The first Enter on this command was pressed, with the query unchanged since.
    fn is_armed(&self, title: &str) -> bool {
        self.armed.as_ref().is_some_and(|(query, armed)| *query == self.query && armed == title)
    }

    fn cancel_actions(&mut self) {
        self.pending = None;
        self.armed = None;
    }

    fn hide(&mut self) {
        self.cancel_actions();
        self.display_overlay.close();
        win::hide();
    }

    /// Observe native visibility before processing search answers or queued input.
    fn sync_visibility(&mut self) -> Option<bool> {
        let visible = win::VISIBLE.load(Ordering::Acquire);
        let just_shown = visible && win::JUST_SHOWN.swap(false, Ordering::Acquire);
        if !visible || just_shown {
            // A hide/show can happen entirely between UI frames.
            self.cancel_actions();
        }
        if !visible {
            self.display_overlay.close();
            return None;
        }
        Some(just_shown)
    }

    fn refresh(&mut self) {
        let text_changed = self.ui_query != self.query;
        let entering_command = self.query.trim_start().starts_with('/') && !self.ui_query.trim_start().starts_with('/');
        if entering_command {
            self.command_target = self.results.get(self.selected).filter(|i| i.saved.is_some()).cloned();
            self.return_query = self.ui_query.clone();
            self.preview_terms = self.return_query.clone();
            self.preview_key = None;
        }
        if self.query.trim().eq_ignore_ascii_case("/settings") && text_changed {
            self.load_settings_draft();
        }
        if self.query.trim().eq_ignore_ascii_case("/perf") && text_changed {
            self.perf_rows = crate::metrics::snapshot();
        }
        if crate::command::parse(&self.query).is_some_and(|(name, _)| name == "windows") && text_changed {
            let _ = self.window_tx.send(()); self.windows_loading = true;
        }
        self.ui_query = self.query.clone();
        let same_query = self.search_key.as_ref().is_some_and(|(query, _)| query == &self.query);
        let previous_index = self.selected;
        let previous_selection = (same_query && self.selection_by_user)
            .then(|| self.results.get(self.selected).map(|item| (item.title.clone(), item.subtitle.clone())))
            .flatten();
        if !same_query {
            self.selection_by_user = false;
            self.scroll_offset = 0.0;
            self.scroll_to_selected = true;
            self.snap_slide = true;
            self.cancel_actions(); // Editing away and back must not revive an earlier Enter.
            crate::metrics::query(&self.query);
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
                self.search_note.clear();
                // Keep the last results on screen until this query's arrive, milliseconds
                // later, so the list does not collapse on every keystroke. Mail takes seconds.
                if t.is_empty() || t.starts_with('/') || crate::mail::query(&t).is_some() {
                    self.found.clear();
                }
            }
            self.search_key = Some(key.clone());
            self.searching = !crate::command::search_query(&t).is_empty();
            if self.searching { let _ = self.search_tx.send((key.0, key.1, self.history.clone())); }
        }
        if self.slash_results(&t) { return; }
        if t.is_empty() {
            self.home_results();
            return;
        }
        let lower = t.to_lowercase();
        if crate::mail::query(&t).is_some() && !crate::settings::get().mail {
            self.results.push(simple("Outlook search is disabled", "Enable Outlook in /settings", "Settings", G_SETTINGS, Action::Query("/settings".into())));
            return;
        }

        if let Some(answer) = crate::convert::units(&t) {
            self.results.push(answer_item(answer, G_CONVERT, "Conversion"));
        } else if let Some(answer) = crate::convert::date(&t) {
            self.results.push(answer_item(answer, G_CALENDAR, "Date"));
        }

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
                saved: None,
                header: None,
                answer: true,
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
            let startup_title = if win::startup_enabled() { "Stop starting Lumen at login" } else { "Start Lumen at login" };
            for (title, glyph, action) in [("Quit Lumen", G_POWER, Action::Quit), (startup_title, G_SETTINGS, Action::ToggleStartup)] {
                if title.to_lowercase().split_whitespace().any(|w| w.starts_with(&lower)) || title.to_lowercase().contains(&lower) {
                    commands.push(simple(title, "Lumen", "Command", glyph, action));
                }
            }
        }

        // Windows Settings and system commands, never behind a filter like "excel ".
        // Exact names go above files; partial names follow all matching files.
        let (exact, partial) =
            if crate::index::split_filter(&self.query).0.is_empty() { crate::system::find(&t) } else { Default::default() };
        let mut files: Vec<Item> = self.found.iter().map(|e| self.entry_item(e)).collect();
        if let Some(shortcut) = self.library.pick(&t) {
            let mut item = self.saved_item(shortcut); item.label = "Alias".into();
            files.retain(|i| i.history_key != item.history_key); self.results.push(item);
        }
        // What was last opened for this query stays first.
        let learned = self.history.pick(&self.query).is_some_and(|path| self.found.first().is_some_and(|e| e.path == path));
        if learned && !files.is_empty() {
            self.results.push(files.remove(0));
        }
        self.results.extend(exact.into_iter().map(system_item));
        self.results.extend(files);
        self.results.extend(partial.into_iter().map(system_item));
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
            self.scroll_to_selected |= self.selected != previous_index;
        }
    }

    fn entry_item(&self, e: &Entry) -> Item {
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
            history_key: Some(e.path.clone()),
            saved: Some(crate::library::Saved::from_entry(e)),
            header: None,
            answer: false,
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
        if !win::VISIBLE.load(Ordering::Acquire) || win::JUST_SHOWN.load(Ordering::Acquire) {
            self.cancel_actions(); return;
        }
        let Some(item) = self.results.get(i).cloned() else { return };
        if item.action.confirm().is_some() && !self.is_armed(&item.title) {
            self.armed = Some((self.query.clone(), item.title.clone())); return;
        }
        // Navigation and preference changes keep the launcher open.
        match &item.action {
            Action::Query(query) => { self.navigate(query.clone()); return; }
            Action::Noop => return,
            Action::Pin(saved) => {
                match self.library.toggle_pin(saved.clone()) {
                    Ok(_) => self.navigate("/pins".into()), Err(error) => self.search_note = error.into(),
                }
                return;
            }
            Action::Unpin(path) => {
                self.library.pins.retain(|p| !p.path.eq_ignore_ascii_case(path)); self.library.save(); self.navigate("/pins".into()); return;
            }
            Action::BeginAlias(saved) => {
                self.command_target = Some(self.saved_item(saved)); self.navigate("/alias ".into()); return;
            }
            Action::SetAlias(name, saved) => {
                match self.library.alias(name, saved.clone()) { Ok(()) => self.navigate("/aliases".into()), Err(error) => self.search_note = error.into() }
                return;
            }
            Action::RemoveAlias(name) => { self.library.aliases.remove(name); self.library.save(); self.navigate("/aliases".into()); return; }
            Action::Preview(saved, terms) => {
                self.command_target = Some(self.saved_item(saved)); self.preview_terms = terms.clone(); self.preview_key = None;
                self.navigate("/preview".into()); return;
            }
            Action::AddFolder(folder) => { self.add_folder(folder); return; }
            Action::Reindex => { self.reindex(); self.navigate("/status".into()); return; }
            _ => {}
        }
        self.hide();
        let opened = match (mode, &item.reveal, &item.action) {
            (Mode::Reveal, Some(path), _) => { win::reveal(path); true }
            (Mode::Admin, _, Action::Open(target)) if item.admin => win::shell_open(target, "runas", None),
            (_, _, Action::Open(target)) if target.starts_with("lumen-outlook:") => { crate::mail::open(target); true }
            (_, _, Action::Open(target)) => win::shell_open(target, "open", None),
            (_, _, Action::Copy(text)) => { win::copy(text); false }
            (_, _, Action::Switch(window)) => { crate::desktop::activate(window); false }
            (_, _, Action::Reveal(path)) => { win::reveal(path); false }
            (_, _, Action::Admin(path)) => { win::shell_open(path, "runas", None); false }
            (_, _, Action::Terminal(folder)) => { crate::desktop::terminal(folder); false }
            (_, _, Action::PickFolder) => {
                // The picker takes focus, so Lumen hides first and reopens with the answer.
                let tx = self.folder_tx.clone();
                std::thread::spawn(move || if let Some(folder) = win::pick_folder() { let _ = tx.send(folder); win::show(); });
                false
            }
            (_, _, Action::Quit) => crate::tray::quit(),
            (_, _, Action::ToggleStartup) => { win::set_startup(!win::startup_enabled()); false }
            (_, _, Action::System(command)) => { crate::system::run(&command.target); false }
            _ => false,
        };
        if opened {
            if let Some(saved) = item.saved { self.library.remember(saved); }
            if let Some(k) = item.history_key.clone() {
                self.history.bump(&k);
                if !self.query.trim_start().starts_with('/') { self.history.learn(&self.query, &k); }
            }
        }
        self.search_key = None;
        self.last_query = None;
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
        ctx.input(|i| {
            if i.key_pressed(Key::Tab) {
                self.keyboard = true;
            }
            if i.pointer.any_pressed() {
                self.keyboard = false;
            }
        });
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F1)) {
            self.navigate("/help".into()); return;
        }
        if self.query.trim().eq_ignore_ascii_case("/settings") {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) { self.navigate(self.return_query.clone()); }
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::K)) {
            self.navigate("/actions".into()); return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::P)) {
            self.navigate("/preview".into()); return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::D)) {
            if let Some(saved) = self.results.get(self.selected).and_then(|i| i.saved.clone()) {
                match self.library.toggle_pin(saved) { Ok(_) => { self.last_query = None; }, Err(error) => self.search_note = error.into() }
            }
            return;
        }
        let n = self.results.len();
        let page_rows = visible_result_rows(ctx.content_rect().height());
        let (at_end, has_selection) = self.caret(ctx);
        let ghost = self.ghost_shown && at_end && !has_selection;
        let mut run = None;
        let mut complete = false;
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                run = Some(None);
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) && n > 0 {
                self.select_result((self.selected + 1).min(n - 1));
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                self.select_result(self.selected.saturating_sub(1));
            }
            if i.consume_key(Modifiers::NONE, Key::PageDown) && n > 0 {
                self.select_result(self.selected.saturating_add(page_rows).min(n - 1));
            }
            if i.consume_key(Modifiers::NONE, Key::PageUp) {
                self.select_result(self.selected.saturating_sub(page_rows));
            }
            if i.consume_key(Modifiers::CTRL, Key::Home) {
                self.select_result(0);
            }
            if i.consume_key(Modifiers::CTRL, Key::End) && n > 0 {
                self.select_result(n - 1);
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
            if !has_selection && i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                && let Some(text) = self.results.get(self.selected).and_then(|r| r.copy.clone()) {
                win::copy(&text);
            }
        });
        if complete {
            self.accept_completion(ctx);
        }
        match run {
            Some(None) if self.query.trim_start().starts_with('/') => self.navigate(self.return_query.clone()),
            Some(None) => self.hide(),
            // Typed fast and pressed Enter before this query's results arrived:
            // open the top result once they do, not what the last query found.
            Some(Some(mode)) if self.searching && !self.selection_by_user && crate::mail::query(&self.query).is_none() => {
                self.pending = Some((self.query.clone(), mode));
            }
            Some(Some(mode)) if n > 0 => self.execute(self.selected, mode),
            _ => {}
        }
    }

    fn select_result(&mut self, selected: usize) {
        self.selected = selected;
        self.selection_by_user = true;
        self.scroll_to_selected = true;
        self.cancel_actions();
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let p = self.palette();
        let now = ctx.input(|i| i.time);
        let open = self.opening(now);
        let screen = ui.max_rect();
        let painter = ui.painter().clone();

        let available_rows = visible_result_rows(screen.height());
        let settings = is_settings(&self.query);
        let rows = Rows::new(&self.results);
        let list_h = rows.total().min(available_rows as f32 * ROW_H);
        let body_h = |list_h: f32| if list_h > 0.0 { 1.0 + LIST_PAD * 2.0 + list_h + FOOTER_H } else { EMPTY_H };
        let w = WIDTH.min(screen.width() - 40.0);
        let full_h = BAR_H + FILTER_H + body_h(available_rows as f32 * ROW_H);
        let h = if settings { full_h } else { BAR_H + FILTER_H + body_h(list_h) };
        // Pin the top where a full list would be centred, so the bar stays still as results change.
        let top = (screen.center().y - full_h / 2.0).min(screen.bottom() - h - 16.0).max(16.0);
        let panel = Rect::from_min_size(pos2(screen.center().x - w / 2.0, top), vec2(w, h));

        match &self.backdrop {
            Some(texture) => {
                // Pixel for pixel, so the first frame matches the desktop it covers.
                let area = Rect::from_min_size(screen.min, (texture.size_vec2() / ctx.pixels_per_point()).min(screen.size()));
                paint::backdrop(&painter, texture.id(), area, panel, &p, open);
            }
            None => {
                painter.rect_filled(screen, 0, Color32::from_gray(24));
            }
        }

        // Click on the dimmed area closes.
        if ui.interact(screen, Id::unique("dim"), Sense::click()).clicked()
            && !ctx.input(|i| i.pointer.interact_pos()).is_some_and(|pos| panel.contains(pos))
        {
            self.hide();
            return;
        }

        paint::panel(&painter, panel, &p, open);
        let bar = Rect::from_min_size(panel.min, vec2(w, BAR_H));
        self.draw_bar(ui, bar, &p, settings);
        let filters = Rect::from_min_size(pos2(panel.left(), bar.bottom()), vec2(w, FILTER_H));
        self.draw_filters(ui, filters, &p);
        let content = Rect::from_min_max(pos2(panel.left(), filters.bottom()), panel.max);
        painter.hline(panel.x_range(), content.top(), Stroke::new(1.0, p.line));
        if settings {
            self.draw_settings(ui, content, &p);
            return;
        }
        if self.results.is_empty() {
            let hint = theme::line("Try excel budget, mail invoice, or /help", theme::text(13.0, 400.0, p.ink2), w - TEXT_X - 20.0);
            paint::text(&painter, pos2(panel.left() + TEXT_X, content.center().y), Align2::LEFT_CENTER, hint);
            return;
        }

        let terms = highlight_terms(&self.query);
        let n = self.results.len();
        let mut budget = 3; // icon loads per frame
        let mut clicked = None;
        let list_rect = Rect::from_min_size(pos2(panel.left() + 8.0, content.top() + 1.0 + LIST_PAD), vec2(w - 16.0, list_h));
        let overflow = rows.total() > list_h + 0.5;
        let scroll = ui
            .scope_builder(UiBuilder::new().id_salt("results-viewport").max_rect(list_rect).layout(Layout::top_down(Align::Min)), |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.spacing_mut().scroll = paint::scrollbar();
                let mut area = egui::ScrollArea::vertical()
                    .id_salt("lumen-results")
                    .max_height(list_h)
                    .min_scrolled_height(list_h)
                    .scroll_bar_visibility(if overflow {
                        egui::scroll_area::ScrollBarVisibility::AlwaysVisible
                    } else {
                        egui::scroll_area::ScrollBarVisibility::AlwaysHidden
                    })
                    .auto_shrink([false, false])
                    .animated(false);
                if self.scroll_to_selected {
                    // A new query or keyboard jump takes over from old scroll inertia.
                    self.suppress_scroll_inertia = true;
                    egui::scroll_area::State::default().store(&ctx, ui.make_persistent_id("lumen-results"));
                    let selected = self.selected.min(n - 1);
                    let offset = self.scroll_offset.clamp((rows.bottom(selected) - list_h).max(0.0), rows.top(selected));
                    area = area.vertical_scroll_offset(offset);
                    self.scroll_to_selected = false;
                } else if ctx.input(|input| input.events.iter().any(|event| {
                    matches!(event, egui::Event::MouseWheel { source, .. } if *source != egui::MouseWheelSource::Momentum)
                })) {
                    self.suppress_scroll_inertia = false;
                }
                if self.suppress_scroll_inertia {
                    ctx.input_mut(|input| input.smooth_scroll_delta = Vec2::ZERO);
                }
                let output = area.show_viewport(ui, |ui, viewport| {
                    ui.set_height(rows.total());
                    let origin = ui.max_rect().min;
                    let width = ui.max_rect().width();
                    if self.selected < n {
                        let animate = self.motion && !std::mem::take(&mut self.snap_slide);
                        self.slide.set(rows.bottom(self.selected) - ROW_H, now, animate, ROW_H * 1.5);
                        if self.slide.running(now) {
                            ctx.request_repaint();
                        }
                        let lit = Rect::from_min_size(origin + vec2(0.0, self.slide.at(now)), vec2(width, ROW_H));
                        paint::selection(ui.painter(), &p, lit);
                    }
                    // Only rows in view are drawn.
                    let mut i = rows.at(viewport.min.y);
                    while i < n && rows.top(i) < viewport.max.y {
                        let rect = Rect::from_min_size(origin + vec2(0.0, rows.top(i)), vec2(width, rows.bottom(i) - rows.top(i)));
                        if self.draw_result(ui, rect, i, &mut budget, &p, &terms) {
                            clicked = Some(i);
                        }
                        i += 1;
                    }
                });
                self.scroll_offset = output.state.clamped_offset().y;
                output
            })
            .inner;

        // Footer: a note or the visible range on the left, keys on the right.
        let footer = Rect::from_min_max(pos2(panel.left(), panel.bottom() - FOOTER_H), panel.max);
        painter.hline(panel.x_range(), footer.top(), Stroke::new(1.0, p.line));
        let cy = footer.center().y;
        let command = self.query.trim_start().starts_with('/');
        let selected = self.results.get(self.selected);
        let mut keys = vec![("Esc", if command { "Back" } else { "Close" })];
        if selected.is_some_and(|item| item.complete.is_some()) {
            keys.push(("Tab", "Complete"));
        }
        if !command && selected.is_some_and(|item| item.reveal.is_some()) {
            keys.push(("Ctrl K", "Actions"));
        }
        let mut x = footer.right() - 20.0;
        for (key, what) in keys {
            let label = theme::line(what, theme::text(12.0, 400.0, p.ink2), f32::INFINITY);
            let label = paint::text(&painter, pos2(x, cy), Align2::RIGHT_CENTER, label);
            x = paint::keycap(&painter, &p, pos2(label.left() - 6.0, cy), Align2::RIGHT_CENTER, key).left() - 16.0;
        }
        let note = if !self.search_note.is_empty() {
            Some(self.search_note.clone())
        } else if overflow {
            let first = rows.at(self.scroll_offset) + 1;
            let last = rows.at(self.scroll_offset + scroll.inner_rect.height() - 0.5) + 1;
            Some(format!("{first}–{last} of {}", thousands(n)))
        } else {
            None
        };
        if let Some(note) = note {
            let note = theme::line(&note, theme::text(12.0, 400.0, p.ink2), x - panel.left() - 32.0);
            paint::text(&painter, pos2(panel.left() + 20.0, cy), Align2::LEFT_CENTER, note);
        }

        if let Some(i) = clicked {
            if i != self.selected {
                self.cancel_actions();
            }
            self.selected = i;
            self.selection_by_user = true;
            self.execute(i, Mode::Open);
        }
    }

    /// The search field, with a glyph for how Lumen reads the query and inline completion.
    fn draw_bar(&mut self, ui: &mut egui::Ui, bar: Rect, p: &Palette, settings: bool) {
        let ctx = ui.ctx().clone();
        let painter = ui.painter().clone();
        let (glyph, special) = mode_glyph(&self.query);
        paint::mode(&painter, p, pos2(bar.left() + GLYPH_X, bar.center().y), glyph, special);

        let text_rect = Rect::from_min_max(pos2(bar.left() + TEXT_X, bar.top()), pos2(bar.right() - 24.0, bar.bottom()));
        let palette = *p;
        let mut layouter = move |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap_width: f32| {
            ui.painter().layout_job(query_job(text.as_str(), &palette))
        };
        let hint = LayoutJob::single_section("Search apps, files, and mail".into(), theme::display(SEARCH_SIZE, 400.0, p.ink3));
        let font = FontId::new(SEARCH_SIZE, FontFamily::Name(theme::DISPLAY.into()));
        let output = ui
            .scope_builder(UiBuilder::new().max_rect(text_rect).layout(Layout::left_to_right(Align::Center)), |ui| {
                TextEdit::singleline(&mut self.query)
                    .id(Self::query_id())
                    .frame(egui::Frame::NONE)
                    .margin(Margin::ZERO)
                    .font(font)
                    .text_color(p.ink)
                    .hint_text(hint)
                    .desired_width(text_rect.width())
                    .lock_focus(!settings)
                    .layouter(&mut layouter)
                    .show(ui)
            })
            .inner;
        if !settings && !output.response.has_focus() {
            output.response.request_focus();
        }

        // Inline autocomplete: the rest of the selected result's name after the caret.
        let (at_end, has_selection) = self.caret(&ctx);
        self.ghost_shown = false;
        if at_end
            && !has_selection
            && let Some(rest) = self.completion()
        {
            let pos = output.galley_pos + vec2(output.galley.size().x, 0.0);
            let ghost = painter.layout_job(LayoutJob::single_section(rest, theme::display(SEARCH_SIZE, 400.0, p.ghost)));
            painter.with_clip_rect(text_rect).galley(pos, ghost, p.ghost);
            self.ghost_shown = true;
        }

        if self.query.is_empty() {
            let cy = bar.center().y;
            let label = theme::line("Commands", theme::text(12.0, 400.0, p.ink3), f32::INFINITY);
            let label = paint::text(&painter, pos2(bar.right() - 20.0, cy), Align2::RIGHT_CENTER, label);
            paint::keycap(&painter, p, pos2(label.left() - 6.0, cy), Align2::RIGHT_CENTER, "/");
        }
    }

    /// Scopes for a search, or where you are within a command; status on the right.
    fn draw_filters(&mut self, ui: &mut egui::Ui, row: Rect, p: &Palette) {
        let painter = ui.painter().clone();
        let cy = row.center().y;
        let command = self.query.trim_start().starts_with('/');
        if command {
            // Search › Settings, where "Search" goes back.
            let back = painter.layout_job(theme::line("Search", theme::text(13.0, 500.0, Color32::PLACEHOLDER), f32::INFINITY));
            let back_rect = Rect::from_min_size(pos2(row.left() + TEXT_X - 8.0, cy - 14.0), vec2(back.size().x + 16.0, 28.0));
            let response = ui.interact(back_rect, Id::unique("lumen-back"), Sense::click()).on_hover_text("Back to your search (Esc)");
            if response.hovered() {
                painter.rect_filled(back_rect, 6, p.hover);
            }
            let color = if response.hovered() { p.ink } else { p.ink2 };
            painter.galley(pos2(back_rect.left() + 8.0, cy - back.size().y / 2.0), back, color);
            let chevron = painter.text(pos2(back_rect.right() + 4.0, cy), Align2::LEFT_CENTER, G_CHEVRON, theme::icon(10.0), p.ink3);
            let name = crate::command::parse(&self.query).map(|(name, _)| name).unwrap_or_default();
            let title = theme::line(command_title(&name), theme::text(13.0, 600.0, p.ink), 360.0);
            paint::text(&painter, pos2(chevron.right() + 10.0, cy), Align2::LEFT_CENTER, title);
            if response.clicked() {
                self.navigate(self.return_query.clone());
            }
        } else {
            let mut x = row.left() + TEXT_X - 10.0;
            for (label, prefix) in [("All", ""), ("Spreadsheets", "excel "), ("Outlook", "mail ")] {
                let active = if prefix.is_empty() {
                    !self.query.starts_with("excel ") && crate::mail::query(&self.query).is_none()
                } else {
                    self.query.starts_with(prefix)
                };
                let weight = if active { 600.0 } else { 500.0 };
                let galley = painter.layout_job(theme::line(label, theme::text(13.0, weight, Color32::PLACEHOLDER), f32::INFINITY));
                let rect = Rect::from_min_size(pos2(x, cy - 14.0), vec2(galley.size().x + 20.0, 28.0));
                let response = ui.interact(rect, Id::unique(("lumen-scope", label)), Sense::click());
                if response.hovered() && !active {
                    painter.rect_filled(rect, 6, p.hover);
                }
                let color = if active || response.hovered() { p.ink } else { p.ink2 };
                painter.galley(pos2(rect.left() + 10.0, cy - galley.size().y / 2.0), galley, color);
                if active {
                    painter.rect_filled(Rect::from_center_size(pos2(rect.center().x, row.bottom() - 3.5), vec2(16.0, 3.0)), 1.5, p.accent);
                }
                if response.clicked() {
                    let term =
                        self.query.strip_prefix("excel ").or_else(|| crate::mail::query(&self.query)).unwrap_or(&self.query).to_string();
                    self.query = format!("{prefix}{term}");
                    self.set_cursor(ui.ctx(), false);
                }
                x = rect.right() + 4.0;
            }
        }

        // The right edge: an update, what Lumen is busy with, or help.
        let right = pos2(row.right() - 12.0, cy);
        let status = if self.searching && self.found.is_empty() {
            Some("Searching…".to_string())
        } else if self.query.trim().is_empty() && self.index.is_building() {
            Some(format!("Indexing · {} items", thousands(self.index.stats().0)))
        } else {
            None
        };
        if let Some(release) = &self.available_update {
            let galley = painter.layout_job(theme::line("Update available", theme::text(12.5, 600.0, p.accent), f32::INFINITY));
            let rect = Align2::RIGHT_CENTER.anchor_size(right, vec2(galley.size().x + 38.0, 28.0));
            let response =
                ui.interact(rect, Id::unique("lumen-update"), Sense::click()).on_hover_text(format!("Download Lumen {}", release.version));
            if response.hovered() {
                painter.rect_filled(rect, 6, p.hover);
            }
            painter.text(pos2(rect.left() + 10.0, cy), Align2::LEFT_CENTER, G_DOWNLOAD, theme::icon(12.0), p.accent);
            painter.galley(pos2(rect.right() - 8.0 - galley.size().x, cy - galley.size().y / 2.0), galley, p.accent);
            if response.clicked() {
                crate::update::open_latest(release);
                self.hide();
            }
        } else if let Some(status) = status {
            let status = theme::line(&status, theme::text(12.0, 400.0, p.ink3), 260.0);
            paint::text(&painter, right - vec2(8.0, 0.0), Align2::RIGHT_CENTER, status);
        } else if !command {
            let galley = painter.layout_job(theme::line("Help", theme::text(12.0, 400.0, Color32::PLACEHOLDER), f32::INFINITY));
            let rect = Align2::RIGHT_CENTER.anchor_size(right, vec2(galley.size().x + 52.0, 28.0));
            let response = ui.interact(rect, Id::unique("lumen-help"), Sense::click());
            if response.hovered() {
                painter.rect_filled(rect, 6, p.hover);
            }
            let label = pos2(rect.right() - 8.0 - galley.size().x, cy - galley.size().y / 2.0);
            paint::keycap(&painter, p, pos2(label.x - 6.0, cy), Align2::RIGHT_CENTER, "F1");
            painter.galley(label, galley, if response.hovered() { p.ink } else { p.ink2 });
            if response.clicked() {
                self.navigate("/help".into());
            }
        }
    }

    /// One result: an optional group header, then icon, title and subtitle, and on the
    /// right its kind or, when selected, what Enter does.
    fn draw_result(&mut self, ui: &mut egui::Ui, rect: Rect, i: usize, budget: &mut u32, p: &Palette, terms: &[String]) -> bool {
        let ctx = ui.ctx().clone();
        let painter = ui.painter().clone(); // Inherit the scroll area's clipping rectangle.
        let row = Rect::from_min_max(pos2(rect.left(), rect.bottom() - ROW_H), rect.max);
        if let Some(header) = self.results[i].header {
            let header = theme::line(header, theme::text(12.0, 600.0, p.ink2), row.width() - 24.0);
            paint::text(&painter, pos2(row.left() + 12.0, row.top() - 6.0), Align2::LEFT_BOTTOM, header);
        }
        let response = ui.interact(row, Id::unique(("lumen-row", i)), Sense::click());
        let selected = i == self.selected;
        if !selected && response.hovered() {
            painter.rect_filled(row, ROW_RADIUS, p.hover);
        }
        response.clone().on_hover_text(&self.results[i].subtitle);

        let cy = row.center().y;
        let icon_center = pos2(row.left() + GLYPH_X - 8.0, cy);
        let (glyph, icon) = (self.results[i].glyph, self.results[i].icon.clone());
        if let Some(glyph) = glyph {
            paint::tile(&painter, p, icon_center, glyph, selected);
        } else if let Some((key, path)) = icon {
            match self.icon(&ctx, &key, &path, budget) {
                Some(texture) => {
                    let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
                    painter.image(texture.id(), Rect::from_center_size(icon_center, Vec2::splat(32.0)), uv, Color32::WHITE);
                }
                None => paint::tile(&painter, p, icon_center, self.results[i].placeholder(), false),
            }
        }

        let item = &self.results[i];
        let armed = item.action.confirm().is_some() && self.is_armed(&item.title);
        let mut edge = row.right() - 12.0;
        if selected && let Some(verb) = item.action.verb() {
            let key = paint::keycap(&painter, p, pos2(edge, cy), Align2::RIGHT_CENTER, "Enter");
            let (verb, color) = if armed { ("Confirm", p.danger) } else { (verb, p.ink2) };
            let verb = theme::line(verb, theme::text(12.0, 500.0, color), 120.0);
            edge = paint::text(&painter, pos2(key.left() - 8.0, cy), Align2::RIGHT_CENTER, verb).left();
        } else if item.label == "Enter" || item.label.starts_with("Ctrl+") {
            // An action's shortcut, drawn as the keys to press.
            edge = paint::keycap(&painter, p, pos2(edge, cy), Align2::RIGHT_CENTER, &item.label.replace('+', " ")).left();
        } else if !item.label.is_empty() {
            let label = theme::line(&item.label, theme::text(12.0, 400.0, p.ink3), 140.0);
            edge = paint::text(&painter, pos2(edge - 2.0, cy), Align2::RIGHT_CENTER, label).left();
        }
        // In search results, mark what is pinned; on the home screen its group already says so.
        if !self.query.trim().is_empty() && item.saved.as_ref().is_some_and(|saved| self.library.pinned(&saved.path)) {
            edge = painter.text(pos2(edge - 8.0, cy), Align2::RIGHT_CENTER, G_PIN, theme::icon(12.0), p.ink3).left();
        }

        let text_x = row.left() + TEXT_X - 8.0;
        let max_w = edge - text_x - 16.0;
        let title = painter.layout_job(title_job(item, terms, p, max_w));
        let subtitle = match item.action.confirm() {
            Some(what) if armed => theme::line(&format!("Press Enter again to {what}"), theme::text(13.0, 500.0, p.danger), max_w),
            _ => theme::line(&item.subtitle, theme::text(13.0, 400.0, p.ink2), max_w),
        };
        let subtitle = painter.layout_job(subtitle);
        let top = (cy - (title.size().y + subtitle.size().y) / 2.0).round();
        let gap = title.size().y;
        painter.galley(pos2(text_x, top), title, p.ink);
        painter.galley(pos2(text_x, top + gap), subtitle, p.ink2);
        response.clicked()
    }
}

impl eframe::App for Lumen {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    /// Cut, copy and paste, which eframe leaves to Lumen (`clipboard_shortcuts: false`).
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let at_end = self.caret(ctx).0;
        raw_input.events = std::mem::take(&mut raw_input.events)
            .into_iter()
            .filter_map(|event| match event {
                egui::Event::Key { key, pressed: true, modifiers, .. } => match clipboard_shortcut(key, modifiers) {
                    Some(egui::Event::Paste(_)) => {
                        win::paste_text().map(|text| egui::Event::Paste(paste_into(&self.query, at_end, text)))
                    }
                    Some(shortcut) => Some(shortcut),
                    None => Some(event),
                },
                event => Some(event),
            })
            .collect();
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // eframe shows the window after frame 1; put it back to hidden on frame 2.
        self.frames = self.frames.saturating_add(1);
        if self.frames == 1 {
            ctx.request_repaint();
        } else if self.frames == 2 && !win::VISIBLE.load(Ordering::Relaxed) {
            if self.show_at_launch { win::show() } else { win::hide() }
        }
        let Some(just_shown) = self.sync_visibility() else { return };
        if just_shown {
            let _ = self.update_opened.send(());
            if self.query.trim().eq_ignore_ascii_case("/perf") { self.perf_rows = crate::metrics::snapshot(); }
            // Windows' mode, accent and animation setting may have changed while Lumen was hidden.
            self.windows_light = win::apps_use_light_theme();
            self.accent = win::accent_ramp().map(theme::Accent::from_ramp).unwrap_or_default();
            self.motion = win::animations_enabled();
            self.opened_at = Some(ctx.input(|i| i.time));
            let dim = if self.motion { 1.0 } else { self.palette().dim_far };
            self.backdrop =
                self.display_overlay.open(brightness(dim)).map(|img| ctx.load_texture("backdrop", img, TextureOptions::LINEAR));
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
            let _ = self.window_tx.send(());
            self.windows_loading = true;
            ctx.request_repaint();
        } else {
            // A DPI change can resize the native window after the initial move.
            self.display_overlay.fit();
        }

        let p = self.palette();
        if self.visuals_for != Some(p) {
            ctx.set_visuals(theme::visuals(&p));
            self.visuals_for = Some(p);
        }
        // The room dims as the panel lights up. Painting continues a little longer, so the
        // first frame after the window moves to its display is never left on screen.
        if let Some(start) = self.opened_at {
            let now = ctx.input(|i| i.time);
            if now - start < OPEN_TIME + 0.25 {
                self.display_overlay.dim(brightness(1.0 + (p.dim_far - 1.0) * self.opening(now)));
                ctx.request_repaint();
            }
        }

        while let Ok(update) = self.update_rx.try_recv() {
            self.available_update = Some(update);
        }

        while let Ok(windows) = self.window_rx.try_recv() {
            self.windows = windows; self.windows_loading = false;
            if self.query.trim_start().starts_with("/windows") { self.last_query = None; }
        }
        while let Ok((request, lines)) = self.preview_rx.try_recv() {
            self.accept_preview(request, lines);
        }
        while let Ok(folder) = self.folder_rx.try_recv() {
            // Show /index's checks for the chosen folder, and add it when they pass.
            self.navigate(format!("/index {folder}"));
            if let Some(Action::AddFolder(folder)) = self.results.first().map(|i| i.action.clone()) {
                self.add_folder(&folder);
            }
        }

        while let Ok((q, generation, found, note)) = self.search_rx.try_recv() {
            if q == self.query {
                // Show any answer for this text, but only the newest search ends the wait,
                // so an Enter pressed meanwhile opens what the refreshed index found.
                self.searching &= self.search_key != Some((q, generation));
                self.found = found;
                self.search_note = note;
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
        if !self.searching
            && let Some((query, mode)) = self.pending.take()
            && query == self.query
            && !self.results.is_empty()
        {
            self.execute(self.selected, mode);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if !win::VISIBLE.load(Ordering::Relaxed) {
            return;
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.draw(ui));
        crate::metrics::painted(&self.query, !self.searching);
        if self.last_query.as_deref() != Some(self.query.as_str()) {
            ui.ctx().request_repaint();
        }
    }
}

fn system_item(command: &'static crate::system::Command) -> Item {
    let label = if command.note == "Windows Settings" { "Settings" } else { "Windows" };
    let mut item = simple(command.title, command.note, label, command.glyph, Action::System(command));
    item.complete = Some(command.title.into());
    item
}

fn visible_result_rows(screen_height: f32) -> usize {
    (((screen_height - 32.0 - BAR_H - FILTER_H - FOOTER_H - LIST_PAD * 2.0 - 1.0) / ROW_H).floor().max(1.0) as usize)
        .min(MAX_VISIBLE_ROWS)
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
        saved: None,
        header: None,
        answer: false,
    }
}

fn answer_item(answer: crate::convert::Answer, glyph: char, label: &str) -> Item {
    let mut item = simple(&answer.display, &answer.note, label, glyph, Action::Copy(answer.copy.clone()));
    item.copy = Some(answer.copy.clone());
    item.complete = Some(answer.copy);
    item.answer = true;
    item
}

fn is_settings(query: &str) -> bool {
    query.trim().eq_ignore_ascii_case("/settings")
}

/// Backdrop brightness as a byte.
fn brightness(level: f32) -> u8 {
    (level * 255.0).round().clamp(0.0, 255.0) as u8
}

fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The query as typed, with the words Lumen treats specially ("excel ", "/help") in the accent.
fn query_job(text: &str, p: &Palette) -> LayoutJob {
    let start = text.len() - text.trim_start().len();
    let split = if text[start..].starts_with('/') {
        text[start..].find(char::is_whitespace).map_or(text.len(), |end| start + end)
    } else {
        crate::index::split_filter(text).0.len()
    };
    let mut job = LayoutJob::default();
    if split > 0 {
        job.append(&text[..split], 0.0, theme::display(SEARCH_SIZE, 400.0, p.accent));
    }
    job.append(&text[split..], 0.0, theme::display(SEARCH_SIZE, 400.0, p.ink));
    job
}

/// How Lumen reads the query, as a glyph (None: the command list), and whether it is a special mode.
fn mode_glyph(query: &str) -> (Option<char>, bool) {
    if let Some((name, _)) = crate::command::parse(query) {
        return (command_glyph(&name), true);
    }
    let filter = crate::index::split_filter(query).0.trim().to_ascii_lowercase();
    if crate::mail::query(query).is_some() {
        (Some(G_MAIL), true)
    } else if ["excel", "sheets", "type:excel"].iter().any(|word| filter.starts_with(word)) {
        (Some(G_SHEET), true)
    } else if filter.starts_with("type:app") {
        (Some(G_APP), true)
    } else {
        (Some(G_SEARCH), false)
    }
}

/// The clipboard event a key press stands for, matching egui's own shortcuts.
fn clipboard_shortcut(key: Key, modifiers: Modifiers) -> Option<egui::Event> {
    let command = modifiers.command_only();
    if key == Key::Cut || (command && key == Key::X) || (modifiers.shift && key == Key::Delete) {
        Some(egui::Event::Cut)
    } else if key == Key::Copy || (command && key == Key::C) || (modifiers.ctrl && key == Key::Insert) {
        Some(egui::Event::Copy)
    } else if key == Key::Paste || (command && key == Key::V) || (modifiers.shift && key == Key::Insert) {
        Some(egui::Event::Paste(String::new()))
    } else {
        None
    }
}

/// Pasting straight after a command name, as in "/index" then Ctrl+V, adds the
/// space that separates the command from its argument.
fn paste_into(query: &str, at_end: bool, text: String) -> String {
    let after_command = at_end
        && !text.starts_with(char::is_whitespace)
        && query.strip_prefix('/').is_some_and(|name| {
            crate::command::COMMANDS.iter().any(|c| c.usage.contains(' ') && c.name.eq_ignore_ascii_case(name))
        });
    if after_command { format!(" {text}") } else { text }
}

fn command_glyph(name: &str) -> Option<char> {
    Some(match name {
        "help" => G_HELP,
        "pins" | "pin" => G_PIN,
        "unpin" => G_UNPIN,
        "recent" => G_HISTORY,
        "alias" | "aliases" | "unalias" => G_LINK,
        "windows" => G_WINDOWS,
        "actions" => G_MORE,
        "preview" => G_PREVIEW,
        "calc" => G_CALC,
        "convert" => G_CONVERT,
        "date" => G_CALENDAR,
        "settings" => G_SETTINGS,
        "index" => G_FOLDER,
        "reindex" => G_REFRESH,
        "status" => G_STATUS,
        "perf" => G_SPEED,
        _ => return None,
    })
}

fn command_title(name: &str) -> &'static str {
    match name {
        "help" => "Help",
        "pins" => "Pins",
        "pin" => "Pin",
        "unpin" => "Unpin",
        "recent" => "Recent",
        "alias" => "New alias",
        "aliases" => "Aliases",
        "unalias" => "Remove an alias",
        "windows" => "Open windows",
        "actions" => "Actions",
        "preview" => "Preview",
        "calc" => "Calculator",
        "convert" => "Convert units",
        "date" => "Dates",
        "settings" => "Settings",
        "index" => "Add a folder",
        "reindex" => "Refresh the index",
        "status" => "Index status",
        "perf" => "Performance",
        _ => "Commands",
    }
}

/// The words to embolden in titles: what was typed, without scope words such as "excel".
fn highlight_terms(query: &str) -> Vec<String> {
    if query.trim_start().starts_with('/') {
        return Vec::new();
    }
    let (_, rest) = crate::index::split_filter(query);
    rest.split_whitespace().filter(|word| !word.contains(':')).map(str::to_string).collect()
}

/// Where the typed words appear in a title, preferring the start of a word, as
/// Windows Search does when it emboldens matches.
fn match_ranges(title: &str, terms: &[String]) -> Vec<std::ops::Range<usize>> {
    let find = |term: &str| {
        let mut first = None;
        for (i, _) in title.char_indices() {
            if !title.get(i..i + term.len()).is_some_and(|s| s.eq_ignore_ascii_case(term)) {
                continue;
            }
            if title[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric()) {
                return Some(i..i + term.len());
            }
            first.get_or_insert(i..i + term.len());
        }
        first
    };
    let mut ranges: Vec<_> = terms.iter().filter(|term| !term.is_empty()).filter_map(|term| find(term)).collect();
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

fn title_job(item: &Item, terms: &[String], p: &Palette, max_width: f32) -> LayoutJob {
    let (size, weight) = if item.answer { (20.0, 500.0) } else { (15.0, 450.0) };
    let ranges = if item.answer { Vec::new() } else { match_ranges(&item.title, terms) };
    let mut job = LayoutJob::default();
    let mut at = 0;
    for range in ranges {
        if range.start > at {
            job.append(&item.title[at..range.start], 0.0, theme::text(size, weight, p.ink));
        }
        job.append(&item.title[range.clone()], 0.0, theme::text(size, 650.0, p.ink));
        at = range.end;
    }
    if at < item.title.len() || job.sections.is_empty() {
        job.append(&item.title[at..], 0.0, theme::text(size, weight, p.ink));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    static WINDOW_STATE: Mutex<()> = Mutex::new(());

    struct RestoreVisibility(bool, bool);

    impl Drop for RestoreVisibility {
        fn drop(&mut self) {
            win::VISIBLE.store(self.0, Ordering::Relaxed);
            win::JUST_SHOWN.store(self.1, Ordering::Release);
        }
    }

    fn visible_window() -> RestoreVisibility {
        RestoreVisibility(win::VISIBLE.swap(true, Ordering::Relaxed), win::JUST_SHOWN.swap(false, Ordering::Acquire))
    }

    /// The UI state with inert channels, without creating native windows or workers.
    fn launcher() -> Lumen {
        let (icon_tx, _) = mpsc::channel();
        let (_, icon_rx) = mpsc::channel();
        let (_, update_rx) = mpsc::channel();
        let (update_opened, _) = mpsc::channel();
        let (search_tx, _) = mpsc::channel();
        let (_, search_rx) = mpsc::channel();
        let (watch_tx, _) = mpsc::channel();
        let (preview_tx, _) = mpsc::channel();
        let (_, preview_rx) = mpsc::channel();
        let (folder_tx, folder_rx) = mpsc::channel();
        let (window_tx, _) = mpsc::channel();
        let (_, window_rx) = mpsc::channel();
        Lumen {
            query: "restart".into(),
            ui_query: "restart".into(),
            last_query: Some("restart".into()),
            last_generation: 0,
            results: Vec::new(),
            selected: 0,
            selection_by_user: false,
            scroll_offset: 0.0,
            scroll_to_selected: true,
            suppress_scroll_inertia: false,
            index: Arc::new(Index::default()),
            history: win::History::default(),
            icons: HashMap::new(),
            icon_tx,
            icon_rx,
            pending_icons: HashSet::new(),
            seen_focus: true,
            ghost_shown: false,
            show_at_launch: false,
            profile: String::new(),
            backdrop: None,
            display_overlay: win::DisplayOverlay::default(),
            update_rx,
            update_opened,
            available_update: None,
            pinned: false,
            frames: 2,
            search_tx,
            search_rx,
            search_key: Some(("restart".into(), 0)),
            found: Vec::new(),
            searching: true,
            search_note: String::new(),
            latest_query: Arc::new(Mutex::new("restart".into())),
            pending: Some(("restart".into(), Mode::Open)),
            armed: Some(("restart".into(), "Restart".into())),
            library: crate::library::Library::default(),
            command_target: None,
            return_query: String::new(),
            watch_tx,
            preview_tx,
            preview_rx,
            folder_tx,
            folder_rx,
            preview_key: None,
            preview_lines: None,
            preview_terms: String::new(),
            window_tx,
            window_rx,
            windows: Vec::new(),
            windows_loading: false,
            settings_draft: crate::settings::Settings::default(),
            settings_folders: String::new(),
            settings_exclusions: String::new(),
            settings_error: String::new(),
            settings_appearance: Appearance::Light,
            perf_rows: Vec::new(),
            appearance: Appearance::Light,
            windows_light: true,
            accent: theme::Accent::default(),
            visuals_for: None,
            motion: false,
            opened_at: None,
            slide: Slide::default(),
            snap_slide: true,
            keyboard: false,
        }
    }

    fn with_results(count: usize) -> Lumen {
        let mut app = launcher();
        app.query = "excel budget".into();
        app.search_key = Some((app.query.clone(), 0));
        app.searching = false;
        app.cancel_actions();
        app.found = (0..count)
            .map(|i| Entry::new(format!("budget {i:05}.xlsx"), format!(r"C:\Reports\budget {i:05}.xlsx"), Kind::File, 1))
            .collect();
        app.refresh();
        app
    }

    #[test]
    fn help_and_slash_completion_do_not_start_file_searches() {
        let mut app = launcher(); app.query = "/he".into(); app.refresh();
        assert!(!app.searching); assert!(matches!(&app.results[0].action, Action::Query(q) if q == "/help"));
        app.query = "/help keyboard".into(); app.refresh();
        assert!(!app.searching); assert!(app.results.iter().any(|i| i.title == "Keyboard shortcuts"));
        assert!(!app.results.iter().any(|i| matches!(&i.action, Action::Open(url) if url.contains("google"))));
        app.query = "/unknown".into(); app.refresh(); assert_eq!(app.results[0].title, "Unknown command");
    }

    #[test]
    fn index_command_offers_only_full_local_or_mapped_folders() {
        let mut app = launcher();
        app.query = "/index".into(); app.refresh();
        assert!(matches!(app.results[0].action, Action::PickFolder));
        for (query, title) in [
            ("/index Work", "Use a full folder path"),
            (r"/index \\server\share", "Use a mapped drive for network folders"),
        ] {
            app.query = query.into(); app.refresh();
            assert_eq!(app.results[0].title, title);
        }
        app.query = r#"/index "Q:/Lumen Test""#.into(); app.refresh();
        assert!(matches!(&app.results[0].action, Action::AddFolder(f) if f == r"Q:\Lumen Test"));
    }

    #[test]
    fn paste_after_a_command_name_starts_its_argument() {
        let paste = |query: &str, at_end| paste_into(query, at_end, r"C:\Work".into());
        assert_eq!(paste("/index", true), r" C:\Work");
        assert_eq!(paste("/INDEX", true), r" C:\Work");
        assert_eq!(paste("/index ", true), r"C:\Work");
        assert_eq!(paste("/index", false), r"C:\Work");
        assert_eq!(paste("/status", true), r"C:\Work");
        assert_eq!(paste("budget", true), r"C:\Work");
        let ctrl = Modifiers::CTRL | Modifiers::COMMAND; // as egui-winit reports Ctrl on Windows
        assert!(matches!(clipboard_shortcut(Key::V, ctrl), Some(egui::Event::Paste(_))));
        assert!(matches!(clipboard_shortcut(Key::Insert, Modifiers::SHIFT), Some(egui::Event::Paste(_))));
        assert!(matches!(clipboard_shortcut(Key::C, ctrl), Some(egui::Event::Copy)));
        assert!(matches!(clipboard_shortcut(Key::X, ctrl), Some(egui::Event::Cut)));
        assert!(clipboard_shortcut(Key::K, ctrl).is_none() && clipboard_shortcut(Key::V, Modifiers::NONE).is_none());
    }

    #[test]
    fn command_navigation_keeps_the_selected_item_across_index_refreshes() {
        let mut app = with_results(12); app.selected = 7;
        let selected = app.results[7].saved.as_ref().unwrap().path.clone();
        app.navigate("/actions".into());
        assert_eq!(app.command_target.as_ref().unwrap().saved.as_ref().unwrap().path, selected);
        assert_eq!(app.return_query, "excel budget");
        app.last_query = None; app.refresh();
        assert_eq!(app.command_target.as_ref().unwrap().saved.as_ref().unwrap().path, selected);
        assert_eq!(app.return_query, "excel budget");
        assert!(app.results.iter().any(|i| matches!(&i.action, Action::Pin(s) if s.path == selected)));
        assert!(app.results.iter().any(|i| matches!(&i.action, Action::Preview(s, _) if s.path == selected)));
    }

    #[test]
    fn settings_refresh_does_not_discard_edits_and_answers_copy_raw_values() {
        let mut app = launcher(); app.query = "/settings".into(); app.refresh();
        app.settings_folders = r"C:\Work".into(); app.last_query = None; app.refresh();
        assert_eq!(app.settings_folders, r"C:\Work");
        app.query = "/convert 10 miles in km".into(); app.refresh();
        assert!(matches!(&app.results[0].action, Action::Copy(value) if value == "16.09344"));
        assert!(!app.searching);
    }

    #[test]
    fn preview_answers_survive_refresh_and_stale_requests_cannot_replace_them() {
        let mut app = launcher();
        let file = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/budget.xlsx");
        let entry = Entry::new("budget.xlsx".into(), file.into(), Kind::File, 1);
        app.query = "excel budget".into(); app.ui_query = app.query.clone(); app.searching = false;
        app.results = vec![app.entry_item(&entry)];
        let (tx, rx) = mpsc::channel(); app.preview_tx = tx;
        app.navigate("/preview".into());
        let request = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let lines = crate::preview::read(&request.0, &request.1).unwrap();
        app.accept_preview(request.clone(), Ok(lines)); app.refresh();
        assert!(app.results.iter().any(|i| i.title.contains('!') && i.copy.is_some()));
        assert!(rx.try_recv().is_err(), "a refresh must not restart an identical preview");
        app.accept_preview(("different.xlsx".into(), "".into()), Err("stale".into()));
        assert!(app.preview_lines.as_ref().unwrap().is_ok());
        assert_eq!(app.preview_key, Some(request));
    }

    #[test]
    fn the_search_bar_accepts_slash_text_and_renders_help() {
        let mut app = launcher(); let ctx = ui_context();
        app.query.clear(); app.refresh();
        ui_frame(&mut app, &ctx, 900., Vec::new());
        ui_frame(&mut app, &ctx, 900., vec![egui::Event::Text("/help".into())]);
        assert_eq!(app.query, "/help"); app.refresh();
        assert!(!app.searching); assert!(app.results.iter().any(|i| i.title == "Keyboard shortcuts"));
    }

    #[test]
    fn favorites_are_available_without_a_file_index_and_explicit_alias_wins() {
        let mut app = launcher();
        let saved = crate::library::Saved { name: "Invoices".into(), path: r"C:\Work\Invoices".into(), kind: Kind::Folder };
        app.library.pins.push(saved.clone()); app.library.aliases.insert("inv".into(), saved.clone());
        app.query.clear(); app.refresh(); assert_eq!(app.results[0].title, "Invoices");
        app.query = "inv".into(); app.refresh(); assert_eq!(app.results[0].label, "Alias");
        assert!(matches!(&app.results[0].action, Action::Open(p) if *p == saved.path));
    }

    fn ui_context() -> egui::Context {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        for family in [theme::ICONS, theme::STRONG, theme::DISPLAY] {
            fonts.families.insert(FontFamily::Name(family.into()), fonts.families[&FontFamily::Proportional].clone());
        }
        ctx.set_fonts(fonts);
        ctx
    }

    fn ui_frame(app: &mut Lumen, ctx: &egui::Context, height: f32, events: Vec<egui::Event>) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, height))),
            time: Some(ctx.input(|i| i.time) + 1.0 / 60.0),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            app.handle_keys(ctx);
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| app.draw(ui));
        });
        // No GPU backend in these input/rendering tests.
        output.textures_delta.clear();
        output
    }

    fn key(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    fn rendered_results(output: &egui::FullOutput) -> Vec<String> {
        output.shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text.starts_with("budget ") => Some(text.galley.job.text.clone()),
            _ => None,
        }).collect()
    }

    #[test]
    fn results_are_not_limited_to_the_visible_rows() {
        let app = with_results(1000);
        assert_eq!(app.results.len(), 1001);
        assert_eq!(app.results[999].title, "budget 00999.xlsx");
        assert!(app.results.last().unwrap().title.starts_with("Search the web"));
    }

    #[test]
    fn refresh_preserves_browsing_and_new_queries_start_at_the_top() {
        let mut app = with_results(100);
        app.selected = 50;
        app.selection_by_user = true;
        app.scroll_offset = 45.0 * ROW_H;
        app.scroll_to_selected = false;
        let selected = app.results[50].title.clone();
        app.refresh();
        assert_eq!(app.results[app.selected].title, selected);
        assert_eq!(app.scroll_offset, 45.0 * ROW_H);
        assert!(!app.scroll_to_selected, "background refresh must not snap a browsed list back");

        app.found.insert(0, Entry::new("budget new.xlsx".into(), r"C:\Reports\budget new.xlsx".into(), Kind::File, 1));
        app.refresh();
        assert_eq!(app.results[app.selected].title, selected);
        assert_eq!(app.selected, 51);
        assert!(app.scroll_to_selected, "keep a moved keyboard selection visible");

        app.query = "excel forecast".into();
        app.refresh();
        assert_eq!(app.selected, 0);
        assert_eq!(app.scroll_offset, 0.0);
        assert!(app.scroll_to_selected);
    }

    #[test]
    fn wheel_and_trackpad_reach_results_beyond_the_first_page() {
        for source in [egui::MouseWheelSource::Unknown, egui::MouseWheelSource::Trackpad] {
            let mut app = with_results(1000);
            let ctx = ui_context();
            let initial = ui_frame(&mut app, &ctx, 900.0, vec![]);
            assert!(rendered_results(&initial).contains(&"budget 00000.xlsx".into()));
            assert!(rendered_results(&initial).len() <= MAX_VISIBLE_ROWS + 2, "large lists must render only a viewport");
            ui_frame(&mut app, &ctx, 900.0, vec![egui::Event::PointerMoved(pos2(500.0, 450.0))]);
            ui_frame(&mut app, &ctx, 900.0, vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, -ROW_H * 20.0),
                phase: egui::TouchPhase::Move,
                source,
                modifiers: Modifiers::NONE,
            }]);
            for _ in 0..12 {
                ui_frame(&mut app, &ctx, 900.0, vec![]);
            }
            assert!(app.scroll_offset > ROW_H * MAX_VISIBLE_ROWS as f32, "scroll input must reveal additional matches");
            let scrolled = ui_frame(&mut app, &ctx, 900.0, vec![]);
            assert!(!rendered_results(&scrolled).contains(&"budget 00000.xlsx".into()));
            assert!(rendered_results(&scrolled).len() <= MAX_VISIBLE_ROWS + 2);
            assert_eq!(app.selected, 0, "browsing must not silently change the activation target");

            ui_frame(&mut app, &ctx, 900.0, vec![key(Key::ArrowDown, Modifiers::NONE)]);
            assert_eq!(app.selected, 1);
            assert!(app.scroll_offset <= ROW_H, "keyboard input must bring selection back into view");

            app.query = "excel forecast".into();
            app.refresh();
            for _ in 0..3 {
                ui_frame(&mut app, &ctx, 900.0, vec![]);
                assert_eq!(app.scroll_offset, 0.0, "a new search must not inherit scroll momentum");
            }
        }
    }

    #[test]
    fn page_keys_and_endpoints_follow_selection_on_short_displays() {
        for height in [300.0, 420.0, 900.0] {
            let mut app = with_results(100);
            let ctx = ui_context();
            ui_frame(&mut app, &ctx, height, vec![]);
            ui_frame(&mut app, &ctx, height, vec![key(Key::PageDown, Modifiers::NONE)]);
            assert_eq!(app.selected, visible_result_rows(height));
            assert!(app.scroll_offset > 0.0);
            ui_frame(&mut app, &ctx, height, vec![key(Key::End, Modifiers::CTRL)]);
            assert_eq!(app.selected, 100);
            assert!(app.scroll_offset >= (101 - visible_result_rows(height)) as f32 * ROW_H - 1.0, "height {height}: offset {}", app.scroll_offset);
            ui_frame(&mut app, &ctx, height, vec![key(Key::PageUp, Modifiers::NONE)]);
            assert_eq!(app.selected, 100 - visible_result_rows(height));
            ui_frame(&mut app, &ctx, height, vec![key(Key::Home, Modifiers::CTRL)]);
            assert_eq!(app.selected, 0);
            assert_eq!(app.scroll_offset, 0.0);
            ui_frame(&mut app, &ctx, height, vec![key(Key::ArrowUp, Modifiers::NONE)]);
            assert_eq!(app.selected, 0);
        }
    }

    #[test]
    fn dragging_the_scrollbar_reaches_later_results() {
        let mut app = with_results(1000);
        let ctx = ui_context();
        ui_frame(&mut app, &ctx, 900.0, vec![]);
        let output = ui_frame(&mut app, &ctx, 900.0, vec![]);
        let thumb = output.shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect) if rect.rect.width() <= 16.0 && rect.rect.left() > 800.0 && rect.rect.top() > 200.0 => Some(rect.rect),
            _ => None,
        }).min_by(|a, b| a.height().total_cmp(&b.height())).expect("overflowing results need a scrollbar");
        let start = thumb.center();
        let end = start + vec2(0.0, 300.0);
        ui_frame(&mut app, &ctx, 900.0, vec![egui::Event::PointerMoved(start)]);
        ui_frame(&mut app, &ctx, 900.0, vec![egui::Event::PointerButton {
            pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE,
        }]);
        ui_frame(&mut app, &ctx, 900.0, vec![egui::Event::PointerMoved(end)]);
        ui_frame(&mut app, &ctx, 900.0, vec![egui::Event::PointerButton {
            pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE,
        }]);
        assert!(app.scroll_offset > ROW_H * MAX_VISIBLE_ROWS as f32);
        assert_eq!(app.selected, 0, "dragging must not activate a row");
    }

    #[test]
    fn a_result_below_the_first_page_is_clickable_and_clipped_to_the_list() {
        let _lock = WINDOW_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let _restore = visible_window();
        let mut app = with_results(100);
        // A first click only arms confirmation, so the test never runs a system action.
        app.results[60] = system_item(crate::system::find("restart").0[0]);
        app.select_result(60);
        let ctx = ui_context();
        let output = ui_frame(&mut app, &ctx, 900.0, vec![]);
        let (position, clip) = output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == "Restart" => Some((text.pos + vec2(10.0, 10.0), shape.clip_rect)),
            _ => None,
        }).expect("keyboard selection must render a result far below the first page");
        assert!(clip.contains(position));
        assert!(clip.height() <= ROW_H * MAX_VISIBLE_ROWS as f32 + 16.0, "rows must not paint over the fixed header or footer");
        ui_frame(&mut app, &ctx, 900.0, vec![egui::Event::PointerMoved(position)]);
        for pressed in [true, false] {
            ui_frame(&mut app, &ctx, 900.0, vec![egui::Event::PointerButton {
                pos: position, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE,
            }]);
        }
        assert_eq!(app.selected, 60);
        assert!(app.is_armed("Restart"), "clicking a scrolled result must dispatch that result");
    }

    #[test]
    fn result_navigation_cancels_a_queued_open_and_power_confirmation() {
        let mut app = launcher();
        app.select_result(2);
        assert!(app.pending.is_none());
        assert!(app.armed.is_none());
        assert!(app.selection_by_user);
    }

    #[test]
    fn hotkey_dismissal_cancels_queued_activation_and_confirmation() {
        let _lock = WINDOW_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let _restore = visible_window();
        let mut app = launcher();
        assert!(app.is_armed("Restart"));

        win::toggle(); // The real callback used by the Win+Space hook.
        assert_eq!(app.sync_visibility(), None);
        assert!(app.pending.is_none(), "dismissed Enter must not execute when results arrive");
        assert!(!app.is_armed("Restart"), "dismissal must require fresh confirmation");
    }

    #[test]
    fn reopening_between_frames_starts_without_old_activation_or_consent() {
        let _lock = WINDOW_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let _restore = visible_window();
        let mut app = launcher();

        win::toggle();
        win::toggle(); // Hide and reopen before the UI observes the hidden state.
        assert_eq!(app.sync_visibility(), Some(true));
        assert!(app.pending.is_none(), "reopening must not revive a queued Enter");
        assert!(!app.is_armed("Restart"), "confirmation belongs to the previous session");
    }

    #[test]
    fn editing_away_and_back_does_not_revive_a_queued_activation() {
        let mut app = launcher();
        app.query = "budget".into();
        app.refresh();
        assert!(app.pending.is_none(), "editing cancels intent even before the replacement search completes");
        assert!(app.armed.is_none());

        app.query = "restart".into();
        app.refresh();
        assert!(app.pending.is_none());
        assert!(!app.is_armed("Restart"));
    }

    #[test]
    fn dismissal_after_visibility_poll_rejects_activation_in_the_same_frame() {
        let _lock = WINDOW_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let _restore = visible_window();
        for reopen in [false, true] {
            win::VISIBLE.store(true, Ordering::Release);
            win::JUST_SHOWN.store(false, Ordering::Release);
            let mut app = launcher();
            // An unarmed command only asks for confirmation, even before the fix.
            app.armed = None;
            app.results = vec![system_item(crate::system::find("restart").0[0])];
            assert_eq!(app.sync_visibility(), Some(false));

            win::toggle(); // The hook dismisses while this frame processes results.
            if reopen {
                win::toggle();
            }
            app.execute(0, Mode::Open);

            assert!(app.pending.is_none(), "a stale activation must be cancelled at execution");
            assert!(!app.is_armed("Restart"), "a dismissed session cannot grant new confirmation");
        }
    }

    #[test]
    fn matches_embolden_word_starts_first_and_never_split_characters() {
        let terms = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(match_ranges("SEAN_BANK_STATEMENTS", &terms(&["statement"])), vec![10..19]);
        assert_eq!(match_ranges("StarlingStatement.csv", &terms(&["statement"])), vec![8..17], "mid-word when no word starts with it");
        assert_eq!(match_ranges("Bank statements", &terms(&["state", "bank", "stat"])), vec![0..4, 5..10], "overlaps merge");
        assert_eq!(match_ranges("Café budget", &terms(&["é", "bud"])), vec![3..5, 6..9]);
        assert!(match_ranges("budget", &terms(&[""])).is_empty());
        assert_eq!(highlight_terms("excel ext:xlsx budget q3"), ["budget", "q3"]);
        assert!(highlight_terms("/help keyboard").is_empty());
    }

    #[test]
    fn the_query_shows_scope_words_and_commands_in_the_accent() {
        let p = Palette::new(false, theme::Accent::default());
        for (query, token) in [("excel budget", "excel "), ("mail  invoice", "mail  "), ("/help keyboard", "/help"), ("budget", ""), ("", "")] {
            let job = query_job(query, &p);
            assert_eq!(job.text, query);
            let accented: String =
                job.sections.iter().filter(|s| s.format.color == p.accent).map(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0]).collect();
            assert_eq!(accented, token, "{query}");
        }
        assert_eq!(mode_glyph("/settings"), (Some(G_SETTINGS), true));
        assert_eq!(mode_glyph("/"), (None, true));
        assert_eq!(mode_glyph("excel budget"), (Some(G_SHEET), true));
        assert_eq!(mode_glyph("budget"), (Some(G_SEARCH), false));
        assert_eq!(thousands(103_158), "103,158");
        assert_eq!(thousands(999), "999");
    }

    #[test]
    fn home_groups_pins_and_recents_under_headers_without_shifting_rows() {
        let mut app = launcher();
        let saved = |name: &str| crate::library::Saved { name: name.into(), path: format!(r"C:\Work\{name}"), kind: Kind::Folder };
        app.library.pins = vec![saved("Invoices"), saved("Budgets")];
        app.library.recent = vec![saved("Budgets"), saved("Q3"), saved("Q4")];
        app.query.clear();
        app.refresh();
        let headers: Vec<_> = app.results.iter().map(|item| item.header).collect();
        assert_eq!(headers, [Some("Pinned"), None, Some("Recent"), None], "a pinned recent appears once");
        let rows = Rows::new(&app.results);
        assert_eq!(rows.top(1), ROW_H + HEADER_H);
        assert_eq!(rows.top(2), 2.0 * ROW_H + HEADER_H);
        assert_eq!(rows.total(), 4.0 * ROW_H + 2.0 * HEADER_H);
        assert_eq!(rows.at(0.0), 0);
        assert_eq!(rows.at(rows.top(2) + 1.0), 2);
        assert_eq!(rows.at(rows.total() + 100.0), 3, "beyond the end is the last row");
    }

    #[test]
    fn the_selection_glides_one_row_and_jumps_further() {
        let mut slide = Slide::default();
        slide.set(56.0, 10.0, true, 84.0);
        assert!(slide.running(10.03));
        let midway = slide.at(10.03);
        assert!(midway > 0.0 && midway < 56.0, "{midway}");
        assert_eq!(slide.at(10.2), 56.0);
        slide.set(560.0, 11.0, true, 84.0);
        assert_eq!(slide.at(11.0), 560.0, "a long jump moves at once");
        slide.set(616.0, 12.0, true, 84.0);
        slide.set(672.0, 12.02, true, 84.0);
        assert_eq!(slide.at(12.02), 672.0, "a held key moves at once");
        slide.set(728.0, 13.0, false, 84.0);
        assert_eq!(slide.at(13.0), 728.0);
    }
}
