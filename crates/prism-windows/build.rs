//! The WinUI 3 projection, generated from the Windows App SDK's metadata
//! when building for Windows:
//!
//! 1. the SDK's component packages are fetched into a cache once
//!    (`prism_build::winappsdk`: `PRISM_WINAPPSDK_DIR`, else
//!    `<target dir>/winappsdk/<version>`);
//! 2. `windows-bindgen` writes `$OUT_DIR/winui.rs` (included by
//!    `src/winui.rs`): only the types in `winui-types.txt` and
//!    `winui-types-embedded.txt` (web views, videos, rich text, input),
//!    against the `windows` crate.
//!
//! The runtime next to the app and the manifest in it come from the app's
//! own build script (`prism_build::link_app()`): Cargo applies link
//! arguments only to the package that prints them.
//!
//! Nothing happens for other targets: the crate's pure helpers build and
//! test everywhere without the SDK.

use std::fs;
use std::path::Path;

use prism_build::winappsdk;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=winui-types.txt");
    println!("cargo:rerun-if-changed=winui-types-embedded.txt");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let sdk = winappsdk::sdk_dir();
    winappsdk::ensure(&sdk);
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    bindings(&sdk, &out.join("winui.rs"));
    println!(
        "cargo:rustc-env=PRISM_WINAPPSDK_VERSION={}",
        winappsdk::VERSION
    );
}

/// The WinUI projection, from the packages' metadata.
fn bindings(sdk: &Path, out: &Path) {
    let types = [
        include_str!("winui-types.txt"),
        include_str!("winui-types-embedded.txt"),
    ]
    .join("\n");
    let mut args: Vec<String> = vec!["--in".into(), "default".into()];
    for p in [
        "winui/metadata/Microsoft.UI.Xaml.winmd",
        "winui/metadata/Microsoft.UI.Text.winmd",
        "interactiveexperiences/metadata/10.0.18362.0/Microsoft.UI.winmd",
        "interactiveexperiences/metadata/10.0.18362.0/Microsoft.Foundation.winmd",
        "interactiveexperiences/metadata/10.0.18362.0/Microsoft.Graphics.winmd",
        // WinUI's WebView2 control speaks CoreWebView2.
        "webview2/lib/Microsoft.Web.WebView2.Core.winmd",
    ] {
        args.push(sdk.join(p).display().to_string());
    }
    // The Foundation types WinUI refers to (resources, app lifecycle…).
    let mut foundation: Vec<String> = fs::read_dir(sdk.join("foundation/metadata"))
        .expect("foundation metadata")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "winmd"))
        .map(|p| p.display().to_string())
        .collect();
    foundation.sort();
    args.extend(foundation);
    args.extend(["--out".into(), out.display().to_string(), "--filter".into()]);
    args.extend(
        types
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(String::from),
    );
    // Windows.* types come from the `windows` crate, except Windows.UI.Xaml
    // (system XAML, not in it), whose few types WinUI still uses
    // (`TypeName`) are generated here.
    for ns in [
        "Windows.ApplicationModel",
        "Windows.Foundation",
        "Windows.Graphics",
        "Windows.Media",
        "Windows.Storage",
        "Windows.System",
        "Windows.UI.Color",
        "Windows.UI.Colors",
        "Windows.UI.Composition",
        "Windows.UI.Core",
        "Windows.UI.Text",
    ] {
        args.extend(["--reference".into(), format!("windows,skip-root,{ns}")]);
    }
    args.push("--no-comment".into());
    // Methods whose types are not in the list are left out: fine.
    let _warnings = windows_bindgen::bindgen(args);
    let code = fs::read_to_string(out).expect("bindings");
    // Types the small crates (windows-collections, windows-numerics) do not
    // have: the windows crate has the collections, `src/winui.rs` the
    // quaternion.
    let code = code
        .replace(
            "windows_collections::IPropertySet",
            "windows::Foundation::Collections::IPropertySet",
        )
        .replace(
            "windows_collections::IObservableVector",
            "windows::Foundation::Collections::IObservableVector",
        )
        .replace(
            "windows_collections::VectorChangedEventHandler",
            "windows::Foundation::Collections::VectorChangedEventHandler",
        )
        .replace("windows_numerics::Quaternion", "crate::winui::Quaternion");
    // `include!` takes items: the inner `#![allow(…)]` goes (src/winui.rs
    // has its own).
    let code = match code.strip_prefix("#![allow(") {
        Some(rest) => rest.split_once(")]").map_or(rest, |(_, r)| r).to_string(),
        None => code,
    };
    fs::write(out, code).expect("bindings");
}
