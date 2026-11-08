//! Read the existing Windows Outlook index; no mailbox credentials are stored.
use crate::index::{Entry, Kind};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// Keep the Outlook subprocess bounded independently of the visible list height.
pub const MAX_RESULTS: usize = 1000;

pub fn query(q: &str) -> Option<&str> {
    let q = q.trim();
    let end = q.find(char::is_whitespace).unwrap_or(q.len());
    match q[..end].to_ascii_lowercase().as_str() {
        "mail" | "email" | "type:email" => Some(q[end..].trim()),
        _ => None,
    }
}

pub fn search(term: &str, cancelled: impl Fn() -> bool) -> Result<Vec<Entry>, ()> {
    if cancelled() {
        return Err(());
    }
    // The native helper has no GUI startup, shell parser, or PowerShell runtime.
    // Isolation preserves cancellation and a hard deadline even if a COM provider hangs.
    let mut child = Command::new(std::env::current_exe().map_err(|_| ())?)
        .arg("--mail-query")
        .env("LUMEN_MAIL_QUERY", term)
        .creation_flags(0x0800_0000)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ())?;
    // Drain stdout concurrently so large subjects cannot fill the pipe and deadlock.
    let stdout = child.stdout.take().ok_or(())?;
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut bytes = Vec::new();
        let _ = stdout.take(16 * 1024 * 1024).read_to_end(&mut bytes);
        bytes
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let bytes = reader.join().map_err(|_| ())?;
                if !status.success() {
                    return Err(());
                }
                let rows: Vec<serde_json::Value> = serde_json::from_slice(&bytes).map_err(|_| ())?;
                return Ok(rows
                    .iter()
                    .filter_map(|row| {
                        let url = row.get("url")?.as_str()?;
                        if !url.starts_with("mapi:") && !url.starts_with("lumen-outlook:") {
                            return None;
                        }
                        let title = row.get("name")?.as_str()?;
                        let mut e = Entry::new(if title.is_empty() { "(No subject)" } else { title }.into(), url.into(), Kind::Email, 0);
                        e.detail = row.get("sender").and_then(|v| v.as_str()).unwrap_or("Outlook").into();
                        Some(e)
                    })
                    .collect());
            }
            Ok(None) if start.elapsed() < Duration::from_secs(7) && !cancelled() => std::thread::sleep(Duration::from_millis(30)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_only_explicit_mail_scope() {
        assert_eq!(query("MAIL invoice"), Some("invoice"));
        assert_eq!(query("email"), Some(""));
        assert_eq!(query("mailing list"), None);
        assert_eq!(query("type:email O'Brien"), Some("O'Brien"));
    }
}

#[derive(serde::Serialize)]
struct Row { name: String, url: String, sender: String }

fn sql(term: &str) -> String {
    let mut sql = format!("SELECT TOP {} System.Subject, System.ItemUrl, System.Message.FromName FROM SystemIndex WHERE System.Kind = 'email' AND System.ItemUrl LIKE 'mapi:%'", MAX_RESULTS);
    for word in term.split_whitespace().take(32) {
        let word = word.replace('\'', "''").replace('"', "");
        sql.push_str(&format!(" AND CONTAINS(*, '\"{word}*\"')"));
    }
    sql.push_str(" ORDER BY System.Message.DateReceived DESC"); sql
}

fn indexed(term: &str) -> windows::core::Result<Vec<Row>> {
    use crate::automation::{Object, text};
    let connection = Object::create("ADODB.Connection")?;
    connection.put("ConnectionTimeout", 3i32.into())?;
    connection.put("CommandTimeout", 4i32.into())?;
    connection.call("Open", &["Provider=Search.CollatorDSO;Extended Properties='Application=Windows';".into()])?;
    let result = (|| {
        let recordset = connection.called("Execute", &[sql(term).as_str().into()])?;
        let mut rows = Vec::new();
        while !bool::try_from(&recordset.get("EOF")?)? && rows.len() < MAX_RESULTS {
            let fields = recordset.object("Fields", &[])?;
            let field = |i: i32| -> windows::core::Result<String> { Ok(text(&fields.object("Item", &[i.into()])?.get("Value")?)) };
            rows.push(Row { name: field(0)?, url: field(1)?, sender: field(2)? });
            recordset.call("MoveNext", &[])?;
        }
        let _ = recordset.call("Close", &[]); Ok(rows)
    })();
    let _ = connection.call("Close", &[]); result
}

fn recent(term: &str) -> windows::core::Result<Vec<Row>> {
    use crate::automation::Object;
    let outlook = Object::active("Outlook.Application")?; // Attach only; never launch or configure Outlook.
    let ns = outlook.called("GetNamespace", &["MAPI".into()])?;
    let stores = ns.object("Stores", &[])?;
    let start = Instant::now(); let mut rows = Vec::new(); let mut seen = std::collections::HashSet::new();
    let words: Vec<_> = term.to_lowercase().split_whitespace().map(str::to_string).collect();
    'stores: for s in 1..=stores.number("Count").min(64) {
        let Ok(store) = stores.called("Item", &[s.into()]) else { continue };
        let store_id = store.text("StoreID");
        for id in [6i32, 5] {
            let Ok(folder) = store.called("GetDefaultFolder", &[id.into()]) else { continue };
            let Ok(messages) = folder.object("Items", &[]) else { continue };
            let _ = messages.call("Sort", &["[ReceivedTime]".into(), true.into()]);
            for i in 1..=messages.number("Count").min(150) {
                if start.elapsed() > Duration::from_secs(2) || rows.len() >= MAX_RESULTS { break 'stores; }
                let Ok(message) = messages.called("Item", &[i.into()]) else { continue };
                if message.number("Class") != 43 { continue; }
                let name = message.text("Subject"); let sender = message.text("SenderName");
                let text = format!("{name} {sender} {}", message.text("Body")).to_lowercase();
                let entry = message.text("EntryID");
                if words.iter().all(|w| text.contains(w)) && !entry.is_empty() && seen.insert(entry.clone()) {
                    rows.push(Row { name, url: format!("lumen-outlook:{store_id}/{entry}"), sender: format!("{sender} · recent mail (COM)") });
                }
            }
        }
    }
    Ok(rows)
}

pub fn worker() {
    let result = (|| {
        let _com = crate::automation::Apartment::new().map_err(|_| ())?;
        let term = std::env::var("LUMEN_MAIL_QUERY").unwrap_or_default();
        if term.len() > 4096 { return Err(()); }
        let indexed = indexed(&term);
        match indexed {
            Ok(rows) if !rows.is_empty() => Ok(rows),
            Ok(rows) => Ok(recent(&term).unwrap_or(rows)),
            Err(_) => recent(&term).map_err(|_| ()),
        }
    })();
    match result {
        Ok(rows) => { let _ = serde_json::to_writer(std::io::stdout().lock(), &rows); }
        Err(()) => std::process::exit(1),
    }
}

/// Display an existing Outlook item on a separate COM apartment.
pub fn open(target: &str) {
    let Some((store, entry)) = target.strip_prefix("lumen-outlook:").and_then(|s| s.split_once('/')) else { return; };
    let (store, entry) = (store.to_string(), entry.to_string());
    std::thread::spawn(move || {
        let Ok(_com) = crate::automation::Apartment::new() else { return };
        let Ok(outlook) = crate::automation::Object::active("Outlook.Application") else { return };
        let Ok(ns) = outlook.called("GetNamespace", &["MAPI".into()]) else { return };
        if let Ok(item) = ns.called("GetItemFromID", &[entry.as_str().into(), store.as_str().into()]) { let _ = item.call("Display", &[]); }
    });
}

#[cfg(test)] mod native_tests {
    use super::*;
    #[test] fn indexed_query_quotes_user_text_and_retains_all_word_semantics() {
        let query = sql("O'Brien invoice"); assert!(query.contains("O''Brien"));
        assert_eq!(query.matches("CONTAINS").count(), 2); assert!(query.contains("TOP 1000"));
    }
}
