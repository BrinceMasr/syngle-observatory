//! The data folder: `snapshots/<id>/`, `current` (the id answers come from) and `history` (the ids kept, oldest first).
//! `current` is switched last and by a rename, so a reader sees the old snapshot or the new one, never half of either.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Snapshots kept: the current one and the two before it, for `--data-version`.
pub const KEEP: usize = 3;

pub struct Store {
    pub root: PathBuf,
}

/// `gov-` and 8 hex digits: the only form that is ever joined to a path.
pub fn valid_id(id: &str) -> bool {
    id.len() == 12 && id.starts_with("gov-") && id[4..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Store {
    pub fn new(root: PathBuf) -> Store {
        Store { root }
    }

    pub fn snapshots(&self) -> PathBuf {
        self.root.join("snapshots")
    }

    pub fn snap_dir(&self, id: &str) -> PathBuf {
        self.snapshots().join(id)
    }

    pub fn has(&self, id: &str) -> bool {
        valid_id(id) && self.snap_dir(id).join("meta.json").is_file()
    }

    pub fn current_id(&self) -> Option<String> {
        let text = fs::read_to_string(self.root.join("current")).ok()?;
        let id = text.trim();
        valid_id(id).then(|| id.to_string())
    }

    /// The ids kept that still exist, oldest first.
    pub fn retained(&self) -> Vec<String> {
        let text = fs::read_to_string(self.root.join("history")).unwrap_or_default();
        text.lines().map(str::trim).filter(|l| self.has(l)).map(str::to_string).collect()
    }

    /// Makes `id` the current snapshot, records it, and removes the snapshots beyond the last `KEEP`.
    pub fn activate(&self, id: &str) -> io::Result<()> {
        let mut kept = self.retained();
        kept.retain(|k| k != id);
        kept.push(id.to_string());
        let drop_count = kept.len().saturating_sub(KEEP);
        kept.drain(..drop_count);
        write_atomic(&self.root.join("history"), kept.join("\n").as_bytes())?;
        write_atomic(&self.root.join("current"), id.as_bytes())?;
        if let Ok(entries) = fs::read_dir(self.snapshots()) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if (valid_id(&name) && !kept.contains(&name)) || (name.starts_with("gov-") && name.contains(".tmp")) {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
        Ok(())
    }
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_one_form_only() {
        assert!(valid_id("gov-1a2b3c4d"));
        assert!(!valid_id("gov-1A2B3C4D"));
        assert!(!valid_id("../etc/passwd"));
        assert!(!valid_id("gov-1a2b3c4"));
    }

    #[test]
    fn the_last_three_are_kept_and_current_moves() {
        let root = std::env::temp_dir().join(format!("obs-gov-store-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = Store::new(root.clone());
        for id in ["gov-00000001", "gov-00000002", "gov-00000003", "gov-00000004"] {
            fs::create_dir_all(store.snap_dir(id)).unwrap();
            fs::write(store.snap_dir(id).join("meta.json"), "{}").unwrap();
            store.activate(id).unwrap();
        }
        assert_eq!(store.current_id().as_deref(), Some("gov-00000004"));
        assert_eq!(store.retained(), ["gov-00000002", "gov-00000003", "gov-00000004"]);
        assert!(!store.snap_dir("gov-00000001").exists());
        fs::remove_dir_all(&root).unwrap();
    }
}
