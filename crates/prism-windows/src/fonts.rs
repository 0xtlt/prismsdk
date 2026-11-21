//! Run-time fonts (`cx.load_font`): XAML loads font files only by URI, and
//! an unpackaged app's `ms-appx:///` is its executable's folder; it ignores
//! files elsewhere (absolute paths, `file:` URIs) and fonts added with
//! `AddFontResourceEx`.
//!
//! So XAML gets its resource manager from the app
//! (`Application::ResourceManagerRequested`): the default one (the
//! `resources.pri` next to the executable), with a fallback for what it
//! does not hold (`ResourceManager::ResourceNotFound`): a name under
//! `prism-fonts/` resolves to that file in the app's data folder
//! (`%LOCALAPPDATA%\<app>\prism-fonts`), always writable. The font is
//! written there (`<hash>.ttf`, named by its content, once) and texts in
//! its family name it `ms-appx:///prism-fonts/<hash>.ttf#Family`, for this
//! app only: nothing is installed, and the app's own folder may be
//! read-only (`C:\Program Files`). Texts are then styled and measured
//! again.
//!
//! Without the fallback (XAML asked for no resource manager), fonts go to
//! `prism-fonts\` next to the executable, which `ms-appx:///` names.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use prism_core::FontFace;
use windows::Foundation::TypedEventHandler;
use windows_core::{HSTRING, IInspectable, Interface, Result};

use crate::imp::State;
use crate::win::log;
use crate::winui::bindings::Microsoft::Windows::ApplicationModel::Resources::{
    IResourceManager, ResourceCandidate, ResourceCandidateKind, ResourceManager,
    ResourceNotFoundEventArgs,
};
use crate::winui::xaml::{Application, ResourceManagerRequestedEventArgs};

thread_local! {
    /// Family (lower case) → its XAML font source.
    static SOURCES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// The app's writable font folder (set at startup).
static DATA_FOLDER: OnceLock<PathBuf> = OnceLock::new();
/// XAML took the resource manager with the fallback.
static RESOLVING: AtomicBool = AtomicBool::new(false);

/// The XAML source of a run-time font's family, if it is one.
pub fn source(family: &str) -> Option<String> {
    SOURCES.with(|s| s.borrow().get(&family.to_lowercase()).cloned())
}

/// The app's data folder (`%LOCALAPPDATA%\<app>`): fonts go to its
/// `prism-fonts`.
pub fn set_data_dir(data: &Path) {
    let _ = DATA_FOLDER.set(data.join("prism-fonts"));
}

/// Gives XAML the resource manager that finds run-time fonts in the data
/// folder. Before XAML's first resource lookup (`host::start`).
pub fn install_resolver(app: &Application) -> Result<()> {
    app.ResourceManagerRequested(&TypedEventHandler::<
        IInspectable,
        ResourceManagerRequestedEventArgs,
    >::new(|_, args| {
        let Some(args) = args.as_ref() else {
            return Ok(());
        };
        let manager = ResourceManager::new()?;
        manager.ResourceNotFound(&TypedEventHandler::<
            ResourceManager,
            ResourceNotFoundEventArgs,
        >::new(|_, e| {
            let Some(e) = e.as_ref() else { return Ok(()) };
            if let Some(path) = resolve(&e.Name()?.to_string()) {
                let found = ResourceCandidate::CreateInstance(
                    ResourceCandidateKind::FilePath,
                    &HSTRING::from(path.as_os_str()),
                )?;
                e.SetResolvedCandidate(&found)?;
            }
            Ok(())
        }))?;
        args.SetCustomResourceManager(&manager.cast::<IResourceManager>()?)?;
        RESOLVING.store(true, Ordering::Relaxed);
        Ok(())
    }))?;
    Ok(())
}

/// The file a resource name under `prism-fonts/` (`Files/prism-fonts/<hash>.ttf`)
/// stands for, if it is in the data folder.
fn resolve(name: &str) -> Option<PathBuf> {
    let file = name.replace('\\', "/");
    let file = file.rsplit_once("prism-fonts/")?.1;
    if file.is_empty() || file.contains(['/', ':']) || file.starts_with('.') {
        return None;
    }
    let path = DATA_FOLDER.get()?.join(file);
    path.is_file().then_some(path)
}

/// Where fonts go: the data folder when XAML resolves them there, else
/// `<exe dir>\prism-fonts` (what `ms-appx:///prism-fonts/` names).
fn folder() -> Option<PathBuf> {
    if RESOLVING.load(Ordering::Relaxed)
        && let Some(dir) = DATA_FOLDER.get()
    {
        return Some(dir.clone());
    }
    Some(std::env::current_exe().ok()?.parent()?.join("prism-fonts"))
}

pub fn load(s: &mut State, face: FontFace) {
    let w = &s.renderer.widgets;
    let sources = w.images.sources.clone();
    let requested = face.family.clone();
    // A URL comes back here with its bytes once loaded.
    let Some(font) = prism_native::fonts::prepare(&mut s.runtime.app, face, &sources) else {
        return;
    };
    let Some(dir) = folder() else { return };
    let Some(path) = prism_native::fonts::save(&font, &dir) else {
        log!(
            "font {requested:?}: cannot write it under {}",
            dir.display()
        );
        return;
    };
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return;
    };
    let src = format!("ms-appx:///prism-fonts/{name}#{}", font.family);
    SOURCES.with(|m| {
        let mut m = m.borrow_mut();
        m.insert(font.family.to_lowercase(), src.clone());
        m.insert(requested.to_lowercase(), src);
    });
    // Every text again (styled and measured anew).
    s.renderer.restyle_all();
    crate::imp::relayout(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_only_plain_files_under_prism_fonts() {
        let dir = crate::temp_dir::TempDir::new("fonts-test");
        let fonts = dir.join("prism-fonts");
        std::fs::create_dir_all(&fonts).unwrap();
        std::fs::write(fonts.join("abc.ttf"), b"x").unwrap();
        set_data_dir(&dir);
        assert_eq!(
            resolve("Files/prism-fonts/abc.ttf"),
            Some(fonts.join("abc.ttf"))
        );
        assert_eq!(resolve("Files/prism-fonts/missing.ttf"), None);
        assert_eq!(resolve("Files/prism-fonts/../abc.ttf"), None);
        assert_eq!(resolve("Files/prism-fonts/sub/abc.ttf"), None);
        assert_eq!(resolve("Files/Assets/abc.ttf"), None);
    }

    #[test]
    fn a_failing_test_leaves_no_temp_folder() {
        let made = std::sync::Mutex::new(None);
        let failed = std::panic::catch_unwind(|| {
            let dir = crate::temp_dir::TempDir::new("guard");
            std::fs::create_dir_all(dir.join("data")).unwrap();
            std::fs::write(dir.join("data/file"), "x").unwrap();
            *made.lock().unwrap() = Some(dir.to_path_buf());
            panic!("a test that fails (on purpose)");
        });
        assert!(failed.is_err());
        let dir = made.lock().unwrap().take().expect("the folder was made");
        assert!(!dir.exists(), "{} is left behind", dir.display());
    }
}
