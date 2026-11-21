//! Secrets a device keeps in a file (its Web Push receiver keys, a
//! UnifiedPush registration token): made once, by whichever process gets
//! there first, and private from creation.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The secret in the file `path`: read, or made with `make` and kept there.
///
/// - The first process to keep it wins: its bytes are written aside, then
///   linked in place, so concurrent first uses all end with the same
///   bytes. (On a file system without hard links the file is created in
///   place, exclusively.)
/// - Unix: the file is `0600` and the directories made for it `0700`,
///   from creation, whatever the umask.
/// - A file whose bytes `valid` refuses is an error, and is left alone; so
///   is any failure to read or keep the secret. A secret that is not kept
///   cannot be used later: a push subscription announced with keys that
///   are lost could never be decrypted.
pub fn load_or_create(
    path: &Path,
    valid: impl Fn(&[u8]) -> bool,
    make: impl FnOnce() -> Vec<u8>,
) -> io::Result<Vec<u8>> {
    if let Some(found) = read(path, &valid)? {
        return Ok(found);
    }
    let data = make();
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        private_dir(dir)?;
    }
    let aside = write_aside(path, &data)?;
    let linked = std::fs::hard_link(&aside, path);
    let _ = std::fs::remove_file(&aside);
    match linked {
        Ok(()) => Ok(data),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => kept_by_another(path, &valid),
        // No hard links here: made in place, exclusively.
        Err(_) => match create_private(path) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(&data).and_then(|()| file.sync_all()) {
                    let _ = std::fs::remove_file(path);
                    return Err(e);
                }
                Ok(data)
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => kept_by_another(path, &valid),
            Err(e) => Err(e),
        },
    }
}

/// The bytes another process kept at `path` first.
fn kept_by_another(path: &Path, valid: &impl Fn(&[u8]) -> bool) -> io::Result<Vec<u8>> {
    read(path, valid)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} vanished as it was made", path.display()),
        )
    })
}

/// The file's bytes; `None` if there is no file yet.
fn read(path: &Path, valid: &impl Fn(&[u8]) -> bool) -> io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) if valid(&bytes) => Ok(Some(bytes)),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} does not hold what it should (remove it to make a new one)",
                path.display()
            ),
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Makes `dir` and its missing parents, `0700` on Unix.
fn private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// A new file at `path` (an error if one is there), `0600` on Unix.
fn create_private(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// `data` in a new private file beside `path`, synced to disk.
fn write_aside(path: &Path, data: &[u8]) -> io::Result<PathBuf> {
    use rand_core::{OsRng, RngCore};
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let aside = path.with_file_name(format!(".{name}.{:016x}.tmp~", OsRng.next_u64()));
    let mut file = create_private(&aside)?;
    if let Err(e) = file.write_all(data).and_then(|()| file.sync_all()) {
        let _ = std::fs::remove_file(&aside);
        return Err(e);
    }
    Ok(aside)
}

#[cfg(test)]
mod tests {
    use crate::webpush::ReceiverKeys;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("prism-push-s31-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// S31: receiver keys are private from creation, whatever the umask,
    /// and read back the same.
    #[cfg(unix)]
    #[test]
    fn s31_receiver_keys_are_private_from_creation() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = scratch("modes");
        let path = dir.join("data/push/prism-push-keys");
        let keys = ReceiverKeys::load_or_create(&path).unwrap();
        assert_eq!(mode(&path), 0o600);
        for d in [dir.clone(), dir.join("data"), dir.join("data/push")] {
            assert_eq!(mode(&d), 0o700, "{}", d.display());
        }
        let left: Vec<_> = std::fs::read_dir(dir.join("data/push"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, ["prism-push-keys"], "a temporary file was left");
        let again = ReceiverKeys::load_or_create(&path).unwrap();
        assert_eq!(again.to_bytes(), keys.to_bytes());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// S31: processes (threads here) using push for the first time at once
    /// all end with the one key kept.
    #[test]
    fn s31_concurrent_first_uses_share_one_key() {
        let dir = scratch("race");
        for round in 0..20 {
            let path = dir.join(format!("{round}/prism-push-keys"));
            let start = std::sync::Arc::new(std::sync::Barrier::new(8));
            let threads: Vec<_> = (0..8)
                .map(|_| {
                    let (path, start) = (path.clone(), start.clone());
                    std::thread::spawn(move || {
                        start.wait();
                        ReceiverKeys::load_or_create(&path).unwrap().to_bytes()
                    })
                })
                .collect();
            let all: Vec<Vec<u8>> = threads.into_iter().map(|t| t.join().unwrap()).collect();
            assert!(
                all.iter().all(|k| *k == all[0]),
                "round {round}: different keys"
            );
            assert_eq!(std::fs::read(&path).unwrap(), all[0]);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// S31: keys that cannot be kept, or a file that holds no keys, are an
    /// error (and the file is left alone) instead of fresh keys that the
    /// next start would not find.
    #[test]
    fn s31_keys_that_cannot_be_kept_are_an_error() {
        let dir = scratch("fail");
        std::fs::create_dir_all(&dir).unwrap();
        // Its directory cannot be made (a file is in the way).
        std::fs::write(dir.join("file"), b"x").unwrap();
        assert!(ReceiverKeys::load_or_create(&dir.join("file/prism-push-keys")).is_err());
        // Not keys.
        let path = dir.join("prism-push-keys");
        std::fs::write(&path, b"not keys").unwrap();
        assert!(ReceiverKeys::load_or_create(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"not keys");
        // A directory the process cannot write to (unless it is root).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let locked = dir.join("locked");
            std::fs::create_dir(&locked).unwrap();
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
            if std::fs::write(locked.join("probe"), b"").is_err() {
                assert!(ReceiverKeys::load_or_create(&locked.join("prism-push-keys")).is_err());
            }
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
