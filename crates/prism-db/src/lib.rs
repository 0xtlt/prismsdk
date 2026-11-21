//! # prism-db
//!
//! SQL with [sqlx], wherever the app runs:
//!
//! | | Where the database lives |
//! |---|---|
//! | iOS | the app's container (App Group: widgets can read it too) |
//! | macOS | Application Support |
//! | Android | the app's files |
//! | Web | **the server**: the app runs there, so this is your backend, shared by every user |
//!
//! ```ignore
//! let db = Db::open(cx, "app.db", &[
//!     "CREATE TABLE notes (id INTEGER PRIMARY KEY, text TEXT NOT NULL)",
//!     "ALTER TABLE notes ADD COLUMN done INTEGER NOT NULL DEFAULT 0",
//! ])?;
//! cx.set_global(db);
//!
//! cx.spawn(async move |this, cx| {
//!     let notes: Vec<(i64, String)> = Db::of(&cx)
//!         .run(&cx, |pool| async move {
//!             sqlx::query_as("SELECT id, text FROM notes").fetch_all(&pool).await
//!         })
//!         .await?;
//! });
//! ```
//!
//! Queries run on a small Tokio runtime in the background; results come back
//! to the UI thread. Migrations are SQL statements applied once each, in
//! order (`PRAGMA user_version` counts them): only ever append to the list.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use prism_core::{App, AsyncApp, Background};

pub use sqlx;
pub use sqlx::SqlitePool;

/// The bundled SQLite is 3.54.0 or later: earlier releases can return stale
/// process memory when they open a database with a crafted `-wal` file
/// (CVE-2026-119375, RUSTSEC-2026-0459). Building against one fails here.
const _: () = assert!(
    libsqlite3_sys::SQLITE_VERSION_NUMBER >= 3_054_000,
    "prism-db needs SQLite 3.54.0 or later (CVE-2026-119375)"
);

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("prism-db")
            .enable_all()
            .build()
            .expect("prism-db: Tokio runtime")
    })
}

/// A database connection pool (cheap to clone).
#[derive(Clone, Debug)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Opens (creating it if needed) the SQLite file `name` in the app's
    /// shared data directory, then applies `migrations`.
    ///
    /// `name` is a file name (`"app.db"`), not a path: one with a separator
    /// (`/`, `\`), a drive or stream (`:`), a control character, or `.` or
    /// `..` is an error, so a name built from a tenant or an import cannot
    /// reach another database. [`open_path`](Self::open_path) opens a path
    /// the app chooses. Without a data directory (a headless runtime, such
    /// as tests) the file goes in a directory only this user can enter, in
    /// the system's temporary directory.
    pub fn open(cx: &App, name: &str, migrations: &[&str]) -> Result<Db, sqlx::Error> {
        if !is_file_name(name) {
            return Err(sqlx::Error::Configuration(
                format!(
                    "{name:?} is not a file name: Db::open takes one (Db::open_path takes a path)"
                )
                .into(),
            ));
        }
        let dir = match cx.shared_data_dir() {
            Some(dir) => {
                std::fs::create_dir_all(dir).map_err(sqlx::Error::Io)?;
                dir.to_path_buf()
            }
            None => private_dir(&std::env::temp_dir()).map_err(sqlx::Error::Io)?,
        };
        Self::open_path(&dir.join(name), migrations)
    }

    /// Opens a SQLite file at `path`. One pool per file is shared by the
    /// whole process (every web session, every window): opening it again is
    /// free and runs the migrations only once.
    pub fn open_path(path: &Path, migrations: &[&str]) -> Result<Db, sqlx::Error> {
        static POOLS: OnceLock<
            std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, Db>>,
        > = OnceLock::new();
        let pools = POOLS.get_or_init(Default::default);
        let key = path
            .parent()
            .and_then(|dir| dir.canonicalize().ok())
            .map_or_else(
                || path.to_path_buf(),
                |dir| dir.join(path.file_name().unwrap_or_default()),
            );
        let mut pools = pools.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(db) = pools.get(&key) {
            // New migrations (an app update while the server runs) still apply.
            let migrations: Vec<String> = migrations.iter().map(|m| m.to_string()).collect();
            let pool = db.pool.clone();
            runtime().block_on(async move { migrate(&pool, &migrations).await })?;
            return Ok(db.clone());
        }
        let db = Self::connect_path(&key, migrations)?;
        pools.insert(key, db.clone());
        Ok(db)
    }

    fn connect_path(path: &Path, migrations: &[&str]) -> Result<Db, sqlx::Error> {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            // Readers do not block the writer (and widgets can read).
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let migrations: Vec<String> = migrations.iter().map(|m| m.to_string()).collect();
        // Opening a local file is quick: do it now, so the app starts with it.
        runtime().block_on(async move {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(4)
                .connect_with(options)
                .await?;
            migrate(&pool, &migrations).await?;
            Ok(Db { pool })
        })
    }

    /// An in-memory database (tests, scratch data).
    pub fn memory(migrations: &[&str]) -> Result<Db, sqlx::Error> {
        let migrations: Vec<String> = migrations.iter().map(|m| m.to_string()).collect();
        runtime().block_on(async move {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await?;
            migrate(&pool, &migrations).await?;
            Ok(Db { pool })
        })
    }

    /// The database stored with `cx.set_global(db)`.
    pub fn of(cx: &AsyncApp) -> Db {
        cx.update(|app| app.global::<Db>().clone())
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Runs `query` in the background; resolves on the UI thread.
    pub fn run<R, F, Fut>(&self, cx: &AsyncApp, query: F) -> Background<R>
    where
        R: Send + 'static,
        F: FnOnce(SqlitePool) -> Fut + Send + 'static,
        Fut: Future<Output = R> + Send,
    {
        let pool = self.pool.clone();
        cx.background(move || runtime().block_on(query(pool)))
    }

    /// Runs `query` and waits for it (outside the UI thread, or in tests).
    pub fn block_on<R, Fut: Future<Output = R>>(&self, query: impl FnOnce(SqlitePool) -> Fut) -> R {
        runtime().block_on(query(self.pool.clone()))
    }
}

/// One plain file name on every platform: not empty, `.` or `..`, without a
/// separator (`/`, `\`), a drive or stream (`:`) or a control character.
fn is_file_name(name: &str) -> bool {
    !matches!(name, "" | "." | "..")
        && !name.contains(|c: char| matches!(c, '/' | '\\' | ':') || c.is_control())
}

/// `base/prism-db-<uid>`, a directory only this user can enter: made with
/// mode `0700`, or one already there that is a directory (not a link) this
/// user owns, its mode set back to `0700`. A shared temporary directory
/// (`/tmp`) lets other users plant a database or a link at a name; this
/// one they cannot. Elsewhere (Windows, where the temporary directory is
/// the user's own) `base/prism-db`.
fn private_dir(base: &Path) -> std::io::Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        // SAFETY: geteuid has no preconditions and cannot fail.
        let uid = unsafe { libc::geteuid() };
        let dir = base.join(format!("prism-db-{uid}"));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let meta = std::fs::symlink_metadata(&dir)?;
        if !meta.is_dir() || meta.uid() != uid {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("{}: not a directory of this user", dir.display()),
            ));
        }
        if meta.mode() & 0o077 != 0 {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(dir)
    }
    #[cfg(not(unix))]
    {
        let dir = base.join("prism-db");
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }
}

/// Applies the migrations not applied yet (`PRAGMA user_version` counts them).
/// They are the app's own SQL, run as written (`AssertSqlSafe`): never build
/// one from user input.
async fn migrate(pool: &SqlitePool, migrations: &[String]) -> Result<(), sqlx::Error> {
    use sqlx::AssertSqlSafe;
    let (done,): (i64,) = sqlx::query_as("PRAGMA user_version")
        .fetch_one(pool)
        .await?;
    for (i, sql) in migrations.iter().enumerate().skip(done.max(0) as usize) {
        let mut tx = pool.begin().await?;
        sqlx::raw_sql(AssertSqlSafe(sql.as_str()))
            .execute(&mut *tx)
            .await?;
        // A number we count, not input.
        sqlx::raw_sql(AssertSqlSafe(format!("PRAGMA user_version = {}", i + 1)))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S38: `Db::open` takes a file name in the app's data directory, never
    /// a path out of it (absolute, `..`, through a link): an app that
    /// derives the name from a tenant or an import cannot be steered to
    /// another database and run its migrations there. `open_path` still
    /// opens any path the app chooses.
    #[test]
    fn s38_open_stays_in_the_data_directory() {
        let base = std::env::temp_dir().join(format!("prism-db-s38-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (data, outside) = (base.join("data"), base.join("outside"));
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        // Another database: an empty file is one SQLite would set up.
        let marker = outside.join("other.db");
        std::fs::write(&marker, b"").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, data.join("link")).unwrap();
        let init: prism_core::AppInit = std::sync::Arc::new(|_: &mut App| {});
        let rt = prism_core::Runtime::with_options(
            &init,
            prism_core::RuntimeOptions {
                data_dir: Some(data.clone()),
                ..Default::default()
            },
        );
        let table = ["CREATE TABLE t (n INTEGER)"];
        let absolute = marker.to_string_lossy().into_owned();
        for name in [
            absolute.as_str(),
            "../outside/other.db",
            "link/other.db",
            "link\\other.db",
            "..",
            ".",
            "",
            "C:other.db",
            "other.db\0",
        ] {
            assert!(Db::open(&rt.app, name, &table).is_err(), "{name:?}");
        }
        assert_eq!(
            std::fs::metadata(&marker).unwrap().len(),
            0,
            "the other database changed"
        );
        // A name opens in the data directory; an explicit path anywhere.
        Db::open(&rt.app, "app.db", &table).unwrap();
        assert!(data.join("app.db").exists());
        Db::open_path(&marker, &table).unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    /// S38: without a data directory, databases go in a directory of this
    /// user only, never at a name of the shared temporary directory where
    /// another user could have planted a link.
    #[cfg(unix)]
    #[test]
    fn s38_the_fallback_directory_is_this_users_only() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("prism-db-s38-tmp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = private_dir(&base).unwrap();
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(private_dir(&base).unwrap(), dir);
        // Opened up by mistake: closed again.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(private_dir(&base).unwrap(), dir);
        assert_eq!(mode(&dir), 0o700);
        // A link planted at the name: refused.
        std::fs::remove_dir(&dir).unwrap();
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &dir).unwrap();
        assert!(private_dir(&base).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// S26: the SQLite that runs queries is 3.54.0 or later, so a database
    /// opened with a crafted `-wal` file cannot read stale process memory
    /// (CVE-2026-119375, RUSTSEC-2026-0459). Asked of the engine at run
    /// time, not of the headers it was built with.
    #[test]
    fn s26_sqlite_is_3_54_or_later() {
        let db = Db::memory(&[]).unwrap();
        let version: String = db
            .block_on(|pool| async move {
                sqlx::query_scalar("SELECT sqlite_version()")
                    .fetch_one(&pool)
                    .await
            })
            .unwrap();
        let parts: Vec<u32> = version.split('.').map(|n| n.parse().unwrap()).collect();
        assert!(
            parts[..2] >= [3, 54][..],
            "SQLite {version} has CVE-2026-119375"
        );
    }

    #[test]
    fn migrations_run_once_in_order() {
        let dir = std::env::temp_dir().join(format!("prism-db-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let v1 = ["CREATE TABLE notes (id INTEGER PRIMARY KEY, text TEXT NOT NULL)"];
        let db = Db::open_path(&path, &v1).unwrap();
        db.block_on(|pool| async move {
            sqlx::query("INSERT INTO notes (text) VALUES (?)")
                .bind("hello")
                .execute(&pool)
                .await
        })
        .unwrap();
        drop(db);
        // Reopen with one more migration: only the new one runs.
        let v2 = [
            v1[0],
            "ALTER TABLE notes ADD COLUMN done INTEGER NOT NULL DEFAULT 0",
        ];
        let db = Db::open_path(&path, &v2).unwrap();
        let rows: Vec<(String, i64)> = db
            .block_on(|pool| async move {
                sqlx::query_as("SELECT text, done FROM notes")
                    .fetch_all(&pool)
                    .await
            })
            .unwrap();
        assert_eq!(rows, [("hello".to_string(), 0)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn queries_resolve_on_the_ui_thread() {
        use std::sync::{Arc, Mutex};
        let db = Db::memory(&["CREATE TABLE t (n INTEGER)"]).unwrap();
        let got: Arc<Mutex<Option<i64>>> = Default::default();
        let out = got.clone();
        let init: prism_core::AppInit = Arc::new(move |cx: &mut App| {
            cx.set_global(db.clone());
            let out = out.clone();
            cx.spawn(move |cx| async move {
                let db = Db::of(&cx);
                let n: i64 = db
                    .run(&cx, |pool| async move {
                        sqlx::query("INSERT INTO t VALUES (41), (1)")
                            .execute(&pool)
                            .await?;
                        sqlx::query_scalar("SELECT SUM(n) FROM t")
                            .fetch_one(&pool)
                            .await
                    })
                    .await
                    .unwrap();
                *out.lock().unwrap() = Some(n);
            });
        });
        let mut rt = prism_core::Runtime::new(&init);
        for _ in 0..200 {
            rt.poll();
            if got.lock().unwrap().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(*got.lock().unwrap(), Some(42));
    }
}
