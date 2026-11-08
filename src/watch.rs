//! Watches the folders Lumen scans, so new, modified, renamed and deleted files show up in
//! search within moments instead of at the next full scan.

use crate::index::{Change, Index};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ACTION_ADDED, FILE_ACTION_MODIFIED, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME, FILE_ACTION_RENAMED_OLD_NAME,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows::core::HSTRING;

/// One thread per watched folder, each waiting on Windows for changes.
pub fn spawn(index: Arc<Index>, on_change: impl Fn() + Send + Sync + 'static) {
    let on_change = Arc::new(on_change);
    for root in watched_roots() {
        let (index, on_change) = (index.clone(), on_change.clone());
        std::thread::spawn(move || watch(&root, &index, on_change));
    }
}

/// The scanned folders that exist, leaving out any inside another, so each change is seen once.
fn watched_roots() -> Vec<PathBuf> {
    let inside = |path: &str, folder: &str| path == folder || path.starts_with(&format!("{folder}\\"));
    let mut kept: Vec<(PathBuf, String)> = Vec::new();
    for root in crate::index::scan_roots().into_iter().filter(|root| root.is_dir()) {
        let lower = root.to_string_lossy().to_lowercase();
        if kept.iter().any(|(_, folder)| inside(&lower, folder)) {
            continue;
        }
        kept.retain(|(_, folder)| !inside(folder, &lower));
        kept.push((root, lower));
    }
    kept.into_iter().map(|(root, _)| root).collect()
}

fn watch(root: &Path, index: &Arc<Index>, on_change: Arc<impl Fn() + Send + Sync + 'static>) {
    let Ok(mut watcher) = DirectoryWatch::open(root) else { return };
    while let Ok(changes) = watcher.read() {
        if let Some(changes) = changes {
            index.apply(root, changes, &*on_change);
        } else {
            // More changes than the buffer holds: scan everything again.
            let on_change = on_change.clone();
            index.rebuild(move || on_change());
        }
    }
}

struct DirectoryWatch {
    folder: HANDLE,
    root: PathBuf,
    buffer: Vec<u32>,
}

impl DirectoryWatch {
    fn open(root: &Path) -> windows::core::Result<Self> {
        let folder = unsafe {
            CreateFileW(
                &HSTRING::from(root.to_string_lossy().as_ref()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }?;
        // Windows keeps collecting changes between reads; 64 KB holds about a thousand.
        Ok(Self { folder, root: root.to_owned(), buffer: vec![0u32; 16 * 1024] })
    }

    /// None means the notification buffer overflowed and a full scan is required.
    fn read(&mut self) -> windows::core::Result<Option<Vec<Change>>> {
        let mut bytes = 0u32;
        unsafe {
            ReadDirectoryChangesW(
                self.folder,
                self.buffer.as_mut_ptr().cast(),
                std::mem::size_of_val(self.buffer.as_slice()) as u32,
                true,
                FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE,
                Some(&mut bytes),
                None,
                None,
            )
        }?;
        Ok((bytes != 0).then(|| unsafe { parse(&self.buffer, bytes as usize, &self.root) }))
    }
}

impl Drop for DirectoryWatch {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.folder) };
    }
}

/// Reads the FILE_NOTIFY_INFORMATION records Windows wrote into `buffer`.
unsafe fn parse(buffer: &[u32], bytes: usize, root: &Path) -> Vec<Change> {
    let base = buffer.as_ptr().cast::<u8>();
    let name_at = std::mem::offset_of!(FILE_NOTIFY_INFORMATION, FileName);
    let mut changes = Vec::new();
    let mut offset = 0;
    while offset + name_at <= bytes {
        let info = unsafe { base.add(offset).cast::<FILE_NOTIFY_INFORMATION>() };
        let (next, action, length) = unsafe { ((*info).NextEntryOffset, (*info).Action, (*info).FileNameLength as usize) };
        if offset + name_at + length > bytes {
            break;
        }
        let name = unsafe { std::slice::from_raw_parts(base.add(offset + name_at).cast::<u16>(), length / 2) };
        let path = root.join(String::from_utf16_lossy(name));
        match action {
            FILE_ACTION_ADDED | FILE_ACTION_RENAMED_NEW_NAME => changes.push(Change::Added(path)),
            FILE_ACTION_MODIFIED => changes.push(Change::Modified(path)),
            FILE_ACTION_REMOVED | FILE_ACTION_RENAMED_OLD_NAME => changes.push(Change::Removed(path)),
            _ => {}
        }
        if next == 0 {
            break;
        }
        offset += next as usize;
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn in_place_save_refreshes_searchable_content() {
        let root = std::env::temp_dir().join(format!("lumen-in-place-save-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("forecast.csv");
        std::fs::write(&path, "initial revenue,10000").unwrap();
        let index = Arc::new(Index::default());
        index.apply(&root, vec![Change::Added(path.clone())], &|| ());
        assert_eq!(index.search("initial revenue", 5, |_| 0).len(), 1);

        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let (worker_root, worker_index, worker_stop) = (root.clone(), index.clone(), stop.clone());
        let worker = std::thread::spawn(move || {
            let mut watcher = DirectoryWatch::open(&worker_root).unwrap();
            ready_tx.send(()).unwrap();
            loop {
                if let Some(changes) = watcher.read().unwrap() {
                    worker_index.apply(&worker_root, changes, &|| ());
                }
                if worker_stop.load(Ordering::Acquire) || !worker_index.search("revised revenue", 5, |_| 0).is_empty() {
                    break;
                }
            }
            done_tx.send(()).unwrap();
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();

        // Repeat the same in-place save until the first blocking read is armed.
        // Equal-length text also exercises saves with no file-size change.
        let deadline = Instant::now() + Duration::from_secs(5);
        let notified = loop {
            std::fs::write(&path, "revised revenue,73000").unwrap();
            if done.recv_timeout(Duration::from_millis(50)).is_ok() {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
        };
        if !notified {
            stop.store(true, Ordering::Release);
            std::fs::write(root.join("stop-watching.txt"), "").unwrap();
            done.recv_timeout(Duration::from_secs(5)).expect("the watcher must stop after its wake-up notification");
        }
        worker.join().unwrap();
        let found = index.search("revised revenue", 5, |_| 0);
        std::fs::remove_dir_all(&root).unwrap();
        assert!(notified, "an in-place save must notify the index without a create or rename");
        assert_eq!(found.len(), 1, "the saved content must replace the earlier text");
        assert!(index.search("initial revenue", 5, |_| 0).is_empty());
    }
}
