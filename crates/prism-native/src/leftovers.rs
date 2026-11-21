//! Folders a process keeps in the system's temp directory while it runs,
//! named `<prefix><pid>` or `<prefix><pid>-<n>`: the font files Core Text
//! reads ([`fonts`](crate::fonts)), the copies a save panel saves
//! ([`save`](crate::save)). Each is removed when the process exits. A
//! process that is killed (how iOS often ends apps) cannot remove its
//! own, so the next process that makes one removes those of processes
//! that are gone.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once};

/// This process's folders, removed at exit.
static AT_EXIT: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// `folder`, this process's, is removed (with what it holds) when the
/// process exits. Already gone by then is fine.
pub(crate) fn remove_at_exit(folder: PathBuf) {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(register);
    AT_EXIT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(folder);
}

/// Runs `remove_all` when the process exits (`exit`, `main` returning,
/// `NSApplication` terminating).
#[cfg(unix)]
fn register() {
    use std::sync::TryLockError;
    unsafe extern "C" {
        fn atexit(f: extern "C" fn()) -> i32;
    }
    extern "C" fn remove_all() {
        // Never panic or wait at exit: a lock another thread holds is left.
        let folders = match AT_EXIT.try_lock() {
            Ok(f) => f,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        for folder in folders.iter() {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
    // SAFETY: `remove_all` is a plain function, there for the whole process.
    unsafe {
        atexit(remove_all);
    }
}

/// Elsewhere (Windows), nothing of prism-native's is kept in the temp
/// directory.
#[cfg(not(unix))]
fn register() {}

/// Removes the folders under `dir` named `<prefix><pid>…` whose process
/// is gone, not this process's nor a running one's.
pub(crate) fn sweep(dir: &Path, prefix: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| owner(n, prefix)) else {
            continue;
        };
        // `file_type` does not follow a link: a link is never entered.
        if pid != std::process::id() && !alive(pid) && entry.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The process id in `name`: `<prefix><pid>` or `<prefix><pid>-…`.
fn owner(name: &str, prefix: &str) -> Option<u32> {
    name.strip_prefix(prefix)?.split('-').next()?.parse().ok()
}

/// Whether process `pid` runs (or may: only "no such process" says not).
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    /// "No such process" (the same on Linux, Android and Apple platforms).
    const ESRCH: i32 = 3;
    let Ok(pid) = i32::try_from(pid) else {
        return true;
    };
    // SAFETY: signal 0 sends nothing: it only checks that `pid` exists.
    pid <= 0
        || unsafe { kill(pid, 0) } == 0
        || std::io::Error::last_os_error().raw_os_error() != Some(ESRCH)
}

#[cfg(not(unix))]
fn alive(_pid: u32) -> bool {
    true
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::temp_dir::TempDir;

    #[test]
    fn folders_of_ended_processes_are_swept_the_others_kept() {
        let dir = TempDir::new("leftovers");
        std::fs::create_dir_all(&dir).unwrap();
        let ended = {
            let mut child = std::process::Command::new("true").spawn().unwrap();
            let pid = child.id();
            child.wait().unwrap();
            pid
        };
        let mut running = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let me = std::process::id();
        let made = |name: String| {
            let path = dir.join(name);
            std::fs::create_dir_all(path.join("inside")).unwrap();
            path
        };
        let gone = [
            made(format!("prism-x-{ended}")),
            made(format!("prism-x-{ended}-3")),
        ];
        let kept = [
            made(format!("prism-x-{me}-0")),
            made(format!("prism-x-{}", running.id())),
            made("prism-x-cache".into()),
            made(format!("prism-y-{ended}")),
        ];
        // A link named like an ended process's folder: left, never entered.
        let target = made("target".into());
        let link = dir.join(format!("prism-x-{ended}-link"));
        std::os::unix::fs::symlink(&target, &link).unwrap();
        sweep(&dir, "prism-x-");
        running.kill().unwrap();
        running.wait().unwrap();
        for p in &gone {
            assert!(!p.exists(), "{} kept", p.display());
        }
        for p in &kept {
            assert!(p.exists(), "{} removed", p.display());
        }
        assert!(link.symlink_metadata().is_ok(), "the link was removed");
        assert!(target.join("inside").exists(), "the link was followed");
    }

    #[test]
    fn owners_come_from_the_name() {
        assert_eq!(owner("prism-fonts-42", "prism-fonts-"), Some(42));
        assert_eq!(owner("prism-save-42-7", "prism-save-"), Some(42));
        assert_eq!(owner("prism-fonts-test-42-0", "prism-fonts-"), None);
        assert_eq!(owner("prism-fonts", "prism-fonts-"), None);
        assert_eq!(owner("other-42", "prism-fonts-"), None);
    }
}
