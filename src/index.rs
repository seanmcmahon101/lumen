//! In-memory index of installed apps and everything under the user profile.

use calamine::{Reader, open_workbook_auto};
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::os::windows::fs::MetadataExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

const MAX_FILES: usize = 250_000;
const MAX_DEPTH: usize = 20;
const SKIP_DIRS: &[&str] = &["appdata", "node_modules", "bin", "obj", "__pycache__", "venv", "site-packages", "packages", "target"];
const HIDDEN_OR_SYSTEM: u32 = 0x2 | 0x4;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Kind {
    App,
    Folder,
    File,
    Email,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub name: String,
    lower: String,
    pub path: String,
    pub kind: Kind,
    depth: u16,
    location: String,
    content: String,
    pub detail: String,
    stamp: (u64, u64, u32), // size, last write and attributes when scanned
    read: bool,             // reusable at `stamp`; transient extraction failures stay unread
    penalty: u16,           // rank points taken off results seldom wanted
}

impl Entry {
    /// Search results carry display metadata; workbook text stays in the index.
    fn result(&self) -> Self {
        Self { name: self.name.clone(), lower: self.lower.clone(), path: self.path.clone(), kind: self.kind,
            depth: self.depth, location: self.location.clone(), content: String::new(), detail: self.detail.clone(),
            stamp: self.stamp, read: self.read, penalty: self.penalty }
    }

    pub(crate) fn new(name: String, path: String, kind: Kind, depth: usize) -> Self {
        let (lower, location) = (name.to_lowercase(), path.to_lowercase());
        Self {
            penalty: penalty(kind, &lower),
            lower,
            location,
            name,
            path,
            kind,
            depth: depth as u16,
            content: String::new(),
            detail: String::new(),
            stamp: (0, 0, 0),
            read: false,
        }
    }
}

/// Source code, scripts and build output.
const CODE_EXTENSIONS: &[&str] = &[
    "c", "h", "cc", "cpp", "cxx", "hpp", "hxx", "inl", "go", "rs", "py", "pyc", "pyd", "pyi", "pyx", "js", "mjs", "cjs", "ts", "tsx", "jsx",
    "java", "class", "kt", "cs", "vb", "rb", "php", "pl", "pm", "lua", "swift", "s", "asm", "bat", "cmd", "ps1", "psm1", "psd1", "ps1xml",
    "sh", "mk", "cmake", "vcxproj", "filters", "props", "targets", "sln", "dll", "pdb", "lib", "obj", "o", "a", "so", "exp", "ilk", "def",
    "rc", "map", "lock", "toml", "mod", "sum", "in", "am", "ac", "m4",
];

/// Code files are seldom what a search is for: they rank below documents but still
/// appear when nothing better matches.
fn penalty(kind: Kind, lower_name: &str) -> u16 {
    if kind == Kind::File && lower_name.rsplit_once('.').is_some_and(|(_, ext)| CODE_EXTENSIONS.contains(&ext)) { 300 } else { 0 }
}

/// A package cache, SDK, vendored code or unpacked versioned release: go\pkg\mod,
/// vendor, third_party, testdata, externals, sdk, libc@v1.55.3, Python-3.14.8 or
/// 10.0.203. Scans list the folder but do not go into it: its files are seldom
/// wanted, and on a developer's PC they can be most of the index.
fn package_folder(location: &str) -> bool {
    let name = location.rsplit('\\').next().unwrap_or(location);
    location.ends_with("\\pkg\\mod")
        || matches!(name, "vendor" | "third_party" | "third-party" | "testdata" | "externals" | "sdk" | "sdks")
        || versioned(name)
}

/// A dotted version, alone or after a name and @ or -: "10.0.203", "libc@v1.55.3",
/// "python-3.14.8". Alone it needs three parts and a short first part, so dates
/// such as 2024.01.15 do not count.
fn versioned(folder: &str) -> bool {
    let parts: Vec<&str> = folder.split('.').collect();
    if parts.len() >= 3 && parts[0].len() <= 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) {
        return true;
    }
    !folder.contains(' ')
        && folder.match_indices(['@', '-']).any(|(i, _)| {
            let rest = &folder[i + 1..];
            let rest = rest.strip_prefix('v').unwrap_or(rest);
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            digits > 0 && rest[digits..].starts_with('.') && rest.as_bytes().get(digits + 1).is_some_and(u8::is_ascii_digit)
        })
}

/// Files created, renamed or deleted in watched folders since the last full scan.
#[derive(Clone, Default)]
struct Changes {
    added: Vec<Entry>,
    added_at: Vec<Instant>,
    shadowed: HashSet<String>,         // locations of `added`, hiding the scanned copies
    removed: HashMap<String, Instant>, // locations gone, with everything under them
}

impl Changes {
    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }

    /// A scanned entry that a later change replaced or deleted.
    fn hides(&self, location: &str) -> bool {
        self.shadowed.contains(location)
            || (!self.removed.is_empty()
                && std::path::Path::new(location).ancestors().any(|a| a.to_str().is_some_and(|a| self.removed.contains_key(a))))
    }

    fn add(&mut self, e: Entry, at: Instant) {
        self.removed.remove(&e.location);
        match self.added.iter().position(|a| a.location == e.location) {
            Some(i) => (self.added[i], self.added_at[i]) = (e, at),
            None => {
                self.shadowed.insert(e.location.clone());
                self.added.push(e);
                self.added_at.push(at);
            }
        }
    }

    fn remove(&mut self, location: String, at: Instant) {
        let folder = format!("{location}\\");
        let mut i = 0;
        while i < self.added.len() {
            if self.added[i].location == location || self.added[i].location.starts_with(&folder) {
                self.shadowed.remove(&self.added[i].location);
                self.added.swap_remove(i);
                self.added_at.swap_remove(i);
            } else {
                i += 1;
            }
        }
        self.removed.insert(location, at);
    }

    /// Forgets changes that a full scan started at `since` has seen for itself.
    fn settle(&mut self, since: Instant) {
        let mut i = 0;
        while i < self.added.len() {
            if self.added_at[i] < since {
                self.shadowed.remove(&self.added[i].location);
                self.added.swap_remove(i);
                self.added_at.swap_remove(i);
            } else {
                i += 1;
            }
        }
        self.removed.retain(|_, at| *at >= since);
    }
}

/// What a folder watch reports.
pub enum Change {
    Added(PathBuf),
    Modified(PathBuf),
    Removed(PathBuf),
}

#[derive(Default)]
pub struct Index {
    entries: RwLock<Arc<Vec<Entry>>>,
    generation: AtomicU64,
    building: AtomicBool,
    rebuild_again: AtomicBool, // a rebuild was asked for while one was running
    mapped: RwLock<HashMap<String, Arc<Vec<Entry>>>>,
    scanning_drives: Mutex<HashSet<String>>,
    changes: RwLock<Arc<Changes>>,
    cached: AtomicBool,
}

impl Index {
    /// Load the previous snapshot off-thread, then reconcile it with the disk.
    pub fn start(self: &Arc<Self>, on_done: impl Fn() + Send + Sync + 'static) {
        let me = self.clone();
        std::thread::spawn(move || {
            if let Some(entries) = load_cache(&crate::settings::get()) {
                *me.entries.write().unwrap() = Arc::new(entries);
                me.cached.store(true, Ordering::Relaxed);
                me.generation.fetch_add(1, Ordering::Relaxed);
                on_done();
            }
            me.rebuild(on_done);
        });
    }

    pub fn stats(&self) -> (usize, bool) {
        (self.entries.read().unwrap().len() + self.mapped.read().unwrap().values().map(|e| e.len()).sum::<usize>()
            + self.changes.read().unwrap().added.len(), self.cached.load(Ordering::Relaxed))
    }

    pub fn is_building(&self) -> bool {
        self.building.load(Ordering::Acquire) || !self.scanning_drives.lock().unwrap().is_empty()
    }
    /// Bumps every time a rebuild finishes, so the UI knows to re-run the query.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub fn rebuild(self: &Arc<Self>, on_done: impl Fn() + Send + Sync + 'static) {
        self.rebuild_impl(Arc::new(on_done));
    }

    // Takes the callback as Arc<dyn Fn> so a requeued rebuild calls itself at
    // runtime. Passing a fresh impl-Fn closure would nest one new generic type
    // per requeue, and the compiler stops instantiating that chain at its
    // recursion limit.
    fn rebuild_impl(self: &Arc<Self>, on_done: Arc<dyn Fn() + Send + Sync + 'static>) {
        let settings = crate::settings::get();
        let roots = network_roots(if settings.profile { mapped_roots() } else { Vec::new() }, &settings.folders);
        {
            let mut mapped = self.mapped.write().unwrap(); let previous = mapped.len();
            mapped.retain(|root, _| roots.iter().any(|r| r.eq_ignore_ascii_case(root)));
            if mapped.len() != previous { self.generation.fetch_add(1, Ordering::Relaxed); on_done(); }
        }
        // Each mapped share has its own worker: a disconnected server cannot hold
        // up local indexing, another drive, or the search/render threads.
        let (network_index, network_done) = (self.clone(), on_done.clone());
        std::thread::spawn(move || {
            if !crate::settings::get().files && !crate::settings::get().excel { return; }
            for root in roots.clone() {
                if crate::settings::get().excluded(&root) { continue; }
                // Folders added inside this root have workers of their own.
                let nested: Vec<String> = roots.iter().filter(|r| inside(r, &root)).cloned().collect();
                if !network_index.scanning_drives.lock().unwrap().insert(root.clone()) {
                    continue;
                }
                let (index, done) = (network_index.clone(), network_done.clone());
                std::thread::spawn(move || {
                    scan_mapped(&root, &nested, |entries| {
                        let current = crate::settings::get();
                        let wanted = current.folders.iter().any(|p| p.eq_ignore_ascii_case(&root)) || (current.profile && root.len() == 3);
                        if (!current.files && !current.excel) || current.excluded(&root) || !wanted { return; }
                        index.mapped.write().unwrap().insert(root.clone(), Arc::new(entries.to_vec()));
                        index.generation.fetch_add(1, Ordering::Relaxed);
                        done();
                    });
                    index.scanning_drives.lock().unwrap().remove(&root);
                    done();
                });
            }
        });
        // A rebuild asked for mid-scan runs straight after it: the scan may already have
        // passed the folders whose changes overflowed a watch. Flag first, then claim.
        self.rebuild_again.store(true, Ordering::SeqCst);
        if self.building.swap(true, Ordering::SeqCst) {
            return;
        }
        self.rebuild_again.store(false, Ordering::SeqCst);
        let me = self.clone();
        std::thread::spawn(move || {
            let settings = crate::settings::get();
            let revision = crate::settings::REVISION.load(Ordering::Acquire);
            let publish = |list: Vec<Entry>| {
                *me.entries.write().unwrap() = Arc::new(list);
                me.generation.fetch_add(1, Ordering::Relaxed);
                on_done();
            };
            let previous = me.entries.read().unwrap().clone();
            let mut list = if settings.apps { start_apps() } else { Vec::new() };
            if list.is_empty() && settings.apps {
                start_menu_shortcuts(&mut list);
            }
            // The first scan shows apps at once; a refresh keeps searching the
            // previous index until the new one is complete.
            if previous.is_empty() {
                publish(list.clone());
            }
            let walk_started = Instant::now();
            if settings.files || settings.excel { profile_files(&mut list); }
            // Text read last time is still right for files that have not changed.
            let mut content_bytes = reuse_content(&mut list, &previous);
            drop(previous);
            publish(list.clone());
            me.update_changes(|changes| changes.settle(walk_started));
            for e in &mut list {
                if content_bytes >= CONTENT_BUDGET {
                    break;
                }
                if !e.read {
                    enrich(e);
                    content_bytes += e.content.len();
                }
            }
            publish(list);
            me.building.store(false, Ordering::SeqCst);
            on_done();
            if revision == crate::settings::REVISION.load(Ordering::Acquire) {
                let snapshot = me.entries.read().unwrap().clone();
                save_cache(&settings, &snapshot);
            }
            if me.rebuild_again.swap(false, Ordering::SeqCst) {
                me.rebuild_impl(on_done.clone());
            }
        });
    }

    /// Changes the overlay under its lock, so watchers on different folders cannot
    /// overwrite each other, then has searches run again.
    fn update_changes(&self, change: impl FnOnce(&mut Changes)) {
        let mut current = self.changes.write().unwrap();
        let mut next = (**current).clone();
        change(&mut next);
        *current = Arc::new(next);
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Applies what a watch on `root` saw: names at once, then new workbook text.
    pub fn apply(&self, root: &std::path::Path, changes: Vec<Change>, on_change: &dyn Fn()) {
        let settings = crate::settings::get();
        if !settings.files && !settings.excel { return; }
        let now = Instant::now();
        // Read the disk first; the overlay is locked only to record the result.
        let mut found: Vec<Result<Entry, String>> = Vec::new();
        for change in changes {
            let is_addition = matches!(&change, Change::Added(_));
            match change {
                Change::Removed(path) => found.push(Err(path.to_string_lossy().to_lowercase())),
                Change::Added(path) | Change::Modified(path) => {
                    if settings.excluded(&path.to_string_lossy()) { continue; }
                    let Some(depth) = watched_depth(root, &path) else { continue };
                    let Ok(md) = std::fs::symlink_metadata(&path) else { continue };
                    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    let Some(e) = scanned_entry(name, &path, &md, depth) else { continue };
                    let walk_in = is_addition && e.kind == Kind::Folder && descends(&e, &md);
                    found.push(Ok(e));
                    // A folder moved or renamed in brings its contents.
                    if walk_in {
                        let mut inside = Vec::new();
                        walk(VecDeque::from([(path, depth)]), &mut inside, 10_000);
                        found.extend(inside.into_iter().map(Ok));
                    }
                }
            }
        }
        let unread: Vec<Entry> = found.iter().flatten().filter(|e| e.kind == Kind::File && needs_reading(&e.lower)).cloned().collect();
        self.update_changes(|changes| {
            for item in found {
                match item {
                    Ok(e) => changes.add(e, now),
                    Err(location) => changes.remove(location, now),
                }
            }
        });
        on_change();
        if unread.is_empty() {
            return;
        }
        // A save can replace or modify the file; read its current text in either case.
        let read: Vec<Entry> = unread
            .into_iter()
            .map(|mut e| {
                enrich(&mut e);
                e
            })
            .collect();
        self.update_changes(|changes| {
            for e in read {
                // Unless a newer change replaced it while the text was read.
                if let Some(i) = changes.added.iter().position(|a| a.location == e.location && a.stamp == e.stamp) {
                    changes.added[i] = e;
                }
            }
        });
        on_change();
    }

    /// The indexed entry with this exact path.
    pub fn get(&self, path: &str) -> Option<Entry> {
        let settings = crate::settings::get();
        let location = path.to_lowercase();
        let changes = self.changes.read().unwrap().clone();
        if let Some(e) = changes.added.iter().find(|e| e.location == location) {
            return allowed(e, &settings).then(|| e.clone());
        }
        if changes.hides(&location) {
            return None;
        }
        let entries = self.entries.read().unwrap().clone();
        if let Some(e) = entries.iter().find(|e| e.location == location) {
            return allowed(e, &settings).then(|| e.clone());
        }
        self.mapped.read().unwrap().values().find_map(|entries| entries.iter().find(|e| e.location == location && allowed(e, &settings)).cloned())
    }

    pub fn search(&self, query: &str, max: usize, uses: impl Fn(&str) -> u32 + Sync) -> Vec<Entry> {
        let mut snapshots = vec![self.entries.read().unwrap().clone()];
        snapshots.extend(self.mapped.read().unwrap().values().cloned());
        let q = query.trim().to_lowercase();
        if q.is_empty() || max == 0 {
            return Vec::new();
        }
        let mut scope = "";
        let mut ext = "";
        let tokens: Vec<&str> = q
            .split_whitespace()
            .enumerate()
            .filter(|(position, t)| match *t {
                "excel" | "sheets" | "type:excel" if *position == 0 => {
                    scope = "excel";
                    false
                }
                "email" | "mail" | "type:email" if *position == 0 => {
                    scope = "email";
                    false
                }
                "type:app" => {
                    scope = "app";
                    false
                }
                _ if t.starts_with("ext:") => {
                    ext = t[4..].trim_start_matches('.');
                    false
                }
                _ => true,
            })
            .map(|(_, token)| token)
            .collect();
        let needle = tokens.join(" ");
        let query = Query { scope, ext, needle: &needle, tokens: &tokens };

        // Score small slices of the index on rayon's pool, which stays running between
        // searches (starting threads costs milliseconds on Windows) and lets idle cores
        // take slices from busy ones, since workbook text is unevenly spread.
        // Scanned entries are checked against later changes; watched additions are not.
        let changes = self.changes.read().unwrap().clone();
        let settings = crate::settings::get();
        let filtering = !settings.apps || !settings.files || !settings.excel || !settings.mail || !settings.exclusions.is_empty();
        let visible = |e: &Entry| allowed(e, &settings);
        let hides = |e: &Entry| changes.hides(&e.location) || !visible(e);
        let check: Option<&(dyn Fn(&Entry) -> bool + Sync)> = (!changes.is_empty() || filtering).then_some(&hides);
        let hides_added = |e: &Entry| !visible(e);
        let added_check: Option<&(dyn Fn(&Entry) -> bool + Sync)> = filtering.then_some(&hides_added);
        let mut chunks: Vec<SearchChunk<'_>> =
            snapshots.iter().flat_map(|entries| entries.chunks(CHUNK)).map(|chunk| (chunk, check)).collect();
        chunks.extend(changes.added.chunks(CHUNK).map(|chunk| (chunk, added_check)));
        let mut best: Vec<(i32, (usize, usize), &Entry)> = chunks
            .par_iter()
            .enumerate()
            .flat_map_iter(|(k, &(chunk, hidden))| {
                // (slice, position) keeps index order on equal scores.
                best_in(chunk, &query, max, &uses, hidden).into_iter().map(move |(s, i, e)| (s, (k, i), e))
            })
            .collect();
        best.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        best.into_iter().take(max).map(|(_, _, e)| e.result()).collect()
    }
}

/// Entries per search slice.
const CHUNK: usize = 2048;
type EntryPredicate<'a> = dyn Fn(&Entry) -> bool + Sync + 'a;
type SearchChunk<'a> = (&'a [Entry], Option<&'a EntryPredicate<'a>>);

fn allowed(e: &Entry, settings: &crate::settings::Settings) -> bool {
    !settings.excluded(&e.path) && match e.kind {
        Kind::App => settings.apps, Kind::Folder => settings.files || settings.excel,
        Kind::File => if spreadsheet(&e.lower) { settings.excel } else { settings.files }, Kind::Email => settings.mail,
    }
}

struct Query<'a> {
    scope: &'a str,
    ext: &'a str,
    needle: &'a str,
    tokens: &'a [&'a str],
}

/// The best `max` entries of `entries` with their scores and positions, leaving out
/// those `hidden` rejects (asked only of entries that would make the list).
fn best_in<'e>(
    entries: &'e [Entry],
    q: &Query,
    max: usize,
    uses: &impl Fn(&str) -> u32,
    hidden: Option<&(dyn Fn(&Entry) -> bool + Sync)>,
) -> Vec<(i32, usize, &'e Entry)> {
    // A scrollable search can request every match without allocating usize::MAX.
    let max = max.min(entries.len());
    // A min-heap: the weakest kept result is on top, and on equal scores the later entry.
    let mut heap: BinaryHeap<Reverse<(i32, Reverse<usize>)>> = BinaryHeap::with_capacity(max + 1);
    for (i, e) in entries.iter().enumerate() {
        if (q.scope == "excel" && (e.kind != Kind::File || !spreadsheet(&e.lower)))
            || (q.scope == "email" && !matches!(extension(&e.lower), "eml" | "msg"))
            || (q.scope == "app" && e.kind != Kind::App)
            || (!q.ext.is_empty() && extension(&e.lower) != q.ext)
        {
            continue;
        }
        let mut s = if q.tokens.is_empty() { 100 } else { score(e, q.needle, q.tokens) };
        if s <= 0 {
            continue;
        }
        // Exact folder intent must beat incidental prefixes, even when a
        // similarly named file has a large launch-history boost.
        if e.kind == Kind::Folder && (e.lower == q.needle || e.location == q.needle) {
            s += 1000;
        }
        s -= e.penalty as i32;
        s += match e.kind {
            Kind::App => 200,
            Kind::Folder => 40,
            Kind::File => 0,
            Kind::Email => 0,
        } - e.depth as i32 * 4;
        let u = uses(&e.path);
        if u > 0 {
            s += (u.min(7) as i32 * 60).min(400);
        }
        let makes_list = heap.len() < max || heap.peek().is_some_and(|Reverse((min, _))| s > *min);
        if !makes_list || hidden.is_some_and(|hidden| hidden(e)) {
            continue;
        }
        if heap.len() == max {
            heap.pop();
        }
        heap.push(Reverse((s, Reverse(i))));
    }
    heap.into_iter().map(|Reverse((s, Reverse(i)))| (s, i, &entries[i])).collect()
}

/// Splits leading filter words ("excel ", "mail ", "type:app ", "ext:xlsm ") from
/// the search term. A filter word only counts once a space follows it.
pub fn split_filter(query: &str) -> (&str, &str) {
    let mut rest = query.trim_start();
    let mut first = true;
    while let Some(end) = rest.find(char::is_whitespace) {
        let word = rest[..end].to_lowercase();
        let filter = word == "type:app"
            || word.starts_with("ext:")
            || (first && matches!(word.as_str(), "excel" | "sheets" | "type:excel" | "email" | "mail" | "type:email"));
        if !filter {
            break;
        }
        rest = rest[end..].trim_start();
        first = false;
    }
    query.split_at(query.len() - rest.len())
}

fn mapped_roots() -> Vec<String> {
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    let drives = unsafe { GetLogicalDrives() };
    (0..26).filter_map(|i| {
        if drives & (1 << i) == 0 { return None; }
        let root = format!("{}:\\", (b'A' + i) as char);
        (unsafe { GetDriveTypeW(&windows::core::HSTRING::from(&root)) } == 4).then_some(root)
    }).collect()
}

/// Mapped drives (with default locations on) plus each network folder added in
/// settings. An added folder is a root of its own, so its depth and size limits
/// count from the folder rather than from the top of a large share.
fn network_roots(drives: Vec<String>, folders: &[String]) -> Vec<String> {
    let mut roots = drives;
    for folder in folders.iter().filter(|f| !local_path(f)) {
        if !roots.iter().any(|r| r.eq_ignore_ascii_case(folder)) {
            roots.push(folder.clone());
        }
    }
    roots
}

/// Whether `folder` lies strictly inside `root`.
fn inside(folder: &str, root: &str) -> bool {
    crate::settings::within(folder, root) && !crate::settings::within(root, folder)
}

/// How many entries, and how long, a network scan of `root` may take. A whole
/// share is sampled; a folder the user added is listed in full. A folder added
/// from recent use gets enough for a working folder, so a broad one such as a
/// department's root cannot slow every search.
fn scan_budget(root: &str, settings: &crate::settings::Settings) -> (usize, Duration) {
    if root.len() <= 3 {
        (20_000, Duration::from_secs(30))
    } else if settings.added_from_recent(root) {
        (5_000, Duration::from_secs(30))
    } else {
        (100_000, Duration::from_secs(60))
    }
}

fn scan_mapped(root: &str, nested: &[String], mut publish: impl FnMut(&[Entry])) {
    let settings = crate::settings::get();
    let (max_entries, max_time) = scan_budget(root, &settings);
    let mut entries = Vec::new();
    let mut queue = VecDeque::from([(PathBuf::from(root), 0usize)]);
    let start = Instant::now();
    let mut last_publish = Instant::now();
    while let Some((dir, depth)) = queue.pop_front() {
        if settings.excluded(&dir.to_string_lossy()) { continue; }
        if start.elapsed() > max_time {
            break;
        }
        let Ok(children) = std::fs::read_dir(dir) else {
            continue;
        };
        for child in children.flatten() {
            if start.elapsed() > max_time || entries.len() >= max_entries {
                publish(&entries);
                return;
            }
            let name = child.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name.starts_with("~$") {
                continue;
            }
            let Ok(kind) = child.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            let path = child.path();
            if settings.excluded(&path.to_string_lossy()) { continue; }
            entries.push(Entry::new(
                name.clone(),
                path.to_string_lossy().into_owned(),
                if kind.is_dir() { Kind::Folder } else { Kind::File },
                depth + 1,
            ));
            let skipped = entries.last().is_some_and(|e: &Entry| SKIP_DIRS.contains(&e.lower.as_str()) || package_folder(&e.location))
                || nested.iter().any(|n| crate::settings::within(&path.to_string_lossy(), n));
            if kind.is_dir() && depth < 4 && !skipped {
                queue.push_back((path, depth + 1));
            }
            if last_publish.elapsed() > Duration::from_secs(1) {
                publish(&entries);
                last_publish = Instant::now();
            }
        }
        // Publish root-level folders before walking deeper into the share.
        if depth == 0 {
            publish(&entries);
            last_publish = Instant::now();
        }
    }
    publish(&entries);
}

fn score(e: &Entry, q: &str, tokens: &[&str]) -> i32 {
    let n = e.lower.as_str();
    let s = if tokens.len() > 1 {
        if !tokens.iter().all(|t| n.contains(t)) {
            return contextual_score(e, tokens);
        }
        if n.starts_with(tokens[0]) { 700 } else { 400 }
    } else if n == q {
        1000
    } else if let Some(rest) = n.strip_prefix(q) {
        if rest.starts_with('.') { 950 } else { 800 } // "report" -> "report.pdf"
    } else if let Some(i) = n.find(q) {
        let prev = n[..i].chars().next_back().unwrap_or(' ');
        if !prev.is_alphanumeric() {
            600 // starts a word
        } else if q.len() >= 2 {
            350
        } else {
            return 0;
        }
    } else if e.kind == Kind::App && q.chars().count() >= 2 && matches_initials(n, q) {
        500 // "vsc" -> Visual Studio Code
    } else {
        return contextual_score(e, tokens);
    };
    s - n.len().min(60) as i32
}

fn extension(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or("")
}
fn spreadsheet(path: &str) -> bool {
    matches!(extension(path), "xlsx" | "xls" | "xlsm" | "xlsb" | "ods" | "csv" | "tsv" | "xltx" | "xltm")
}

fn contextual_score(e: &Entry, tokens: &[&str]) -> i32 {
    let mut rank = 450;
    for token in tokens {
        if e.lower.contains(token) {
            continue;
        }
        if e.location.contains(token) {
            rank = rank.min(280);
            continue;
        }
        if e.content.contains(token) {
            rank = rank.min(180);
            continue;
        }
        if token.len() >= 4 && near_word(&e.lower, token) {
            rank = rank.min(120);
            continue;
        }
        return 0;
    }
    rank
}

/// A word of `name` is within one edit of `token`.
fn near_word(name: &str, token: &str) -> bool {
    // Most names are ASCII: split bytes rather than decode characters.
    if name.is_ascii() && token.is_ascii() {
        let token = token.as_bytes();
        return name.as_bytes().split(|b| !b.is_ascii_alphanumeric()).any(|word| one_edit_in(word, token));
    }
    name.split(|c: char| !c.is_alphanumeric()).any(|word| one_edit(word, token))
}

fn one_edit(a: &str, b: &str) -> bool {
    // Bytes are characters in ASCII, which spares an allocation per word on every search.
    if a.is_ascii() && b.is_ascii() {
        return one_edit_in(a.as_bytes(), b.as_bytes());
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    one_edit_in(&a, &b)
}

fn one_edit_in<T: PartialEq>(a: &[T], b: &[T]) -> bool {
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let (mut i, mut j, mut edits) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            i += 1;
            j += 1;
            continue;
        }
        edits += 1;
        if edits > 1 {
            return false;
        }
        if a.len() == b.len() && i + 1 < a.len() && a[i] == b[j + 1] && a[i + 1] == b[j] {
            i += 2;
            j += 2;
        } else {
            if a.len() >= b.len() {
                i += 1;
            }
            if b.len() >= a.len() {
                j += 1;
            }
        }
    }
    edits + (a.len() - i) + (b.len() - j) <= 1
}

/// Workbooks, CSV/TSV and saved mail have text to read.
fn needs_reading(lower_name: &str) -> bool {
    spreadsheet(lower_name) || extension(lower_name) == "eml"
}

// Never hydrate cloud placeholders while building the local content index.
fn enrich(e: &mut Entry) {
    e.read = false;
    if !allowed(e, &crate::settings::get()) { return; }
    if e.kind != Kind::File || !needs_reading(&e.lower) {
        e.read = true;
        return;
    }
    let ext = extension(&e.lower);
    let Ok(md) = std::fs::metadata(&e.path) else {
        return;
    };
    if md.len() > 16 * 1024 * 1024 || md.file_attributes() & (0x1000 | 0x40000 | 0x400000) != 0 {
        e.read = true; // Deliberately skipped; a changed stamp makes it eligible again.
        return;
    }
    if matches!(ext, "csv" | "tsv" | "eml") {
        use std::io::Read;
        if let Ok(f) = std::fs::File::open(&e.path) {
            let mut bytes = Vec::new();
            if f.take(CONTENT_LIMIT as u64).read_to_end(&mut bytes).is_ok() {
                let mut words = Words::default();
                words.add(&String::from_utf8_lossy(&bytes).to_lowercase());
                e.content = words.text;
                e.detail = if ext == "eml" { "Saved email · text searchable" } else { "Cell text searchable" }.into();
                e.read = true;
            }
        }
        return;
    }
    if let Ok(mut book) = open_workbook_auto(&e.path) {
        let names = book.sheet_names();
        e.detail = format!("Sheets: {}", names.join(", "));
        let mut words = Words::default();
        let mut complete = true;
        words.add(&names.join(" ").to_lowercase());
        for name in names.iter().take(32) {
            if words.full() {
                break;
            }
            if let Ok(range) = book.worksheet_range(name) {
                for (_, _, cell) in range.used_cells() {
                    if words.full() {
                        break;
                    }
                    words.add(&cell.to_string().to_lowercase().chars().take(512).collect::<String>());
                }
            } else {
                complete = false;
            }
        }
        e.content = words.text;
        e.read = complete;
    }
}

const CONTENT_LIMIT: usize = 128 * 1024;
const CONTENT_BUDGET: usize = 32 * 1024 * 1024; // text kept across all files

/// A file's text as its distinct words, each once. Search tokens never contain
/// whitespace, so they match this exactly where they match the full text, and
/// repeated labels and values no longer slow every search or use up the budget.
#[derive(Default)]
struct Words {
    text: String,
    seen: HashSet<String>,
}

impl Words {
    fn full(&self) -> bool {
        self.text.len() >= CONTENT_LIMIT
    }

    fn add(&mut self, text: &str) {
        for word in text.split_whitespace() {
            if self.full() {
                return;
            }
            if !self.seen.contains(word) {
                self.seen.insert(word.to_string());
                if !self.text.is_empty() {
                    self.text.push(' ');
                }
                self.text.push_str(word);
            }
        }
    }
}

fn matches_initials(name: &str, q: &str) -> bool {
    let mut want = q.chars().peekable();
    let mut boundary = true;
    for c in name.chars() {
        let word = c.is_alphanumeric();
        if boundary && word && want.peek() == Some(&c) {
            want.next();
            if want.peek().is_none() {
                return true;
            }
        }
        boundary = !word;
    }
    false
}

/// Every Start menu app, Store apps included. Launch target is shell:AppsFolder\<AppID>.
fn start_apps() -> Vec<Entry> {
    let script = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-StartApps | Select-Object Name,AppID | ConvertTo-Json -Compress";
    let Ok(out) = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return Vec::new();
    };
    let items = match json {
        serde_json::Value::Array(a) => a,
        v => vec![v],
    };
    items
        .iter()
        .filter_map(|v| {
            let name = v.get("Name")?.as_str()?.trim();
            let id = v.get("AppID")?.as_str()?;
            if name.is_empty() || name.to_lowercase().starts_with("uninstall") {
                return None;
            }
            Some(Entry::new(name.to_string(), format!(r"shell:AppsFolder\{id}"), Kind::App, 0))
        })
        .collect()
}

fn start_menu_shortcuts(list: &mut Vec<Entry>) {
    let roots = [
        std::env::var("ProgramData").map(|p| PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs")),
        std::env::var("APPDATA").map(|p| PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs")),
    ];
    let mut queue: VecDeque<PathBuf> = roots.into_iter().flatten().collect();
    while let Some(dir) = queue.pop_front() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for de in rd.flatten() {
            let path = de.path();
            if path.is_dir() {
                queue.push_back(path);
            } else if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
                let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                if !name.to_lowercase().starts_with("uninstall") {
                    list.push(Entry::new(name, path.to_string_lossy().into_owned(), Kind::App, 0));
                }
            }
        }
    }
}

/// The folders scanned first and watched for changes: Documents, Desktop, Downloads
/// and OneDrive. The rest of the profile is scanned after them.
pub fn scan_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let settings = crate::settings::get();
    if let Ok(profile) = std::env::var("USERPROFILE") && settings.profile {
        roots.extend(["Documents", "Desktop", "Downloads"].iter().map(|name| PathBuf::from(&profile).join(name)));
    }
    for variable in ["OneDrive", "OneDriveCommercial", "OneDriveConsumer"] {
        if !settings.profile { break; }
        if let Ok(path) = std::env::var(variable) {
            roots.push(PathBuf::from(path));
        }
    }
    roots.extend(settings.folders.iter().map(PathBuf::from));
    roots.retain(|root| !settings.excluded(&root.to_string_lossy()));
    roots
}

fn profile_files(list: &mut Vec<Entry>) {
    let Ok(profile) = std::env::var("USERPROFILE") else {
        return;
    };
    let mut queue: VecDeque<_> = scan_roots().into_iter().filter(|p| local_path(&p.to_string_lossy())).map(|dir| (dir, 0)).collect();
    if crate::settings::get().profile { queue.push_back((PathBuf::from(profile), 0)); }
    walk(queue, list, MAX_FILES);
}

/// Breadth-first walk, so a hit cap drops the deepest files first.
fn walk(mut queue: VecDeque<(PathBuf, usize)>, list: &mut Vec<Entry>, limit: usize) {
    let settings = crate::settings::get();
    let mut visited = HashSet::new();
    let mut count = 0;
    while let Some((dir, depth)) = queue.pop_front() {
        if settings.excluded(&dir.to_string_lossy()) { continue; }
        if !visited.insert(dir.to_string_lossy().to_lowercase()) {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for de in rd.flatten() {
            let name = de.file_name().to_string_lossy().into_owned();
            let Ok(md) = de.metadata() else { continue };
            let path = de.path();
            if settings.excluded(&path.to_string_lossy()) { continue; }
            let Some(e) = scanned_entry(name, &path, &md, depth + 1) else { continue };
            let descend = e.kind == Kind::Folder && descends(&e, &md);
            list.push(e);
            count += 1;
            if count >= limit {
                return;
            }
            if descend {
                queue.push_back((path, depth + 1));
            }
        }
    }
}

/// An entry for a file or folder on disk, unless Lumen leaves it out.
fn scanned_entry(name: String, path: &Path, md: &std::fs::Metadata, depth: usize) -> Option<Entry> {
    if name.starts_with('.') || name.starts_with("~$") || md.file_attributes() & HIDDEN_OR_SYSTEM != 0 {
        return None;
    }
    let kind = if md.is_dir() { Kind::Folder } else { Kind::File };
    let mut e = Entry::new(name, path.to_string_lossy().into_owned(), kind, depth);
    e.stamp = (md.len(), md.last_write_time(), md.file_attributes());
    Some(e)
}

/// The walk goes into this folder.
fn descends(folder: &Entry, md: &std::fs::Metadata) -> bool {
    !md.file_type().is_symlink()
        && (folder.depth as usize) < MAX_DEPTH
        && !SKIP_DIRS.contains(&folder.lower.as_str())
        && !package_folder(&folder.location)
}

/// How deep the walk from `root` would find `path`, or None if it never goes there.
fn watched_depth(root: &Path, path: &Path) -> Option<usize> {
    let depth = path.strip_prefix(root).ok()?.components().count();
    let skipped = path.ancestors().skip(1).take(depth.saturating_sub(1)).any(|folder| {
        let location = folder.to_string_lossy().to_lowercase();
        let name = location.rsplit('\\').next().unwrap_or("");
        name.starts_with('.') || SKIP_DIRS.contains(&name) || package_folder(&location)
    });
    (depth > 0 && !skipped && depth <= MAX_DEPTH).then_some(depth)
}

/// Copies text read by the previous scan into files whose size, date and attributes
/// have not changed, and returns how much it copied.
fn reuse_content(list: &mut [Entry], previous: &[Entry]) -> usize {
    let settings = crate::settings::get();
    let known: HashMap<&str, &Entry> = previous.iter().filter(|e| e.read).map(|e| (e.path.as_str(), e)).collect();
    let mut bytes = 0;
    for e in list.iter_mut().filter(|e| e.kind == Kind::File) {
        if !allowed(e, &settings) { continue; }
        if let Some(old) = known.get(e.path.as_str())
            && old.stamp == e.stamp
        {
            e.content = old.content.clone();
            e.detail = old.detail.clone();
            e.read = true;
            bytes += e.content.len();
        }
    }
    bytes
}

fn cache_file() -> Option<PathBuf> {
    std::env::var_os("LUMEN_DATA_DIR").map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Lumen")))
        .map(|p| p.join("index-v1.json"))
}

fn decode_cache(bytes: &[u8], settings: &crate::settings::Settings) -> Option<Vec<Entry>> {
    let (version, saved_settings, entries): (u8, crate::settings::Settings, Vec<Entry>) = serde_json::from_slice(bytes).ok()?;
    (version == 1 && saved_settings == *settings && entries.len() <= MAX_FILES + 10_000).then_some(entries)
}

fn load_cache(settings: &crate::settings::Settings) -> Option<Vec<Entry>> {
    let file = cache_file()?;
    if std::fs::metadata(&file).ok()?.len() > 256 * 1024 * 1024 { return None; }
    decode_cache(&std::fs::read(file).ok()?, settings)
}

fn save_cache(settings: &crate::settings::Settings, entries: &[Entry]) {
    if let (Some(file), Ok(bytes)) = (cache_file(), serde_json::to_vec(&(1u8, settings, entries))) {
        let _ = crate::storage::atomic_write(&file, &bytes);
    }
}

/// Preview workers never try to hydrate a remote file or block on an SMB share.
pub fn local_path(path: &str) -> bool {
    if path.starts_with("\\\\") { return false; }
    let path = path.replace('/', "\\");
    path.get(..3).is_some_and(|root| unsafe { windows::Win32::Storage::FileSystem::GetDriveTypeW(&windows::core::HSTRING::from(root)) != 4 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_roundtrip_keeps_search_text_and_rejects_stale_configuration() {
        let settings = crate::settings::Settings::default();
        let mut e = Entry::new("budget.xlsx".into(), r"C:\Work\budget.xlsx".into(), Kind::File, 2);
        e.content = "revenue forecast".into(); e.read = true; e.stamp = (42, 123, 0);
        let bytes = serde_json::to_vec(&(1u8, &settings, vec![e])).unwrap();
        let loaded = decode_cache(&bytes, &settings).unwrap(); assert_eq!(loaded[0].content, "revenue forecast");
        assert_eq!(loaded[0].stamp, (42, 123, 0));
        let mut changed = settings.clone(); changed.exclusions.push(r"C:\Work".into());
        assert!(decode_cache(&bytes, &changed).is_none()); assert!(decode_cache(b"broken", &settings).is_none());
    }

    fn idx(items: &[(&str, Kind)]) -> Index {
        let index = Index::default();
        *index.entries.write().unwrap() = Arc::new(items.iter().map(|(n, k)| Entry::new(n.to_string(), n.to_string(), *k, 1)).collect());
        index
    }

    #[test]
    fn ranks_prefix_and_apps_first() {
        let index = idx(&[
            ("my report.docx", Kind::File),
            ("report.pdf", Kind::File),
            ("Remote Desktop", Kind::App),
            ("Visual Studio Code", Kind::App),
        ]);
        let names = |q: &str| index.search(q, 5, |_| 0).into_iter().map(|e| e.name).collect::<Vec<_>>();
        assert_eq!(names("report"), ["report.pdf", "my report.docx"]);
        assert_eq!(names("re")[0], "Remote Desktop");
        assert_eq!(names("vsc"), ["Visual Studio Code"]);
        assert_eq!(names("my rep"), ["my report.docx"]);
    }

    #[test]
    fn ranks_across_search_slices_like_one_pass() {
        let mut items: Vec<(String, Kind)> = (0..3 * CHUNK).map(|i| (format!("file {i}.txt"), Kind::File)).collect();
        items[10].0 = "budget old.xlsx".into();
        items[2 * CHUNK + 5].0 = "budget.xlsx".into();
        items[CHUNK + 7].0 = "tie a".into();
        items[3].0 = "tie b".into();
        let refs: Vec<(&str, Kind)> = items.iter().map(|(n, k)| (n.as_str(), *k)).collect();
        let index = idx(&refs);
        let names = |q: &str, max| index.search(q, max, |_| 0).into_iter().map(|e| e.name).collect::<Vec<_>>();
        assert_eq!(names("budget", 5), ["budget.xlsx", "budget old.xlsx"]);
        assert_eq!(names("tie", 1), ["tie b"], "equal scores keep index order");
        assert_eq!(names("file 1", 3).len(), 3);
    }

    #[test]
    fn unlimited_search_returns_every_match_in_stable_rank_order() {
        let count = CHUNK * 3 + 17;
        let items: Vec<_> = (0..count).map(|i| (format!("budget {i:05}.xlsx"), Kind::File)).collect();
        let refs: Vec<_> = items.iter().map(|(name, kind)| (name.as_str(), *kind)).collect();
        let index = idx(&refs);
        let all = index.search("excel budget", usize::MAX, |_| 0);
        assert_eq!(all.len(), count);
        for (i, entry) in all.iter().enumerate() {
            assert_eq!(entry.name, items[i].0, "ties stay in index order across search slices");
        }
        let first = index.search("excel budget", 7, |_| 0);
        assert_eq!(first.iter().map(|e| &e.path).collect::<Vec<_>>(), all[..7].iter().map(|e| &e.path).collect::<Vec<_>>());
        assert!(index.search("excel missing", usize::MAX, |_| 0).is_empty());
        assert!(index.search("excel budget", 0, |_| 0).is_empty());
    }

    #[test]
    fn watched_changes_show_up_and_hide_what_they_replace() {
        let root = std::env::temp_dir().join(format!("lumen-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Reports")).unwrap();
        std::fs::write(root.join("Reports").join("q3 forecast.txt"), "x").unwrap();
        let index = Index::default();
        // The last full scan saw a file that has since been deleted.
        let old = root.join("old plan.txt").to_string_lossy().into_owned();
        *index.entries.write().unwrap() = Arc::new(vec![Entry::new("old plan.txt".into(), old, Kind::File, 1)]);
        let names = |q: &str| index.search(q, 5, |_| 0).into_iter().map(|e| e.name).collect::<Vec<_>>();
        assert_eq!(names("plan"), ["old plan.txt"]);

        let changes = vec![
            Change::Removed(root.join("old plan.txt")),
            Change::Added(root.join("Reports")),
            Change::Added(root.join("node_modules").join("plan.js")),
        ];
        index.apply(&root, changes, &|| ());
        assert!(names("plan").is_empty(), "deleted, and node_modules is never indexed");
        assert_eq!(names("forecast"), ["q3 forecast.txt"], "a folder that appears brings its files");

        std::fs::rename(root.join("Reports"), root.join("Archive")).unwrap();
        index.apply(&root, vec![Change::Removed(root.join("Reports")), Change::Added(root.join("Archive"))], &|| ());
        let found = index.search("forecast", 5, |_| 0);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, root.join("Archive").join("q3 forecast.txt").to_string_lossy());
        assert_eq!(index.get(&found[0].path).map(|e| e.name), Some("q3 forecast.txt".into()));

        // A full scan that starts after these changes sees them for itself.
        index.update_changes(|changes| changes.settle(Instant::now()));
        assert!(index.changes.read().unwrap().is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn exact_lookup_does_not_resurrect_deleted_files_or_folders() {
        let root = Path::new(r"C:\Lumen lookup regression");
        let deleted = root.join("old plan.txt");
        let folder = root.join("Reports");
        let nested = folder.join("forecast.xlsx");
        let index = Index::default();
        *index.entries.write().unwrap() = Arc::new(vec![
            Entry::new("old plan.txt".into(), deleted.to_string_lossy().into_owned(), Kind::File, 1),
            Entry::new("forecast.xlsx".into(), nested.to_string_lossy().into_owned(), Kind::File, 2),
        ]);
        let mapped = r"W:\Accounts\budget.xlsx";
        index.mapped.write().unwrap().insert("W:\\".into(), Arc::new(vec![Entry::new("budget.xlsx".into(), mapped.into(), Kind::File, 2)]));

        assert!(index.get(&deleted.to_string_lossy()).is_some());
        assert!(index.get(&nested.to_string_lossy()).is_some());
        assert!(index.get(mapped).is_some());
        index.apply(
            root,
            vec![Change::Removed(deleted.clone()), Change::Removed(folder), Change::Removed(PathBuf::from(r"W:\Accounts"))],
            &|| (),
        );

        assert!(index.search("plan", 5, |_| 0).is_empty());
        assert!(index.get(&deleted.to_string_lossy()).is_none(), "a learned file must stay deleted");
        assert!(index.get(&nested.to_string_lossy()).is_none(), "deleting a folder also hides its children");
        assert!(index.get(mapped).is_none(), "mapped snapshots obey the same deletion policy");
    }

    #[test]
    fn refresh_reuses_text_only_of_unchanged_files() {
        let scanned = |name: &str, stamp| {
            let mut e = Entry::new(name.into(), format!(r"C:\nowhere\{name}"), Kind::File, 1);
            e.stamp = stamp;
            e
        };
        let previous: Vec<Entry> = ["same.xlsx", "edited.xlsx"]
            .iter()
            .map(|name| {
                let mut e = scanned(name, (10, 20, 32));
                (e.read, e.content) = (true, "cashflow".into());
                e
            })
            .collect();
        let mut list = vec![scanned("same.xlsx", (10, 20, 32)), scanned("edited.xlsx", (11, 25, 32))];
        assert_eq!(reuse_content(&mut list, &previous), "cashflow".len());
        assert!(list[0].read && list[0].content == "cashflow");
        assert!(!list[1].read && list[1].content.is_empty(), "edited files are read again");
    }

    #[test]
    fn refresh_retries_a_transient_content_read_failure_without_a_file_change() {
        use std::os::windows::fs::OpenOptionsExt;

        let root = std::env::temp_dir().join(format!("lumen-content-retry-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("forecast.csv");
        std::fs::write(&path, "projected revenue,42000").unwrap();
        let scan = || scanned_entry("forecast.csv".into(), &path, &std::fs::metadata(&path).unwrap(), 1).unwrap();
        let mut previous = scan();

        // A saving application can deny access without changing the file itself.
        let locked = std::fs::OpenOptions::new().read(true).share_mode(0).open(&path).unwrap();
        enrich(&mut previous);
        assert!(previous.content.is_empty(), "the exclusive handle must prevent extraction");
        drop(locked);

        let mut refreshed = vec![scan()];
        assert_eq!(previous.stamp, refreshed[0].stamp, "recovery must not depend on a changed stamp");
        reuse_content(&mut refreshed, &[previous]);
        for entry in &mut refreshed {
            if !entry.read {
                enrich(entry);
            }
        }
        let index = Index::default();
        *index.entries.write().unwrap() = Arc::new(refreshed);
        let found = index.search("projected revenue", 5, |_| 0);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(found.len(), 1, "the next refresh must recover text after a temporary lock is released");
    }

    #[test]
    fn code_files_rank_below_documents_but_still_appear() {
        let entry = |path: &str, kind| Entry::new(path.rsplit('\\').next().unwrap().into(), path.into(), kind, 3);
        let index = Index::default();
        *index.entries.write().unwrap() = Arc::new(vec![
            entry(r"C:\Users\u\source\libc\src\env.c", Kind::File),
            entry(r"C:\Users\u\Downloads\tools\env.bat", Kind::File),
            entry(r"W:\Accounts\BL2\Environmental Costs", Kind::Folder),
            entry(r"C:\Users\u\Documents\env notes.docx", Kind::File),
        ]);
        let names: Vec<String> = index.search("env", 8, |_| 0).into_iter().map(|e| e.name).collect();
        assert_eq!(names[..2], ["Environmental Costs", "env notes.docx"]);
        assert_eq!(names.len(), 4, "code files are ranked down, not hidden");
        assert_eq!(index.search("env.bat", 8, |_| 0)[0].name, "env.bat", "named exactly, it still comes first");
    }

    #[test]
    fn recognizes_versioned_package_folders() {
        for folder in ["python-3.14.8", "libc@v1.55.3", "toolchain@v0.0.1-go1.26.10.windows-amd64", "node-v20.1.0", "10.0.203", "8.0.420"] {
            assert!(versioned(folder), "{folder}");
        }
        for folder in ["2024-01-15", "2024.01.15", "1.2", "fy2024-25", "q3 report-1.2", "accounts", "v1.2", "budget-"] {
            assert!(!versioned(folder), "{folder}");
        }
        assert!(package_folder(r"c:\users\u\go\pkg\mod"));
        assert!(package_folder(r"c:\users\u\dotnet\sdk"));
        assert!(!package_folder(r"c:\users\u\documents\pkg"));
        assert!(!package_folder(r"c:\users\u\documents\mod"));
    }

    #[test]
    fn scans_list_package_folders_without_going_in() {
        let root = std::env::temp_dir().join(format!("lumen-packages-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in [r"Python-3.14.8\PCbuild", r"go\pkg\mod\libc@v1.55.3", "vendor", "Reports"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        for file in [r"Python-3.14.8\PCbuild\env.bat", r"go\pkg\mod\libc@v1.55.3\env.c", r"vendor\lib.c", r"Reports\env notes.txt"] {
            std::fs::write(root.join(file), "x").unwrap();
        }
        let mut found = Vec::new();
        walk(VecDeque::from([(root.clone(), 0)]), &mut found, 1000);
        let mut names: Vec<&str> = found.iter().map(|e| e.name.as_str()).collect();
        names.sort();
        assert_eq!(names, ["Python-3.14.8", "Reports", "env notes.txt", "go", "mod", "pkg", "vendor"]);

        // The watcher agrees with the walk.
        assert_eq!(watched_depth(&root, &root.join(r"Python-3.14.8\PCbuild\new.bat")), None);
        assert_eq!(watched_depth(&root, &root.join(r"go\pkg\mod\x\y.go")), None);
        assert_eq!(watched_depth(&root, &root.join(r"go\pkg\mod")), Some(3));
        assert_eq!(watched_depth(&root, &root.join(r"Reports\q3.xlsx")), Some(2));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn distinct_words_find_what_full_text_finds() {
        let text = "q1 revenue 42000\tq2 revenue 42000\nprojected revenue total";
        let mut words = Words::default();
        words.add(text);
        assert_eq!(words.text, "q1 revenue 42000 q2 projected total");
        for token in ["rev", "venue", "42000", "q2", "total", "proj", "xyz", "q3"] {
            assert_eq!(words.text.contains(token), text.contains(token), "{token}");
        }
    }

    #[test]
    fn splits_filter_words_from_term() {
        assert_eq!(split_filter("excel bud"), ("excel ", "bud"));
        assert_eq!(split_filter("Mail  invoice q3"), ("Mail  ", "invoice q3"));
        assert_eq!(split_filter("excel ext:xlsm fore"), ("excel ext:xlsm ", "fore"));
        assert_eq!(split_filter("excel"), ("", "excel"));
        assert_eq!(split_filter("budget excel"), ("", "budget excel"));
        assert_eq!(split_filter("report mail x"), ("", "report mail x"));
    }

    #[test]
    fn history_boosts() {
        let index = idx(&[("notes a.txt", Kind::File), ("notes b.txt", Kind::File)]);
        let top = index.search("notes", 2, |p| if p == "notes b.txt" { 5 } else { 0 });
        assert_eq!(top[0].name, "notes b.txt");
    }

    #[test]
    fn spreadsheet_filters_and_typos() {
        let index = idx(&[
            ("Budget 2026.XLSX", Kind::File),
            ("Budget.docx", Kind::File),
            ("Budget planner", Kind::App),
            ("Forecast.xlsm", Kind::File),
        ]);
        assert_eq!(index.search("excel budegt", 5, |_| 0)[0].name, "Budget 2026.XLSX");
        assert_eq!(index.search("excel", 10, |_| 0).len(), 2);
        assert_eq!(index.search("ext:xlsm forecast", 10, |_| 0).len(), 1);
        assert!(index.search("excel budget unrelated", 10, |_| 0).is_empty());
        assert!(index.search("budget", 0, |_| 0).is_empty());
    }

    #[test]
    fn matches_folder_and_workbook_content() {
        let index = Index::default();
        let mut e = Entry::new("FY26.xlsx".into(), r"C:\Finance\FY26.xlsx".into(), Kind::File, 2);
        e.content = "cashflow projected revenue".into();
        *index.entries.write().unwrap() = Arc::new(vec![e]);
        assert_eq!(index.search("excel finance revenue", 5, |_| 0).len(), 1);
        assert_eq!(index.search("cashflow", 5, |_| 0).len(), 1);
        assert!(index.search("cashflow missing", 5, |_| 0).is_empty());
    }

    #[test]
    fn typo_matching_handles_unicode_and_boundaries() {
        assert!(one_edit("budget", "budegt"));
        assert!(one_edit("budget", "buget"));
        assert!(one_edit("café", "cafe"));
        assert!(!one_edit("budget", "bxxget"));
        assert!(!one_edit("budget", "bud"));
    }

    #[test]
    fn history_cannot_overflow() {
        let index = idx(&[("Budget.xlsx", Kind::File)]);
        assert_eq!(index.search("budget", 1, |_| u32::MAX).len(), 1);
        let apps = idx(&[("Excel", Kind::App), ("Budget.xlsx", Kind::File)]);
        assert_eq!(apps.search("type:app excel", 5, |_| 0)[0].name, "Excel");
    }

    #[test]
    fn reads_real_workbook_sheet_names_and_cells() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/budget.xlsx");
        let mut e = Entry::new("budget.xlsx".into(), path.into(), Kind::File, 0);
        enrich(&mut e);
        assert!(e.detail.contains("Cashflow"));
        assert!(e.content.contains("projected revenue"));
        assert!(e.content.contains("42000"));
    }

    #[test]
    fn exact_mapped_folder_beats_module_files_and_history() {
        let index = idx(&[("Accounts.types.ps1xml", Kind::File), ("Accounts.format.ps1xml", Kind::File), ("Az.Accounts", Kind::Folder)]);
        index.mapped.write().unwrap().insert(
            "W:\\".into(),
            Arc::new(vec![
                Entry::new("Accounts".into(), r"W:\Accounts".into(), Kind::Folder, 1),
                Entry::new("PPV accounts.xlsx".into(), r"W:\Accounts\PPV accounts.xlsx".into(), Kind::File, 2),
            ]),
        );
        assert_eq!(index.search("accounts", 8, |p| if p.ends_with("ps1xml") { 100 } else { 0 })[0].path, r"W:\Accounts");
        assert_eq!(index.search("excel accounts", 8, |_| 0)[0].name, "PPV accounts.xlsx");
        assert_eq!(index.search(r"W:\Accounts", 8, |_| 0)[0].kind as u8, Kind::Folder as u8);
    }

    /// Times searches over this PC's real profile. Run with
    /// `cargo test --release -- --ignored --nocapture search_speed`.
    #[test]
    #[ignore]
    fn search_speed() {
        let start = Instant::now();
        let mut list = start_apps();
        profile_files(&mut list);
        let walked = start.elapsed();
        let mut content_bytes = 0;
        for e in &mut list {
            if content_bytes >= 32 * 1024 * 1024 {
                break;
            }
            enrich(e);
            content_bytes += e.content.len();
        }
        println!("{} entries, walk {walked:?}, content {} MB in {:?}", list.len(), content_bytes >> 20, start.elapsed() - walked);
        println!("{} code files ranked down", list.iter().filter(|e| e.penalty > 0).count());
        for e in list.iter().filter(|e| e.kind == Kind::Folder && package_folder(&e.location)) {
            println!("not scanned: {}", e.path);
        }
        let index = Index::default();
        let cache_started = Instant::now();
        let settings = crate::settings::get();
        let cache = serde_json::to_vec(&(1u8, &settings, &list)).unwrap();
        let encode_time = cache_started.elapsed();
        let load_started = Instant::now();
        let loaded = decode_cache(&cache, &settings).unwrap();
        println!("cache {:.1} MB: encode {encode_time:?}, decode {:?} ({} entries)", cache.len() as f64 / 1048576., load_started.elapsed(), loaded.len());
        drop(loaded); drop(cache);
        let without_content: Vec<Entry> = list.iter().map(|e| Entry { content: String::new(), ..e.clone() }).collect();
        let bare = Index::default();
        *bare.entries.write().unwrap() = Arc::new(without_content);
        *index.entries.write().unwrap() = Arc::new(list);
        rayon::broadcast(|_| ());
        for q in ["b", "bu", "bud", "budget", "excel budget", "excel bud", "report 2026", "accounts", "zzqx", "zzqx yyqw"] {
            let rounds = 20;
            let time = |index: &Index, max| {
                let start = Instant::now();
                for _ in 0..rounds {
                    std::hint::black_box(index.search(q, max, |_| 0));
                }
                start.elapsed() / rounds
            };
            println!("{q:>14}: top 7 {:?}, all {:?} (names only {:?})", time(&index, 7), time(&index, usize::MAX), time(&bare, 7));
        }
    }

    #[test]
    fn drive_scan_publishes_top_level_before_nested_files() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/tests");
        let mut snapshots = Vec::new();
        scan_mapped(root, &[], |entries| snapshots.push(entries.to_vec()));
        assert!(snapshots[0].iter().any(|e| e.name == "fixtures" && e.kind == Kind::Folder));
        assert!(!snapshots[0].iter().any(|e| e.name == "budget.xlsx"));
        assert!(snapshots.last().unwrap().iter().any(|e| e.name == "budget.xlsx"));
    }

    /// Run on a PC with the share: LUMEN_SCAN_ROOT=W:\accounts\sm LUMEN_SCAN_FIND=0000340836
    #[test]
    #[ignore = "needs a real network folder"]
    fn added_network_folder_reaches_deep_files() {
        let (root, find) = (std::env::var("LUMEN_SCAN_ROOT").unwrap(), std::env::var("LUMEN_SCAN_FIND").unwrap());
        let start = Instant::now();
        let mut last = Vec::new();
        scan_mapped(&root, &[], |entries| last = entries.to_vec());
        println!("{} entries in {:?}", last.len(), start.elapsed());
        assert!(last.iter().any(|e| e.name.contains(&find)), "{find} not found under {root}");
    }

    #[test]
    fn folders_added_from_recent_use_get_a_smaller_scan() {
        let settings = crate::settings::Settings {
            folders: vec![r"W:\Team".into(), r"W:\Busy".into()],
            recent_added: vec![r"W:\Busy".into()],
            ..Default::default()
        };
        assert_eq!(scan_budget(r"W:\", &settings).0, 20_000);
        assert_eq!(scan_budget(r"W:\Team", &settings).0, 100_000);
        assert_eq!(scan_budget(r"w:\busy", &settings).0, 5_000);
    }

    #[test]
    fn drive_scan_leaves_added_folders_to_their_own_scan() {
        assert!(inside(r"W:\accounts\sm", r"W:\"));
        assert!(!inside(r"W:\", r"W:\accounts\sm"));
        assert!(!inside(r"W:\Accounts\SM", r"w:\accounts\sm"));
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/tests");
        let mut last = Vec::new();
        scan_mapped(root, &[format!(r"{root}\fixtures")], |entries| last = entries.to_vec());
        assert!(last.iter().any(|e| e.name == "fixtures" && e.kind == Kind::Folder));
        assert!(!last.iter().any(|e| e.name == "budget.xlsx"), "the added folder's own worker lists its files");
    }
}
