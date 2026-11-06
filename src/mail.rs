//! Read the existing Windows Outlook index; no mailbox credentials are stored.
use crate::index::{Entry, Kind};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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
    // Pass data through the environment, never interpolate it into executable PowerShell.
    let script = r#"
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.Encoding]::UTF8
$items=@()
$indexed=$false
$c=$null
try {
  $c=New-Object -ComObject ADODB.Connection
  $c.ConnectionTimeout=3
  $c.CommandTimeout=4
  $c.Open("Provider=Search.CollatorDSO;Extended Properties='Application=Windows';")
  $where="System.Kind = 'email' AND System.ItemUrl LIKE 'mapi:%'"
  $words=@($env:LUMEN_MAIL_QUERY -split '\s+' | Where-Object { $_ })
  foreach($word in $words) {
    $word=$word.Replace("'","''").Replace('"','')
    $where += " AND CONTAINS(*, '" + '"' + $word + '*"' + "')"
  }
  $rs=$c.Execute("SELECT TOP 6 System.Subject, System.ItemUrl, System.Message.FromName FROM SystemIndex WHERE " + $where + " ORDER BY System.Message.DateReceived DESC")
  $items=@()
  while(!$rs.EOF) {
    $items += @{name=[string]$rs.Fields.Item(0).Value;url=[string]$rs.Fields.Item(1).Value;sender=[string]$rs.Fields.Item(2).Value}
    $rs.MoveNext()
  }
  $indexed=$true
} catch {} finally { if($null -ne $c -and $c.State -eq 1) { $c.Close() } }
if($items.Count -eq 0) {
  try {
    # Attach only: never start Outlook or prompt to configure a profile.
    $outlook=[Runtime.InteropServices.Marshal]::GetActiveObject('Outlook.Application')
    $ns=$outlook.GetNamespace('MAPI')
    $timer=[Diagnostics.Stopwatch]::StartNew()
    $words=@($env:LUMEN_MAIL_QUERY.ToLowerInvariant() -split '\s+' | Where-Object { $_ })
    $seen=New-Object 'Collections.Generic.HashSet[string]'
    foreach($store in $ns.Stores) {
      foreach($folderId in @(6,5)) {
        if($timer.Elapsed.TotalSeconds -gt 2 -or $items.Count -ge 6) { break }
        try { $folder=$store.GetDefaultFolder($folderId); $messages=$folder.Items; $messages.Sort('[ReceivedTime]',$true) } catch { continue }
        for($i=1; $i -le [Math]::Min($messages.Count,150); $i++) {
          if($timer.Elapsed.TotalSeconds -gt 2 -or $items.Count -ge 6) { break }
          $msg=$messages.Item($i)
          if($msg.Class -ne 43) { continue }
          $text=([string]$msg.Subject+' '+[string]$msg.SenderName+' '+[string]$msg.Body).ToLowerInvariant()
          $match=$true
          foreach($word in $words) { if(!$text.Contains($word)) { $match=$false; break } }
          if($match -and $seen.Add($msg.EntryID)) {
            $items += @{name=[string]$msg.Subject;url=('lumen-outlook:'+$store.StoreID+'/'+$msg.EntryID);sender=([string]$msg.SenderName+' · recent mail (COM)')}
          }
          [void][Runtime.InteropServices.Marshal]::ReleaseComObject($msg)
        }
      }
      if($timer.Elapsed.TotalSeconds -gt 2 -or $items.Count -ge 6) { break }
    }
    $indexed=$true
  } catch {}
}
if(!$indexed) { exit 1 }
ConvertTo-Json -InputObject $items -Compress
"#;
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
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
        let _ = stdout.take(1024 * 1024).read_to_end(&mut bytes);
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

/// Open a classic Outlook COM result without blocking the launcher.
pub fn open(target: &str) {
    let Some((store, entry)) = target.strip_prefix("lumen-outlook:").and_then(|s| s.split_once('/')) else {
        return;
    };
    let (store, entry) = (store.to_string(), entry.to_string());
    std::thread::spawn(move || {
        let _ = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "$o=[Runtime.InteropServices.Marshal]::GetActiveObject('Outlook.Application'); $o.GetNamespace('MAPI').GetItemFromID($env:LUMEN_ENTRY,$env:LUMEN_STORE).Display()"])
            .env("LUMEN_ENTRY", entry).env("LUMEN_STORE", store)
            .creation_flags(0x0800_0000).status();
    });
}
