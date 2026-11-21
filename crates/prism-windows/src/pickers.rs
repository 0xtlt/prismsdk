//! File, folder and photo pickers and the save dialog: the shell's common
//! item dialogs (`IFileOpenDialog`, `IFileSaveDialog`), modal to the app's
//! window, as every Windows app shows them.
//!
//! The dialogs run their own message loop: `services.rs` starts them from
//! the services window (`shell::later`), never while the app's state is in
//! use. Picked files are copied where the app keeps them (like the other
//! backends); a folder's files are read in place.

use std::path::{Path, PathBuf};

use prism_core::{DeviceError, PickedFile};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::System::Registry::{HKEY_CLASSES_ROOT, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOLDERID_Pictures, FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM,
    FOS_OVERWRITEPROMPT, FOS_PICKFOLDERS, FileOpenDialog, FileSaveDialog, IFileDialog,
    IFileOpenDialog, IFileSaveDialog, IShellItem, KF_FLAG_DEFAULT, SHGetKnownFolderItem,
    SIGDN_FILESYSPATH,
};
use windows::core::{HRESULT, HSTRING, PCWSTR};

use crate::win::wide;

/// The filters' names, in the app's language (`App::chrome_text`), as the
/// other backends' dialogs name theirs in the system's: Windows gives none
/// for a list of patterns.
#[derive(Clone, Debug)]
pub struct FilterNames {
    pub files: String,
    pub images: String,
    pub all_files: String,
}

impl FilterNames {
    pub fn of(app: &prism_core::App) -> FilterNames {
        FilterNames {
            files: app.chrome_text("files"),
            images: app.chrome_text("images"),
            all_files: app.chrome_text("all_files"),
        }
    }
}

impl Default for FilterNames {
    fn default() -> Self {
        FilterNames {
            files: "Files".into(),
            images: "Images".into(),
            all_files: "All files".into(),
        }
    }
}

/// What to pick.
pub enum Pick {
    /// Files of these types (MIME types or `.ext`), one or several.
    Files { types: Vec<String>, multiple: bool },
    /// Pictures, from the Pictures folder; `limit` 0: any number.
    Images { limit: u32 },
    /// A folder.
    Folder { title: Option<String> },
}

fn cancelled(e: &windows::core::Error) -> bool {
    e.code() == HRESULT::from_win32(ERROR_CANCELLED.0)
}

fn failed(e: windows::core::Error) -> DeviceError {
    if cancelled(&e) {
        DeviceError::Cancelled
    } else {
        DeviceError::Failed(e.message())
    }
}

/// Shows an open dialog over `owner` and returns the chosen paths.
pub fn open(owner: HWND, pick: &Pick, names: &FilterNames) -> Result<Vec<PathBuf>, DeviceError> {
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).map_err(failed)?;
        let mut options = dialog.GetOptions().map_err(failed)? | FOS_FORCEFILESYSTEM;
        // Kept alive while the dialog shows.
        let mut filters: Vec<(Vec<u16>, Vec<u16>)> = Vec::new();
        match pick {
            Pick::Files { types, multiple } => {
                options |= FOS_FILEMUSTEXIST;
                if *multiple {
                    options |= FOS_ALLOWMULTISELECT;
                }
                if let Some(spec) = filter_spec(types) {
                    let all_images = types.iter().all(|t| t.trim() == "image/*");
                    let name = if all_images {
                        &names.images
                    } else {
                        &names.files
                    };
                    filters.push((wide(&format!("{name} ({spec})")), wide(&spec)));
                }
            }
            Pick::Images { limit } => {
                options |= FOS_FILEMUSTEXIST;
                if *limit != 1 {
                    options |= FOS_ALLOWMULTISELECT;
                }
                let spec = patterns("image/*").join(";");
                filters.push((wide(&format!("{} ({spec})", names.images)), wide(&spec)));
                if let Ok(pictures) =
                    SHGetKnownFolderItem::<IShellItem>(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None)
                {
                    // Used unless the user picked elsewhere before.
                    let _ = dialog.SetDefaultFolder(&pictures);
                }
            }
            Pick::Folder { title } => {
                options |= FOS_PICKFOLDERS;
                if let Some(t) = title {
                    let _ = dialog.SetTitle(&HSTRING::from(t.as_str()));
                }
            }
        }
        dialog.SetOptions(options).map_err(failed)?;
        set_filters(&dialog, &filters)?;
        dialog.Show(Some(owner)).map_err(failed)?;
        let items = dialog.GetResults().map_err(failed)?;
        let mut paths = Vec::new();
        for i in 0..items.GetCount().map_err(failed)? {
            if let Ok(item) = items.GetItemAt(i)
                && let Some(p) = item_path(&item)
            {
                paths.push(p);
            }
        }
        if let Pick::Images { limit } = pick
            && *limit > 0
        {
            paths.truncate(*limit as usize);
        }
        Ok(paths)
    }
}

/// Shows a save dialog over `owner` for `name` (Windows asks before
/// replacing a file) and returns the chosen path.
pub fn save(owner: HWND, name: &str, names: &FilterNames) -> Result<PathBuf, DeviceError> {
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).map_err(failed)?;
        let options = dialog.GetOptions().map_err(failed)?;
        dialog
            .SetOptions(options | FOS_FORCEFILESYSTEM | FOS_OVERWRITEPROMPT)
            .map_err(failed)?;
        let mut filters = Vec::new();
        if let Some((_, ext)) = name.rsplit_once('.').filter(|(_, e)| !e.is_empty()) {
            let spec = format!("*.{ext}");
            filters.push((
                wide(&format!("{} ({spec})", ext.to_uppercase())),
                wide(&spec),
            ));
            let _ = dialog.SetDefaultExtension(&HSTRING::from(ext));
        }
        filters.push((wide(&format!("{} (*.*)", names.all_files)), wide("*.*")));
        set_filters(&dialog, &filters)?;
        dialog.SetFileName(&HSTRING::from(name)).map_err(failed)?;
        dialog.Show(Some(owner)).map_err(failed)?;
        let item = dialog.GetResult().map_err(failed)?;
        item_path(&item).ok_or_else(|| DeviceError::Failed("not a file on disk".into()))
    }
}

fn set_filters(dialog: &IFileDialog, filters: &[(Vec<u16>, Vec<u16>)]) -> Result<(), DeviceError> {
    if filters.is_empty() {
        return Ok(());
    }
    let specs: Vec<COMDLG_FILTERSPEC> = filters
        .iter()
        .map(|(name, spec)| COMDLG_FILTERSPEC {
            pszName: PCWSTR(name.as_ptr()),
            pszSpec: PCWSTR(spec.as_ptr()),
        })
        .collect();
    unsafe {
        dialog.SetFileTypes(&specs).map_err(failed)?;
        dialog.SetFileTypeIndex(1).map_err(failed)
    }
}

fn item_path(item: &IShellItem) -> Option<PathBuf> {
    unsafe {
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// The dialog's filter for Prism's types (`"image/*"`, `"application/pdf"`,
/// `".csv"`): `*.png;*.jpg;…`. `None`: any file.
pub fn filter_spec(types: &[String]) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for t in types {
        let t = t.trim();
        if t.is_empty() || t == "*/*" || t == "*" {
            return None;
        }
        for p in patterns(t) {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    (!out.is_empty()).then(|| out.join(";"))
}

/// The file patterns of one type.
fn patterns(t: &str) -> Vec<String> {
    if let Some(ext) = t.strip_prefix('.') {
        return vec![format!("*.{}", ext.to_ascii_lowercase())];
    }
    let t = t.to_ascii_lowercase();
    let exts: &[&str] = match t.as_str() {
        "image/*" => &[
            "png", "jpg", "jpeg", "gif", "bmp", "webp", "heic", "heif", "tif", "tiff", "ico",
            "avif", "svg",
        ],
        "video/*" => &["mp4", "m4v", "mov", "avi", "wmv", "mkv", "webm"],
        "audio/*" => &["mp3", "m4a", "wav", "wma", "flac", "aac", "ogg", "opus"],
        "text/*" => &[
            "txt", "csv", "md", "log", "html", "htm", "xml", "ics", "vcf",
        ],
        "image/jpeg" => &["jpg", "jpeg"],
        "image/tiff" => &["tif", "tiff"],
        "image/heic" | "image/heif" => &["heic", "heif"],
        "text/html" => &["html", "htm"],
        "text/plain" => &["txt", "log"],
        "text/markdown" => &["md", "markdown"],
        "application/pdf" => &["pdf"],
        "application/json" => &["json"],
        "application/zip" => &["zip"],
        "application/xml" | "text/xml" => &["xml"],
        "text/csv" => &["csv"],
        _ => &[],
    };
    if !exts.is_empty() {
        return exts.iter().map(|e| format!("*.{e}")).collect();
    }
    // `image/png` → `.png`; else the registry's extension for the type.
    if let Some(sub) = t.split_once('/').map(|(_, s)| s)
        && !sub.contains(['*', '.', '+', '-'])
        && t.starts_with("image/")
    {
        return vec![format!("*.{sub}")];
    }
    match registry_extension(&t) {
        Some(ext) => vec![format!("*{ext}")],
        // Unknown: let the user see every file rather than none.
        None => vec!["*.*".into()],
    }
}

fn reg_string(key: &str, value: &str) -> Option<String> {
    let (key, value) = (wide(key), wide(value));
    let mut buf = [0u16; 260];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CLASSES_ROOT,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if r.is_err() {
        return None;
    }
    let n = (size as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..n])).filter(|s| !s.is_empty())
}

/// `.pdf` for `application/pdf` (`HKCR\MIME\Database\Content Type`).
fn registry_extension(mime: &str) -> Option<String> {
    reg_string(
        &format!("MIME\\Database\\Content Type\\{mime}"),
        "Extension",
    )
}

/// A file's MIME type: the registry's `Content Type` for its extension,
/// else Prism's table.
pub fn mime_of(path: &Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !ext.is_empty()
        && let Some(m) = reg_string(&format!(".{ext}"), "Content Type")
    {
        return m;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    prism_core::SaveFile::bytes(Vec::new(), name).mime_type()
}

/// Copies a picked file where the app keeps them (`<data>\picked`), like
/// the other backends: the app owns the copy.
pub fn copy_picked(src: &Path, dir: &Path) -> Option<PickedFile> {
    std::fs::create_dir_all(dir).ok()?;
    let name = src.file_name()?.to_string_lossy().into_owned();
    let ext = src
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dest = dir.join(format!("{unique}{ext}"));
    let size = std::fs::copy(src, &dest).ok()?;
    Some(PickedFile {
        path: dest.to_string_lossy().into_owned(),
        name,
        mime: mime_of(src),
        size,
    })
}

/// A folder and its files (#225): the folder first
/// ([`prism_core::FOLDER_MIME`]), then every file under it, read in place
/// and named by its path in it; hidden ones left out.
pub fn folder_files(root: &Path) -> Vec<PickedFile> {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    let mut out = vec![PickedFile {
        path: root.to_string_lossy().into_owned(),
        name: root
            .file_name()
            .map_or_else(|| "folder".into(), |n| n.to_string_lossy().into_owned()),
        mime: prism_core::FOLDER_MIME.into(),
        size: 0,
    }];
    let mut todo = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = todo.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = read.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(meta) = e.metadata() else { continue };
            if name.starts_with('.') || meta.file_attributes() & HIDDEN != 0 {
                continue;
            }
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if meta.is_dir() {
                todo.push((e.path(), rel));
            } else if meta.is_file() {
                let path = e.path();
                out.push(PickedFile {
                    size: meta.len(),
                    mime: mime_of(&path),
                    path: path.to_string_lossy().into_owned(),
                    name: rel,
                });
            }
        }
    }
    out
}
