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
    pub fn open(cx: &App, name: &str, migrations: &[&str]) -> Result<Db, sqlx::Error> {
        let dir = cx
            .shared_data_dir()
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&dir).map_err(sqlx::Error::Io)?;
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

/// Applies the migrations not applied yet (`PRAGMA user_version` counts them).
async fn migrate(pool: &SqlitePool, migrations: &[String]) -> Result<(), sqlx::Error> {
    let (done,): (i64,) = sqlx::query_as("PRAGMA user_version")
        .fetch_one(pool)
        .await?;
    for (i, sql) in migrations.iter().enumerate().skip(done.max(0) as usize) {
        let mut tx = pool.begin().await?;
        sqlx::raw_sql(sql).execute(&mut *tx).await?;
        sqlx::raw_sql(&format!("PRAGMA user_version = {}", i + 1))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
