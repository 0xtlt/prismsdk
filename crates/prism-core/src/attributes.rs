//! App-defined attributes of an element on the web (`.attr(name, value)`):
//! what browser integrations and end-to-end tests read, such as
//! `data-1p-ignore` for password managers, `data-testid`, or
//! `autocomplete="one-time-code"`.
//!
//! Only names that cannot run code or change how Prism draws are taken:
//! `data-*` (except the ones the web backend uses itself) and
//! [`SAFE_ATTRIBUTES`]. Native backends ignore them.

/// The attributes outside `data-*` an app may set. Others are either
/// unsafe (`on*` handlers, `style`, `href`, `src`) or already derived from
/// Prism's own APIs: `inputmode` and `type` from the text input's keyboard,
/// `enterkeyhint` from its return key, `autocapitalize`, `autocorrect` and
/// `spellcheck` from its other settings, `aria-*` and `role` from
/// [`Accessible`](crate::Accessible), `id` from `.id(…)`.
pub const SAFE_ATTRIBUTES: &[&str] = &[
    // Autofill: `one-time-code`, `new-password`, `off`, `email`…
    "autocomplete",
    // `translate="no"` keeps browser translators off a name or a code.
    "translate",
];

/// `data-*` attributes the web backend sets itself (`data-<name>`):
/// refused, like every name starting with `data-prism`.
pub const RESERVED_DATA_ATTRIBUTES: &[&str] = &[
    "base",
    "bar",
    "clickable",
    "end",
    "group",
    "icon",
    "island",
    "keys",
    "kind",
    "link",
    "loaded",
    "medium",
    "open",
    "page",
    "path",
    "pid",
    "ps",
    "role",
    "row",
    "side",
    "size",
    "src",
    "value",
];

/// Whether an app may set the attribute `name` (lower case):
/// a `data-*` name made of `a-z`, `0-9`, `-`, `_` and `.`, not one of
/// [`RESERVED_DATA_ATTRIBUTES`] nor `data-prism…`, or one of
/// [`SAFE_ATTRIBUTES`].
pub fn is_app_attribute(name: &str) -> bool {
    if SAFE_ATTRIBUTES.contains(&name) {
        return true;
    }
    let Some(rest) = name.strip_prefix("data-") else {
        return false;
    };
    !rest.is_empty()
        && rest.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.')
        })
        && !rest.starts_with("prism")
        && !RESERVED_DATA_ATTRIBUTES.contains(&rest)
}
