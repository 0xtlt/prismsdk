//! Key-value storage: memory, files, the app's stores and cloud changes.

use prism_core::prelude::*;
use prism_core::{FileStorage, MemoryStorage, RuntimeOptions, Storage};

use prism_testing::{Harness, mount};

use crate::temp_dir::TempDir;

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[test]
fn memory_storage() {
    let mut s = MemoryStorage::default();
    assert_eq!(s.get("a"), None);
    s.set("a", "1");
    s.set("a", "2");
    s.set("b", "");
    assert_eq!(s.get("a").as_deref(), Some("2"));
    assert_eq!(s.get("b").as_deref(), Some(""));
    assert_eq!(sorted(s.keys()), ["a", "b"]);
    s.remove("a");
    s.remove("missing");
    assert_eq!(s.keys(), ["b"]);
}

#[test]
fn file_storage_persists_across_instances() {
    let dir = TempDir::new("persist");
    let mut s = FileStorage::new(dir.join("nested/dir"));
    s.set("todos", "[1,2]");
    s.set("todos", "[1,2,3]");
    s.set("ünï/cødé key", "✓");
    let s2 = FileStorage::new(dir.join("nested/dir"));
    assert_eq!(s2.get("todos").as_deref(), Some("[1,2,3]"));
    assert_eq!(s2.get("ünï/cødé key").as_deref(), Some("✓"));
    assert_eq!(sorted(s2.keys()), ["todos", "ünï/cødé key"]);
    // Keys never escape the directory.
    assert!(std::fs::read_dir(&*dir).unwrap().count() == 1);
}

#[test]
fn file_storage_removes_and_ignores_temporary_files() {
    let dir = TempDir::new("tmp");
    let mut s = FileStorage::new(&*dir);
    s.set("a", "1");
    std::fs::write(dir.join("b.tmp~"), "half-written").unwrap();
    assert_eq!(s.keys(), ["a"]);
    s.remove("a");
    s.remove("a");
    assert!(s.keys().is_empty());
    assert_eq!(s.get("a"), None);
}

#[test]
fn keys_that_differ_after_a_dot_have_their_own_temp_files() {
    // `a.b` and `a.c` used to share `a.tmp~`; the app and its widget
    // extension (#331) can also write one key at once.
    let dir = TempDir::new("dots");
    let mut s = FileStorage::new(&*dir);
    s.set("board.today", "1");
    s.set("board.tomorrow", "2");
    assert_eq!(s.get("board.today").as_deref(), Some("1"));
    assert_eq!(s.get("board.tomorrow").as_deref(), Some("2"));
    // No temp file is left behind.
    assert_eq!(std::fs::read_dir(&*dir).unwrap().count(), 2);
}

#[test]
fn file_storage_lists_stray_files_without_crashing() {
    let dir = TempDir::new("stray");
    let s = FileStorage::new(&*dir);
    // Files the app did not write: a lone `%`, bad escapes, non-ASCII after `%`.
    for name in ["50%", "%zz", "%aé", "%+1", "ok%41"] {
        std::fs::write(dir.join(name), "x").unwrap();
    }
    assert_eq!(sorted(s.keys()), ["%+1", "%aé", "%zz", "50%", "okA"]);
}

#[test]
fn a_missing_directory_has_no_keys() {
    let dir = TempDir::new("missing");
    let s = FileStorage::new(&*dir);
    std::fs::remove_dir_all(&*dir).unwrap();
    assert!(s.keys().is_empty());
    assert_eq!(s.get("x"), None);
}

struct Counter;

impl Render for Counter {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let n = cx.cloud_storage().get("n").unwrap_or_default();
        text(format!("n = {n}"))
    }
}

#[test]
fn the_app_reads_its_stores_and_hears_about_cloud_changes() {
    let mut h = Harness::with_runtime(
        |init| {
            let mut local = MemoryStorage::default();
            local.set("theme", "dark");
            prism_core::Runtime::with_options(
                init,
                RuntimeOptions {
                    storage: Some(Box::new(local)),
                    cloud_storage: Some(Box::new(MemoryStorage::default())),
                    ..Default::default()
                },
            )
        },
        |cx| {
            assert_eq!(cx.storage().get("theme").as_deref(), Some("dark"));
            let root = cx.new_entity(|_| Counter);
            mount(cx, root);
            cx.on_cloud_change(move |keys, app| {
                if keys.iter().any(|k| k == "n") {
                    root.notify(app);
                }
            });
        },
    );
    h.host.by_text("n = ");
    // Another device wrote "n": the backend stores it, then reports the key.
    h.rt.app.cloud_storage().set("n", "4");
    let p = h.rt.cloud_changed(&["n".into()]);
    h.apply(p);
    h.host.by_text("n = 4");
    // Unrelated keys re-render nothing.
    assert!(h.rt.cloud_changed(&["other".into()]).is_empty());
}

/// S10: a browser's uploads moved with its data: their URLs keep working.
#[test]
fn local_files_follow_a_moved_directory() {
    use prism_core::device::{local_file, move_local_files, register_local_file};
    use std::path::Path;
    let (from, to) = (Path::new("/data/s10-old"), Path::new("/data/s10-new"));
    register_local_file("/files/s10-a.png", from.join("files/s10-a.png"));
    register_local_file(
        "/files/s10-b.png",
        Path::new("/data/s10-other/files/s10-b.png").into(),
    );
    move_local_files(from, to);
    assert_eq!(
        local_file("/files/s10-a.png").as_deref(),
        Some(to.join("files/s10-a.png").as_path())
    );
    assert_eq!(
        local_file("/files/s10-b.png").as_deref(),
        Some(Path::new("/data/s10-other/files/s10-b.png"))
    );
}

#[test]
fn file_storage_keys_leave_out_folders() {
    // The data directory holds the storage's files and folders that are
    // not keys: cloud storage, Prism's own (`widget-links`), the app's.
    let dir = TempDir::new("folders");
    let mut s = FileStorage::new(&*dir);
    s.set("a", "1");
    std::fs::create_dir_all(dir.join("cloud")).unwrap();
    std::fs::create_dir_all(dir.join("widget-links")).unwrap();
    std::fs::write(dir.join("widget-links/widget-link-ledger"), "{}").unwrap();
    assert_eq!(s.keys(), ["a"]);
}
