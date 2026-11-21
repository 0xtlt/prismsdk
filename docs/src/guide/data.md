# Data: local, synced, SQL

```rust
cx.storage().set("draft", &text);              // this device (web: this browser, on the server)
cx.cloud_storage().set("theme", "dark");       // the user's devices: iCloud key-value store
cx.on_cloud_change(|keys, app| { … });         // another device changed something

// SQL with sqlx (`prism` feature `db`)
let db = prism::db::Db::open(cx, "app.db", &["CREATE TABLE notes (id INTEGER PRIMARY KEY, text TEXT)"])?;
cx.set_global(db);
cx.spawn(async move |this, cx| {
    let notes: Vec<(i64, String)> = Db::of(&cx)
        .run(&cx, |pool| async move { sqlx::query_as("SELECT id, text FROM notes").fetch_all(&pool).await })
        .await?;
});
```

| | `storage` | `cloud_storage` | `prism::db` (SQLite) |
|---|---|---|---|
| iOS | App Group container | iCloud KVS (`[ios] icloud = true`; signed builds) | on the device |
| macOS | Application Support | iCloud KVS (needs a Mac provisioning profile) | on the device |
| Web | server, per browser | server, per browser | **on the server, shared by every user**: the app runs there, so this is your backend |
| Android | app files | backed up by Android Backup (key/value), restored on a new device or a reinstall | on the device |

`prism::db` is sqlx 0.9 with a bundled SQLite (3.54 or later; prism-db does not build with an older one). sqlx 0.9 runs a query string only if it is a literal (`&'static str`) or wrapped in `sqlx::AssertSqlSafe(..)`, its reminder that a string built at run time must not carry user input: put values in `?` parameters with `.bind(..)`. Migrations are the app's own SQL and take any `&str`.

`Db::open(cx, name, …)` takes a file name in the app's data directory (the server's on the web): a name with a separator (`/`, `\`), a drive (`:`), `.` or `..` is an error, so a name built from user input cannot reach another database; `Db::open_path(path, …)` opens a path the app chooses. Without a data directory (a headless runtime, such as tests), the file goes in `prism-db-<uid>`, a directory only this user can enter, in the system's temporary directory. The database runs queries on a small Tokio runtime and hands results back to the UI thread. Migrations are SQL statements applied once each, in order (`PRAGMA user_version`). On the web the same sqlx code can point at Postgres instead of SQLite. `cx.capabilities().cloud_sync` says whether `cloud_storage` really syncs (iCloud signed in); `cloud_backup` that it is backed up without live sync (Android: `BackupAgent` + `BackupManager.dataChanged()` after each change; Block Store needs the Play services library, so it is not used); elsewhere it is local storage. Secrets go in [`secure_storage`](#security-secrets-biometrics-sign-in), which is never synced or backed up.
