use crate::index::{Entry, Kind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Saved {
    pub name: String,
    pub path: String,
    pub kind: Kind,
}
impl Saved {
    pub fn entry(&self) -> Entry {
        Entry::new(self.name.clone(), self.path.clone(), self.kind, 0)
    }
    pub fn from_entry(e: &Entry) -> Self {
        Self {
            name: e.name.clone(),
            path: e.path.clone(),
            kind: e.kind,
        }
    }
    pub fn from_path(input: &str) -> Option<Self> {
        let p = input.trim().trim_matches('"');
        if !(std::path::Path::new(p).is_absolute()
            || p.starts_with("https://")
            || p.starts_with("http://"))
        {
            return None;
        }
        let name = std::path::Path::new(p)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.into());
        Some(Self {
            name,
            path: p.into(),
            kind: Kind::File,
        })
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    pub pins: Vec<Saved>,
    pub aliases: BTreeMap<String, Saved>,
    pub recent: Vec<Saved>,
}
impl Library {
    pub fn load() -> Self {
        let mut value: Self = crate::storage::load("library.json");
        value.pins.truncate(64);
        value.recent.truncate(30);
        value
    }
    pub fn save(&self) {
        crate::storage::save("library.json", self);
    }
    pub fn pinned(&self, path: &str) -> bool {
        self.pins.iter().any(|p| p.path.eq_ignore_ascii_case(path))
    }
    pub fn toggle_pin(&mut self, target: Saved) -> Result<bool, &'static str> {
        if let Some(i) = self
            .pins
            .iter()
            .position(|p| p.path.eq_ignore_ascii_case(&target.path))
        {
            self.pins.remove(i);
            self.save();
            return Ok(false);
        }
        if self.pins.len() >= 64 {
            return Err("Your 64 pins are full. Unpin an item first.");
        }
        self.pins.push(target);
        self.save();
        Ok(true)
    }
    pub fn remember(&mut self, target: Saved) {
        self.recent
            .retain(|p| !p.path.eq_ignore_ascii_case(&target.path));
        self.recent.insert(0, target);
        self.recent.truncate(30);
        self.save();
    }
    pub fn alias(&mut self, name: &str, target: Saved) -> Result<(), &'static str> {
        let key = name.trim().to_lowercase();
        if key.is_empty()
            || key.len() > 40
            || key
                .chars()
                .any(|c| !c.is_alphanumeric() && !matches!(c, '-' | '_'))
        {
            return Err("Use a short alias with letters, numbers, - or _.");
        }
        if self.aliases.len() >= 128 && !self.aliases.contains_key(&key) {
            return Err("Your 128 aliases are full. Remove one with /unalias.");
        }
        self.aliases.insert(key, target);
        self.save();
        Ok(())
    }
    pub fn pick(&self, query: &str) -> Option<&Saved> {
        self.aliases.get(&query.trim().to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn library_json_preserves_explicit_shortcuts_and_recent_order() {
        let a = Saved {
            name: "Invoices".into(),
            path: r"C:\Invoices".into(),
            kind: Kind::Folder,
        };
        let mut l = Library::default();
        l.pins.push(a.clone());
        l.aliases.insert("inv".into(), a.clone());
        l.recent.push(a);
        let restored: Library = serde_json::from_slice(&serde_json::to_vec(&l).unwrap()).unwrap();
        assert!(restored.pinned(r"c:\invoices"));
        assert_eq!(restored.pick("INV").unwrap().name, "Invoices");
        assert!(Saved::from_path("relative").is_none());
    }
}
