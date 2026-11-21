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
//! deleted when the session ends.
//!
//! [`DeviceEvent::Upload`]: prism_core::DeviceEvent::Upload

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use serde_json::Value;

use crate::app::WebApp;

/// The most bytes in one `upload_chunk` (before base64), as `client.js`
/// sends them.
pub(crate) const CHUNK: usize = 256 * 1024;

/// The most drops uploading at once in one session.
pub(crate) const MAX_DROPS: usize = 4;

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
    /// The app asked to pick files; the user has not chosen yet.
    Waiting,
    Receiving(Batch),
}

struct Batch {
    files: Vec<Staged>,
    total: u64,
    sent: u64,
    /// A chunk was refused: the files are not what the user chose.
    failed: bool,
}

struct Staged {
    name: String,
    mime: String,
    size: u64,
    stored: String,
    path: PathBuf,
    file: Option<std::fs::File>,
    written: u64,
}

impl Batch {
    fn delete(&mut self) {
        for f in &mut self.files {
            f.file = None;
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

impl Uploads {
    /// The app asked to pick files (or images) with call `id`.
    pub(crate) fn expect(&mut self, id: u64) {
        self.calls.insert(Key::Call(id), Call::Waiting);
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
        self.calls.insert(Key::Drop(drop), Call::Waiting);
        true
    }

    /// Whether `key` is an upload still waiting for its files.
    pub(crate) fn has(&self, key: Key) -> bool {
        self.calls.contains_key(&key)
    }

    /// `upload_start`: the files the user chose. Returns `(sent, total)`,
    /// `None` for a call that picks nothing or already started.
    pub(crate) fn start(
        &mut self,
        app: &WebApp,
        sid: &str,
        id: Key,
        files: &[Value],
    ) -> Option<(u64, u64)> {
        if !matches!(self.calls.get(&id), Some(Call::Waiting)) {
            return None;
        }
        let dir = app.files_dir(sid);
        std::fs::create_dir_all(&dir).ok()?;
        let mut batch = Batch {
            files: Vec::new(),
            total: 0,
            sent: 0,
            failed: false,
        };
        for f in files {
            let name = f.get("name").and_then(Value::as_str).unwrap_or("file");
            let mime = f
                .get("mime")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty())
                .unwrap_or("application/octet-stream");
            let size = f.get("size").and_then(Value::as_u64).unwrap_or(0);
            let ext = std::path::Path::new(name)
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .filter(|e| stored_name(&format!("{}.{e}", "0".repeat(32))))
                .unwrap_or_else(|| "bin".into());
            let stored = format!("{}.{ext}", crate::app::new_session_id());
            let path = dir.join(&stored);
            let Ok(file) = std::fs::File::create(&path) else {
                batch.delete();
                return None;
            };
            batch.total += size;
            batch.files.push(Staged {
                name: name.to_string(),
                mime: mime.to_string(),
                size,
                stored,
                path,
                file: Some(file),
                written: 0,
            });
        }
        let progress = (0, batch.total);
        self.calls.insert(id, Call::Receiving(batch));
        Some(progress)
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
            let bytes = crate::session::base64_decode(data)?;
            let f = batch.files.get_mut(index)?;
            let room = f.size.checked_sub(f.written)?;
            if bytes.len() > CHUNK || bytes.len() as u64 > room {
                return None;
            }
            f.file.as_mut()?.write_all(&bytes).ok()?;
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
    ) -> Option<Result<Vec<prism_core::PickedFile>, String>> {
        match self.calls.remove(&id)? {
            Call::Waiting if failed => None,
            Call::Waiting => Some(Ok(Vec::new())),
            Call::Receiving(mut batch) => {
                if failed || batch.failed || batch.files.iter().any(|f| f.written != f.size) {
                    batch.delete();
                    return if failed {
                        None
                    } else {
                        Some(Err("the files did not arrive whole".into()))
                    };
                }
                let files = std::mem::take(&mut batch.files);
                Some(Ok(files
                    .into_iter()
                    .map(|f| {
                        drop(f.file);
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
