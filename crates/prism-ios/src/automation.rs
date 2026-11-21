//! Development hooks, like the macOS backend's, for simulators that cannot
//! be driven from outside (tvOS) and for scripted checks on iOS:
//!
//! - `PRISM_CLICK="+1|Save"` presses buttons by title, or clickable views
//!   by accessibility label, in order;
//! - `PRISM_SNAPSHOT=/abs/out.png` then writes the window to a PNG and quits;
//! - `PRISM_A11Y_DUMP=/abs/tree.json` writes the visible view tree with what
//!   VoiceOver sees (label, traits as roles, title, text, frame) before the
//!   snapshot (and quits if there is none);
//! - `PRISM_DELAY` (seconds, default 1) before clicking, `PRISM_AFTER`
//!   (default 0.5) between the clicks and the snapshot.

use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, class, msg_send};
use objc2_foundation::{NSData, NSString};
use objc2_ui_kit::{UIButton, UIControlEvents, UIControlState, UIView};
use prism_core::{NodeId, UiEvent};

fn seconds(var: &str, default: f64) -> Duration {
    Duration::from_secs_f64(
        std::env::var(var)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default),
    )
}

fn after(delay: Duration, f: impl FnOnce() + Send + 'static) {
    let when = dispatch2::DispatchTime::try_from(delay).unwrap_or(dispatch2::DispatchTime::NOW);
    let _ = dispatch2::DispatchQueue::main().after(when, f);
}

/// Schedules the hooks if any is set.
pub(crate) fn schedule() {
    let snapshot = std::env::var("PRISM_SNAPSHOT").ok();
    let clicks = std::env::var("PRISM_CLICK").ok();
    let dump = std::env::var("PRISM_A11Y_DUMP").ok();
    if snapshot.is_none() && clicks.is_none() && dump.is_none() {
        return;
    }
    after(seconds("PRISM_DELAY", 1.), move || {
        let mtm = MainThreadMarker::new().expect("main thread");
        for title in clicks
            .iter()
            .flat_map(|c| c.split('|'))
            .filter(|t| !t.is_empty())
        {
            if !press(mtm, title) {
                eprintln!("prism-ios: PRISM_CLICK: nothing labelled {title:?}");
            }
        }
        if snapshot.is_none() && dump.is_none() {
            return;
        }
        after(seconds("PRISM_AFTER", 0.5), move || {
            let mtm = MainThreadMarker::new().expect("main thread");
            if let Some(path) = dump {
                write_dump(mtm, &path);
            }
            if let Some(path) = snapshot {
                match write_snapshot(mtm, &path) {
                    true => eprintln!("prism-ios: snapshot written to {path}"),
                    false => eprintln!("prism-ios: snapshot to {path} failed"),
                }
            }
            std::process::exit(0);
        });
    });
}

fn key_window(mtm: MainThreadMarker) -> Option<Retained<UIView>> {
    let app = objc2_ui_kit::UIApplication::sharedApplication(mtm);
    #[allow(deprecated)]
    let windows = app.windows();
    let window = windows
        .iter()
        .find(|w| w.isKeyWindow())
        .or_else(|| windows.firstObject())?;
    Some(Retained::into_super(window))
}

/// Presses the first visible button titled `title`, or clickable view
/// labelled `title`.
fn press(mtm: MainThreadMarker, title: &str) -> bool {
    let Some(window) = key_window(mtm) else {
        return false;
    };
    let Some(view) = find(&window, title) else {
        return false;
    };
    if let Ok(button) = view.clone().downcast::<UIButton>() {
        button.sendActionsForControlEvents(
            UIControlEvents::PrimaryActionTriggered | UIControlEvents::TouchUpInside,
        );
    } else {
        crate::imp::dispatch_event(UiEvent::Click(view.tag() as NodeId));
    }
    true
}

fn find(view: &UIView, title: &str) -> Option<Retained<UIView>> {
    for sub in view.subviews().iter() {
        if sub.isHidden() || sub.alpha() < 0.01 {
            continue;
        }
        if let Ok(b) = sub.clone().downcast::<UIButton>()
            && b.titleForState(UIControlState::Normal)
                .is_some_and(|t| t.to_string() == title)
        {
            return Some(sub.clone());
        }
        let label: Option<Retained<NSString>> = unsafe { msg_send![&*sub, accessibilityLabel] };
        if sub.tag() > 0
            && !sub.gestureRecognizers().is_none_or(|g| g.is_empty())
            && label.is_some_and(|l| l.to_string() == title)
        {
            return Some(sub.clone());
        }
        if let Some(found) = find(&sub, title) {
            return Some(found);
        }
    }
    None
}

/// Draws the key window into a PNG at `path`.
fn write_snapshot(mtm: MainThreadMarker, path: &str) -> bool {
    let Some(window) = key_window(mtm) else {
        return false;
    };
    let bounds = window.bounds();
    let target = window.clone();
    let draw = RcBlock::new(move |_context: *mut AnyObject| {
        let _: bool = unsafe {
            msg_send![&*target, drawViewHierarchyInRect: bounds, afterScreenUpdates: true]
        };
    });
    unsafe {
        let renderer: Retained<AnyObject> =
            msg_send![msg_send![class!(UIGraphicsImageRenderer), alloc], initWithBounds: bounds];
        let png: Retained<NSData> = msg_send![&*renderer, PNGDataWithActions: &*draw];
        png.writeToFile_atomically(&NSString::from_str(path), true)
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
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

/// The macOS-style role of these accessibility traits (`AXButton`,
/// `AXLink`, `AXHeading`, `AXStaticText`), if any.
fn role_of(traits: objc2_ui_kit::UIAccessibilityTraits) -> Option<String> {
    use objc2_ui_kit::{
        UIAccessibilityTraitButton, UIAccessibilityTraitHeader, UIAccessibilityTraitLink,
        UIAccessibilityTraitStaticText,
    };
    let (button, link, header, text) = unsafe {
        (
            UIAccessibilityTraitButton,
            UIAccessibilityTraitLink,
            UIAccessibilityTraitHeader,
            UIAccessibilityTraitStaticText,
        )
    };
    let role = if traits & button != 0 {
        "AXButton"
    } else if traits & link != 0 {
        "AXLink"
    } else if traits & header != 0 {
        "AXHeading"
    } else if traits & text != 0 {
        "AXStaticText"
    } else {
        return None;
    };
    Some(role.into())
}

/// One view and its visible subviews as JSON, in the macOS dump's shape
/// (`role`: `AXButton`, `AXHeading`… from the accessibility traits).
fn dump_node(mtm: MainThreadMarker, view: &UIView) -> String {
    use objc2_ui_kit::{
        NSObjectUIAccessibility, UIAccessibilityTraitSelected, UILabel, UITabBar, UITextField,
    };
    let class = view.class().name().to_str().unwrap_or("?").to_string();
    let traits = view.accessibilityTraits(mtm);
    let selected = unsafe { UIAccessibilityTraitSelected };
    let role = role_of(traits);
    let label = view.accessibilityLabel(mtm).map(|l| l.to_string());
    let title = view
        .downcast_ref::<UIButton>()
        .and_then(|b| b.titleForState(UIControlState::Normal))
        .map(|t| t.to_string());
    let value = view
        .downcast_ref::<UILabel>()
        .and_then(|l| l.text())
        .or_else(|| view.downcast_ref::<UITextField>().and_then(|f| f.text()))
        .map(|t| t.to_string());
    // Tab bars: their items are not views (iOS 26 draws them in a platter).
    let segments = view.downcast_ref::<UITabBar>().map(|bar| {
        let items = bar.items().map(|i| i.to_vec()).unwrap_or_default();
        let selected = bar.selectedItem();
        let labels: Vec<String> = items
            .iter()
            .map(|i| opt(i.title().map(|t| t.to_string())))
            .collect();
        let index = items
            .iter()
            .position(|i| selected.as_deref().is_some_and(|s| std::ptr::eq(s, &**i)))
            .map_or(-1, |i| i as i64);
        format!(r#"{{"labels":[{}],"selected":{index}}}"#, labels.join(","))
    });
    // Tables (#284): `"table":{rows, columns, headers, answers}` (answers:
    // the element the table gives VoiceOver for each cell, row by row, as
    // `{class, element}`), `"cell":[row, column]`, and a cell holding a
    // control's `"parts"`: the elements VoiceOver reaches in it, each
    // `{class, role, label, cell}` (`cell`: the row and column it reports,
    // `null` for a view read as itself).
    let table = match crate::table::dump(mtm, view) {
        Some(crate::table::Dump::Table(rows, columns, headers, answers)) => {
            let headers: Vec<String> = headers.into_iter().map(opt).collect();
            let answers: Vec<String> = answers
                .into_iter()
                .map(|row| {
                    let row: Vec<String> = row
                        .iter()
                        .map(|a| {
                            format!(
                                r#"{{"class":{},"element":{}}}"#,
                                json_str(&a.class),
                                a.element
                            )
                        })
                        .collect();
                    format!("[{}]", row.join(","))
                })
                .collect();
            format!(
                r#","table":{{"rows":{rows},"columns":{columns},"headers":[{}],"answers":[{}]}}"#,
                headers.join(","),
                answers.join(",")
            )
        }
        Some(crate::table::Dump::Cell(r, c, parts)) => {
            let parts = parts.map_or_else(String::new, |parts| {
                let parts: Vec<String> = parts
                    .into_iter()
                    .map(|p| {
                        format!(
                            r#"{{"class":{},"role":{},"label":{},"cell":{}}}"#,
                            json_str(&p.class),
                            opt(role_of(p.traits)),
                            opt(p.label),
                            p.at.map_or("null".into(), |(r, c)| format!("[{r},{c}]"))
                        )
                    })
                    .collect();
                format!(r#","parts":[{}]"#, parts.join(","))
            });
            format!(r#","cell":[{r},{c}]{parts}"#)
        }
        None => String::new(),
    };
    let f = view.convertRect_toView(view.bounds(), None);
    let children: Vec<String> = view
        .subviews()
        .iter()
        .filter(|c| !c.isHidden() && c.alpha() >= 0.01)
        .map(|c| dump_node(mtm, &c))
        .collect();
    format!(
        r#"{{"class":{},"role":{},"label":{},"title":{},"value":{},"segments":{},"selected":{},"a11y_element":{}{table},"frame":[{},{},{},{}],"children":[{}]}}"#,
        json_str(&class),
        opt(role),
        opt(label),
        opt(title),
        opt(value),
        segments.unwrap_or_else(|| "null".into()),
        traits & selected != 0,
        view.isAccessibilityElement(mtm),
        f.origin.x.round(),
        f.origin.y.round(),
        f.size.width.round(),
        f.size.height.round(),
        children.join(",")
    )
}

/// Writes `{"root": <key window>, "sheets": []}` to `path`.
fn write_dump(mtm: MainThreadMarker, path: &str) {
    let Some(window) = key_window(mtm) else {
        eprintln!("prism-ios: no window to dump");
        return;
    };
    let json = format!(r#"{{"root":{},"sheets":[]}}"#, dump_node(mtm, &window));
    match std::fs::write(path, json) {
        Ok(()) => eprintln!("prism-ios: accessibility tree written to {path}"),
        Err(e) => eprintln!("prism-ios: cannot write {path}: {e}"),
    }
}
