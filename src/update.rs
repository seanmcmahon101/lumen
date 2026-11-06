//! Look for newer Windows installers on Lumen's GitHub releases.

use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

pub struct Update {
    pub version: String,
}

impl Update {
    pub fn release_url(&self) -> String {
        format!("https://github.com/seanmcmahon101/lumen/releases/tag/v{}", self.version)
    }
}

pub fn watch(ctx: eframe::egui::Context) -> mpsc::Receiver<Update> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        loop {
            if let Some(update) = check() {
                if sender.send(update).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
            std::thread::sleep(Duration::from_secs(6 * 60 * 60));
        }
    });
    receiver
}

fn check() -> Option<Update> {
    let script = "[Console]::OutputEncoding = [Text.Encoding]::UTF8; \
        $ErrorActionPreference = 'Stop'; \
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; \
        Invoke-RestMethod -TimeoutSec 8 'https://api.github.com/repos/seanmcmahon101/lumen/releases/latest' | ConvertTo-Json -Depth 5 -Compress";
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(0x0800_0000)
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
    let has_installer = release
        .get("assets")?
        .as_array()?
        .iter()
        .any(|asset| asset.get("name").and_then(serde_json::Value::as_str) == Some("Lumen-Setup-x64.exe"));
    if has_installer && version_parts(version)? > version_parts(installed)? { Some(Update { version: version.into() }) } else { None }
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
        release["tag_name"] = json!("v1.0.0-rc.1");
        assert!(newer_release(&release, "0.9.0").is_none());
        release["tag_name"] = json!("v1.0.0");
        release["assets"] = json!([]);
        assert!(newer_release(&release, "0.9.0").is_none());
    }
}
