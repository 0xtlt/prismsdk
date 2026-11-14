//! Persistent key-value storage: `cx.storage().set("todos", data)`.
//!
//! Backends choose where data lives: the App Group container on iOS (so
//! widgets can read it), Application Support on macOS, a per-browser
//! directory on the web server (cookie-identified). Values are strings;
//! serialize with any format you like.

use std::collections::HashMap;
use std::path::PathBuf;

pub trait Storage {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&mut self, key: &str, value: &str);
    fn remove(&mut self, key: &str);
    fn keys(&self) -> Vec<String>;
}

/// Volatile storage (tests, or when no location is available).
#[derive(Default)]
pub struct MemoryStorage(HashMap<String, String>);

impl Storage for MemoryStorage {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set(&mut self, key: &str, value: &str) {
        self.0.insert(key.into(), value.into());
    }
    fn remove(&mut self, key: &str) {
        self.0.remove(key);
    }
    fn keys(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }
}

/// One file per key in a directory; writes are atomic (temp file + rename).
pub struct FileStorage {
    dir: PathBuf,
}

impl FileStorage {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let _ = std::fs::create_dir_all(&dir);
        FileStorage { dir }
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    fn path(&self, key: &str) -> PathBuf {
        // Percent-encode anything that is not a safe file name character.
        let mut name = String::new();
        for b in key.bytes() {
            if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' {
                name.push(b as char);
            } else {
                name.push_str(&format!("%{b:02X}"));
            }
        }
        self.dir.join(name)
    }
}

impl Storage for FileStorage {
    fn get(&self, key: &str) -> Option<String> {
        std::fs::read_to_string(self.path(key)).ok()
    }

    fn set(&mut self, key: &str, value: &str) {
        let path = self.path(key);
        let tmp = path.with_extension("tmp~");
        if std::fs::write(&tmp, value)
            .and_then(|_| std::fs::rename(&tmp, &path))
            .is_err()
        {
            eprintln!("prism: could not persist {key:?} in {}", self.dir.display());
        }
    }

    fn remove(&mut self, key: &str) {
        let _ = std::fs::remove_file(self.path(key));
    }

    fn keys(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !n.ends_with(".tmp~"))
            .map(|n| percent_decode(&n))
            .collect()
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len() + 1
            && let Ok(b) = u8::from_str_radix(&s[i + 1..(i + 3).min(s.len())], 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
