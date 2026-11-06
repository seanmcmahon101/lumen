//! In-memory index of installed apps and everything under the user profile.

use calamine::{Reader, open_workbook_auto};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::os::windows::fs::MetadataExt;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
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
        }
    }
}

#[derive(Default)]
pub struct Index {
    entries: RwLock<Arc<Vec<Entry>>>,
    generation: AtomicU64,
    building: AtomicBool,
    mapped: RwLock<HashMap<String, Arc<Vec<Entry>>>>,
    scanning_drives: Mutex<HashSet<String>>,
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
            let mut list = start_apps();
            if list.is_empty() {
                start_menu_shortcuts(&mut list);
            }
            *me.entries.write().unwrap() = Arc::new(list.clone());
            me.generation.fetch_add(1, Ordering::Relaxed);
            on_done();
            profile_files(&mut list);
            *me.entries.write().unwrap() = Arc::new(list.clone());
            me.generation.fetch_add(1, Ordering::Relaxed);
            on_done();
            let mut content_bytes = 0;
            for e in &mut list {
                if content_bytes >= 32 * 1024 * 1024 {
                    break;
                }
                enrich(e);
                content_bytes += e.content.len();
            }
            *me.entries.write().unwrap() = Arc::new(list);
            me.generation.fetch_add(1, Ordering::Relaxed);
            me.building.store(false, Ordering::Release);
            on_done();
        });
    }

    pub fn search(&self, query: &str, max: usize, uses: impl Fn(&str) -> u32) -> Vec<Entry> {
        let mut snapshots = vec![self.entries.read().unwrap().clone()];
        snapshots.extend(self.mapped.read().unwrap().values().cloned());
        let entries: Vec<&Entry> = snapshots.iter().flat_map(|entries| entries.iter()).collect();
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

        // min-heap of the best `max` (score, index)
        let mut heap: BinaryHeap<Reverse<(i32, usize)>> = BinaryHeap::with_capacity(max + 1);
        for (i, e) in entries.iter().enumerate() {
            if (scope == "excel" && (e.kind != Kind::File || !spreadsheet(&e.lower)))
                || (scope == "email" && !matches!(extension(&e.lower), "eml" | "msg"))
                || (scope == "app" && e.kind != Kind::App)
                || (!ext.is_empty() && extension(&e.lower) != ext)
            {
                continue;
            }
            let mut s = if tokens.is_empty() { 100 } else { score(e, &needle, &tokens) };
            if s <= 0 {
                continue;
            }
            // Exact folder intent must beat incidental prefixes, even when a
            // similarly named file has a large launch-history boost.
            if e.kind == Kind::Folder && (e.lower == needle || e.location == needle) {
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
            if heap.len() < max {
                heap.push(Reverse((s, i)));
            } else if heap.peek().is_some_and(|Reverse((min, _))| s > *min) {
                heap.pop();
                heap.push(Reverse((s, i)));
            }
        }
        let mut best: Vec<(i32, usize)> = heap.into_iter().map(|Reverse(x)| x).collect();
        best.sort_by(|a, b| b.0.cmp(&a.0));
        best.into_iter().map(|(_, i)| (*entries[i]).clone()).collect()
    }
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
        if token.len() >= 4 && e.lower.split(|c: char| !c.is_alphanumeric()).any(|w| one_edit(w, token)) {
            rank = rank.min(120);
            continue;
        }
        return 0;
    }
    rank
}

fn one_edit(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() && a.len().abs_diff(b.len()) > 1 { return false; }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
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

// Never hydrate cloud placeholders while building the local content index.
fn enrich(e: &mut Entry) {
    if e.kind != Kind::File {
        return;
    }
    let ext = extension(&e.lower);
    if !spreadsheet(&e.lower) && ext != "eml" {
        return;
    }
    let Ok(md) = std::fs::metadata(&e.path) else {
        return;
    };
    if md.len() > 16 * 1024 * 1024 || md.file_attributes() & (0x1000 | 0x40000 | 0x400000) != 0 {
        return;
    }
    const LIMIT: usize = 128 * 1024;
    if matches!(ext, "csv" | "tsv" | "eml") {
        use std::io::Read;
        if let Ok(f) = std::fs::File::open(&e.path) {
            let mut bytes = Vec::new();
            if f.take(LIMIT as u64).read_to_end(&mut bytes).is_ok() {
                e.content = String::from_utf8_lossy(&bytes).to_lowercase();
                e.detail = if ext == "eml" { "Saved email · text searchable" } else { "Cell text searchable" }.into();
            }
        }
        return;
    }
    if let Ok(mut book) = open_workbook_auto(&e.path) {
        let names = book.sheet_names();
        e.detail = format!("Sheets: {}", names.join(", "));
        e.content = names.join(" ").to_lowercase();
        for name in names.iter().take(32) {
            if e.content.len() >= LIMIT {
                break;
            }
            if let Ok(range) = book.worksheet_range(name) {
                for (_, _, cell) in range.used_cells() {
                    if e.content.len() >= LIMIT {
                        break;
                    }
                    e.content.push(' ');
                    e.content.extend(cell.to_string().to_lowercase().chars().take(512));
                }
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

/// Breadth-first walk of the user profile, so a hit cap drops the deepest files first.
fn profile_files(list: &mut Vec<Entry>) {
    let Ok(root) = std::env::var("USERPROFILE") else {
        return;
    };
    let root = PathBuf::from(root);
    let mut queue: VecDeque<_> = ["Documents", "Desktop", "Downloads"].iter().map(|name| (root.join(name), 0usize)).collect();
    for variable in ["OneDrive", "OneDriveCommercial", "OneDriveConsumer"] {
        if let Ok(path) = std::env::var(variable) {
            queue.push_back((PathBuf::from(path), 0));
        }
    }
    queue.push_back((root, 0));
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
            if name.starts_with('.') || name.starts_with("~$") {
                continue;
            }
            let Ok(md) = de.metadata() else { continue };
            if md.file_attributes() & HIDDEN_OR_SYSTEM != 0 {
                continue;
            }
            let is_dir = md.is_dir();
            let path = de.path();
            list.push(Entry::new(
                name.clone(),
                path.to_string_lossy().into_owned(),
                if is_dir { Kind::Folder } else { Kind::File },
                depth + 1,
            ));
            count += 1;
            if count >= MAX_FILES {
                return;
            }
            if is_dir
                && !de.file_type().is_ok_and(|t| t.is_symlink())
                && depth + 1 < MAX_DEPTH
                && !SKIP_DIRS.contains(&name.to_lowercase().as_str())
            {
                queue.push_back((path, depth + 1));
            }
        }
    }
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
