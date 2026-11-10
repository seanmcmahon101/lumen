//! Look for newer Windows installers on Lumen's GitHub releases.

use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub struct Update {
    pub version: String,
    download: Option<String>, // the versioned installer, when the release has one
}

impl Update {
    /// Downloads the versioned installer directly, or opens the release page.
    pub fn url(&self) -> String {
        self.download.clone().unwrap_or_else(|| format!("https://github.com/seanmcmahon101/lumen/releases/tag/v{}", self.version))
    }
}

/// Checks at startup and every six hours. Send on the returned sender each time
/// Lumen opens to check again if the last check was over an hour ago.
pub fn watch(ctx: eframe::egui::Context) -> (mpsc::Receiver<Update>, mpsc::Sender<()>) {
    let (sender, receiver) = mpsc::channel();
    let (opened_tx, opened) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut last: Option<Instant> = None;
        loop {
            if last.is_none_or(|t| t.elapsed() >= Duration::from_secs(60 * 60)) {
                last = Some(Instant::now());
                if let Some(update) = check() {
                    if sender.send(update).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            }
            if let Err(mpsc::RecvTimeoutError::Disconnected) = opened.recv_timeout(Duration::from_secs(6 * 60 * 60)) {
                break;
            }
        }
    });
    (receiver, opened_tx)
}

/// Opens the newest installer, checking GitHub again so a release published
/// since the last check isn't missed. Falls back to `known` if the check fails.
pub fn open_latest(known: &Update) {
    let fallback = known.url();
    std::thread::spawn(move || {
        let url = check().map_or(fallback, |latest| latest.url());
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        }
        crate::win::shell_open(&url, "open", None);
    });
}

fn check() -> Option<Update> {
    let script = "[Console]::OutputEncoding = [Text.Encoding]::UTF8; \
        $ErrorActionPreference = 'Stop'; \
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; \
        Invoke-RestMethod -TimeoutSec 8 'https://api.github.com/repos/seanmcmahon101/lumen/releases/latest' | ConvertTo-Json -Depth 5 -Compress";
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(crate::win::NO_WINDOW)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let release = serde_json::from_slice(&output.stdout).ok()?;
    newer_release(&release, env!("CARGO_PKG_VERSION"))
}

fn version_parts(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.split('.');
    let parse = |part: &str| part.bytes().all(|b| b.is_ascii_digit()).then(|| part.parse::<u32>().ok()).flatten();
    let version = (parse(parts.next()?)?, parse(parts.next()?)?, parse(parts.next()?)?);
    parts.next().is_none().then_some(version)
}

fn newer_release(release: &serde_json::Value, installed: &str) -> Option<Update> {
    let version = release.get("tag_name")?.as_str()?.strip_prefix('v')?;
    let assets = release.get("assets")?.as_array()?;
    let named = |name: &str| assets.iter().find(|asset| asset.get("name").and_then(serde_json::Value::as_str) == Some(name));
    let download =
        named(&format!("Lumen-Setup-{version}-x64.exe")).and_then(|asset| asset.get("browser_download_url")?.as_str()).map(String::from);
    let has_installer = download.is_some() || named("Lumen-Setup-x64.exe").is_some();
    if has_installer && version_parts(version)? > version_parts(installed)? { Some(Update { version: version.into(), download }) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn offers_only_a_newer_stable_release_with_an_installer() {
        let mut release = json!({"tag_name": "v0.10.0", "assets": [{"name": "Lumen-Setup-x64.exe"}]});
        assert_eq!(newer_release(&release, "0.9.0").unwrap().version, "0.10.0");
        assert!(newer_release(&release, "0.10.0").is_none());
        assert!(newer_release(&release, "0.11.0").is_none());
        assert!(newer_release(&release, "0.9.0").unwrap().url().ends_with("/releases/tag/v0.10.0"));
        release["assets"] =
            json!([{"name": "Lumen-Setup-0.10.0-x64.exe", "browser_download_url": "https://example.test/Lumen-Setup-0.10.0-x64.exe"}]);
        assert_eq!(newer_release(&release, "0.9.0").unwrap().url(), "https://example.test/Lumen-Setup-0.10.0-x64.exe");
        release["tag_name"] = json!("v1.0.0-rc.1");
        assert!(newer_release(&release, "0.9.0").is_none());
        release["tag_name"] = json!("v1.0.0");
        release["assets"] = json!([]);
        assert!(newer_release(&release, "0.9.0").is_none());
    }
}
