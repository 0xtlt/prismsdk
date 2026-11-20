//! `apple/generate_widgets.py`: the WidgetKit entry point generated from an
//! app's `PRISM_DESCRIBE=widgets`. Skipped where `python3` is missing (or
//! is only Windows' Microsoft Store alias).

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Runs the script on `widgets`, in a directory of its own: the tests run in
/// parallel, and two calls for the same platform must not share a file (#123).
fn generate(widgets: &str, platform: &str) -> Option<String> {
    if !has_python3() {
        eprintln!("python3 not found: skipped");
        return None;
    }
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

/// A working `python3`. On Windows, the Microsoft Store's `python3.exe`
/// alias only says that Python is missing, and fails.
fn has_python3() -> bool {
    Command::new("python3")
        .args(["-c", ""])
        .output()
        .is_ok_and(|o| o.status.success())
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

/// #279: a timer is aligned like a text. `Text(timerInterval:)` takes the
/// width it is offered and lays its digits out by its own alignment, so
/// `"ta"` must reach it; frames wider than a text or a timer place it by
/// `"ta"` too, not at the leading edge.
#[test]
fn a_timer_follows_the_text_alignment() {
    let apple = Path::new(env!("CARGO_MANIFEST_DIR")).join("apple");
    let renderer = std::fs::read_to_string(apple.join("Extension/PrismRenderer.swift")).unwrap();
    let timer = renderer
        .split(r#"case "m":"#)
        .nth(1)
        .and_then(|s| s.split("case \"p\":").next())
        .expect("the timer case");
    assert!(timer.contains("PrismTimerText(end: end"), "{timer}");
    assert!(renderer.contains("Text(timerInterval: now...end, countsDown: true)"));
    assert!(
        timer.contains(".multilineTextAlignment(prismTextAlign(n))"),
        "{timer}"
    );
    // Stretched by a column, filling the width, cut on one line.
    assert!(!renderer.contains("alignment: .leading)"), "{renderer}");
    assert_eq!(renderer.matches("alignment: prismFrameAlign(").count(), 3);
    assert!(renderer.contains(r#"guard kind == "t" || kind == "m" else { return .leading }"#));

    let watch = std::fs::read_to_string(apple.join("Watch/PrismWatch.swift")).unwrap();
    let timer = watch
        .split(r#"case "timer":"#)
        .find(|s| s.trim_start().starts_with("// Aligned like a text"))
        .and_then(|s| s.split("case \"picture\":").next())
        .expect("the watch timer case");
    assert!(timer.contains("PrismWatchTimer(end:"), "{timer}");
    assert!(watch.contains("Text(end, style: .timer)"));
    assert!(
        timer.contains(".multilineTextAlignment(textAlign(n.str(\"talign\")))")
            && timer
                .contains(".frame(maxWidth: .infinity, alignment: frameAlign(n.str(\"talign\")))"),
        "{timer}"
    );
}

/// #281: an ended timer shows its `after` text (`"ph"`). A widget is drawn
/// again when such a timer ends (an entry at its end), and its timers compare
/// their end with the entry's instant, not with the moment WidgetKit draws it
/// (ahead of time). The watch host draws a timer again at its end.
#[test]
fn an_ended_timer_shows_its_after_text() {
    let apple = Path::new(env!("CARGO_MANIFEST_DIR")).join("apple");
    let renderer = std::fs::read_to_string(apple.join("Extension/PrismRenderer.swift")).unwrap();
    assert!(renderer.contains(r#"PrismTimerText(end: end, after: n["ph"] as? String)"#));
    let view = renderer
        .split("struct PrismTimerText: View {")
        .nth(1)
        .and_then(|s| s.split("\n}\n").next())
        .expect("PrismTimerText");
    assert!(view.contains("let now = entryDate ?? .now"), "{view}");
    assert!(view.contains("if end > now {"), "{view}");
    assert!(
        view.contains(r#"Text(after.flatMap { $0.isEmpty ? nil : $0 } ?? "0:00")"#),
        "{view}"
    );

    let kit = std::fs::read_to_string(apple.join("Extension/PrismWidgetKit.swift")).unwrap();
    assert!(kit.contains(".environment(\\.prismEntryDate, entry.date)"));
    // Both providers build their timeline from the snapshot's timer ends.
    assert_eq!(kit.matches("prismTimeline(").count(), 3, "{kit}");
    assert!(kit.contains(
        "for end in prismTimerEnds(drawing.doc, after: drawing.from) where end < next {"
    ));
    assert!(kit.contains(
        r#"if n["k"] as? String == "m", let after = n["ph"] as? String, !after.isEmpty,"#
    ));
    assert!(
        !kit.contains("Timeline(entries: [PrismEntry(date: .now"),
        "one entry only"
    );

    let watch = std::fs::read_to_string(apple.join("Watch/PrismWatch.swift")).unwrap();
    assert!(watch.contains(
        "PrismWatchTimer(end: Date(timeIntervalSince1970: n.value), after: n.placeholder)"
    ));
    assert!(watch.contains("TimelineView(.explicit([end]))"));
}

/// #281: a widget drawn ahead (`"ahead"`) gets one timeline entry per
/// drawing, from its instant, so WidgetKit switches drawings without the
/// app. `.atEnd` while drawings are left, `.never` for a single one; a
/// configuration's own drawings ahead replace the default's.
#[test]
fn drawings_ahead_become_timeline_entries() {
    let apple = Path::new(env!("CARGO_MANIFEST_DIR")).join("apple");
    let kit = std::fs::read_to_string(apple.join("Extension/PrismWidgetKit.swift")).unwrap();
    let body = |name: &str| {
        kit.split(&format!("func {name}("))
            .nth(1)
            .and_then(|s| s.split("\n}\n").next())
            .unwrap_or_else(|| panic!("{name}"))
            .to_string()
    };
    let timeline = body("prismTimeline");
    assert!(
        timeline.contains("let drawings = prismDrawings(doc, now: now)"),
        "{timeline}"
    );
    assert!(timeline.contains("entries.append(PrismEntry(date: drawing.from, doc: drawing.doc))"));
    assert!(
        timeline.contains("let ahead = drawings.count > 1 || more"),
        "{timeline}"
    );
    assert!(
        timeline.contains("policy: ahead ? .atEnd : .never"),
        "{timeline}"
    );

    let drawings = body("prismDrawings");
    assert!(drawings.contains(r#"guard var doc, let ahead = doc["ahead"] as? [PNode], !ahead.isEmpty else { return [(now, doc)] }"#));
    assert!(drawings.contains(r#"guard let at = (e["at"] as? NSNumber)?.doubleValue, let families = e["families"] else { continue }"#));
    // One whose instant has come replaces the drawing written.
    assert!(
        drawings.contains("if from <= now {\n            drawings[0] = (now, drawn)"),
        "{drawings}"
    );

    let configured = body("prismConfigured");
    assert!(
        configured.contains(r#"doc["ahead"] = config?["ahead"]"#),
        "{configured}"
    );
    // The system's placeholder and snapshot show the drawing of now too.
    assert_eq!(kit.matches("prismEntryNow(").count(), 4, "{kit}");
    assert!(
        !kit.contains("PrismEntry(date: .now, doc: prismLoadWidget"),
        "{kit}"
    );
}
