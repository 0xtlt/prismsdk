//! Persistent key-value storage: `cx.storage().set("todos", data)`, and
//! storage synced between the user's devices: `cx.cloud_storage()`.
//!
//! | | Local (`storage`) | Synced (`cloud_storage`) |
//! |---|---|---|
//! | iOS / macOS | App Group / Application Support | iCloud key-value store (`[ios] icloud = true`) |
//! | Web | server, per browser | server, per browser |
//! | Android (planned) | app files | Block Store / Drive app data |
//!
//! Synced storage is for small things (settings, progress, favorites: iCloud
//! allows 1 MB). For more, use a database (`prism-db`).
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
    let hex = |b: u8| (b as char).to_digit(16);
    while i < bytes.len() {
        // Only `%` and two hex digits (other file names in the directory
        // are listed as they are).
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) = (
                bytes.get(i + 1).copied().and_then(hex),
                bytes.get(i + 2).copied().and_then(hex),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

type CloudHandler = std::rc::Rc<dyn Fn(&[String], &mut crate::App)>;

/// Synced storage kept by the [`App`](crate::App).
pub(crate) struct Cloud {
    pub store: Box<dyn Storage>,
    pub handlers: Vec<CloudHandler>,
    /// Per-user data directory (web: per browser).
    pub data_dir: Option<PathBuf>,
    /// Shared by every user (web: the server's; native: same as `data_dir`).
    pub shared_data_dir: Option<PathBuf>,
}

impl Default for Cloud {
    fn default() -> Self {
        Cloud {
            store: Box::new(MemoryStorage::default()),
            handlers: Vec::new(),
            data_dir: None,
            shared_data_dir: None,
        }
    }
}

impl crate::App {
    /// Key-value storage synced between the user's devices (iCloud on Apple
    /// platforms). Falls back to local storage where there is no sync.
    pub fn cloud_storage(&mut self) -> &mut dyn Storage {
        &mut *self.cloud.store
    }

    pub fn set_cloud_storage(&mut self, store: Box<dyn Storage>) {
        self.cloud.store = store;
    }

    /// Runs `f` with the keys another device changed.
    pub fn on_cloud_change(&mut self, f: impl Fn(&[String], &mut crate::App) + 'static) {
        self.cloud.handlers.push(std::rc::Rc::new(f));
    }

    pub(crate) fn cloud_changed(&mut self, keys: &[String]) {
        for handler in self.cloud.handlers.clone() {
            handler(keys, self);
        }
    }

    /// Where the app may keep files for this user (databases, caches…).
    pub fn data_dir(&self) -> Option<&std::path::Path> {
        self.cloud.data_dir.as_deref()
    }

    /// Where data shared by every user lives: on the web, the server's data
    /// directory (a database there is your backend); natively, `data_dir`.
    pub fn shared_data_dir(&self) -> Option<&std::path::Path> {
        self.cloud
            .shared_data_dir
            .as_deref()
            .or(self.cloud.data_dir.as_deref())
    }

    pub(crate) fn set_data_dirs(&mut self, data: Option<PathBuf>, shared: Option<PathBuf>) {
        self.cloud.data_dir = data;
        self.cloud.shared_data_dir = shared;
    }
}
