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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    App,
    Folder,
    File,
    Email,
}

#[derive(Clone)]
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
    read: bool,             // `content` reflects the file as of `stamp`
}

impl Entry {
    pub(crate) fn new(name: String, path: String, kind: Kind, depth: usize) -> Self {
        Self {
            lower: name.to_lowercase(),
            location: path.to_lowercase(),
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
    Removed(PathBuf),
}

#[derive(Default)]
pub struct Index {
    entries: RwLock<Arc<Vec<Entry>>>,
    generation: AtomicU64,
    building: AtomicBool,
    mapped: RwLock<HashMap<String, Arc<Vec<Entry>>>>,
    scanning_drives: Mutex<HashSet<String>>,
    changes: RwLock<Arc<Changes>>,
}

impl Index {
    pub fn is_building(&self) -> bool {
        self.building.load(Ordering::Acquire) || !self.scanning_drives.lock().unwrap().is_empty()
    }
    /// Bumps every time a rebuild finishes, so the UI knows to re-run the query.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub fn rebuild(self: &Arc<Self>, on_done: impl Fn() + Send + Sync + 'static) {
        let on_done = Arc::new(on_done);
        // Each mapped share has its own worker: a disconnected server cannot hold
        // up local indexing, another drive, or the search/render threads.
        let (network_index, network_done) = (self.clone(), on_done.clone());
        std::thread::spawn(move || {
            for root in mapped_roots() {
                if !network_index.scanning_drives.lock().unwrap().insert(root.clone()) {
                    continue;
                }
                let (index, done) = (network_index.clone(), network_done.clone());
                std::thread::spawn(move || {
                    scan_mapped(&root, |entries| {
                        index.mapped.write().unwrap().insert(root.clone(), Arc::new(entries.to_vec()));
                        index.generation.fetch_add(1, Ordering::Relaxed);
                        done();
                    });
                    index.scanning_drives.lock().unwrap().remove(&root);
                    done();
                });
            }
        });
        if self.building.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            let publish = |list: Vec<Entry>| {
                *me.entries.write().unwrap() = Arc::new(list);
                me.generation.fetch_add(1, Ordering::Relaxed);
                on_done();
            };
            let previous = me.entries.read().unwrap().clone();
            let mut list = start_apps();
            if list.is_empty() {
                start_menu_shortcuts(&mut list);
            }
            // The first scan shows apps at once; a refresh keeps searching the
            // previous index until the new one is complete.
            if previous.is_empty() {
                publish(list.clone());
            }
            let walk_started = Instant::now();
            profile_files(&mut list);
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
            me.building.store(false, Ordering::Release);
            on_done();
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
        let now = Instant::now();
        // Read the disk first; the overlay is locked only to record the result.
        let mut found: Vec<Result<Entry, String>> = Vec::new();
        for change in changes {
            match change {
                Change::Removed(path) => found.push(Err(path.to_string_lossy().to_lowercase())),
                Change::Added(path) => {
                    let Some(depth) = watched_depth(root, &path) else { continue };
                    let Ok(md) = std::fs::symlink_metadata(&path) else { continue };
                    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    let Some(e) = scanned_entry(name, &path, &md, depth) else { continue };
                    let walk_in = e.kind == Kind::Folder && descends(&e.lower, depth, &md);
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
        // Saving a workbook replaces the file, so read the new text rather than lose it.
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
        if let Some(e) = self.changes.read().unwrap().added.iter().find(|e| e.path == path) {
            return Some(e.clone());
        }
        let entries = self.entries.read().unwrap().clone();
        if let Some(e) = entries.iter().find(|e| e.path == path) {
            return Some(e.clone());
        }
        self.mapped.read().unwrap().values().find_map(|entries| entries.iter().find(|e| e.path == path).cloned())
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
        let hides = |e: &Entry| changes.hides(&e.location);
        let check: Option<&(dyn Fn(&Entry) -> bool + Sync)> = (!changes.is_empty()).then_some(&hides);
        let mut chunks: Vec<(&[Entry], Option<&(dyn Fn(&Entry) -> bool + Sync)>)> =
            snapshots.iter().flat_map(|entries| entries.chunks(CHUNK)).map(|chunk| (chunk, check)).collect();
        chunks.extend(changes.added.chunks(CHUNK).map(|chunk| (chunk, None)));
        let mut best: Vec<(i32, (usize, usize), &Entry)> = chunks
            .par_iter()
            .enumerate()
            .flat_map_iter(|(k, &(chunk, hidden))| {
                // (slice, position) keeps index order on equal scores.
                best_in(chunk, &query, max, &uses, hidden).into_iter().map(move |(s, i, e)| (s, (k, i), e))
            })
            .collect();
        best.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        best.into_iter().take(max).map(|(_, _, e)| e.clone()).collect()
    }
}

/// Entries per search slice.
const CHUNK: usize = 2048;

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
        if e.kind == Kind::File && matches!(extension(&e.lower), "ps1xml" | "psd1" | "psm1" | "dll" | "pdb") {
            s -= 300;
        }
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
    let script = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; $roots=@([IO.DriveInfo]::GetDrives() | Where-Object {$_.DriveType -eq 'Network'} | ForEach-Object {$_.Name}); ConvertTo-Json -InputObject $roots -Compress";
    Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice(&output.stdout).ok())
        .unwrap_or_default()
}

fn scan_mapped(root: &str, mut publish: impl FnMut(&[Entry])) {
    let mut entries = Vec::new();
    let mut queue = VecDeque::from([(PathBuf::from(root), 0usize)]);
    let start = Instant::now();
    let mut last_publish = Instant::now();
    while let Some((dir, depth)) = queue.pop_front() {
        if start.elapsed() > Duration::from_secs(30) {
            break;
        }
        let Ok(children) = std::fs::read_dir(dir) else {
            continue;
        };
        for child in children.flatten() {
            if start.elapsed() > Duration::from_secs(30) || entries.len() >= 20_000 {
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
            entries.push(Entry::new(
                name.clone(),
                path.to_string_lossy().into_owned(),
                if kind.is_dir() { Kind::Folder } else { Kind::File },
                depth + 1,
            ));
            if kind.is_dir() && depth < 4 && !SKIP_DIRS.contains(&name.to_lowercase().as_str()) {
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
    e.read = true;
    if e.kind != Kind::File || !needs_reading(&e.lower) {
        return;
    }
    let ext = extension(&e.lower);
    let Ok(md) = std::fs::metadata(&e.path) else {
        return;
    };
    if md.len() > 16 * 1024 * 1024 || md.file_attributes() & (0x1000 | 0x40000 | 0x400000) != 0 {
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
            }
        }
        return;
    }
    if let Ok(mut book) = open_workbook_auto(&e.path) {
        let names = book.sheet_names();
        e.detail = format!("Sheets: {}", names.join(", "));
        let mut words = Words::default();
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
            }
        }
        e.content = words.text;
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
    if let Ok(profile) = std::env::var("USERPROFILE") {
        roots.extend(["Documents", "Desktop", "Downloads"].iter().map(|name| PathBuf::from(&profile).join(name)));
    }
    for variable in ["OneDrive", "OneDriveCommercial", "OneDriveConsumer"] {
        if let Ok(path) = std::env::var(variable) {
            roots.push(PathBuf::from(path));
        }
    }
    roots
}

fn profile_files(list: &mut Vec<Entry>) {
    let Ok(profile) = std::env::var("USERPROFILE") else {
        return;
    };
    let mut queue: VecDeque<_> = scan_roots().into_iter().map(|dir| (dir, 0)).collect();
    queue.push_back((PathBuf::from(profile), 0));
    walk(queue, list, MAX_FILES);
}

/// Breadth-first walk, so a hit cap drops the deepest files first.
fn walk(mut queue: VecDeque<(PathBuf, usize)>, list: &mut Vec<Entry>, limit: usize) {
    let mut visited = HashSet::new();
    let mut count = 0;
    while let Some((dir, depth)) = queue.pop_front() {
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
            let Some(e) = scanned_entry(name, &path, &md, depth + 1) else { continue };
            let descend = e.kind == Kind::Folder && descends(&e.lower, depth + 1, &md);
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

/// The walk goes into this folder, found at `depth`.
fn descends(lower_name: &str, depth: usize, md: &std::fs::Metadata) -> bool {
    !md.file_type().is_symlink() && depth < MAX_DEPTH && !SKIP_DIRS.contains(&lower_name)
}

/// How deep the walk from `root` would find `path`, or None if it never goes there.
fn watched_depth(root: &Path, path: &Path) -> Option<usize> {
    let parts: Vec<String> =
        path.strip_prefix(root).ok()?.components().map(|part| part.as_os_str().to_string_lossy().to_lowercase()).collect();
    let (_, folders) = parts.split_last()?;
    let skipped = folders.iter().any(|folder| folder.starts_with('.') || SKIP_DIRS.contains(&folder.as_str()));
    (!skipped && parts.len() <= MAX_DEPTH).then_some(parts.len())
}

/// Copies text read by the previous scan into files whose size, date and attributes
/// have not changed, and returns how much it copied.
fn reuse_content(list: &mut [Entry], previous: &[Entry]) -> usize {
    let known: HashMap<&str, &Entry> = previous.iter().filter(|e| e.read).map(|e| (e.path.as_str(), e)).collect();
    let mut bytes = 0;
    for e in list.iter_mut().filter(|e| e.kind == Kind::File) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let index = Index::default();
        let without_content: Vec<Entry> = list.iter().map(|e| Entry { content: String::new(), ..e.clone() }).collect();
        let bare = Index::default();
        *bare.entries.write().unwrap() = Arc::new(without_content);
        *index.entries.write().unwrap() = Arc::new(list);
        rayon::broadcast(|_| ());
        for q in ["b", "bu", "bud", "budget", "excel budget", "excel bud", "report 2026", "accounts", "zzqx", "zzqx yyqw"] {
            let rounds = 20;
            let time = |index: &Index| {
                let start = Instant::now();
                for _ in 0..rounds {
                    std::hint::black_box(index.search(q, 7, |_| 0));
                }
                start.elapsed() / rounds
            };
            println!("{q:>14}: {:?} (names only {:?})", time(&index), time(&bare));
        }
    }

    #[test]
    fn drive_scan_publishes_top_level_before_nested_files() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/tests");
        let mut snapshots = Vec::new();
        scan_mapped(root, |entries| snapshots.push(entries.to_vec()));
        assert!(snapshots[0].iter().any(|e| e.name == "fixtures" && e.kind == Kind::Folder));
        assert!(!snapshots[0].iter().any(|e| e.name == "budget.xlsx"));
        assert!(snapshots.last().unwrap().iter().any(|e| e.name == "budget.xlsx"));
    }
}
