//! Islands on the web: the WASM islands `prism` built, the URLs of island
//! modules, and the content security policy they need.
//!
//! `prism run/dev/build web` builds every island crate of `[web] islands`
//! (`Prism.toml`) into a directory holding content-hashed files and a
//! `manifest.json`, and passes it as `PRISM_ISLANDS_DIR`:
//!
//! ```json
//! { "chart": { "module": "prism-island-chart.1a2b3c4d5e6f.js",
//!              "files": ["prism-island-chart.1a2b3c4d5e6f.js", "prism-island-chart.6f5e4d3c2b1a.wasm"] } }
//! ```
//!
//! The files are served as `/_prism/islands/<file>` (immutable). Pages that
//! show no island load none of them: `client.js` imports an island's module
//! the first time a page shows it.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use prism_core::IslandSource;

/// The URL of the module defining an island's element, if there is one.
pub type ModuleUrls = Arc<dyn Fn(&IslandSource) -> Option<String> + Send + Sync>;

/// The WASM islands of the app (from `PRISM_ISLANDS_DIR`).
#[derive(Clone, Debug, Default)]
pub(crate) struct Islands {
    /// Island name → served path of its module (`_prism/islands/…js`).
    pub modules: HashMap<String, String>,
    /// Served path → contents.
    pub files: Vec<(String, Vec<u8>)>,
}

impl Islands {
    /// Reads `dir/manifest.json` and the files it lists. A missing directory
    /// or manifest means no islands.
    pub fn load(dir: &Path) -> Self {
        let mut islands = Islands::default();
        let Ok(text) = std::fs::read_to_string(dir.join("manifest.json")) else {
            return islands;
        };
        let Ok(serde_json::Value::Object(manifest)) = serde_json::from_str(&text) else {
            eprintln!(
                "prism-web: {} is not valid JSON",
                dir.join("manifest.json").display()
            );
            return islands;
        };
        for (name, entry) in manifest {
            let Some(module) = entry["module"].as_str() else {
                continue;
            };
            let files = entry["files"].as_array().cloned().unwrap_or_default();
            for file in files.iter().filter_map(|f| f.as_str()) {
                if !crate::assets::safe(file) {
                    continue;
                }
                match std::fs::read(dir.join(file)) {
                    Ok(body) => islands.files.push((format!("_prism/islands/{file}"), body)),
                    Err(e) => eprintln!("prism-web: island {name}: cannot read {file}: {e}"),
                }
            }
            islands
                .modules
                .insert(name, format!("_prism/islands/{module}"));
        }
        islands
    }

    /// Whether the app has any WASM island (its CSP needs `'wasm-unsafe-eval'`).
    pub fn any(&self) -> bool {
        !self.modules.is_empty()
    }
}

/// Island names already reported as missing (once per process).
static MISSING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Says once that island `name` was not built.
pub(crate) fn report_missing(name: &str) {
    let mut missing = MISSING.lock().unwrap_or_else(|e| e.into_inner());
    if missing
        .get_or_insert_with(HashSet::new)
        .insert(name.to_string())
    {
        eprintln!(
            "prism-web: island {name:?} is not built: list its crate in Prism.toml \
             ([web] islands = [\"islands/{name}\"]) and start the app with `prism run web` \
             (or set PRISM_ISLANDS_DIR). Pages show its fallback."
        );
    }
}

impl crate::WebApp {
    /// The URL of the module defining an island's element: a WASM island's
    /// generated module, an app asset or a URL for a web component.
    pub fn island_module_url(&self, source: &IslandSource) -> Option<String> {
        match source {
            IslandSource::Wasm(name) => match self.inner.islands.modules.get(name) {
                Some(path) => Some(format!("{}/{path}", self.static_prefix())),
                None => {
                    report_missing(name);
                    None
                }
            },
            IslandSource::Module(src) => Some(self.picture_url(src)),
            IslandSource::Page => None,
        }
    }

    /// Island module URLs for the encoder.
    pub(crate) fn module_urls(&self) -> ModuleUrls {
        let app = self.clone();
        Arc::new(move |source: &IslandSource| app.island_module_url(source))
    }

    /// A strict `Content-Security-Policy` for this app's pages, with this
    /// page's nonce: what Prism needs and nothing more. WASM islands add
    /// `'wasm-unsafe-eval'` to `script-src` (compiling WebAssembly, never
    /// `eval`); a CDN (`asset_url`) is added where static files load from.
    pub fn content_security_policy(&self, nonce: &crate::CspNonce) -> String {
        let cdn = self
            .inner
            .asset_url
            .as_deref()
            .and_then(origin_of)
            .map(|o| format!(" {o}"))
            .unwrap_or_default();
        let wasm = if self.inner.islands.any() {
            " 'wasm-unsafe-eval'"
        } else {
            ""
        };
        // Web views (#62): the origins their pages come from.
        let frames = if self.inner.frame_origins.is_empty() {
            String::new()
        } else {
            format!(" frame-src 'self' {};", self.inner.frame_origins.join(" "))
        };
        let ancestors = self.frame_ancestors();
        let n = &nonce.0;
        format!(
            "default-src 'self'; script-src 'self' 'nonce-{n}'{wasm}{cdn}; \
             style-src 'self' 'nonce-{n}'{cdn}; img-src 'self' data: blob: https:; \
             font-src 'self' data:{cdn}; connect-src 'self'{cdn}; media-src 'self' blob: https:;{frames} \
             object-src 'none'; base-uri 'none'; frame-ancestors {ancestors}"
        )
    }

    /// An origin (`https://preview.example.com`) whose pages
    /// [`web_view`](prism_core::web_view())s may show under the
    /// [`strict_csp`](Self::strict_csp) (`frame-src`), which otherwise allows
    /// the app's own origin only. Hosts with their own CSP add it to their
    /// `frame-src`.
    pub fn frame_origin(self, origin: &str) -> Self {
        let origin = origin.trim().trim_end_matches('/').to_string();
        self.configure(|i| {
            if !i.frame_origins.contains(&origin) {
                i.frame_origins.push(origin);
            }
        })
    }

    /// A site (`https://portal.example.com`) that may show this app's pages
    /// in a frame, besides the app's own origin. By default only the app's
    /// own origin may: pages and the sign-in callback send
    /// `Content-Security-Policy: frame-ancestors 'self'` and
    /// `X-Frame-Options: SAMEORIGIN`, so another site cannot overlay them to
    /// steer a signed-in visitor's clicks. Each call adds one, and
    /// `X-Frame-Options` is then left out (it cannot name other sites);
    /// `"*"` lets any site. A value with spaces, quotes, `;` or `,` is not
    /// an origin and is ignored.
    pub fn frame_ancestor(self, origin: &str) -> Self {
        let origin = origin.trim().trim_end_matches('/').to_string();
        let bad =
            |c: char| c.is_whitespace() || c.is_control() || matches!(c, ';' | ',' | '\'' | '"');
        if origin.is_empty() || origin.contains(bad) {
            eprintln!("prism-web: frame_ancestor({origin:?}) ignored: not an origin");
            return self;
        }
        self.configure(|i| {
            if !i.frame_ancestors.contains(&origin) {
                i.frame_ancestors.push(origin);
            }
        })
    }

    /// Who may frame the app's pages, as `frame-ancestors` lists them.
    pub(crate) fn frame_ancestors(&self) -> String {
        std::iter::once("'self'")
            .chain(self.inner.frame_ancestors.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Sends [`content_security_policy`](Self::content_security_policy) with
    /// every response of the built-in server and router, with a fresh nonce
    /// per request (also `PRISM_STRICT_CSP=1`). Hosts with their own CSP
    /// middleware insert their [`CspNonce`](crate::CspNonce) instead.
    pub fn strict_csp(self) -> Self {
        self.configure(|i| i.strict_csp = true)
    }

    /// Serves the WASM islands built in `dir` (what `prism` passes as
    /// `PRISM_ISLANDS_DIR`: content-hashed files plus `manifest.json`).
    pub fn islands_dir(self, dir: impl AsRef<Path>) -> Self {
        let islands = Islands::load(dir.as_ref());
        self.configure(|i| {
            i.assets.add_islands(&islands);
            i.islands = islands;
        })
    }

    /// Whether [`strict_csp`](Self::strict_csp) is on.
    pub fn has_strict_csp(&self) -> bool {
        self.inner.strict_csp
    }
}

/// `https://cdn.example.com/x` → `https://cdn.example.com`.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split('/').next()?;
    (!host.is_empty()).then(|| format!("{scheme}://{host}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temp_dir::TempDir;

    #[test]
    fn the_manifest_lists_modules_and_files() {
        let dir = TempDir::new("islands");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"chart":{"module":"prism-island-chart.abc.js","files":["prism-island-chart.abc.js","prism-island-chart.def.wasm","../secret"]}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("prism-island-chart.abc.js"), "js").unwrap();
        std::fs::write(dir.join("prism-island-chart.def.wasm"), b"\0asm").unwrap();
        let islands = Islands::load(&dir);
        assert_eq!(
            islands.modules["chart"],
            "_prism/islands/prism-island-chart.abc.js"
        );
        let served: Vec<&str> = islands.files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            served,
            [
                "_prism/islands/prism-island-chart.abc.js",
                "_prism/islands/prism-island-chart.def.wasm"
            ]
        );
        assert!(islands.any());
        assert!(!Islands::load(&dir.join("nope")).any());
    }

    use prism_core::prelude::*;
    use prism_core::{AppConfig, AppInit, Backend, IslandSource, Patch, Runtime};

    struct Page;
    impl Render for Page {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(
                    island("chart")
                        .prop("points", &[1, 2])
                        .on(
                            "select",
                            std::rc::Rc::new(|_: &IslandEvent, _: &mut App| {}),
                        )
                        .child(text("12 points")),
                )
                .child(
                    web_component("time-ago")
                        .module("time-ago.js")
                        .prop("datetime", &5)
                        .prop("label", "a \"b\"")
                        .prop("live", &true)
                        .prop("list", &[1])
                        .child(text("just now")),
                )
        }
    }

    fn init() -> AppInit {
        Arc::new(|cx: &mut App| {
            let root = cx.new_entity(|_| Page);
            cx.set_root(root);
        })
    }

    #[test]
    fn the_first_paint_has_the_custom_elements_around_their_fallback() {
        let mut rt = Runtime::new(&init());
        let mut html = crate::HtmlBackend::default();
        html.apply(&rt.render());
        let out = html.to_html();
        assert!(out.contains("<prism-island-chart data-pid="), "{out}");
        assert!(out.contains(" data-island>"), "{out}");
        assert!(
            out.contains("12 points</div></prism-island-chart>"),
            "{out}"
        );
        // A web component's strings, numbers and booleans are attributes too.
        assert!(
            out.contains(r#" data-island datetime="5" label="a &quot;b&quot;" live>"#),
            "{out}"
        );
        assert!(out.contains("just now</div></time-ago>"), "{out}");
    }

    #[test]
    fn island_patches_on_the_wire() {
        let mut e = crate::Encoder::default();
        e.module_urls = Some(Arc::new(|s: &IslandSource| match s {
            IslandSource::Wasm(n) => Some(format!("/islands/{n}.js")),
            _ => None,
        }));
        let ops = e.encode(&[
            Patch::SetIsland {
                id: 3,
                tag: "prism-island-chart".into(),
                source: IslandSource::Wasm("chart".into()),
                events: vec!["select".into()],
            },
            Patch::SetIsland {
                id: 4,
                tag: "my-widget".into(),
                source: IslandSource::Page,
                events: vec![],
            },
            Patch::SetIslandProps {
                id: 3,
                props: vec![
                    ("points".into(), "[1,2]".into()),
                    ("gone".into(), "null".into()),
                ],
            },
        ]);
        assert_eq!(
            ops[0],
            serde_json::json!({"op": "island", "id": 3, "tag": "prism-island-chart",
                "module": "/islands/chart.js", "wasm": true, "events": ["select"]})
        );
        assert_eq!(ops[1]["module"], serde_json::Value::Null);
        assert_eq!(ops[1]["wasm"], false);
        assert_eq!(
            ops[2],
            serde_json::json!({"op": "props", "id": 3, "props": {"points": [1, 2], "gone": null}})
        );
    }

    #[test]
    fn island_events_from_the_page() {
        let v = serde_json::json!({"t": "island", "id": 3, "name": "select", "detail": [1, 2.5]});
        assert_eq!(
            crate::session::parse_event(&v),
            Some(prism_core::UiEvent::Island(
                3,
                "select".into(),
                "[1,2.5]".into()
            ))
        );
        let bare = serde_json::json!({"t": "island", "id": 3, "name": "ping"});
        assert_eq!(
            crate::session::parse_event(&bare),
            Some(prism_core::UiEvent::Island(3, "ping".into(), "null".into()))
        );
    }

    #[test]
    fn built_islands_are_served_immutable_and_need_wasm_in_the_csp() {
        let dir = TempDir::new("islands-app");
        let built = dir.join("islands");
        std::fs::create_dir_all(&built).unwrap();
        std::fs::write(
            built.join("manifest.json"),
            r#"{"chart":{"module":"prism-island-chart.abc.js","files":["prism-island-chart.abc.js","prism-island-chart.def.wasm"]}}"#,
        )
        .unwrap();
        std::fs::write(built.join("prism-island-chart.abc.js"), "export {}").unwrap();
        std::fs::write(built.join("prism-island-chart.def.wasm"), b"\0asm").unwrap();
        let config = AppConfig {
            title: "Islands".into(),
            assets: Some(dir.join("assets").to_string_lossy().into()),
            ..Default::default()
        };
        let plain = crate::WebApp::new(config.clone(), init());
        let nonce = crate::CspNonce("n0nce".into());
        let csp = plain.content_security_policy(&nonce);
        assert!(csp.contains("script-src 'self' 'nonce-n0nce';"), "{csp}");
        assert!(!csp.contains("wasm-unsafe-eval"));
        assert_eq!(
            plain.island_module_url(&IslandSource::Wasm("chart".into())),
            None
        );

        let app = crate::WebApp::new(config, init())
            .islands_dir(&built)
            .asset_url("https://cdn.example.com/static");
        let csp = app.content_security_policy(&nonce);
        assert!(
            csp.contains(
                "script-src 'self' 'nonce-n0nce' 'wasm-unsafe-eval' https://cdn.example.com;"
            ),
            "{csp}"
        );
        assert!(
            csp.contains("connect-src 'self' https://cdn.example.com;"),
            "{csp}"
        );
        assert_eq!(
            app.island_module_url(&IslandSource::Wasm("chart".into()))
                .as_deref(),
            Some("https://cdn.example.com/static/_prism/islands/prism-island-chart.abc.js")
        );
        assert_eq!(app.island_module_url(&IslandSource::Page), None);
        let req = http::Request::get("/_prism/islands/prism-island-chart.def.wasm")
            .body(())
            .unwrap();
        let wasm = app.handle(&req);
        assert_eq!(wasm.status(), 200);
        assert_eq!(wasm.headers()["content-type"], "application/wasm");
        assert_eq!(wasm.headers()["cache-control"], crate::IMMUTABLE);
        let out = dir.join("public");
        app.export_static(&out).unwrap();
        assert!(
            out.join("_prism/islands/prism-island-chart.def.wasm")
                .exists()
        );
        let manifest = std::fs::read_to_string(out.join("manifest.json")).unwrap();
        assert!(
            manifest.contains(r#""islands/chart": "_prism/islands/prism-island-chart.abc.js""#),
            "{manifest}"
        );
    }

    #[test]
    fn origins_of_cdn_urls() {
        assert_eq!(
            origin_of("https://cdn.example.com/static").as_deref(),
            Some("https://cdn.example.com")
        );
        assert_eq!(origin_of("/relative"), None);
    }

    /// A web component styled like the app's fields (#294).
    struct StyledHost;
    impl Render for StyledHost {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div().child(
                web_component("rich-editor")
                    .border(1.)
                    .text_size(15.)
                    .font_weight(FontWeight::Semibold)
                    .text_color(rgb(0x123456))
                    .line_height(22.)
                    .hover(|s| s.text_color(rgb(0x654321)))
                    .focus_within(|s| s.font_weight(FontWeight::Bold))
                    .when_width_at_least(500., |s| s.text_size(18.))
                    .child(text("fallback")),
            )
        }
    }

    #[test]
    fn a_web_component_host_carries_its_typography() {
        let init: AppInit = Arc::new(|cx: &mut App| {
            let root = cx.new_entity(|_| StyledHost);
            cx.set_root(root);
        });
        let mut rt = Runtime::new(&init);
        let patches = rt.render();
        // The first paint.
        let mut html = crate::HtmlBackend::default();
        html.apply(&patches);
        let out = html.to_html();
        let host = out
            .split("<rich-editor ")
            .nth(1)
            .and_then(|s| s.split('>').next())
            .unwrap();
        for decl in [
            "font-size:0.9375rem;",
            "font-weight:600;",
            "color:rgba(18,52,86,1);",
            "line-height:1.375rem;",
        ] {
            assert!(host.contains(decl), "{decl} in {host}");
        }
        assert!(
            out.contains(":hover{color:rgba(101,67,33,1) !important;}"),
            "{out}"
        );
        assert!(
            out.contains(":focus-within{font-weight:700 !important;}"),
            "{out}"
        );
        // Live: the style op, its states and its width variants.
        let mut e = crate::Encoder::default();
        let ops = e.encode(&patches);
        let style = ops
            .iter()
            .find(|o| {
                o["op"] == "style" && o["css"].as_str().is_some_and(|c| c.contains("border:"))
            })
            .unwrap();
        let css = style["css"].as_str().unwrap();
        assert!(
            css.contains("font-size:0.9375rem;font-weight:600;"),
            "{css}"
        );
        assert!(
            style["states"]
                .as_str()
                .unwrap()
                .contains("color:rgba(101,67,33,1)")
        );
        let wide = style["cq"][0][1].as_str().unwrap();
        assert!(wide.contains("font-size:1.125rem;"), "{wide}");
        // A plain view still has none (its children style their own text).
        let view = ops
            .iter()
            .find(|o| {
                o["op"] == "style" && o["css"].as_str().is_some_and(|c| !c.contains("border:"))
            })
            .unwrap();
        assert!(
            !view["css"].as_str().unwrap().contains("font-size"),
            "{view}"
        );
    }
}
