//! Dev hook: `PRISM_A11Y_DUMP=tree.json` writes the visible native view tree
//! with what assistive technologies see (role, label, value, title, frame in
//! window coordinates), for smoke tests (`scripts/macos-smoke.sh`).

use objc2::rc::Retained;
use objc2_app_kit::{NSAccessibility, NSButton, NSSegmentedControl, NSTextField, NSView};

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn opt(s: Option<String>) -> String {
    s.map_or("null".into(), |s| json_str(&s))
}

fn node(view: &NSView) -> String {
    let class = view.class().name().to_str().unwrap_or("?").to_string();
    let role = view.accessibilityRole().map(|r| r.to_string());
    let label = view.accessibilityLabel().map(|l| l.to_string());
    let title = view
        .downcast_ref::<NSButton>()
        .map(|b| b.title().to_string());
    let value = view
        .downcast_ref::<NSTextField>()
        .map(|t| t.stringValue().to_string());
    let segments = view.downcast_ref::<NSSegmentedControl>().map(|s| {
        let labels: Vec<String> = (0..s.segmentCount())
            .map(|i| opt(s.labelForSegment(i).map(|l| l.to_string())))
            .collect();
        format!(
            r#"{{"labels":[{}],"selected":{}}}"#,
            labels.join(","),
            s.selectedSegment()
        )
    });
    let f = view.convertRect_toView(view.bounds(), None);
    let children: Vec<String> = view
        .subviews()
        .iter()
        .filter(|c| !c.isHidden())
        .map(|c| node(&c))
        .collect();
    format!(
        r#"{{"class":{},"role":{},"label":{},"title":{},"value":{},"segments":{},"a11y_element":{},"frame":[{},{},{},{}],"children":[{}]}}"#,
        json_str(&class),
        opt(role),
        opt(label),
        opt(title),
        opt(value),
        segments.unwrap_or_else(|| "null".into()),
        view.isAccessibilityElement(),
        f.origin.x.round(),
        f.origin.y.round(),
        f.size.width.round(),
        f.size.height.round(),
        children.join(",")
    )
}

/// Writes `{"root": …, "sheets": [...]}` to `path`.
pub fn write(path: &str, root: &NSView, sheets: &[Retained<NSView>]) {
    let sheets: Vec<String> = sheets.iter().map(|s| node(s)).collect();
    let json = format!(
        r#"{{"root":{},"sheets":[{}]}}"#,
        node(root),
        sheets.join(",")
    );
    match std::fs::write(path, json) {
        Ok(()) => println!("prism-macos: accessibility tree written to {path}"),
        Err(e) => eprintln!("prism-macos: cannot write {path}: {e}"),
    }
}
