//! `apple/generate_widgets.py`: the WidgetKit entry point generated from an
//! app's `PRISM_DESCRIBE=widgets`. Skipped where `python3` is missing.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Runs the script on `widgets`, in a directory of its own: the tests run in
/// parallel, and two calls for the same platform must not share a file (#123).
fn generate(widgets: &str, platform: &str) -> Option<String> {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "prism-generate-widgets-{}-{}",
        std::process::id(),
        CALLS.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let json = dir.join("widgets.json");
    std::fs::write(&json, widgets).unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("apple/generate_widgets.py");
    let out = Command::new("python3")
        .arg(script)
        .arg(&json)
        .arg(platform)
        .output();
    std::fs::remove_dir_all(&dir).ok();
    let out = out.ok()?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(String::from_utf8(out.stdout).unwrap())
}

const CONFIGURABLE: &str = r#"[{"kind":"reading","name":"Reading","description":"A room",
  "families":["systemSmall","accessoryInline"],
  "parameters":[{"id":"place","title":"Place"}]}]"#;

/// #49: an optional parameter starts empty and the « Edit widget » sheet
/// shows its title; a required one is filled with the query's default.
#[test]
fn parameters_are_required_and_default_to_the_apps_default_option() {
    let Some(swift) = generate(CONFIGURABLE, "ios") else {
        return;
    };
    assert!(
        swift.contains(r#"@Parameter(title: "Place") var p0: PrismEntity_reading_0"#)
            && !swift.contains("var p0: PrismEntity_reading_0?"),
        "{swift}"
    );
    // Never nil, so the parameter is never left unset.
    assert!(
        swift.contains(
            r#"prismDefaultOption("reading", "place").map { PrismEntity_reading_0(id: $0.id, title: $0.title) } ?? PrismEntity_reading_0(id: "", title: "Place")"#
        ),
        "{swift}"
    );
    // Read without trapping, and the empty placeholder means « the default ».
    assert!(
        swift.contains(r#"if let e = prismValue($p0), !e.id.isEmpty { v["place"] = e.id }"#),
        "{swift}"
    );
    assert!(swift.contains("AppIntentConfiguration(kind: \"reading\""));
    assert!(swift.contains(".accessoryInline"), "lock screen on iOS");
}

/// #277: an instance whose option is gone, or that kept the empty
/// placeholder, is still resolved. Filtered out, the required parameter
/// would be missing and WidgetKit would never ask for a timeline.
#[test]
fn every_requested_id_is_answered_unknown_ones_included() {
    let Some(swift) = generate(CONFIGURABLE, "ios") else {
        return;
    };
    assert!(
        swift.contains(
            r#"func entities(for identifiers: [String]) async throws -> [PrismEntity_reading_0] {
        prismResolve("reading", "place", identifiers, placeholder: "Place").map { PrismEntity_reading_0(id: $0.id, title: $0.title) }"#
        ),
        "{swift}"
    );
    assert!(!swift.contains("identifiers.contains"), "{swift}");
    // One answer per id, an unknown one titled like the default option.
    let kit = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("apple/Extension/PrismWidgetKit.swift"),
    )
    .unwrap();
    assert!(
        kit.contains("return ids.map { id in p.options.first { $0.id == id } ?? (id, fallback) }"),
        "prismResolve answers every id"
    );
    assert!(
        kit.contains("let fallback = p.options.first { $0.id == p.default }?.title ?? placeholder")
    );
}

#[test]
fn macos_leaves_lock_screen_families_and_live_activities_out() {
    let Some(swift) = generate(CONFIGURABLE, "macos") else {
        return;
    };
    assert!(!swift.contains("accessoryInline"));
    assert!(!swift.contains("PrismLiveActivity()"));
    let Some(ios) = generate("[]", "ios") else {
        return;
    };
    assert!(ios.contains("PrismLiveActivity()"));
}

/// Widget buttons (#47) need nothing generated per kind: the intent is
/// shared (`Shared/PrismWidgetAction.swift`), compiled into the app and the
/// extension.
#[test]
fn the_widget_action_intent_is_shared_by_the_app_and_the_extension() {
    let apple = Path::new(env!("CARGO_MANIFEST_DIR")).join("apple");
    let shared = std::fs::read_to_string(apple.join("Shared/PrismWidgetAction.swift")).unwrap();
    // Run in the app's process (LiveActivityIntent), not in the extension.
    assert!(shared.contains("struct PrismWidgetActionIntent: LiveActivityIntent"));
    let renderer = std::fs::read_to_string(apple.join("Extension/PrismRenderer.swift")).unwrap();
    assert!(renderer.contains("Button(intent: intent)"));
    assert!(
        renderer.contains("import AppIntents"),
        "Button(intent:) comes from the AppIntents overlay"
    );
    let bridge = std::fs::read_to_string(apple.join("Bridge/PrismBridge.swift")).unwrap();
    assert!(bridge.contains("@_silgen_name(\"prism_widget_action\")"));
}
