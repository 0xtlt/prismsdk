//! The app's secret for widget-action links (S19). A macOS widget button
//! opens the app on a link, and any app or web page can open the same
//! link: the app runs the action only when the link carries the token its
//! own widget snapshot gave the button, and only once
//! ([`App::set_widget_key`]). The key lives in the [`DIR`] folder of the
//! app's data directory, readable by this user only, so links drawn in an
//! earlier run still work; the ledger of used links (`widget-link-ledger`)
//! sits next to it. A folder of their own keeps both out of the app's
//! storage (`cx.storage().keys()` lists the data directory's files).
//!
//! [`App::set_widget_key`]: prism_core::App::set_widget_key

use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// The folder, in the app's data directory, of the key and the ledger.
pub(crate) const DIR: &str = "widget-links";
const FILE: &str = "widget-link-key";

/// The key kept in `dir`, created on first use. A key that cannot be kept
/// still works for this run (widgets are redrawn with it).
pub(crate) fn load(dir: &Path) -> [u8; 32] {
    let path = dir.join(FILE);
    let mut key = [0u8; 32];
    if let Ok(mut f) = std::fs::File::open(&path)
        && f.read_exact(&mut key).is_ok()
        && f.read(&mut [0u8; 1]).is_ok_and(|n| n == 0)
    {
        return key;
    }
    getrandom::fill(&mut key).expect("prism: no system random source");
    // Not a key (cut short, or longer): replaced.
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::create_dir_all(dir);
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .and_then(|mut f| f.write_all(&key));
    if created.is_err() {
        // Another instance wrote it first: use theirs.
        if let Ok(mut f) = std::fs::File::open(&path)
            && f.read_exact(&mut key).is_ok()
        {
            return key;
        }
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_key_is_private_and_kept_between_runs() {
        let dir = std::env::temp_dir().join(format!("prism-widget-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let key = load(&dir);
        assert_ne!(key, [0; 32]);
        let mode = std::fs::metadata(dir.join(FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "readable by this user only");
        assert_eq!(load(&dir), key, "the next run signs the same links");
        // A file that is not a key is replaced.
        std::fs::write(dir.join(FILE), b"short").unwrap();
        let other = load(&dir);
        assert_ne!(other, key);
        assert_eq!(load(&dir), other);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
