//! App assets compiled into the binary.
//!
//! `Application::assets(dir)` names a folder read at run time; a server that
//! ships as one file (or a scratch container) has no folder next to it.
//! [`EmbeddedAssets`] holds the same files in memory, by the name a folder
//! would give them (`logo.png`, `fonts/Inter.ttf`): `picture("logo.png")`
//! and the fonts of `fonts/` find them first, the folder (if any) adds the
//! rest.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

/// App assets held in memory (`include_bytes!`, rust-embed…), by the path
/// they would have under the assets folder. Cheap to clone (shared).
///
/// ```
/// use prism_core::EmbeddedAssets;
///
/// let assets = EmbeddedAssets::new()
///     .file("logo.png", &b"\x89PNG..."[..])
///     .file("fonts/Inter.ttf", vec![0u8; 4]);
/// assert_eq!(assets.get("logo.png"), Some(&b"\x89PNG..."[..]));
/// assert_eq!(assets.names().collect::<Vec<_>>(), ["fonts/Inter.ttf", "logo.png"]);
/// ```
#[derive(Clone, Default, PartialEq, Eq)]
pub struct EmbeddedAssets {
    files: Arc<BTreeMap<String, Cow<'static, [u8]>>>,
}

impl EmbeddedAssets {
    /// No files.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds (or replaces) the file `name` (`img/logo.png`: `/` separated, no
    /// leading `/`, no `..`; other names are ignored).
    pub fn file(mut self, name: impl Into<String>, bytes: impl Into<Cow<'static, [u8]>>) -> Self {
        self.insert(name, bytes);
        self
    }

    /// Adds (or replaces) the file `name`; see [`file`](Self::file).
    pub fn insert(&mut self, name: impl Into<String>, bytes: impl Into<Cow<'static, [u8]>>) {
        let name = name.into().replace('\\', "/");
        let name = name.trim_start_matches("./");
        if is_relative_path(name) {
            Arc::make_mut(&mut self.files).insert(name.to_string(), bytes.into());
        }
    }

    /// Adds every file of `other`, replacing those of the same name.
    pub fn merge(&mut self, other: &EmbeddedAssets) {
        if self.is_empty() {
            self.files = other.files.clone();
        } else if !other.is_empty() {
            let files = Arc::make_mut(&mut self.files);
            for (name, bytes) in other.files.iter() {
                files.insert(name.clone(), bytes.clone());
            }
        }
    }

    /// The contents of `name`.
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.files.get(name).map(|b| &**b)
    }

    /// Every file name, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// Every file and its contents, sorted by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.files.iter().map(|(n, b)| (n.as_str(), &**b))
    }

    /// How many files.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// No files.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

impl<N: Into<String>, B: Into<Cow<'static, [u8]>>> FromIterator<(N, B)> for EmbeddedAssets {
    fn from_iter<I: IntoIterator<Item = (N, B)>>(iter: I) -> Self {
        let mut assets = Self::new();
        assets.extend(iter);
        assets
    }
}

impl<N: Into<String>, B: Into<Cow<'static, [u8]>>> Extend<(N, B)> for EmbeddedAssets {
    fn extend<I: IntoIterator<Item = (N, B)>>(&mut self, iter: I) {
        for (name, bytes) in iter {
            self.insert(name, bytes);
        }
    }
}

impl std::fmt::Debug for EmbeddedAssets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.files.iter().map(|(n, b)| (n, b.len())))
            .finish()
    }
}

/// A relative path that stays inside its folder: not empty, no leading `/`,
/// no empty, `.` or `..` segment.
fn is_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path
            .split('/')
            .any(|seg| seg.is_empty() || seg == "." || seg == "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_inside_the_assets_folder() {
        let assets: EmbeddedAssets = [
            ("./img/logo.png", &b"a"[..]),
            ("fonts\\Inter.ttf", &b"b"[..]),
            ("../secret", &b"c"[..]),
            ("/etc/passwd", &b"d"[..]),
            ("img//x.png", &b"e"[..]),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            assets.names().collect::<Vec<_>>(),
            ["fonts/Inter.ttf", "img/logo.png"]
        );
        assert_eq!(assets.get("img/logo.png"), Some(&b"a"[..]));
        assert_eq!(assets.len(), 2);
        assert_eq!(
            format!("{assets:?}"),
            r#"{"fonts/Inter.ttf": 1, "img/logo.png": 1}"#
        );
    }

    #[test]
    fn clones_share_until_changed() {
        let a = EmbeddedAssets::new().file("a.txt", &b"1"[..]);
        let b = a.clone().file("a.txt", &b"2"[..]);
        assert_eq!(
            (a.get("a.txt"), b.get("a.txt")),
            (Some(&b"1"[..]), Some(&b"2"[..]))
        );
        let mut c = EmbeddedAssets::new().file("c.txt", &b"3"[..]);
        c.merge(&b);
        c.merge(&EmbeddedAssets::new());
        assert_eq!(c.names().collect::<Vec<_>>(), ["a.txt", "c.txt"]);
        let mut d = EmbeddedAssets::new();
        d.merge(&a);
        assert_eq!(d, a);
    }
}
