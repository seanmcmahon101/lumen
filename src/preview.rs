//! On-demand, bounded workbook reads on a dedicated worker.
use calamine::{Reader, open_workbook_auto};
use std::os::windows::fs::MetadataExt;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Line {
    pub location: String,
    pub text: String,
}
pub type Request = (String, String); // path, search
pub type Response = (Request, Result<Vec<Line>, String>);
pub fn spawn(
    wake: impl Fn() + Send + 'static,
) -> (mpsc::Sender<Request>, mpsc::Receiver<Response>) {
    let (tx, requests) = mpsc::channel::<Request>();
    let (answers, rx) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(mut request) = requests.recv() {
            while let Ok(newer) = requests.try_recv() {
                request = newer;
            }
            let result = read(&request.0, &request.1);
            if answers.send((request, result)).is_err() {
                break;
            }
            wake();
        }
    });
    (tx, rx)
}
/// The same files the index reads workbook text from.
pub fn supported(path: &str) -> bool {
    crate::index::spreadsheet(&path.to_lowercase())
}
pub fn read(path: &str, query: &str) -> Result<Vec<Line>, String> {
    if !supported(path) {
        return Err("Select a spreadsheet or CSV file to preview.".into());
    }
    if path.starts_with("\\\\") || !crate::index::local_path(path) {
        return Err("Network previews are unavailable. Open the workbook to read it.".into());
    }
    let md = std::fs::metadata(path)
        .map_err(|_| "This file is unavailable. Check its location and try again.")?;
    if md.len() > crate::index::MAX_SOURCE_BYTES {
        return Err("This workbook exceeds the 16 MB preview limit. Open it to read more.".into());
    }
    if md.file_attributes() & crate::index::CLOUD_PLACEHOLDER != 0 {
        return Err(
            "This file is online only. Download it in File Explorer before previewing.".into(),
        );
    }
    let (_, query) = crate::index::split_filter(query);
    let query = query.to_lowercase();
    let location = path.to_lowercase();
    let tokens: Vec<_> = query
        .split_whitespace()
        .filter(|word| !word.starts_with("ext:") && !location.contains(word))
        .collect();
    let matches = |text: &str| {
        let lower = text.to_lowercase();
        tokens.is_empty() || tokens.iter().any(|t| lower.contains(t))
    };
    let mut lines = Vec::new();
    if path.to_lowercase().ends_with(".csv") || path.to_lowercase().ends_with(".tsv") {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take(512 * 1024)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        for (i, text) in String::from_utf8_lossy(&bytes).lines().enumerate() {
            if !text.is_empty() && matches(text) {
                lines.push(Line {
                    location: format!("Line {}", i + 1),
                    text: excerpt(text),
                });
            }
            if lines.len() == 8 {
                break;
            }
        }
    } else {
        let mut book = open_workbook_auto(path).map_err(
            |_| "This workbook could not be read. It may be locked or password protected.",
        )?;
        let names = book.sheet_names();
        let started = Instant::now();
        let mut visited = 0;
        'sheets: for name in names.iter().take(32) {
            if started.elapsed() > Duration::from_secs(2) {
                break;
            }
            let Ok(range) = book.worksheet_range(name) else {
                continue;
            };
            let (start_row, start_col) = range.start().unwrap_or((0, 0));
            for (row, col, cell) in range.used_cells() {
                visited += 1;
                if visited > 100_000 || started.elapsed() > Duration::from_secs(2) {
                    break 'sheets;
                }
                let text = cell.to_string();
                if !text.is_empty() && (matches(&text) || matches(name)) {
                    lines.push(Line {
                        location: format!(
                            "{name}!{}{}",
                            column(start_col as usize + col),
                            start_row as usize + row + 1
                        ),
                        text: excerpt(&text),
                    });
                    if lines.len() == 8 {
                        break 'sheets;
                    }
                }
            }
        }
        if lines.is_empty() {
            for name in names.iter().take(8) {
                lines.push(Line {
                    location: name.clone(),
                    text: "Worksheet · no matching cells in the bounded preview".into(),
                });
            }
        }
    }
    if lines.is_empty() {
        return Err("No matching text in the preview. Open the file to read more.".into());
    }
    Ok(lines)
}
fn excerpt(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(220)
        .collect()
}
fn column(mut index: usize) -> String {
    let mut name = Vec::new();
    loop {
        name.push((b'A' + (index % 26) as u8) as char);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    name.into_iter().rev().collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workbook_preview_returns_real_sheet_and_cell_context() {
        let file = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/budget.xlsx");
        let lines = read(file, "").unwrap();
        assert!(!lines.is_empty());
        assert!(
            lines
                .iter()
                .all(|line| line.location.contains('!') && !line.text.is_empty())
        );
        assert!(lines.len() <= 8);
        assert_eq!(column(26), "AA");
        assert_eq!(column(701), "ZZ");
    }
    #[test]
    fn preview_rejects_network_files_and_other_formats() {
        assert!(read(r"\\server\share\a.xlsx", "").is_err());
        assert!(!supported("file.exe"));
    }
}
