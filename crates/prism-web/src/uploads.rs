//! Picked files sent over the socket in chunks (#112).
//!
//! Once the user has chosen, the page sends `upload_start` (`id` of the
//! pick call, `files: [{name, mime, size}]`), then the bytes as
//! `upload_chunk` messages (`id`, `file` index, `data` in base64, in order,
//! at most [`CHUNK`] bytes each), then the call's `device_reply` with
//! `uploaded: true`. Each chunk is written to the session's files directory
//! as it arrives and moves the app's progress ([`DeviceEvent::Upload`]);
//! the reply hands the app the files. A `device_reply` with an error (the
//! user or the app cancelled) deletes what arrived.
//!
//! Files dropped on a drop target (#111) travel the same way: `drop_start`
//! (a `drop` number the page picks, the target `node`, the files), the
//! chunks with that `drop` number, then `drop_end` (with `err` if it
//! failed).
//!
//! Only pick calls the app made and drops on the page's drop targets are
//! accepted (at most [`MAX_DROPS`] drops at once), each file gets no more
//! than the size the page declared, and files of an unfinished upload are
//! deleted when the session ends. Before anything is stored, the files are
//! checked against the app's limits (S16): how many one pick (or drop)
//! takes, the bytes one browser keeps on the server, the bytes all sessions
//! receive at once. Received files are deleted after
//! [`keep_uploads`](WebApp::keep_uploads).
//!
//! [`DeviceEvent::Upload`]: prism_core::DeviceEvent::Upload

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use serde_json::Value;

use crate::app::WebApp;

/// The most bytes in one `upload_chunk` (before base64), as `client.js`
/// sends them.
pub(crate) const CHUNK: usize = 256 * 1024;

/// The longest `data` of a chunk: [`CHUNK`] bytes in base64.
const CHUNK_BASE64: usize = CHUNK.div_ceil(3) * 4;

/// The most drops uploading at once in one session.
pub(crate) const MAX_DROPS: usize = 4;

/// The longest file name and MIME type a page may declare, in bytes.
const MAX_NAME: usize = 1024;
const MAX_MIME: usize = 255;

/// What an upload is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    /// A pick call (its device call id).
    Call(u64),
    /// Files dropped on a drop target (the page's drop number).
    Drop(u64),
}

/// The uploads of one session.
#[derive(Default)]
pub(crate) struct Uploads {
    calls: HashMap<Key, Call>,
}

enum Call {
    /// The app asked to pick files (at most this many, if it said); the
    /// user has not chosen yet.
    Waiting(Option<usize>),
    /// The files the page announced were refused (why): the call fails.
    Refused(Refusal),
    Receiving(Batch),
}

/// Why an upload was refused or failed: the kind a drop target's
/// `on_drop_error` gets (#290), and the details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub kind: prism_core::DropError,
    pub why: String,
}

impl Refusal {
    fn new(kind: prism_core::DropError, why: impl Into<String>) -> Self {
        Refusal {
            kind,
            why: why.into(),
        }
    }
}

struct Batch {
    files: Vec<Staged>,
    total: u64,
    sent: u64,
    /// A chunk was refused: the files are not what the user chose.
    failed: bool,
    /// Its bytes, counted in the server's uploads in progress until it ends.
    _reserved: Reservation,
}

/// A file on its way. It exists on disk from its first chunk on, and no
/// handle stays open between chunks.
struct Staged {
    name: String,
    mime: String,
    size: u64,
    stored: String,
    path: PathBuf,
    written: u64,
}

impl Batch {
    fn delete(&mut self) {
        for f in &self.files {
            let _ = std::fs::remove_file(&f.path);
        }
    }
}

impl Drop for Uploads {
    fn drop(&mut self) {
        for call in self.calls.values_mut() {
            if let Call::Receiving(batch) = call {
                batch.delete();
            }
        }
    }
}

/// Bytes of uploads in progress, counted server-wide
/// ([`max_uploading_bytes`](WebApp::max_uploading_bytes)) and given back
/// when dropped.
pub(crate) struct Reservation {
    pool: Arc<AtomicU64>,
    bytes: u64,
}

impl Reservation {
    /// `bytes` more of `pool`, unless that passes `max`.
    pub(crate) fn take(pool: &Arc<AtomicU64>, bytes: u64, max: u64) -> Option<Reservation> {
        let mut used = pool.load(Ordering::SeqCst);
        loop {
            let after = used.checked_add(bytes).filter(|n| *n <= max)?;
            match pool.compare_exchange_weak(used, after, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => {
                    return Some(Reservation {
                        pool: pool.clone(),
                        bytes,
                    });
                }
                Err(now) => used = now,
            }
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.pool.fetch_sub(self.bytes, Ordering::SeqCst);
    }
}

impl Uploads {
    /// The app asked to pick files (or images) with call `id`, at most
    /// `max` of them when it said (a single file, an image limit).
    pub(crate) fn expect(&mut self, id: u64, max: Option<usize>) {
        self.calls.insert(Key::Call(id), Call::Waiting(max));
    }

    /// A drop is about to upload: `false` when its number is taken or too
    /// many drops are uploading already.
    pub(crate) fn expect_drop(&mut self, drop: u64) -> bool {
        let drops = self
            .calls
            .keys()
            .filter(|k| matches!(k, Key::Drop(_)))
            .count();
        if drops >= MAX_DROPS || self.calls.contains_key(&Key::Drop(drop)) {
            return false;
        }
        self.calls.insert(Key::Drop(drop), Call::Waiting(None));
        true
    }

    /// The browser's data moved from `from` to `to` (S10): so did the
    /// files on their way.
    pub(crate) fn moved(&mut self, from: &Path, to: &Path) {
        for call in self.calls.values_mut() {
            if let Call::Receiving(batch) = call {
                for f in &mut batch.files {
                    if let Ok(rest) = f.path.strip_prefix(from) {
                        f.path = to.join(rest);
                    }
                }
            }
        }
    }

    /// Whether `key` is an upload still waiting for its files.
    pub(crate) fn has(&self, key: Key) -> bool {
        self.calls.contains_key(&key)
    }

    /// `upload_start`: the files the user chose. Returns `(sent, total)`,
    /// `None` for a call that picks nothing or already started, or files
    /// past the app's limits (the call then fails: nothing was stored).
    pub(crate) fn start(
        &mut self,
        app: &WebApp,
        sid: &str,
        id: Key,
        files: &[Value],
    ) -> Option<(u64, u64)> {
        let Some(Call::Waiting(max)) = self.calls.get(&id) else {
            return None;
        };
        match Batch::check(app, sid, *max, files) {
            Ok(batch) => {
                let progress = (0, batch.total);
                self.calls.insert(id, Call::Receiving(batch));
                Some(progress)
            }
            Err(why) => {
                self.calls.insert(id, Call::Refused(why));
                None
            }
        }
    }

    /// `upload_chunk`: bytes of file `index`. Returns `(sent, total)`, or
    /// `None` when the chunk is refused (the upload then fails: see
    /// [`finish`](Self::finish)).
    pub(crate) fn chunk(&mut self, id: Key, index: usize, data: &str) -> Option<(u64, u64)> {
        let Some(Call::Receiving(batch)) = self.calls.get_mut(&id) else {
            return None;
        };
        if batch.failed {
            return None;
        }
        let ok = (|| {
            // Measured before it is decoded.
            if data.len() > CHUNK_BASE64 {
                return None;
            }
            let bytes = crate::session::base64_decode(data)?;
            let f = batch.files.get_mut(index)?;
            let room = f.size.checked_sub(f.written)?;
            if bytes.is_empty() || bytes.len() > CHUNK || bytes.len() as u64 > room {
                return None;
            }
            // Created by its first chunk; a file deleted since (expired)
            // fails the upload rather than lose its start.
            let mut open = std::fs::OpenOptions::new();
            if f.written == 0 {
                open.write(true).create_new(true);
            } else {
                open.append(true);
            }
            open.open(&f.path)
                .and_then(|mut file| file.write_all(&bytes))
                .ok()?;
            f.written += bytes.len() as u64;
            Some(bytes.len() as u64)
        })();
        match ok {
            Some(n) => {
                batch.sent += n;
                Some((batch.sent, batch.total))
            }
            None => {
                // Too much, or unreadable.
                batch.delete();
                batch.failed = true;
                None
            }
        }
    }

    /// The call's `device_reply` arrived: the files, once every byte came
    /// (`Err` with a message otherwise); `None` for a call that is not an
    /// upload. An error reply (`failed`) deletes what arrived.
    pub(crate) fn finish(
        &mut self,
        app: &WebApp,
        id: Key,
        failed: bool,
    ) -> Option<Result<Vec<prism_core::PickedFile>, Refusal>> {
        match self.calls.remove(&id)? {
            // The page was told to stop (or failed anyway): the refusal says why.
            Call::Refused(why) => Some(Err(why)),
            Call::Waiting(_) if failed => None,
            Call::Waiting(_) => Some(Ok(Vec::new())),
            Call::Receiving(mut batch) => {
                let whole = !batch.failed
                    && batch.files.iter().all(|f| {
                        f.written == f.size
                            // An empty file never had a chunk.
                            && (f.size > 0 || std::fs::File::create(&f.path).is_ok())
                    });
                if failed || !whole {
                    batch.delete();
                    return if failed {
                        None
                    } else {
                        Some(Err(Refusal::new(
                            prism_core::DropError::Failed,
                            "the files did not arrive whole",
                        )))
                    };
                }
                let files = std::mem::take(&mut batch.files);
                Some(Ok(files
                    .into_iter()
                    .map(|f| {
                        let url = app.file_url(&f.stored);
                        // `PickedFile::read` finds the bytes behind the URL.
                        prism_core::device::register_local_file(&url, f.path);
                        prism_core::PickedFile {
                            path: url,
                            name: f.name,
                            mime: f.mime,
                            size: f.size,
                        }
                    })
                    .collect()))
            }
        }
    }
}

impl Batch {
    /// The files the page announced, checked against the app's limits
    /// before anything is stored: how many (`max` for this pick), their
    /// names, and their bytes against what the browser keeps already and
    /// what the server receives at once.
    fn check(
        app: &WebApp,
        sid: &str,
        max: Option<usize>,
        files: &[Value],
    ) -> Result<Batch, Refusal> {
        use prism_core::DropError::{Busy, Failed, Limits};
        let limits = app.upload_limits();
        let most = max.map_or(limits.files, |m| m.min(limits.files));
        if files.len() > most {
            return Err(Refusal::new(
                Limits,
                format!("too many files (at most {most})"),
            ));
        }
        let dir = app.files_dir(sid);
        let mut staged = Vec::with_capacity(files.len());
        let mut total = 0u64;
        for f in files {
            let text = |k: &str| f.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
            let name = text("name").unwrap_or("file");
            let mime = text("mime").unwrap_or("application/octet-stream");
            if name.len() > MAX_NAME || mime.len() > MAX_MIME {
                return Err(Refusal::new(Limits, "a file name or type is too long"));
            }
            let size = f.get("size").and_then(Value::as_u64).unwrap_or(0);
            total = total
                .checked_add(size)
                .ok_or_else(|| Refusal::new(Limits, "the files are too large"))?;
            let ext = Path::new(name)
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .filter(|e| stored_name(&format!("{}.{e}", "0".repeat(32))))
                .unwrap_or_else(|| "bin".into());
            let stored = format!("{}.{ext}", crate::app::new_session_id());
            staged.push(Staged {
                name: name.to_string(),
                mime: mime.to_string(),
                size,
                path: dir.join(&stored),
                stored,
                written: 0,
            });
        }
        // What this browser keeps on the server (expired files go first).
        let kept = sweep(app, &dir, SystemTime::now());
        if kept.saturating_add(total) > limits.bytes {
            return Err(Refusal::new(
                Limits,
                format!(
                    "the files are too large (at most {} bytes per browser)",
                    limits.bytes
                ),
            ));
        }
        let reserved =
            Reservation::take(app.uploading(), total, limits.uploading).ok_or_else(|| {
                Refusal::new(
                    Busy,
                    "the server is receiving too many files: try again later",
                )
            })?;
        std::fs::create_dir_all(&dir).map_err(|e| {
            Refusal::new(
                Failed,
                format!("the data directory {} is not writable: {e}", dir.display()),
            )
        })?;
        Ok(Batch {
            files: staged,
            total,
            sent: 0,
            failed: false,
            _reserved: reserved,
        })
    }
}

/// Deletes the uploaded files in `dir` last written
/// [`keep_uploads`](WebApp::keep_uploads) or longer before `now`, and
/// forgets their URLs. Returns the bytes of the files left.
pub(crate) fn sweep(app: &WebApp, dir: &Path, now: SystemTime) -> u64 {
    let keep = app.upload_limits().keep;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut left = 0u64;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|n| stored_name(n)) else {
            continue;
        };
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let age = meta
            .modified()
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .unwrap_or_default();
        if age >= keep {
            let _ = std::fs::remove_file(entry.path());
            prism_core::device::forget_local_file(&app.file_url(name));
        } else {
            left = left.saturating_add(meta.len());
        }
    }
    left
}

/// [`sweep`]s the files of every browser under the app's data directory
/// (those that never come back too).
pub(crate) fn sweep_all(app: &WebApp, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(&app.inner.data_root) else {
        return;
    };
    for entry in entries.flatten() {
        sweep(app, &entry.path().join("files"), now);
    }
}

/// Whether `name` is one this module stores files under: 32 lowercase hex
/// digits, a dot, an extension of 1 to 16 lowercase letters or digits. The
/// only names `/files/…` reads.
pub(crate) fn stored_name(name: &str) -> bool {
    name.split_once('.').is_some_and(|(id, ext)| {
        id.len() == 32
            && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            && (1..=16).contains(&ext.len())
            && ext
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::stored_name;

    /// S4 (security review): `/files/…` reads a stored name or nothing, on
    /// every host.
    #[test]
    fn s4_only_stored_names_are_read() {
        let id = crate::app::new_session_id();
        assert!(stored_name(&format!("{id}.png")));
        assert!(stored_name(&format!("{id}.bin")));
        for name in [
            "C:/Windows/win.ini".to_string(),
            format!("C:{id}.png"),
            format!("\\\\server\\share\\{id}.png"),
            format!("//server/share/{id}.png"),
            format!("\\\\?\\C:\\{id}.png"),
            format!("{id}.png:secret"),
            format!("{id}.png/../../x.png"),
            format!("../{id}.png"),
            format!("{id}.PNG"),
            format!("{id}."),
            format!("{id}.png."),
            format!("{id}.{}", "a".repeat(17)),
            "NUL.png".to_string(),
            format!(".{id}.png"),
            format!("{}.png", &id[1..]),
            format!("{}.png", id.to_ascii_uppercase()),
        ] {
            assert!(!stored_name(&name), "{name:?} must be refused");
        }
    }
}
