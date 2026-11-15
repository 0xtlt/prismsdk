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
| Android (planned) | app files | Block Store / Drive app data | on the device |

The database runs queries on a small Tokio runtime and hands results back to the UI thread. Migrations are SQL statements applied once each, in order (`PRAGMA user_version`). On the web the same sqlx code can point at Postgres instead of SQLite. `cx.capabilities().cloud_sync` says whether `cloud_storage` really syncs (iCloud signed in); elsewhere it is local storage.
