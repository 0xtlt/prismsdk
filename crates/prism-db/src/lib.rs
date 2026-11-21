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
use std::path::Path;
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
    /// as tests) the database is in memory, one per name for the whole
    /// process: nothing is written to disk (no file left in the system's
    /// temporary directory, none another user could plant there), and no
    /// run sees the data of an earlier one.
    pub fn open(cx: &App, name: &str, migrations: &[&str]) -> Result<Db, sqlx::Error> {
        if !is_file_name(name) {
            return Err(sqlx::Error::Configuration(
                format!(
                    "{name:?} is not a file name: Db::open takes one (Db::open_path takes a path)"
                )
                .into(),
            ));
        }
        let Some(dir) = cx.shared_data_dir() else {
            return Self::open_memory(name, migrations);
        };
        std::fs::create_dir_all(dir).map_err(sqlx::Error::Io)?;
        Self::open_path(&dir.join(name), migrations)
    }

    /// [`open`](Self::open) without a data directory: the in-memory
    /// database `name`, made once per process; opening it again runs only
    /// the new migrations.
    fn open_memory(name: &str, migrations: &[&str]) -> Result<Db, sqlx::Error> {
        static DBS: OnceLock<std::sync::Mutex<std::collections::HashMap<String, Db>>> =
            OnceLock::new();
        let mut dbs = DBS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(db) = dbs.get(name) {
            let migrations: Vec<String> = migrations.iter().map(|m| m.to_string()).collect();
            let pool = db.pool.clone();
            runtime().block_on(async move { migrate(&pool, &migrations).await })?;
            return Ok(db.clone());
        }
        let db = Self::memory(migrations)?;
        dbs.insert(name.to_string(), db.clone());
        Ok(db)
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

    /// An in-memory database (tests, scratch data). Its one connection is
    /// never closed for being idle or old: closing it would drop the data.
    pub fn memory(migrations: &[&str]) -> Result<Db, sqlx::Error> {
        let migrations: Vec<String> = migrations.iter().map(|m| m.to_string()).collect();
        runtime().block_on(async move {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .idle_timeout(None)
                .max_lifetime(None)
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
mod temp_dir;
#[cfg(test)]
mod tests {
    use super::*;
    use crate::temp_dir::TempDir;

    /// Closes `db`'s connections, so its files go with the test's folder
    /// (Windows does not remove a file that is open). Its pool stays in
    /// the process's list, unused.
    fn close(db: Db) {
        db.block_on(|pool| async move { pool.close().await });
    }

    /// S38: `Db::open` takes a file name in the app's data directory, never
    /// a path out of it (absolute, `..`, through a link): an app that
    /// derives the name from a tenant or an import cannot be steered to
    /// another database and run its migrations there. `open_path` still
    /// opens any path the app chooses.
    #[test]
    fn s38_open_stays_in_the_data_directory() {
        let base = TempDir::new("db-s38");
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
        close(Db::open(&rt.app, "app.db", &table).unwrap());
        assert!(data.join("app.db").exists());
        close(Db::open_path(&marker, &table).unwrap());
    }

    /// S38: without a data directory (a headless runtime), the database is
    /// in memory: nothing is written at a name of the shared temporary
    /// directory, where another user could have planted a link, and
    /// nothing is left there. Opened again, the name finds its data.
    #[test]
    fn s38_without_a_data_directory_the_database_is_in_memory() {
        let init: prism_core::AppInit = std::sync::Arc::new(|_: &mut App| {});
        let rt = prism_core::Runtime::new(&init);
        let table = ["CREATE TABLE t (n INTEGER)"];
        let db = Db::open(&rt.app, "s38-memory.db", &table).unwrap();
        let file: String = db
            .block_on(|pool| async move {
                sqlx::query_scalar("SELECT file FROM pragma_database_list WHERE name = 'main'")
                    .fetch_one(&pool)
                    .await
            })
            .unwrap();
        assert_eq!(file, "", "the database is a file");
        db.block_on(
            |pool| async move { sqlx::query("INSERT INTO t VALUES (7)").execute(&pool).await },
        )
        .unwrap();
        // Again (another window, a widget): the same data; only the new
        // migration runs.
        let column = "ALTER TABLE t ADD COLUMN m INTEGER NOT NULL DEFAULT 1";
        let again = Db::open(&rt.app, "s38-memory.db", &[table[0], column]).unwrap();
        let rows: Vec<(i64, i64)> = again
            .block_on(
                |pool| async move { sqlx::query_as("SELECT n, m FROM t").fetch_all(&pool).await },
            )
            .unwrap();
        assert_eq!(rows, [(7, 1)]);
        // Another name: another database.
        let other = Db::open(&rt.app, "s38-other.db", &[]).unwrap();
        let tables: i64 = other
            .block_on(|pool| async move {
                sqlx::query_scalar("SELECT count(*) FROM sqlite_master")
                    .fetch_one(&pool)
                    .await
            })
            .unwrap();
        assert_eq!(tables, 0);
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
        let dir = TempDir::new("db-test");
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
        close(db);
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
