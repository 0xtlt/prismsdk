//! Web costs on a long list, through the public API (`docs/src/performance.md`):
//!
//! ```sh
//! cargo test -p prismsdk-web --release --test bench -- --ignored --nocapture
//! ```
//!
//! For 100, 1,000 and 5,000 rows: the server-rendered page (time, size,
//! gzipped size), the live session's first tree (time, JSON size, deflated
//! size), then a click that changes one row and a click that changes every
//! row (patch size, and the time from the message to the patch leaving).
//! `PRISM_BENCH_ROWS=5000` runs one size.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use http::{HeaderMap, Request};
use prism_core::prelude::*;
use prism_core::{AppConfig, AppInit, Backend, Runtime};
use prism_web::{Encoder, Frame, HtmlBackend, WebApp};
use serde_json::Value;

struct Row {
    n: usize,
    hits: u32,
}

impl Render for Row {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_row()
            .gap(8.)
            .p(8.)
            .child(text(format!("Row {}", self.n)))
            .child(text(format!("{} hits", self.hits)))
    }
}

struct List {
    rows: Vec<Entity<Row>>,
}

fn hit(row: &Entity<Row>, cx: &mut App) {
    row.update(cx, |r, cx| {
        r.hits += 1;
        cx.notify();
    });
}

impl Render for List {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        scroll()
            .p(16.)
            .gap(4.)
            .child(button("One").on_click(
                cx.listener(|this: &mut List, _, cx| hit(&this.rows[this.rows.len() / 2], cx)),
            ))
            .child(
                button("All").on_click(cx.listener(|this: &mut List, _, cx| {
                    for row in &this.rows {
                        hit(row, cx);
                    }
                })),
            )
            .children(self.rows.iter().copied())
    }
}

fn init(n: usize) -> AppInit {
    Arc::new(move |cx: &mut App| {
        let list = cx.new_entity(|cx: &mut Context<List>| List {
            rows: (0..n).map(|n| cx.new(|_| Row { n, hits: 0 })).collect(),
        });
        cx.set_root(list);
    })
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn kb(bytes: usize) -> String {
    format!("{:.1} KB", bytes as f64 / 1024.)
}

/// The next message whose type is `t`, and when it arrived.
fn next(rx: &mpsc::Receiver<String>, t: &str) -> (String, Instant) {
    loop {
        let m = rx.recv_timeout(Duration::from_secs(60)).expect("a message");
        let at = Instant::now();
        if serde_json::from_str::<Value>(&m).unwrap()["t"] == t {
            return (m, at);
        }
    }
}

fn run(n: usize) {
    let dir = std::env::temp_dir().join("prism-web-bench");
    let app = WebApp::new(AppConfig::default(), init(n)).data_dir(dir);
    // The page.
    let page = |gzip: bool| {
        let mut req = Request::get("/");
        if gzip {
            req = req.header("accept-encoding", "gzip");
        }
        app.handle(&req.body(()).unwrap())
    };
    let times: Vec<Duration> = (0..9)
        .map(|_| {
            let t = Instant::now();
            page(false);
            t.elapsed()
        })
        .collect();
    let html = page(false).into_body().len();
    let gz = page(true).into_body().len();
    println!(
        "{n:>5} rows | page {:?}, {} ({} gzip)",
        median(times),
        kb(html),
        kb(gz)
    );

    // The live session's first tree.
    let uri = "/ws?path=%2F&tz=0&w=1280&h=800".parse().unwrap();
    let mut times = Vec::new();
    let mut reset = String::new();
    for _ in 0..5 {
        let (tx, rx) = mpsc::channel();
        let t = Instant::now();
        let session = app.connect(&uri, &HeaderMap::new(), move |m| tx.send(m).is_ok());
        let (m, at) = next(&rx, "reset");
        times.push(at - t);
        reset = m;
        session.close();
    }
    let t = Instant::now();
    let Frame::Binary(z) = Frame::encode(reset.clone(), true) else {
        panic!("a first tree is compressed");
    };
    let deflate = t.elapsed();
    println!(
        "      first tree {:?}, {} JSON ({} deflated in {deflate:?})",
        median(times),
        kb(reset.len()),
        kb(z.len()),
    );

    // Where a first tree's time goes: the core's render, then this crate's
    // HTML and JSON.
    let mut runtime = Runtime::new(&init(n));
    let t = Instant::now();
    let patches = runtime.render();
    let render = t.elapsed();
    let t = Instant::now();
    let mut html = HtmlBackend::default();
    html.apply(&patches);
    let _ = html.to_page();
    let to_html = t.elapsed();
    let t = Instant::now();
    let _ = serde_json::to_string(&Encoder::default().encode(&patches));
    let to_json = t.elapsed();
    println!("      core render {render:?}, HTML {to_html:?}, JSON {to_json:?}");

    // Clicks: one row, then every row.
    let v: Value = serde_json::from_str(&reset).unwrap();
    let buttons: Vec<u64> = v["p"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["op"] == "create" && p["kind"] == "button")
        .filter_map(|p| p["id"].as_u64())
        .collect();
    let (tx, rx) = mpsc::channel();
    let session = app.connect(&uri, &HeaderMap::new(), move |m| tx.send(m).is_ok());
    next(&rx, "reset");
    for (name, id, runs) in [("1 row", buttons[0], 21), ("every row", buttons[1], 5)] {
        let mut times = Vec::new();
        let mut size = 0;
        let mut ops = 0;
        for _ in 0..runs {
            let t = Instant::now();
            session.receive(format!(r#"{{"t":"click","id":{id}}}"#));
            let (patch, at) = next(&rx, "patch");
            times.push(at - t);
            size = patch.len();
            ops = serde_json::from_str::<Value>(&patch).unwrap()["p"]
                .as_array()
                .map_or(0, Vec::len);
        }
        println!(
            "      click, {name} changed: {:?} to the patch, {ops} ops, {}",
            median(times),
            if size < 4096 {
                format!("{size} B")
            } else {
                kb(size)
            }
        );
    }
}

#[test]
#[ignore]
fn bench() {
    // `PRISM_BENCH_ROWS=5000`: one size (for a profiler).
    let sizes = match std::env::var("PRISM_BENCH_ROWS") {
        Ok(n) => vec![n.parse().expect("a number of rows")],
        Err(_) => vec![100, 1000, 5000],
    };
    for n in sizes {
        run(n);
    }
}
