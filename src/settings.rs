use serde::{Deserialize, Serialize};
use std::sync::{
    OnceLock, RwLock,
    atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[allow(
    clippy::enum_variant_names,
    reason = "The saved values name complete key chords."
)]
pub enum Hotkey {
    #[default]
    WinSpace,
    AltSpace,
    CtrlSpace,
    CtrlAltSpace,
}

impl Hotkey {
    pub const ALL: [Self; 4] = [
        Self::WinSpace,
        Self::AltSpace,
        Self::CtrlSpace,
        Self::CtrlAltSpace,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::WinSpace => "Win+Space",
            Self::AltSpace => "Alt+Space",
            Self::CtrlSpace => "Ctrl+Space",
            Self::CtrlAltSpace => "Ctrl+Alt+Space",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub hotkey: Hotkey,
    pub apps: bool,
    pub files: bool,
    pub excel: bool,
    pub mail: bool,
    pub profile: bool,
    pub folders: Vec<String>,
    pub exclusions: Vec<String>,
    /// Shared folders already added for this user, so one removed in
    /// preferences is not added again.
    pub shared_added: Vec<String>,
}

/// Team folders indexed for everyone whose PC can reach them.
const SHARED_FOLDERS: [&str; 1] = [r"W:\Accounts\SM\AP INVOICE PROJECT"];

/// Adds each reachable shared folder not added before. Returns whether settings
/// changed. Call off the UI thread: a mapped drive may answer slowly.
pub fn add_shared_folders() -> bool {
    let reachable: Vec<&str> = SHARED_FOLDERS
        .into_iter()
        .filter(|f| std::path::Path::new(f).is_dir())
        .collect();
    let mut settings = get();
    let changed = settings.add_shared(&reachable);
    if changed {
        set(settings);
    }
    changed
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: Hotkey::default(),
            apps: true,
            files: true,
            excel: true,
            mail: true,
            profile: true,
            folders: Vec::new(),
            exclusions: Vec::new(),
            shared_added: Vec::new(),
        }
    }
}

static CURRENT: OnceLock<RwLock<Settings>> = OnceLock::new();
pub static REVISION: AtomicU64 = AtomicU64::new(0);

fn current() -> &'static RwLock<Settings> {
    CURRENT.get_or_init(|| RwLock::new(crate::storage::load("settings.json")))
}
pub fn get() -> Settings {
    current().read().unwrap().clone()
}

pub fn set(settings: Settings) {
    *current().write().unwrap() = settings.clone();
    REVISION.fetch_add(1, Ordering::Release);
    crate::hook::set_hotkey(settings.hotkey);
    crate::tray::refresh_tip();
    crate::storage::save("settings.json", &settings);
}

pub fn within(path: &str, folder: &str) -> bool {
    let folder = folder.trim_end_matches(['\\', '/']);
    path.eq_ignore_ascii_case(folder)
        || path
            .get(..folder.len())
            .is_some_and(|s| s.eq_ignore_ascii_case(folder))
            && path
                .as_bytes()
                .get(folder.len())
                .is_some_and(|c| *c == b'\\' || *c == b'/')
}

impl Settings {
    fn add_shared(&mut self, reachable: &[&str]) -> bool {
        let mut changed = false;
        for folder in reachable {
            if self.shared_added.iter().any(|f| f.eq_ignore_ascii_case(folder)) {
                continue;
            }
            self.shared_added.push((*folder).into());
            if !self.folders.iter().any(|f| f.eq_ignore_ascii_case(folder)) {
                self.folders.push((*folder).into());
            }
            changed = true;
        }
        changed
    }
    pub fn excluded(&self, path: &str) -> bool {
        self.exclusions.iter().any(|f| within(path, f))
    }
    pub fn validate(&mut self) -> Result<(), &'static str> {
        for values in [&mut self.folders, &mut self.exclusions] {
            let mut clean: Vec<String> = Vec::new();
            for value in values.iter() {
                let value = value.trim().trim_matches('"').replace('/', "\\");
                if value.is_empty() {
                    continue;
                }
                if !std::path::Path::new(&value).is_absolute() {
                    return Err("Use full folder paths, such as C:\\Work.");
                }
                if value.starts_with("\\\\") {
                    return Err("Use a mapped drive for network folders.");
                }
                if !clean.iter().any(|s| s.eq_ignore_ascii_case(&value)) {
                    clean.push(value);
                }
            }
            *values = clean;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exclusions_respect_folder_boundaries_and_case() {
        assert!(within(r"C:\Work\Private\a.xlsx", r"c:\work\private\"));
        assert!(!within(r"C:\Work\PrivateNotes\a.xlsx", r"C:\Work\Private"));
        assert!(within(r"C:\Work", r"C:\Work"));
    }
    #[test]
    fn shared_folder_is_added_once_and_stays_removed() {
        let mut s = Settings::default();
        assert!(!s.add_shared(&[]), "an unreachable folder is left for a later launch");
        assert!(s.add_shared(&[r"W:\Team\Invoices"]));
        assert_eq!(s.folders, [r"W:\Team\Invoices"]);
        s.folders.clear();
        assert!(!s.add_shared(&[r"w:\team\invoices"]));
        assert!(s.folders.is_empty());
    }
    #[test]
    fn invalid_preferences_keep_existing_state() {
        let mut s = Settings {
            folders: vec!["relative".into()],
            ..Default::default()
        };
        assert!(s.validate().is_err());
        assert_eq!(
            serde_json::from_str::<Settings>("{}").unwrap(),
            Settings::default()
        );
    }
}
