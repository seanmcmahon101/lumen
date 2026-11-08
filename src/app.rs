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

mod commands;

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
const MAX_VISIBLE_ROWS: usize = 8;
const RADIUS: u8 = 18;

// Segoe Fluent Icons code points
const G_SEARCH: char = '\u{E721}';
const G_CALC: char = '\u{E8EF}';
const G_GLOBE: char = '\u{E774}';
const G_POWER: char = '\u{E7E8}';
const G_SETTINGS: char = '\u{E713}';
const G_OPEN: char = '\u{E8A7}';

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
    perf_rows: Vec<(String, String)>,
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
        let settings = crate::settings::get();
        crate::hook::set_hotkey(settings.hotkey);
        let repaint = ctx.clone();
        let (preview_tx, preview_rx) = crate::preview::spawn(move || repaint.request_repaint());
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
            perf_rows: Vec::new(),
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

        if let Some(answer) = crate::convert::units(&t).or_else(|| crate::convert::date(&t)) {
            self.results.push(answer_item(answer));
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

        let available_rows = visible_result_rows(screen.height());
        let settings = self.query.trim().eq_ignore_ascii_case("/settings");
        let rows = self.results.len().min(available_rows);
        let w = WIDTH.min(screen.width() - 40.0);
        let list_h = |rows: usize| if rows > 0 { 1.0 + LIST_PAD * 2.0 + rows as f32 * ROW_H + FOOTER_H } else { 48.0 };
        let settings_h = 500.0_f32.min((screen.height() - BAR_H - FILTER_H - 48.0).max(180.0));
        let h = BAR_H + FILTER_H + if settings { settings_h } else { list_h(rows) };
        // Pin the top where a full list would be centred, so the bar stays still as results change.
        let full_h = BAR_H + FILTER_H + list_h(available_rows);
        let panel = Rect::from_min_size(pos2(screen.center().x - w / 2.0, (screen.center().y - full_h / 2.0).min(screen.bottom() - h - 16.0).max(16.0)), vec2(w, h));

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
                    .hint_text(RichText::new("Search, or type / for commands").font(font.clone()).color(HINT))
                    .desired_width(text_rect.width())
                    .lock_focus(!settings)
                    .show(ui)
            })
            .inner;
        if !settings && !output.response.has_focus() {
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
            if self.query.trim_start().starts_with('/') {
                ui.label(RichText::new("Commands").size(16.0).color(SUBTLE));
                if ui.link(RichText::new("Back to search").size(14.0)).clicked() { self.navigate(self.return_query.clone()); }
            } else {
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
            }
            if ui.link(RichText::new("Help").size(14.0)).clicked() { self.navigate("/help".into()); }
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
                    .on_hover_text(format!("Download Lumen {}", release.version));
                if response.clicked() {
                    crate::update::open_latest(release);
                    self.hide();
                }
            } else if let Some(status) = status {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(status).size(14.0).color(SUBTLE));
                });
            }
        });
        if settings {
            self.draw_settings(ui, Rect::from_min_max(pos2(panel.left(), bar.bottom() + FILTER_H), panel.max));
            return;
        }
        if rows == 0 {
            painter.text(
                pos2(panel.left() + TEXT_INSET, panel.bottom() - 24.0),
                Align2::LEFT_CENTER,
                "Try excel budget, mail invoice, or /help",
                FontId::proportional(15.0),
                SUBTLE,
            );
            return;
        }
        painter.hline(panel.x_range(), bar.bottom() + FILTER_H, (1.0, DIVIDER));

        let mut budget = 3; // icon loads per frame
        let mut clicked = None;
        let list_rect = Rect::from_min_size(
            pos2(panel.left() + 8.0, bar.bottom() + FILTER_H + 1.0 + LIST_PAD),
            vec2(w - 16.0, rows as f32 * ROW_H),
        );
        let scroll = ui
            .scope_builder(UiBuilder::new().id_salt("results-viewport").max_rect(list_rect).layout(Layout::top_down(Align::Min)), |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                // Reserve room for a draggable scrollbar rather than covering row labels.
                ui.spacing_mut().scroll = egui::style::ScrollStyle {
                    bar_width: 8.0,
                    handle_min_length: 24.0,
                    dormant_handle_opacity: 0.6,
                    ..egui::style::ScrollStyle::solid()
                };
                let mut area = egui::ScrollArea::vertical()
                    .id_salt("lumen-results")
                    .max_height(list_rect.height())
                    .scroll_bar_visibility(if self.results.len() > rows {
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
                    let top = self.selected as f32 * ROW_H;
                    let offset = self.scroll_offset.clamp((top + ROW_H - list_rect.height()).max(0.0), top);
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
                let output = area.show_rows(ui, ROW_H, self.results.len(), |ui, visible| {
                    for i in visible {
                        if self.draw_result(ui, i, &mut budget) {
                            clicked = Some(i);
                        }
                    }
                });
                self.scroll_offset = output.state.clamped_offset().y;
                output
            })
            .inner;

        // Key hints
        let mut x = panel.left() + 22.0;
        let fy = panel.bottom() - FOOTER_H / 2.0;
        painter.hline(panel.x_range(), panel.bottom() - FOOTER_H, (1.0, DIVIDER));
        let first = (self.scroll_offset / ROW_H).floor() as usize + 1;
        let last = ((self.scroll_offset + scroll.inner_rect.height()) / ROW_H).ceil() as usize;
        let counter = painter.text(
            pos2(panel.right() - 22.0, fy),
            Align2::RIGHT_CENTER,
            format!("{first}–{} of {}", last.min(self.results.len()), self.results.len()),
            FontId::proportional(14.0),
            SUBTLE,
        );
        let painter = painter.with_clip_rect(Rect::from_min_max(panel.min, pos2(counter.left() - 18.0, panel.bottom())));
        let hints = if !self.search_note.is_empty() {
            vec![("", self.search_note.as_str())]
        } else {
            let selected = self.results.get(self.selected);
            let copy_action = selected.is_some_and(|item| matches!(item.action, Action::Copy(_)));
            let asks = selected.is_some_and(|item| item.action.confirm().is_some());
            let mut hints = if selected.is_some_and(|item| matches!(item.action, Action::Noop)) { vec![("↑↓", "Browse")] }
                else { vec![("Enter", if copy_action { "Copy" } else if asks { "Confirm" } else { "Open" })] };
            if selected.is_some_and(|item| item.reveal.is_some()) {
                hints.push(("Ctrl+K", "Actions"));
            }
            if selected.is_some_and(|item| item.complete.is_some()) {
                hints.push(("Tab", "Complete"));
            }
            hints.push(("Esc", if self.query.trim_start().starts_with('/') { "Back" } else { "Close" }));
            hints
        };
        for (key, what) in hints {
            let k = painter.text(pos2(x, fy), Align2::LEFT_CENTER, key, FontId::proportional(14.0), SUBTLE);
            let d = painter.text(pos2(k.right() + 5.0, fy), Align2::LEFT_CENTER, what, FontId::proportional(14.0), HINT);
            x = d.right() + 18.0;
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

    fn draw_result(&mut self, ui: &mut egui::Ui, i: usize, budget: &mut u32) -> bool {
        let ctx = ui.ctx().clone();
        let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
        resp.clone().on_hover_text(&self.results[i].subtitle);
        let painter = ui.painter(); // Inherit the scroll area's clipping rectangle.
        if i == self.selected {
            painter.rect_filled(row, 10, SELECTED);
        } else if resp.hovered() {
            painter.rect_filled(row, 10, HOVER);
        }

        let icon_rect = Rect::from_center_size(pos2(row.left() + 28.0, row.center().y), Vec2::splat(32.0));
        let (glyph, icon) = (self.results[i].glyph, self.results[i].icon.clone());
        if let Some(g) = glyph {
            painter.rect_filled(icon_rect, 7, GLYPH_BG);
            painter.text(icon_rect.center(), Align2::CENTER_CENTER, g, icon_font(18.0), TEXT);
        } else if let Some((key, path)) = icon
            && let Some(tex) = self.icon(&ctx, &key, &path, budget)
        {
            painter.image(tex.id(), icon_rect, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        }

        let item = &self.results[i];
        let label = painter.text(pos2(row.right() - 14.0, row.center().y), Align2::RIGHT_CENTER, &item.label, FontId::proportional(14.0), HINT);
        let text_x = row.left() + TEXT_INSET - 8.0;
        let max_w = label.left() - text_x - 16.0;
        let title = painter.layout_job(single_line(&item.title, 19.0, TEXT, max_w));
        let sub = match item.action.confirm() {
            Some(what) if self.is_armed(&item.title) => {
                painter.layout_job(single_line(&format!("Press Enter again to {what}"), 15.0, TEXT, max_w))
            }
            _ => painter.layout_job(single_line(&item.subtitle, 15.0, SUBTLE, max_w)),
        };
        let top = row.center().y - (title.size().y + 3.0 + sub.size().y) / 2.0;
        painter.galley(pos2(text_x, top), title.clone(), TEXT);
        painter.galley(pos2(text_x, top + title.size().y + 3.0), sub, SUBTLE);
        resp.clicked()
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
            if self.show_at_launch { win::show() } else { win::hide() }
        }
        let Some(just_shown) = self.sync_visibility() else { return };
        if just_shown {
            let _ = self.update_opened.send(());
            if self.query.trim().eq_ignore_ascii_case("/perf") { self.perf_rows = crate::metrics::snapshot(); }
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
            let _ = self.window_tx.send(());
            self.windows_loading = true;
            ctx.request_repaint();
        } else {
            // A DPI change can resize the native window after the initial move.
            self.display_overlay.fit();
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
    }
}

fn answer_item(answer: crate::convert::Answer) -> Item {
    let mut item = simple(&answer.display, &answer.note, "Copy answer", G_CALC, Action::Copy(answer.copy.clone()));
    item.copy = Some(answer.copy.clone()); item.complete = Some(answer.copy); item
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
            perf_rows: Vec::new(),
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
        let mut fonts = FontDefinitions::default();
        fonts.families.insert(FontFamily::Name("icons".into()), fonts.families[&FontFamily::Proportional].clone());
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
            assert!(app.scroll_offset >= (101 - visible_result_rows(height)) as f32 * ROW_H - 1.0);
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
}
