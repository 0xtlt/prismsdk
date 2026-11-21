//! Files dragged from Explorer (or any app) onto `on_drop_files` targets
//! (#111), as on macOS and GTK: XAML's own drop target on the node's outer
//! element (`AllowDrop`, OLE underneath).
//!
//! When a drag enters, its files are read (`GetStorageItemsAsync`, under
//! the event's deferral): only a drag holding a file the target accepts
//! (its MIME types or extensions, `PickFiles::accepts`) gets the copy
//! cursor and the `drop_hover` state, like `draggingEntered:` on macOS; a
//! folder never counts. On the drop, the accepted files are copied off the
//! UI thread to the app's `<data>\picked`, with the files it picks (the
//! originals stay where the user keeps them), then reported
//! (`UiEvent::DropFiles`).
//!
//! XAML's drag events are routed: a child's `DragEnter` and `DragLeave`
//! bubble up to the target while the drag stays over it. A leave counts
//! only once the pointer is outside the target's bounds, and an enter
//! while the files are known (or being read) reuses that answer.
//!
//! WinRT completions may come on another thread: they come back to the UI
//! thread through its dispatcher queue, where the event's arguments and
//! deferral wait (they are not agile).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use prism_core::{Interaction, NodeId, PickFiles, PickedFile, UiEvent};
use windows::ApplicationModel::DataTransfer::{DataPackageOperation, StandardDataFormats};
use windows::Storage::{IStorageItem, StorageItemTypes};
use windows_collections::IVectorView;
use windows_core::{Interface, Result};
use windows_future::{AsyncOperationCompletedHandler, IAsyncOperation};

use crate::imp::{defer, dispatch};
use crate::win::{env, log};
use crate::winui::bindings::Microsoft::UI::Dispatching::{DispatcherQueue, DispatcherQueueHandler};
use crate::winui::xaml::Controls::Canvas;
use crate::winui::xaml::{
    DragEventArgs, DragEventHandler, DragOperationDeferral, FrameworkElement,
};

/// Where dropped files are copied: the app's `<data>\picked`, where
/// picked files go too (`pickers::copy_picked`). The app owns the copies,
/// as on the other backends. Not `%TEMP%`: nothing would remove them
/// there.
static FOLDER: OnceLock<PathBuf> = OnceLock::new();

/// Sets [`FOLDER`], once, before the window takes drops.
pub fn set_folder(dir: PathBuf) {
    let _ = FOLDER.set(dir);
}

/// A node taking files.
#[derive(Default)]
struct Target {
    accept: Option<PickFiles>,
    wired: bool,
    /// The drag over it holds a file it takes (`None`: not known yet).
    over: Option<bool>,
    /// Its files are being read.
    checking: bool,
    /// Counts the drags that entered: an answer for an older one is
    /// dropped.
    entered: u32,
}

/// An enter waiting for the files to be read.
type Waiting = (DragEventArgs, DragOperationDeferral);

thread_local! {
    static TARGETS: RefCell<HashMap<NodeId, Target>> = RefCell::new(HashMap::new());
    /// The entering drag's arguments and deferrals (its enters, bubbled
    /// from the children too), while its files are read.
    static PENDING: RefCell<HashMap<(NodeId, u32), Vec<Waiting>>> = RefCell::new(HashMap::new());
}

fn with<R>(id: NodeId, f: impl FnOnce(&mut Target) -> R) -> R {
    TARGETS.with(|t| f(t.borrow_mut().entry(id).or_default()))
}

/// Whether node `id` takes files (its element must be hit-testable).
pub fn accepts(id: NodeId) -> bool {
    TARGETS.with(|t| t.borrow().get(&id).is_some_and(|t| t.accept.is_some()))
}

/// The node is gone.
pub fn forget(id: NodeId) {
    TARGETS.with(|t| t.borrow_mut().remove(&id));
    PENDING.with(|p| p.borrow_mut().retain(|(n, _), _| *n != id));
}

/// The MIME type Prism gives a file named `name`.
fn mime_of(name: &str) -> String {
    prism_core::SaveFile::path(std::path::Path::new(name)).mime_type()
}

/// Whether the dragged `items` (name, is a file) hold one `accept` takes.
fn takes(accept: &PickFiles, items: &[(String, bool)]) -> bool {
    items
        .iter()
        .any(|(name, file)| *file && accept.accepts(name, &mime_of(name)))
}

fn trace(what: impl FnOnce() -> String) {
    if env("PRISM_TRACE_INPUT").is_some() {
        log!("input: {}", what());
    }
}

/// The drag's data holds files.
fn has_files(a: &DragEventArgs) -> bool {
    a.DataView()
        .and_then(|v| v.Contains(&StandardDataFormats::StorageItems()?))
        .unwrap_or(false)
}

/// The dragged items, once read (off the UI thread maybe).
type Items = IAsyncOperation<IVectorView<IStorageItem>>;

fn items_of(op: &Items) -> Result<Vec<(String, String, bool)>> {
    let items = op.GetResults()?;
    let mut out = Vec::new();
    for i in 0..items.Size()? {
        let item: IStorageItem = items.GetAt(i)?;
        out.push((
            item.Name()?.to_string(),
            item.Path()?.to_string(),
            item.IsOfType(StorageItemTypes::File)?,
        ));
    }
    Ok(out)
}

/// Node `id` takes files of `types` dropped from Explorer (`None`: none).
pub fn set_target(outer: &Canvas, id: NodeId, types: Option<&[String]>) -> Result<()> {
    let accept = types.map(|t| PickFiles::new().types(t.to_vec()));
    let wire = with(id, |t| {
        t.accept = accept.clone();
        !std::mem::replace(&mut t.wired, true)
    });
    outer.SetAllowDrop(accept.is_some())?;
    if !wire {
        return Ok(());
    }
    outer.DragEnter(&DragEventHandler::new(move |_, a| {
        let Some(a) = a.as_ref() else { return Ok(()) };
        if let Some(accept) = with(id, |t| t.accept.clone())
            && has_files(a)
        {
            entered(id, a, accept)?;
        }
        Ok(())
    }))?;
    outer.DragOver(&DragEventHandler::new(move |_, a| {
        if let Some(a) = a.as_ref()
            && let Some(ok) = with(id, |t| t.accept.as_ref().and(t.over))
        {
            a.SetAcceptedOperation(if ok {
                DataPackageOperation::Copy
            } else {
                DataPackageOperation::None
            })?;
        }
        Ok(())
    }))?;
    outer.DragLeave(&DragEventHandler::new(move |s, a| {
        // Bubbled from a child the drag left for another: still over it.
        if let (Some(s), Some(a)) = (s.as_ref(), a.as_ref())
            && let Ok(e) = s.cast::<FrameworkElement>()
            && inside(&e, a)
        {
            return Ok(());
        }
        with(id, |t| {
            t.over = None;
            t.checking = false;
            t.entered += 1;
        });
        hover(id, false);
        Ok(())
    }))?;
    outer.Drop(&DragEventHandler::new(move |_, a| {
        let over = with(id, |t| {
            t.entered += 1;
            t.checking = false;
            t.over.take()
        });
        hover(id, false);
        let Some(a) = a.as_ref() else { return Ok(()) };
        let Some(accept) = with(id, |t| t.accept.clone()) else {
            return Ok(());
        };
        if over == Some(false) || !has_files(a) {
            return Ok(());
        }
        a.SetHandled(true)?;
        dropped(id, a, accept)
    }))?;
    Ok(())
}

/// The drag's pointer is inside element `e`.
fn inside(e: &FrameworkElement, a: &DragEventArgs) -> bool {
    let Ok(el) = e.cast::<crate::winui::xaml::UIElement>() else {
        return false;
    };
    let (Ok(p), Ok(w), Ok(h)) = (a.GetPosition(&el), e.ActualWidth(), e.ActualHeight()) else {
        return false;
    };
    p.X >= 0. && p.Y >= 0. && (p.X as f64) < w && (p.Y as f64) < h
}

/// A drag holding files entered node `id` (or a child of it): its files
/// are read under the event's deferral, then it gets the copy cursor and
/// `drop_hover`, or not.
fn entered(id: NodeId, a: &DragEventArgs, accept: PickFiles) -> Result<()> {
    let (known, checking, n) = with(id, |t| (t.over, t.checking, t.entered));
    if let Some(ok) = known {
        a.SetAcceptedOperation(if ok {
            DataPackageOperation::Copy
        } else {
            DataPackageOperation::None
        })?;
        return Ok(());
    }
    let deferral = a.GetDeferral()?;
    if checking {
        PENDING.with(|p| {
            p.borrow_mut()
                .entry((id, n))
                .or_default()
                .push((a.clone(), deferral))
        });
        return Ok(());
    }
    let n = with(id, |t| {
        t.entered += 1;
        t.checking = true;
        t.entered
    });
    PENDING.with(|p| p.borrow_mut().insert((id, n), vec![(a.clone(), deferral)]));
    let op = a.DataView()?.GetStorageItemsAsync()?;
    let queue = DispatcherQueue::GetForCurrentThread()?;
    op.SetCompleted(&AsyncOperationCompletedHandler::new(move |op, _| {
        let names: Vec<(String, bool)> = op
            .as_ref()
            .and_then(|op| items_of(op).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|(name, _, file)| (name, file))
            .collect();
        let ok = takes(&accept, &names);
        let _ = queue.TryEnqueue(&DispatcherQueueHandler::new(move || {
            answer(id, n, ok, &names);
            Ok(())
        }));
        Ok(())
    }))?;
    Ok(())
}

/// The files of drag `n` over node `id` were read: `ok` if one is taken.
fn answer(id: NodeId, n: u32, ok: bool, names: &[(String, bool)]) {
    let waiting = PENDING.with(|p| p.borrow_mut().remove(&(id, n)).unwrap_or_default());
    let current = with(id, |t| {
        let current = t.entered == n && t.checking;
        if current {
            t.over = Some(ok);
            t.checking = false;
        }
        current
    });
    trace(|| {
        let names: Vec<&str> = names.iter().map(|(n, _)| n.as_str()).collect();
        format!(
            "files over node {id}: {names:?}, {}",
            if ok { "taken" } else { "refused" }
        )
    });
    for (args, deferral) in waiting {
        let _ = args.SetAcceptedOperation(if ok && current {
            DataPackageOperation::Copy
        } else {
            DataPackageOperation::None
        });
        let _ = deferral.Complete();
    }
    if current {
        hover(id, ok);
    }
}

/// Files were dropped on node `id`: the accepted ones are copied off the
/// UI thread, then reported.
fn dropped(id: NodeId, a: &DragEventArgs, accept: PickFiles) -> Result<()> {
    let op = a.DataView()?.GetStorageItemsAsync()?;
    let queue = DispatcherQueue::GetForCurrentThread()?;
    op.SetCompleted(&AsyncOperationCompletedHandler::new(move |op, _| {
        let Some(op) = op.as_ref() else { return Ok(()) };
        let paths: Vec<String> = items_of(op)?
            .into_iter()
            .filter(|(name, _, file)| *file && accept.accepts(name, &mime_of(name)))
            .map(|(_, path, _)| path)
            .collect();
        let queue = queue.clone();
        let Some(dir) = FOLDER.get() else {
            return Ok(());
        };
        std::thread::spawn(move || {
            let files: Vec<PickedFile> = paths
                .into_iter()
                .filter_map(|p| copy_dropped(Path::new(&p), dir))
                .collect();
            if files.is_empty() {
                return;
            }
            let _ = queue.TryEnqueue(&DispatcherQueueHandler::new(move || {
                let e = UiEvent::DropFiles(id, files.clone());
                trace(|| format!("{e:?}"));
                dispatch(e);
                Ok(())
            }));
        });
        Ok(())
    }))?;
    Ok(())
}

/// Accepted files are over node `id` (or not anymore): `DropHover`.
fn hover(id: NodeId, on: bool) {
    defer(Box::new(move |s| {
        if s.renderer.set_interaction(id, Interaction::DropHover, on) {
            crate::imp::relayout(s);
        }
    }));
}

/// A copy of a dropped file in `dir`, which the app may keep: the original
/// stays where the user has it.
fn copy_dropped(src: &Path, dir: &Path) -> Option<PickedFile> {
    if !src.is_file() {
        return None;
    }
    std::fs::create_dir_all(dir).ok()?;
    let name = src.file_name()?.to_string_lossy().to_string();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dest = dir.join(format!("{unique}-{name}"));
    let size = std::fs::copy(src, &dest).ok()?;
    Some(PickedFile {
        path: dest.to_string_lossy().to_string(),
        mime: mime_of(&name),
        name,
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dropped file is copied to the folder given (the app's
    /// `<data>\picked`), never to the temp directory, under a name of its
    /// own; the original stays.
    #[test]
    fn dropped_files_are_copied_where_picked_ones_go() {
        let dir = crate::temp_dir::TempDir::new("dropped");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("Report Q3.pdf");
        std::fs::write(&src, b"%PDF").unwrap();
        let picked = dir.join("data").join("picked");
        let a = copy_dropped(&src, &picked).unwrap();
        assert_eq!(a.name, "Report Q3.pdf");
        assert_eq!(a.size, 4);
        assert_eq!(Path::new(&a.path).parent(), Some(picked.as_path()));
        assert_eq!(std::fs::read(&a.path).unwrap(), b"%PDF");
        assert!(src.exists(), "the original stays");
        // A folder is not copied.
        assert!(copy_dropped(&picked, &picked).is_none());
    }
}
