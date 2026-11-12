//! Small plain-text settings under ~/.config/journal. Nothing secret lives here.

use std::path::PathBuf;

pub fn file(name: &str) -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    let dir = base.join("verso");
    crate::vault::adopt(&base.join("jot"), &dir, |old| {
        ["font-size", "veil-chars", "sync-host", "backup-dir"].iter().any(|f| old.join(f).is_file())
    });
    dir.join(name)
}

pub fn get(name: &str) -> Option<String> {
    let v = std::fs::read_to_string(file(name)).ok()?.trim().to_string();
    (!v.is_empty()).then_some(v)
}

pub fn set(name: &str, value: Option<&str>) {
    let path = file(name);
    match value {
        Some(v) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, v);
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}
