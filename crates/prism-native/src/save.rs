//! Files the app hands to the user (`cx.save_file`, #210). Platform UIs
//! save or export a file on disk, so bytes in memory, or a file that must
//! carry another name, are first written to a staging folder
//! (`<dir>/prism-save-<pid>-<n>/<name>`). The platform removes that folder
//! once it is done ([`discard`] or its own code: any parent folder whose
//! name starts with [`STAGING_PREFIX`]). One still there when the process
//! exits (a save panel left open) is removed then, and one of a process
//! that was killed by the next staging.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use prism_core::{SaveFile, SaveSource};

/// The name of staging folders starts with this.
pub const STAGING_PREFIX: &str = "prism-save-";

/// A file on disk with `file`'s name and content, staged under `dir` when
/// it isn't one already (a [`SaveSource::Path`] with that name).
pub fn stage(file: &SaveFile, dir: &Path) -> std::io::Result<PathBuf> {
    let name = file.file_name();
    if let SaveSource::Path(p) = &file.source
        && p.file_name().is_some_and(|n| n.to_string_lossy() == name)
    {
        if !p.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} is not a file", p.display()),
            ));
        }
        return Ok(p.clone());
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let folder = dir.join(format!(
        "{STAGING_PREFIX}{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    crate::leftovers::sweep(dir, STAGING_PREFIX);
    std::fs::create_dir_all(&folder)?;
    crate::leftovers::remove_at_exit(folder.clone());
    let path = folder.join(name);
    if let Err(e) = file.write_to(&path) {
        let _ = std::fs::remove_dir_all(&folder);
        return Err(e);
    }
    Ok(path)
}

/// Whether `path` was staged by [`stage`].
pub fn is_staged(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n.to_string_lossy().starts_with(STAGING_PREFIX))
}

/// Removes `path`'s staging folder, if it was staged.
pub fn discard(path: &Path) {
    if is_staged(path)
        && let Some(folder) = path.parent()
    {
        let _ = std::fs::remove_dir_all(folder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_staged_under_their_name_unless_they_have_it() {
        let dir = crate::temp_dir::TempDir::new("stage-test");
        std::fs::create_dir_all(&dir).unwrap();

        let staged = stage(&SaveFile::bytes(b"%PDF".to_vec(), "a/b.pdf"), &dir).unwrap();
        assert_eq!(staged.file_name().unwrap(), "a_b.pdf");
        assert!(is_staged(&staged));
        assert_eq!(std::fs::read(&staged).unwrap(), b"%PDF");
        discard(&staged);
        assert!(!staged.parent().unwrap().exists());

        let own = dir.join("report.pdf");
        std::fs::write(&own, b"pdf").unwrap();
        assert_eq!(stage(&SaveFile::path(&own), &dir).unwrap(), own);
        discard(&own);
        assert!(own.exists(), "the app's own file is never removed");

        let renamed = stage(&SaveFile::path(&own).name("Q3.pdf"), &dir).unwrap();
        assert_eq!(renamed.file_name().unwrap(), "Q3.pdf");
        assert_eq!(std::fs::read(&renamed).unwrap(), b"pdf");
        discard(&renamed);

        assert!(stage(&SaveFile::path(dir.join("missing.pdf")), &dir).is_err());
    }

    #[test]
    fn a_failing_test_leaves_no_temp_folder() {
        let made = std::sync::Mutex::new(None);
        let failed = std::panic::catch_unwind(|| {
            let dir = crate::temp_dir::TempDir::new("guard");
            std::fs::create_dir_all(dir.join("data")).unwrap();
            std::fs::write(dir.join("data/file"), "x").unwrap();
            *made.lock().unwrap() = Some(dir.to_path_buf());
            panic!("a test that fails (on purpose)");
        });
        assert!(failed.is_err());
        let dir = made.lock().unwrap().take().expect("the folder was made");
        assert!(!dir.exists(), "{} is left behind", dir.display());
    }
}
