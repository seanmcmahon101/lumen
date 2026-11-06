//! In-memory index of installed apps and everything under the user profile.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::os::windows::fs::MetadataExt;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

const MAX_FILES: usize = 250_000;
const MAX_DEPTH: usize = 10;
const SKIP_DIRS: &[&str] = &["appdata", "node_modules", "bin", "obj", "__pycache__", "venv", "site-packages", "packages", "target"];
const HIDDEN_OR_SYSTEM: u32 = 0x2 | 0x4;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    App,
    Folder,
    File,
}

#[derive(Clone)]
pub struct Entry {
    pub name: String,
    lower: String,
    pub path: String,
    pub kind: Kind,
    depth: u16,
}

impl Entry {
    fn new(name: String, path: String, kind: Kind, depth: usize) -> Self {
        Self { lower: name.to_lowercase(), name, path, kind, depth: depth as u16 }
    }
}

#[derive(Default)]
pub struct Index {
    entries: RwLock<Arc<Vec<Entry>>>,
    generation: AtomicU64,
    building: AtomicBool,
}

impl Index {
    /// Bumps every time a rebuild finishes, so the UI knows to re-run the query.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub fn rebuild(self: &Arc<Self>, on_done: impl FnOnce() + Send + 'static) {
        if self.building.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            let mut list = start_apps();
            if list.is_empty() {
                start_menu_shortcuts(&mut list);
            }
            profile_files(&mut list);
            *me.entries.write().unwrap() = Arc::new(list);
            me.generation.fetch_add(1, Ordering::Relaxed);
            me.building.store(false, Ordering::Release);
            on_done();
        });
    }

    pub fn search(&self, query: &str, max: usize, uses: impl Fn(&str) -> u32) -> Vec<Entry> {
        let entries = self.entries.read().unwrap().clone();
        let q = query.trim().to_lowercase();
        if q.is_empty() || max == 0 {
            return Vec::new();
        }
        let tokens: Vec<&str> = q.split_whitespace().collect();

        // min-heap of the best `max` (score, index)
        let mut heap: BinaryHeap<Reverse<(i32, usize)>> = BinaryHeap::with_capacity(max + 1);
        for (i, e) in entries.iter().enumerate() {
            let mut s = score(e, &q, &tokens);
            if s <= 0 {
                continue;
            }
            s += match e.kind {
                Kind::App => 200,
                Kind::Folder => 40,
                Kind::File => 0,
            } - e.depth as i32 * 4;
            let u = uses(&e.path);
            if u > 0 {
                s += (u as i32 * 60).min(400);
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
        best.into_iter().map(|(_, i)| entries[i].clone()).collect()
    }
}

fn score(e: &Entry, q: &str, tokens: &[&str]) -> i32 {
    let n = e.lower.as_str();
    let s = if tokens.len() > 1 {
        if !tokens.iter().all(|t| n.contains(t)) {
            return 0;
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
        return 0;
    };
    s - n.len().min(60) as i32
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
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
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
    let Ok(root) = std::env::var("USERPROFILE") else { return };
    let mut queue = VecDeque::from([(PathBuf::from(root), 0usize)]);
    let mut count = 0;
    while let Some((dir, depth)) = queue.pop_front() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
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
            if is_dir && depth + 1 < MAX_DEPTH && !SKIP_DIRS.contains(&name.to_lowercase().as_str()) {
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
        *index.entries.write().unwrap() =
            Arc::new(items.iter().map(|(n, k)| Entry::new(n.to_string(), n.to_string(), *k, 1)).collect());
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
}
