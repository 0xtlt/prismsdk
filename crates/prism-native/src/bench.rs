//! Timings of the paths every native backend shares: render and reconcile,
//! patch application, layout. Each case runs on fresh state several times
//! and keeps its fastest run (the machine may be busy), with the heap
//! allocations it made and the native calls it caused.
//!
//! `cargo test -p prismsdk-native --release -- --ignored --nocapture bench`

use super::*;
use prism_core::{
    App, AppInit, Context, Entity, IntoElement, ParentElement, Platform, Render, Runtime,
    RuntimeOptions, Styled, div, rgb, scroll, text, text_input,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Counts heap allocations (and reallocations), for the whole test binary.
struct Counting;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Widgets that do nothing but count what they are asked.
#[derive(Default)]
struct Null {
    frames: usize,
    measures: Cell<usize>,
    styles: usize,
}

impl Widgets for Null {
    type View = NodeId;
    fn create(&mut self, id: NodeId, _: NodeKind) -> NodeId {
        id
    }
    fn set_style(&mut self, _: &NodeId, _: &Style) {
        self.styles += 1;
    }
    fn set_text(&mut self, _: &NodeId, _: &str) {}
    fn set_placeholder(&mut self, _: &NodeId, _: &str) {}
    fn set_clickable(&mut self, _: &NodeId, _: bool) {}
    fn set_children(&mut self, _: &NodeId, _: &[NodeId]) {}
    fn set_root(&mut self, _: &NodeId) {}
    fn destroy(&mut self, _: &NodeId) {}
    fn measure(&mut self, _: &NodeId, w: Option<f32>) -> (f32, f32) {
        self.measures.set(self.measures.get() + 1);
        (w.unwrap_or(200.).min(200.), 20.)
    }
    fn set_frame(&mut self, _: &NodeId, _: f32, _: f32, _: f32, _: f32) {
        self.frames += 1;
    }
    fn set_accessibility(&mut self, _: &NodeId, _: &Accessibility) {}
    fn set_value(&mut self, _: &NodeId, _: f64) {}
    fn screen_viewport(&mut self, _: &NodeId, _: &NodeId) -> Viewport {
        Viewport::default()
    }
}

struct Row {
    n: usize,
    hits: u32,
    /// A context menu, a click handler and a hover style.
    rich: bool,
}

impl Render for Row {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let row = div()
            .flex_row()
            .gap(8.)
            .p(8.)
            .child(text(format!("Row {}", self.n)))
            .child(text(format!("{} hits", self.hits)));
        if !self.rich {
            return row;
        }
        let hit = cx.listener(|this: &mut Row, _, _| this.hits += 1);
        row.rounded(6.)
            .bg(rgb(0xF4F4F5))
            .hover(|s| s.bg(rgb(0xE4E4E7)))
            .on_click(hit.clone())
            .context_menu(
                prism_core::menu::Menu::new()
                    .action("Open", hit.clone())
                    .action("Delete", hit),
            )
    }
}

struct List {
    rows: Vec<Entity<Row>>,
}

impl Render for List {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        scroll().p(16.).gap(4.).children(self.rows.iter().copied())
    }
}

/// Rows with a text field each, sharing a keyboard bar (#242).
struct Fields {
    rows: Vec<Entity<Row>>,
}

impl Render for Fields {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(
                scroll().children(
                    self.rows
                        .iter()
                        .map(|row| div().flex_row().child(*row).child(text_input())),
                ),
            )
            .keyboard_toolbar(div().h(44.).child(text("Done")))
    }
}

/// Rows whose badge is drawn above the next row (`z_index`, #232).
struct Badges {
    rows: Vec<Entity<Row>>,
}

impl Render for Badges {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        scroll().children(
            self.rows
                .iter()
                .map(|row| div().child(*row).child(text("new").z_index(1))),
        )
    }
}

/// A list of plain keyed rows (no entities), in `order`.
struct Keyed {
    order: Vec<usize>,
}

impl Render for Keyed {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        scroll().p(16.).gap(4.).children(self.order.iter().map(|n| {
            div()
                .key(n.to_string())
                .flex_row()
                .p(8.)
                .child(text(format!("Row {n}")))
        }))
    }
}

/// `depth` nested views around a row entity.
struct Deep {
    depth: usize,
    leaf: Entity<Row>,
}

impl Render for Deep {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        let mut el = div().p(1.).child(self.leaf);
        for _ in 1..self.depth {
            el = div().p(1.).child(el);
        }
        el
    }
}

struct Fixture<T: 'static> {
    rt: Runtime,
    r: NativeRenderer<Null>,
    root: Entity<T>,
}

impl<T: Render> Fixture<T> {
    /// The app's first render, applied but not laid out.
    fn new(build: impl Fn(&mut Context<T>) -> T + Send + Sync + 'static) -> Self {
        Self::on(None, build)
    }

    /// The same, on `platform` (the host's by default).
    fn on(
        platform: Option<Platform>,
        build: impl Fn(&mut Context<T>) -> T + Send + Sync + 'static,
    ) -> Self {
        let root: Arc<std::sync::Mutex<Option<Entity<T>>>> = Default::default();
        let out = root.clone();
        let init: AppInit = Arc::new(move |cx: &mut App| {
            let e = cx.new_entity(&build);
            *out.lock().unwrap() = Some(e);
            cx.set_root(e);
        });
        let mut rt = Runtime::with_options(
            &init,
            RuntimeOptions {
                path: "/".into(),
                platform,
                ..Default::default()
            },
        );
        let patches = rt.render();
        let mut r = NativeRenderer::new(Null::default());
        r.apply(&patches);
        let root = root.lock().unwrap().take().unwrap();
        Fixture { rt, r, root }
    }

    fn layout(&mut self) {
        self.r.layout(400., 800., Insets::default());
    }

    /// Re-renders what changed and applies it: the number of patches.
    fn commit(&mut self) -> usize {
        let patches = self.rt.poll();
        self.r.apply(&patches);
        patches.len()
    }
}

fn list(n: usize, rich: bool) -> Fixture<List> {
    Fixture::new(move |cx: &mut Context<List>| List {
        rows: (0..n)
            .map(|n| cx.new(|_| Row { n, hits: 0, rich }))
            .collect(),
    })
}

fn bump(rt: &mut Runtime, row: Entity<Row>) {
    row.update(&mut rt.app, |r, cx| {
        r.hits += 1;
        cx.notify();
    });
}

/// What one run of a case measured.
struct Run {
    time: Duration,
    allocations: usize,
    /// Patches, native frames or measures: what the case counts.
    count: usize,
}

const RUNS: usize = 5;

/// Runs `case` on a fresh `setup()` [`RUNS`] times and keeps the fastest.
fn best<S>(setup: impl Fn() -> S, case: impl Fn(&mut S) -> usize) -> Run {
    let mut best: Option<Run> = None;
    for _ in 0..RUNS {
        let mut state = setup();
        let allocations = ALLOCATIONS.load(Ordering::Relaxed);
        let t = Instant::now();
        let count = case(&mut state);
        let time = t.elapsed();
        let allocations = ALLOCATIONS.load(Ordering::Relaxed) - allocations;
        drop(state);
        if best.as_ref().is_none_or(|b| time < b.time) {
            best = Some(Run {
                time,
                allocations,
                count,
            });
        }
    }
    best.unwrap()
}

fn report(case: &str, unit: &str, run: Run) {
    println!(
        "{case:<44} {:>12.3?}  {:>9} allocs  {:>7} {unit}",
        run.time, run.allocations, run.count
    );
}

fn rows(n: usize) {
    println!("— {n} rows ({} nodes)", 3 * n + 1);
    report(
        "first render (core)",
        "patches",
        best(
            || (),
            |_| {
                let init: AppInit = Arc::new(move |cx: &mut App| {
                    let list = cx.new_entity(|cx: &mut Context<List>| List {
                        rows: (0..n)
                            .map(|n| {
                                cx.new(|_| Row {
                                    n,
                                    hits: 0,
                                    rich: false,
                                })
                            })
                            .collect(),
                    });
                    cx.set_root(list);
                });
                Runtime::new(&init).render().len()
            },
        ),
    );
    report(
        "first apply (native)",
        "styles",
        best(
            || {
                let init: AppInit = Arc::new(move |cx: &mut App| {
                    let list = cx.new_entity(|cx: &mut Context<List>| List {
                        rows: (0..n)
                            .map(|n| {
                                cx.new(|_| Row {
                                    n,
                                    hits: 0,
                                    rich: false,
                                })
                            })
                            .collect(),
                    });
                    cx.set_root(list);
                });
                Runtime::new(&init).render()
            },
            |patches| {
                let mut r = NativeRenderer::new(Null::default());
                r.apply(patches);
                r.widgets.styles
            },
        ),
    );
    report(
        "first layout",
        "measures",
        best(
            || list(n, false),
            |f| {
                f.layout();
                f.r.widgets.measures.get()
            },
        ),
    );
    let laid_out = || {
        let mut f = list(n, false);
        f.layout();
        f
    };
    report(
        "1 row changed: render + apply",
        "patches",
        best(laid_out, |f| {
            let row = f.root.read(&f.rt.app).rows[n / 2];
            bump(&mut f.rt, row);
            f.commit()
        }),
    );
    report(
        "1 row changed: relayout",
        "frames",
        best(
            || {
                let mut f = laid_out();
                let row = f.root.read(&f.rt.app).rows[n / 2];
                bump(&mut f.rt, row);
                f.commit();
                f.r.widgets.frames = 0;
                f
            },
            |f| {
                f.layout();
                f.r.widgets.frames
            },
        ),
    );
    report(
        "list re-rendered, rows clean",
        "patches",
        best(laid_out, |f| {
            f.root.notify(&mut f.rt.app);
            f.commit()
        }),
    );
    report(
        "row inserted at the top",
        "patches",
        best(laid_out, |f| {
            f.root.update(&mut f.rt.app, |l, cx| {
                let row = cx.new(|_| Row {
                    n: usize::MAX,
                    hits: 0,
                    rich: false,
                });
                l.rows.insert(0, row);
                cx.notify();
            });
            f.commit()
        }),
    );
    report(
        "every row changed (list clean)",
        "patches",
        best(laid_out, |f| {
            for row in f.root.read(&f.rt.app).rows.clone() {
                bump(&mut f.rt, row);
            }
            f.commit()
        }),
    );
    report(
        "window resized: relayout",
        "frames",
        best(laid_out, |f| {
            f.r.widgets.frames = 0;
            f.r.layout(500., 800., Insets::default());
            f.r.widgets.frames
        }),
    );
    report(
        "keyed plain rows reversed",
        "patches",
        best(
            || {
                let mut f = Fixture::new(move |_: &mut Context<Keyed>| Keyed {
                    order: (0..n).collect(),
                });
                f.layout();
                f
            },
            |f| {
                f.root.update(&mut f.rt.app, |k, cx| {
                    k.order.reverse();
                    cx.notify();
                });
                f.commit()
            },
        ),
    );
    report(
        "rows with menu + hover: first render + apply",
        "patches",
        best(
            || (),
            |_| {
                let f = list(n, true);
                f.r.nodes.len()
            },
        ),
    );
    report(
        "rows with menu + hover: 1 row changed",
        "patches",
        best(
            || list(n, true),
            |f| {
                let row = f.root.read(&f.rt.app).rows[n / 2];
                bump(&mut f.rt, row);
                f.commit()
            },
        ),
    );
    report(
        "rows with a field + keyboard bar: 1 row changed",
        "patches",
        best(
            || {
                let mut f =
                    Fixture::on(Some(Platform::Android), move |cx: &mut Context<Fields>| {
                        Fields {
                            rows: (0..n)
                                .map(|n| {
                                    cx.new(|_| Row {
                                        n,
                                        hits: 0,
                                        rich: false,
                                    })
                                })
                                .collect(),
                        }
                    });
                f.layout();
                f
            },
            |f| {
                let row = f.root.read(&f.rt.app).rows[n / 2];
                bump(&mut f.rt, row);
                f.commit()
            },
        ),
    );
    let badges = move || {
        let mut f = Fixture::new(move |cx: &mut Context<Badges>| Badges {
            rows: (0..n)
                .map(|n| {
                    cx.new(|_| Row {
                        n,
                        hits: 0,
                        rich: false,
                    })
                })
                .collect(),
        });
        f.layout();
        f
    };
    report(
        "rows with a z_index badge: first render + apply",
        "patches",
        best(|| (), |_| badges().r.nodes.len()),
    );
    report(
        "rows with a z_index badge: 1 row changed",
        "patches",
        best(badges, |f| {
            let row = f.root.read(&f.rt.app).rows[n / 2];
            bump(&mut f.rt, row);
            f.commit()
        }),
    );
    report(
        "rows with menu + hover: all removed",
        "patches",
        best(
            || list(n, true),
            |f| {
                f.root.update(&mut f.rt.app, |l, cx| {
                    l.rows.clear();
                    cx.notify();
                });
                f.commit()
            },
        ),
    );
}

fn deep(depth: usize) {
    println!("— {depth} nested views");
    let fixture = || {
        Fixture::new(move |cx: &mut Context<Deep>| Deep {
            depth,
            leaf: cx.new(|_| Row {
                n: 0,
                hits: 0,
                rich: false,
            }),
        })
    };
    report(
        "first render + apply + layout",
        "frames",
        best(
            || (),
            |_| {
                let mut f = fixture();
                f.layout();
                f.r.widgets.frames
            },
        ),
    );
    report(
        "innermost row changed: render + apply + relayout",
        "patches",
        best(
            || {
                let mut f = fixture();
                f.layout();
                f
            },
            |f| {
                let leaf = f.root.read(&f.rt.app).leaf;
                bump(&mut f.rt, leaf);
                let patches = f.commit();
                f.layout();
                patches
            },
        ),
    );
}

#[test]
#[ignore]
fn bench() {
    for n in [1000, 5000] {
        rows(n);
    }
    deep(100);
}
