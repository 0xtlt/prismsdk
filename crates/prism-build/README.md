# prism-build

Build-time helpers for Prism apps: checked, typed translations from locales/*.json.

Build-time helpers for Prism apps. `translations` turns `locales/*.json` (one
file per language, CLDR plurals) into one typed function per key, checked at
build time: a missing key, an unknown placeholder or a plural without `other`
is a cargo warning (or an error with `.strict()`).

```rust,ignore
// build.rs
fn main() {
    prism_build::translations("locales").fallback("en").generate();
}
```

Part of [Prism](https://github.com/0xtlt/prism). Guide: [https://0xtlt.github.io/prism/](https://0xtlt.github.io/prism/) · API: [docs.rs/prism-build](https://docs.rs/prism-build)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
