//! Watches the folders Lumen scans, so new, renamed and deleted files show up in
//! search within moments instead of at the next full scan.

use crate::index::{Change, Index};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ACTION_ADDED, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME, FILE_ACTION_RENAMED_OLD_NAME,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_INFORMATION,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
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
    let Ok(folder) = (unsafe {
        CreateFileW(
            &HSTRING::from(root.to_string_lossy().as_ref()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }) else {
        return;
    };
    // Windows keeps collecting changes between calls; 64 KB holds about a thousand.
    let mut buffer = vec![0u32; 16 * 1024];
    loop {
        let mut bytes = 0u32;
        let read = unsafe {
            ReadDirectoryChangesW(
                folder,
                buffer.as_mut_ptr().cast(),
                std::mem::size_of_val(buffer.as_slice()) as u32,
                true,
                FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME,
                Some(&mut bytes),
                None,
                None,
            )
        };
        if read.is_err() {
            break; // the folder itself went away
        }
        if bytes == 0 {
            // More changes than the buffer holds: scan everything again.
            let on_change = on_change.clone();
            index.rebuild(move || on_change());
            continue;
        }
        let changes = unsafe { parse(&buffer, bytes as usize, root) };
        index.apply(root, changes, &*on_change);
    }
    unsafe {
        let _ = CloseHandle(folder);
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
