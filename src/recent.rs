//! Finds the folders this user opens most, from Windows jump lists, so Lumen can
//! index busy folders that its default locations and drive samples miss.
//!
//! Every app that uses the Windows shell keeps a jump list in
//! `Recent\AutomaticDestinations`. Each is an OLE compound file whose `DestList`
//! stream records, per item, its path, open count, last use and pin state. The
//! format is undocumented, so anything unexpected is skipped rather than guessed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A folder must be opened this many times to be suggested, unless pinned.
const MIN_OPENS: u32 = 10;
/// ...and used within this long.
const MAX_AGE: Duration = Duration::from_secs(180 * 24 * 60 * 60);
/// At most this many folders are suggested at a time.
const LIMIT: usize = 15;

struct Item {
    path: String,
    opens: u32,
    last: Option<SystemTime>,
    pinned: bool,
}

/// The busiest folders on fixed and mapped drives that `covered` does not already
/// reach, pinned folders first. Checks that each still exists, so call it off the
/// UI thread.
pub fn folders(covered: impl Fn(&str) -> bool) -> Vec<String> {
    let Some(dir) = recent_dir() else { return Vec::new() };
    let mut items = Vec::new();
    for file in std::fs::read_dir(dir.join("AutomaticDestinations")).into_iter().flatten().flatten() {
        if let Some(list) = std::fs::read(file.path()).ok().and_then(|bytes| stream(&bytes, "DestList")) {
            items.extend(dest_list(&list));
        }
    }
    let candidates: Vec<String> = rank(items, SystemTime::now()).into_iter().filter(|f| wanted(f) && !covered(f)).take(LIMIT * 3).collect();
    // A server that has gone away can hold a check for many seconds, so run them together.
    let exists: Vec<bool> = std::thread::scope(|scope| {
        let checks: Vec<_> = candidates.iter().map(|f| scope.spawn(move || Path::new(f).is_dir())).collect();
        checks.into_iter().map(|check| check.join().unwrap_or(false)).collect()
    });
    candidates.into_iter().zip(exists).filter_map(|(folder, exists)| exists.then_some(folder)).take(LIMIT).collect()
}

fn recent_dir() -> Option<PathBuf> {
    use windows::Win32::UI::Shell::{FOLDERID_Recent, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Recent, KF_FLAG_DEFAULT, None).ok()?;
        let text = path.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(path.0 as *const std::ffi::c_void));
        text.map(PathBuf::from)
    }
}

/// On a fixed or mapped drive, and not a drive root, Windows or program folder.
fn wanted(folder: &str) -> bool {
    const FIXED: u32 = 3;
    const REMOTE: u32 = 4;
    let drive = |root: &str| unsafe { windows::Win32::Storage::FileSystem::GetDriveTypeW(&windows::core::HSTRING::from(root)) };
    folder.len() > 3
        && matches!(drive(&folder[..3]), FIXED | REMOTE)
        && !["SystemRoot", "ProgramFiles", "ProgramFiles(x86)", "ProgramData"]
            .iter()
            .filter_map(|name| std::env::var(name).ok())
            .any(|system| crate::settings::within(folder, &system))
}

/// Credits folders to themselves and files to their parent folder, then keeps
/// folders opened often and recently, or pinned. Busiest first.
fn rank(items: Vec<Item>, now: SystemTime) -> Vec<String> {
    struct Folder {
        path: String,
        opens: u32,
        last: Option<SystemTime>,
        pinned: bool,
    }
    let mut folders: HashMap<String, Folder> = HashMap::new();
    for item in items {
        let Some((path, is_folder)) = folder_of(&item.path) else { continue };
        let folder = folders.entry(path.to_lowercase()).or_insert(Folder { path, opens: 0, last: None, pinned: false });
        folder.opens = folder.opens.saturating_add(item.opens);
        folder.last = folder.last.max(item.last);
        // A pinned file is not a reason to index the whole folder around it.
        folder.pinned |= item.pinned && is_folder;
    }
    let recent = |last: Option<SystemTime>| last.is_some_and(|t| now.duration_since(t).unwrap_or_default() <= MAX_AGE);
    let mut kept: Vec<Folder> = folders.into_values().filter(|f| f.pinned || f.opens >= MIN_OPENS && recent(f.last)).collect();
    kept.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.opens.cmp(&a.opens)).then_with(|| a.path.cmp(&b.path)));
    kept.into_iter().map(|f| f.path).collect()
}

/// The folder an item counts toward, and whether the item is that folder. Only
/// drive-letter paths: Lumen's settings take mapped drives, not `\\server` paths.
fn folder_of(path: &str) -> Option<(String, bool)> {
    let bytes = path.as_bytes();
    if !(bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\') {
        return None;
    }
    let path = Path::new(path.trim_end_matches('\\'));
    // Folder names often hold dots ("1. Reports"); a file extension is short and has no spaces.
    let file = path.extension().is_some_and(|x| x.len() <= 8 && x.to_string_lossy().chars().all(|c| c.is_ascii_alphanumeric()));
    if file { Some((path.parent()?.to_str()?.to_string(), false)) } else { Some((path.to_str()?.to_string(), true)) }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}
fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}
fn i64_at(b: &[u8], at: usize) -> Option<i64> {
    Some(i64::from_le_bytes(b.get(at..at.checked_add(8)?)?.try_into().ok()?))
}
fn utf16(b: &[u8]) -> String {
    String::from_utf16_lossy(&b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
}

/// Sector ids at or above this are markers (end of chain, free, FAT sector).
const MAX_SECTOR: u32 = 0xFFFF_FFFA;

/// Reads one named stream from an OLE compound file.
fn stream(b: &[u8], name: &str) -> Option<Vec<u8>> {
    if b.get(..8)? != [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1] {
        return None;
    }
    let sector = 1usize.checked_shl(u16_at(b, 0x1E)?.into())?;
    let mini = 1usize.checked_shl(u16_at(b, 0x20)?.into())?;
    if !matches!(sector, 512 | 4096) || mini != 64 {
        return None;
    }
    let cutoff = u32_at(b, 0x38)? as usize;

    // Sectors holding the FAT: 109 listed in the header, the rest in a DIFAT chain.
    let mut fat_sectors: Vec<u32> = (0..109).filter_map(|i| u32_at(b, 0x4C + 4 * i)).collect();
    let mut next = u32_at(b, 0x44)?;
    for _ in 0..b.len() / sector {
        if next >= MAX_SECTOR {
            break;
        }
        let at = (next as usize + 1) * sector;
        fat_sectors.extend((0..sector / 4 - 1).filter_map(|i| u32_at(b, at + 4 * i)));
        next = u32_at(b, at + sector - 4)?;
    }
    let mut fat = Vec::new();
    for s in fat_sectors.into_iter().filter(|&s| s < MAX_SECTOR) {
        let at = (s as usize + 1) * sector;
        fat.extend(b.get(at..at + sector)?.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])));
    }

    let dir = chain(b, &fat, u32_at(b, 0x30)?, sector, sector, None);
    let (mut root, mut found) = (None, None);
    for entry in dir.chunks_exact(128) {
        let name_bytes = u16_at(entry, 0x40)? as usize;
        if !(2..=64).contains(&name_bytes) {
            continue;
        }
        let start_size = (u32_at(entry, 0x74)?, u32_at(entry, 0x78)? as usize);
        match entry[0x42] {
            5 => root = Some(start_size),
            2 if utf16(&entry[..name_bytes - 2]) == name => found = Some(start_size),
            _ => {}
        }
    }
    let (start, size) = found?;
    if size >= cutoff {
        return Some(chain(b, &fat, start, sector, sector, Some(size)));
    }
    // Small streams live in the mini stream, addressed through the mini FAT.
    let (root_start, root_size) = root?;
    let mini_stream = chain(b, &fat, root_start, sector, sector, Some(root_size));
    let mini_fat: Vec<u32> =
        chain(b, &fat, u32_at(b, 0x3C)?, sector, sector, None).chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    Some(chain(&mini_stream, &mini_fat, start, mini, 0, Some(size)))
}

/// Joins a chain of sectors, stopping at its end, a bad link or a loop.
fn chain(src: &[u8], table: &[u32], start: u32, size: usize, base: usize, len: Option<usize>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut s = start as usize;
    for _ in 0..table.len() {
        if s >= table.len() || len.is_some_and(|len| out.len() >= len) {
            break;
        }
        let at = base + s * size;
        let Some(data) = src.get(at..(at + size).min(src.len())) else { break };
        out.extend_from_slice(data);
        s = table[s] as usize;
    }
    if let Some(len) = len {
        out.truncate(len);
    }
    out
}

/// The items in a DestList stream, in the layout of Windows 10 and 11.
fn dest_list(d: &[u8]) -> Vec<Item> {
    let mut items = Vec::new();
    if !u32_at(d, 0).is_some_and(|version| (2..=6).contains(&version)) {
        return items;
    }
    let mut at = 32;
    while let (Some(opens), Some(len)) = (u32_at(d, at + 0x74), u16_at(d, at + 0x80)) {
        let path_at = at + 0x82;
        let extra_at = path_at + 2 * len as usize;
        // After the path, a size and a block of extra properties (Office uses it).
        let (Some(path), Some(extra)) = (d.get(path_at..extra_at), u32_at(d, extra_at)) else { break };
        items.push(Item {
            path: utf16(path),
            opens: (opens as i32).max(1) as u32,
            last: i64_at(d, at + 0x64).and_then(filetime),
            pinned: u32_at(d, at + 0x6C).is_some_and(|pin| pin != u32::MAX),
        });
        at = extra_at + 4 + extra as usize;
    }
    items
}

fn filetime(t: i64) -> Option<SystemTime> {
    // 100 ns ticks from 1601 to 1970.
    let ticks = u64::try_from(t).ok()?.checked_sub(116_444_736_000_000_000)?;
    UNIX_EPOCH.checked_add(Duration::from_nanos(ticks.checked_mul(100)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const END: u32 = 0xFFFF_FFFE;
    const FREE: u32 = 0xFFFF_FFFF;

    fn put(buf: &mut [u8], at: usize, bytes: &[u8]) {
        buf[at..at + bytes.len()].copy_from_slice(bytes);
    }

    fn words(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn dir_entry(entry: &mut [u8], name: &str, kind: u8, start: u32, size: usize) {
        let units: Vec<u8> = name.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        put(entry, 0, &units);
        put(entry, 0x40, &(units.len() as u16).to_le_bytes());
        entry[0x42] = kind;
        put(entry, 0x74, &start.to_le_bytes());
        put(entry, 0x78, &(size as u32).to_le_bytes());
    }

    /// A minimal compound file holding one stream: sector 0 is the FAT, 1 the
    /// directory, 2 the mini FAT, and the stream (or the mini stream) follows.
    fn compound(name: &str, data: &[u8]) -> Vec<u8> {
        let small = data.len() < 4096;
        let minis = data.len().div_ceil(64).max(1);
        let mut body = data.to_vec();
        if small {
            body.resize(minis * 64, 0);
        }
        let sectors = body.len().div_ceil(512);
        body.resize(sectors * 512, 0);
        let mut fat = vec![0xFFFF_FFFD, END, END];
        fat.extend((0..sectors).map(|i| if i + 1 < sectors { 4 + i as u32 } else { END }));
        fat.resize(128, FREE);
        let mut mini_fat: Vec<u32> =
            if small { (0..minis).map(|i| if i + 1 < minis { i as u32 + 1 } else { END }).collect() } else { Vec::new() };
        mini_fat.resize(128, FREE);

        let mut out = vec![0u8; 512];
        put(&mut out, 0, &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
        put(&mut out, 0x1E, &9u16.to_le_bytes());
        put(&mut out, 0x20, &6u16.to_le_bytes());
        put(&mut out, 0x2C, &words(&[1, 1]));
        put(&mut out, 0x38, &words(&[4096, 2, 1, END, 0]));
        let mut header_fat = vec![0];
        header_fat.resize(109, FREE);
        put(&mut out, 0x4C, &words(&header_fat));
        out.extend(words(&fat));
        let mut dir = vec![0u8; 512];
        let (root_start, root_size) = if small { (3, minis * 64) } else { (END, 0) };
        dir_entry(&mut dir[..128], "Root Entry", 5, root_start, root_size);
        dir_entry(&mut dir[128..256], name, 2, if small { 0 } else { 3 }, data.len());
        out.extend(dir);
        out.extend(words(&mini_fat));
        out.extend(body);
        out
    }

    /// A DestList stream: (path, opens, last use as a FILETIME, pin state).
    fn dest(items: &[(&str, u32, i64, u32)]) -> Vec<u8> {
        let mut d = vec![0u8; 32];
        d[0] = 4;
        for (n, (path, opens, time, pin)) in items.iter().enumerate() {
            let mut entry = vec![0u8; 0x82];
            put(&mut entry, 0x64, &time.to_le_bytes());
            put(&mut entry, 0x6C, &pin.to_le_bytes());
            put(&mut entry, 0x74, &opens.to_le_bytes());
            put(&mut entry, 0x80, &(path.encode_utf16().count() as u16).to_le_bytes());
            entry.extend(path.encode_utf16().flat_map(u16::to_le_bytes));
            // Every other entry carries an extra property block.
            let extra = if n % 2 == 1 { vec![7u8; 12] } else { Vec::new() };
            entry.extend((extra.len() as u32).to_le_bytes());
            entry.extend(extra);
            d.extend(entry);
        }
        d
    }

    #[test]
    fn reads_small_and_large_streams_from_compound_files() {
        for len in [10, 4095, 4096, 9000] {
            let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let file = compound("DestList", &data);
            assert_eq!(stream(&file, "DestList").as_deref(), Some(&data[..]), "{len} bytes");
            assert!(stream(&file, "Other").is_none());
            for cut in [0, 8, 100, 600, file.len() / 2] {
                let _ = stream(&file[..cut], "DestList");
            }
        }
        assert!(stream(b"not a compound file", "DestList").is_none());
    }

    #[test]
    fn parses_dest_list_entries() {
        let at = 134_000_000_000_000_000; // a FILETIME in 2025
        let items = dest_list(&dest(&[(r"W:\Team\a.xlsx", 3, at, FREE), (r"W:\Team", 0, 0, 0)]));
        assert_eq!(items.len(), 2);
        assert_eq!((items[0].path.as_str(), items[0].opens, items[0].pinned), (r"W:\Team\a.xlsx", 3, false));
        assert_eq!(items[0].last, filetime(at));
        assert_eq!((items[1].path.as_str(), items[1].opens, items[1].pinned, items[1].last), (r"W:\Team", 1, true, None));
        let mut old = dest(&[(r"W:\Team", 3, at, FREE)]);
        old[0] = 1;
        assert!(dest_list(&old).is_empty(), "Windows 7 and 8 lists use another layout");
    }

    #[test]
    fn ranks_busy_recent_and_pinned_folders() {
        let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let ago = |days: u64| Some(now - Duration::from_secs(days * 24 * 60 * 60));
        let item = |path: &str, opens, last, pinned| Item { path: path.into(), opens, last, pinned };
        let ranked = rank(
            vec![
                item(r"W:\Reports\jan.xlsx", 6, ago(3), false),
                item(r"w:\reports\feb.xlsx", 6, ago(400), false),
                item(r"W:\Production\.Boards", 20, ago(10), false),
                item(r"W:\Production\1. Daily Reports", 15, ago(10), false),
                item(r"W:\Old", 50, ago(200), false),
                item(r"W:\Rare", 3, ago(1), false),
                item(r"\\server\share\Busy", 99, ago(1), false),
                item(r"W:\Pinned\", 1, None, true),
                item(r"W:\PinnedFile\a.xlsx", 1, ago(1), true),
            ],
            now,
        );
        assert_eq!(ranked, [r"W:\Pinned", r"W:\Production\.Boards", r"W:\Production\1. Daily Reports", r"W:\Reports"]);
    }

    /// Prints what discovery finds on this PC.
    #[test]
    #[ignore]
    fn recent_folders_on_this_pc() {
        let dir = recent_dir().unwrap().join("AutomaticDestinations");
        let mut items = Vec::new();
        for file in std::fs::read_dir(&dir).unwrap().flatten() {
            let list = std::fs::read(file.path()).ok().and_then(|b| stream(&b, "DestList"));
            let version = list.as_deref().and_then(|l| u32_at(l, 0));
            let parsed = list.as_deref().map(dest_list).unwrap_or_default();
            println!("{}: version {version:?}, {} items", file.file_name().to_string_lossy(), parsed.len());
            for item in parsed.iter().filter(|i| i.pinned) {
                println!("  pinned {}", item.path);
            }
            items.extend(parsed);
        }
        println!("ranked: {:#?}", rank(items, SystemTime::now()).into_iter().take(40).collect::<Vec<_>>());
        let started = std::time::Instant::now();
        let chosen = folders(|_| false);
        println!("chosen in {:?}: {chosen:#?}", started.elapsed());
    }
}
