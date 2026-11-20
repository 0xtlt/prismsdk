//! Build-time helpers for Prism apps.
//!
//! ## Link settings
//!
//! Call [`link_app`] first in the app's `build.rs` (`prism new` does):
//!
//! ```ignore
//! fn main() {
//!     prism_build::link_app();
//! }
//! ```
//!
//! ## Translations
//!
//! One file per language in `locales/` (`en.json`, `fr.json`, `pt-BR.json`…),
//! flat or nested:
//!
//! ```json
//! { "home": { "title": "Welcome {name}!", "open": { "one": "{count} todo", "other": "{count} todos" } } }
//! ```
//!
//! `build.rs`:
//!
//! ```ignore
//! fn main() {
//!     prism_build::translations("locales").fallback("en").generate();
//! }
//! ```
//!
//! `src/main.rs`:
//!
//! ```ignore
//! mod strings { prism::include_strings!(); }
//!
//! cx.set_translations(strings::translations());
//! text(strings::home_title(cx, &user.name));   // placeholders become arguments
//! text(strings::home_open(cx, todos.len() as i64)); // plural forms take a count
//! ```
//!
//! Every key becomes a function, so a typo or a forgotten argument is a
//! **compile error**. The files are checked against the fallback language:
//! a missing key, an unknown key, a placeholder that does not exist or a
//! plural without `other` is a **cargo warning** (an error with `.strict()`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Link settings the app's own executable needs: on Apple targets, the Swift
/// runtime's search path (`-rpath /usr/lib/swift`) and, when the environment
/// sets none, Prism's deployment target (iOS 17, tvOS 17, watchOS 10,
/// visionOS 26, macOS 13) for the final link. No-op on other targets.
///
/// Prism's backends cannot add these themselves: Cargo applies a build
/// script's `rustc-link-arg` only to the package that printed it. Without
/// them, a plain `cargo build` for iOS links for rustc's default (iOS 14),
/// where Swift Concurrency is an `@rpath` library the app does not ship, and
/// the app stops at launch. See [`prism_apple_build::link_app`].
pub fn link_app() {
    prism_apple_build::link_app();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winappsdk::link_app();
    }
}

pub mod winappsdk;

/// Starts the translations build step for the `*.json` files in `dir`
/// (relative to the package).
pub fn translations(dir: impl AsRef<Path>) -> TranslationsBuild {
    TranslationsBuild {
        dir: dir.as_ref().to_path_buf(),
        fallback: "en".into(),
        strict: false,
        out: None,
    }
}

pub struct TranslationsBuild {
    dir: PathBuf,
    fallback: String,
    strict: bool,
    out: Option<PathBuf>,
}

const PLURAL_FORMS: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];

/// A problem found in the catalogs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub file: String,
    pub message: String,
}

/// The parsed catalogs and the code generated from them.
#[derive(Debug)]
pub struct Output {
    pub languages: Vec<String>,
    pub issues: Vec<Issue>,
    pub code: String,
}

impl TranslationsBuild {
    /// The reference language: every other file is checked against it, and
    /// missing keys fall back to it at runtime (default `en`).
    pub fn fallback(mut self, lang: &str) -> Self {
        self.fallback = lang.to_string();
        self
    }

    /// Fails the build on any issue instead of warning.
    pub fn strict(mut self) -> Self {
        self.strict = true;
        self
    }

    /// Writes the code somewhere other than `$OUT_DIR/prism_strings.rs`.
    pub fn out(mut self, path: impl AsRef<Path>) -> Self {
        self.out = Some(path.as_ref().to_path_buf());
        self
    }

    /// Checks the catalogs, reports issues to cargo and writes the code.
    pub fn generate(self) {
        let manifest = std::env::var("CARGO_MANIFEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_default();
        let dir = manifest.join(&self.dir);
        println!("cargo:rerun-if-changed={}", dir.display());
        let output = match self.process(&dir) {
            Ok(o) => o,
            Err(e) => panic!("prism translations: {e}"),
        };
        for issue in &output.issues {
            println!("cargo:warning={}: {}", issue.file, issue.message);
        }
        if self.strict && !output.issues.is_empty() {
            panic!(
                "prism translations: {} problem(s) in {} (strict mode)",
                output.issues.len(),
                self.dir.display()
            );
        }
        println!(
            "cargo:rustc-env=PRISM_LANGUAGES={}",
            output.languages.join(",")
        );
        let out = self.out.clone().unwrap_or_else(|| {
            PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR: call from build.rs"))
                .join("prism_strings.rs")
        });
        std::fs::write(&out, output.code).expect("prism translations: cannot write the code");
    }

    /// Reads, checks and generates (without talking to cargo).
    pub fn process(&self, dir: &Path) -> Result<Output, String> {
        let mut catalogs: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let lang = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            println!("cargo:rerun-if-changed={}", path.display());
            let text =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let json: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| format!("{}: invalid JSON: {e}", path.display()))?;
            let mut flat = BTreeMap::new();
            flatten("", &json, &mut flat).map_err(|e| format!("{lang}.json: {e}"))?;
            catalogs.insert(lang, flat);
        }
        let Some(reference) = catalogs.get(&self.fallback) else {
            return Err(format!(
                "no {}.json in {} (the fallback language)",
                self.fallback,
                dir.display()
            ));
        };
        let keys = Keys::from_catalog(reference);
        let mut issues = Vec::new();
        for (key, forms) in &keys.plurals {
            if !forms.contains("other") {
                issues.push(Issue {
                    file: format!("{}.json", self.fallback),
                    message: format!("plural \"{key}\" has no \"other\" form"),
                });
            }
        }
        for (lang, catalog) in &catalogs {
            if *lang != self.fallback {
                check(lang, catalog, reference, &keys, &mut issues);
            }
        }
        let code = generate(&self.fallback, &catalogs, reference, &keys)?;
        Ok(Output {
            languages: catalogs.keys().cloned().collect(),
            issues,
            code,
        })
    }
}

/// `{"a": {"b": "x"}}` → `a.b = x`.
fn flatten(
    prefix: &str,
    value: &serde_json::Value,
    out: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(&key, v, out)?;
            }
            Ok(())
        }
        serde_json::Value::String(s) => {
            out.insert(prefix.to_string(), s.clone());
            Ok(())
        }
        other => Err(format!(
            "\"{prefix}\" must be a string or an object, not {other}"
        )),
    }
}

/// `{name}` placeholders, in order of appearance.
fn placeholders(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else { break };
        let name = after[..end].trim().to_string();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
        rest = &after[end + 1..];
    }
    out
}

/// The keys of the fallback catalog: simple strings and plural groups.
struct Keys {
    /// key → placeholders
    simple: BTreeMap<String, Vec<String>>,
    /// base key → forms present
    plurals: BTreeMap<String, BTreeSet<String>>,
    /// base key → placeholders (all forms, without `count`)
    plural_args: BTreeMap<String, Vec<String>>,
}

fn split_plural(key: &str) -> Option<(&str, &str)> {
    let (base, form) = key.rsplit_once('.')?;
    PLURAL_FORMS.contains(&form).then_some((base, form))
}

impl Keys {
    fn from_catalog(catalog: &BTreeMap<String, String>) -> Self {
        let mut keys = Keys {
            simple: BTreeMap::new(),
            plurals: BTreeMap::new(),
            plural_args: BTreeMap::new(),
        };
        for (key, text) in catalog {
            match split_plural(key) {
                Some((base, form)) => {
                    keys.plurals
                        .entry(base.to_string())
                        .or_default()
                        .insert(form.to_string());
                    let args = keys.plural_args.entry(base.to_string()).or_default();
                    for p in placeholders(text) {
                        if p != "count" && !args.contains(&p) {
                            args.push(p);
                        }
                    }
                }
                None => {
                    keys.simple.insert(key.clone(), placeholders(text));
                }
            }
        }
        keys
    }

    fn known(&self, key: &str) -> bool {
        self.simple.contains_key(key)
            || split_plural(key).is_some_and(|(base, _)| self.plurals.contains_key(base))
    }

    fn allowed_placeholders(&self, key: &str) -> Vec<String> {
        match split_plural(key) {
            Some((base, _)) if self.plurals.contains_key(base) => {
                let mut v = self.plural_args.get(base).cloned().unwrap_or_default();
                v.push("count".into());
                v
            }
            _ => self.simple.get(key).cloned().unwrap_or_default(),
        }
    }
}

fn check(
    lang: &str,
    catalog: &BTreeMap<String, String>,
    reference: &BTreeMap<String, String>,
    keys: &Keys,
    issues: &mut Vec<Issue>,
) {
    let file = format!("{lang}.json");
    let mut push = |message: String| {
        issues.push(Issue {
            file: file.clone(),
            message,
        })
    };
    // Plurals may use other forms than the fallback (Arabic has six), but
    // need at least `other`.
    for key in reference.keys() {
        if split_plural(key).is_some() {
            continue;
        }
        if !catalog.contains_key(key) {
            push(format!(
                "missing \"{key}\" (falls back to the reference language)"
            ));
        }
    }
    for base in keys.plurals.keys() {
        let present = PLURAL_FORMS
            .iter()
            .filter(|f| catalog.contains_key(&format!("{base}.{f}")))
            .count();
        if present == 0 {
            push(format!(
                "missing plural \"{base}\" (falls back to the reference language)"
            ));
        } else if !catalog.contains_key(&format!("{base}.other")) {
            push(format!("plural \"{base}\" has no \"other\" form"));
        }
    }
    for (key, text) in catalog {
        if !keys.known(key) {
            push(format!(
                "\"{key}\" is not in the reference language (unused)"
            ));
            continue;
        }
        let allowed = keys.allowed_placeholders(key);
        for p in placeholders(text) {
            if !allowed.contains(&p) {
                push(format!(
                    "\"{key}\" uses {{{p}}}, which the reference text does not have"
                ));
            }
        }
    }
}

const KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try", "gen",
];

/// `home.open-count` → `home_open_count`.
fn ident(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    let s = s.trim_matches('_').to_string();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{s}")
    } else if KEYWORDS.contains(&s.as_str()) {
        format!("r#{s}")
    } else {
        s
    }
}

fn doc(text: &str) -> String {
    text.replace('\n', "\\n")
}

fn generate(
    fallback: &str,
    catalogs: &BTreeMap<String, BTreeMap<String, String>>,
    reference: &BTreeMap<String, String>,
    keys: &Keys,
) -> Result<String, String> {
    let mut c = String::new();
    let w = &mut c;
    let _ = writeln!(
        w,
        "// @generated by prism-build from the locales/*.json files. Do not edit."
    );
    let langs: Vec<String> = catalogs.keys().map(|l| format!("{l:?}")).collect();
    let _ = writeln!(w, "/// Languages with a catalog.");
    let _ = writeln!(
        w,
        "#[allow(dead_code)]\npub const LANGUAGES: &[&str] = &[{}];",
        langs.join(", ")
    );
    let _ = writeln!(w, "/// The reference language.");
    let _ = writeln!(
        w,
        "#[allow(dead_code)]\npub const FALLBACK: &str = {fallback:?};"
    );
    let _ = writeln!(w, "/// Every catalog, for `cx.set_translations(…)`.");
    let _ = writeln!(
        w,
        "#[allow(dead_code)]\npub fn translations() -> ::prism::Translations {{"
    );
    let _ = writeln!(
        w,
        "    ::prism::Translations::new()\n        .fallback(FALLBACK)"
    );
    for (lang, catalog) in catalogs {
        let _ = writeln!(w, "        .add({lang:?}, &[");
        for (k, v) in catalog {
            let _ = writeln!(w, "            ({k:?}, {v:?}),");
        }
        let _ = writeln!(w, "        ])");
    }
    let _ = writeln!(w, "}}");

    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut claim = |name: &str, key: &str| -> Result<(), String> {
        if let Some(other) = names.insert(name.to_string(), key.to_string()) {
            return Err(format!(
                "\"{other}\" and \"{key}\" both become strings::{name}()"
            ));
        }
        Ok(())
    };
    for (key, args) in &keys.simple {
        let name = ident(key);
        claim(&name, key)?;
        let _ = writeln!(w, "\n/// `{key}`: \"{}\"", doc(&reference[key]));
        let params: Vec<String> = args
            .iter()
            .map(|a| format!(", {}: impl ::std::fmt::Display", ident(a)))
            .collect();
        let pairs: Vec<String> = args
            .iter()
            .map(|a| format!("({a:?}, &{})", ident(a)))
            .collect();
        let _ = writeln!(
            w,
            "#[allow(dead_code)]\npub fn {name}(cx: &::prism::App{}) -> String {{\n    cx.t_with({key:?}, &[{}])\n}}",
            params.concat(),
            pairs.join(", ")
        );
    }
    for (base, forms) in &keys.plurals {
        let name = ident(base);
        claim(&name, base)?;
        let sample = forms
            .iter()
            .filter_map(|f| reference.get(&format!("{base}.{f}")))
            .map(|t| format!("\"{}\"", doc(t)))
            .collect::<Vec<_>>()
            .join(" / ");
        let args = &keys.plural_args[base];
        let _ = writeln!(w, "\n/// `{base}` (plural): {sample}");
        let params: Vec<String> = args
            .iter()
            .map(|a| format!(", {}: impl ::std::fmt::Display", ident(a)))
            .collect();
        let pairs: Vec<String> = args
            .iter()
            .map(|a| format!("({a:?}, &{})", ident(a)))
            .collect();
        let _ = writeln!(
            w,
            "#[allow(dead_code)]\npub fn {name}(cx: &::prism::App, count: i64{}) -> String {{\n    cx.t_count_with({base:?}, count, &[{}])\n}}",
            params.concat(),
            pairs.join(", ")
        );
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(files: &[(&str, &str)]) -> Result<Output, String> {
        let dir = std::env::temp_dir().join(format!(
            "prism-build-test-{}-{}",
            std::process::id(),
            files.len()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in files {
            std::fs::write(dir.join(name), text).unwrap();
        }
        let out = translations(&dir).process(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn generates_typed_functions_and_reports_issues() {
        let out = run(&[
            (
                "en.json",
                r#"{"home": {"title": "Hi {name}!", "open": {"one": "{count} todo", "other": "{count} todos in {list}"}}, "type": "Type"}"#,
            ),
            (
                "fr.json",
                r#"{"home": {"title": "Salut {nom} !", "open": {"one": "{count} tâche"}}, "extra": "x"}"#,
            ),
        ])
        .unwrap();
        assert_eq!(out.languages, ["en", "fr"]);
        assert!(out.code.contains(
            "pub fn home_title(cx: &::prism::App, name: impl ::std::fmt::Display) -> String"
        ));
        assert!(out.code.contains(
            "pub fn home_open(cx: &::prism::App, count: i64, list: impl ::std::fmt::Display)"
        ));
        assert!(out.code.contains("pub fn r#type(cx: &::prism::App)"));
        let messages: Vec<&str> = out.issues.iter().map(|i| i.message.as_str()).collect();
        assert!(
            messages.contains(&"missing \"type\" (falls back to the reference language)"),
            "{messages:?}"
        );
        assert!(messages.contains(&"plural \"home.open\" has no \"other\" form"));
        assert!(messages.contains(&"\"extra\" is not in the reference language (unused)"));
        assert!(
            messages.contains(&"\"home.title\" uses {nom}, which the reference text does not have")
        );
    }

    #[test]
    fn errors_on_bad_files() {
        assert!(
            run(&[("fr.json", "{}")])
                .unwrap_err()
                .contains("no en.json")
        );
        assert!(
            run(&[("en.json", "{ nope")])
                .unwrap_err()
                .contains("invalid JSON")
        );
        assert!(
            run(&[("en.json", r#"{"a": 1}"#)])
                .unwrap_err()
                .contains("must be a string")
        );
        assert!(
            run(&[("en.json", r#"{"a.b": "x", "a_b": "y"}"#)])
                .unwrap_err()
                .contains("both become strings::a_b()")
        );
    }
}
