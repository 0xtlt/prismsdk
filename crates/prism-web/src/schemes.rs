//! App schemes on the web (#343). Natively a web view's `app://main/x`
//! is answered by the app's handler (`Application::web_scheme`); on the
//! web the page is an `<iframe>` of a path the app serves itself, with its
//! own routes: [`WebApp::web_scheme_path`](crate::WebApp::web_scheme_path)
//! (`"app"`, `"/canvas"`) loads `app://main/x?q` from `/canvas/main/x?q`.
//! The handler is not called on the web.
//!
//! The rewrite ([`rewrite`]) is the same here (the first paint's
//! `data-src`) and in `client.js` (`schemeSrc`, the frame's `src`): only a
//! scheme the app serves ([`prism_core::web_view::is_web_scheme`]) with a
//! declared path, a host of letters, digits, `-` and `.`, and a path whose
//! segments are never `.` or `..` (escaped or not) nor hold an escaped `/`
//! or `\`, so the URL stays on this origin, under the declared path.

/// Checks a path declared for a scheme: an absolute path on this origin
/// (`/canvas`), without `//` at its start, `\`, a query, a fragment,
/// spaces or control characters, nor `.` / `..` segments. Returns it
/// without its trailing `/` (`""` for `/`).
pub(crate) fn check_path(path: &str) -> Result<String, String> {
    let bad = |why: &str| Err(format!("\"{path}\" is not a path for an app scheme: {why}"));
    if !path.starts_with('/') || path.starts_with("//") {
        return bad("an absolute path on the app's origin starts with a single /");
    }
    if path
        .chars()
        .any(|c| c <= ' ' || c == '\u{7f}' || matches!(c, '\\' | '?' | '#'))
    {
        return bad("no spaces, control characters, \\, ? or #");
    }
    if !segments_ok(path) {
        return bad("no . or .. segments, nor escaped / or \\");
    }
    Ok(path.trim_end_matches('/').to_string())
}

/// Whether no segment of `path` is `.` or `..`, or holds `/` or `\`, once
/// decoded (`%2e%2e`, `%2F`).
fn segments_ok(path: &str) -> bool {
    path.split('/').all(|segment| {
        let decoded = crate::app::percent_decode(segment);
        decoded != "." && decoded != ".." && !decoded.contains(['/', '\\'])
    })
}

/// Where the web view of `url` loads on the web: `app://Main/x?q#f` →
/// `/canvas/main/x?q#f`, for a scheme the app serves and declared a path
/// for in `paths` (scheme → path, as [`check_path`] returns it). `None`
/// for any other URL, or one that could leave the declared path.
pub(crate) fn rewrite(paths: &[(String, String)], url: &str) -> Option<String> {
    let (scheme, after) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let prefix = &paths.iter().find(|(s, _)| *s == scheme)?.1;
    if !prism_core::web_view::is_web_scheme(&scheme) {
        return None;
    }
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let host = after[..end].to_ascii_lowercase();
    let label =
        |l: &str| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if !host.split('.').all(label) {
        return None;
    }
    let rest = &after[end..];
    if rest.chars().any(|c| c <= ' ' || c == '\u{7f}' || c == '\\') {
        return None;
    }
    let path_end = rest.find(['?', '#']).unwrap_or(rest.len());
    let (path, tail) = rest.split_at(path_end);
    if !segments_ok(path) {
        return None;
    }
    let path = if path.is_empty() { "/" } else { path };
    Some(format!("{prefix}/{host}{path}{tail}"))
}

/// The paths for the page's script (`data-prism-schemes`): `{"app":"/canvas"}`,
/// for the schemes the app serves only (`None` when there are none).
pub(crate) fn script_attr(paths: &[(String, String)]) -> Option<String> {
    let map: serde_json::Map<String, serde_json::Value> = paths
        .iter()
        .filter(|(s, _)| prism_core::web_view::is_web_scheme(s))
        .map(|(s, p)| (s.clone(), serde_json::Value::from(p.clone())))
        .collect();
    (!map.is_empty()).then(|| serde_json::Value::Object(map).to_string())
}

/// Says once that the web view of `url` stays empty: its scheme has no
/// path on the web.
pub(crate) fn unserved(url: &str) {
    static SAID: std::sync::Once = std::sync::Once::new();
    let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
    SAID.call_once(|| {
        eprintln!(
            "prism-web: web_view(\"{url}\") stays empty: on the web, serve the page with your \
             own routes and declare where with WebApp::web_scheme_path(\"{scheme}\", \"/path\")"
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_paths_stay_on_the_origin() {
        assert_eq!(check_path("/canvas").as_deref(), Ok("/canvas"));
        assert_eq!(check_path("/a/b/").as_deref(), Ok("/a/b"));
        assert_eq!(check_path("/").as_deref(), Ok(""));
        for bad in [
            "",
            "canvas",
            "//evil.example/x",
            "/a\\b",
            "/a?x",
            "/a#x",
            "/a b",
            "/a/../b",
            "/a/./b",
            "/a/%2e%2e/b",
            "/a%2Fb",
            "/a%5cb",
            "https://evil.example/",
        ] {
            assert!(check_path(bad).is_err(), "{bad:?}");
        }
    }
}
