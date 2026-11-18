//! The clipboard's content as several representations (#246): text, HTML,
//! an image, files, or the app's own type, side by side.
//!
//! ```ignore
//! cx.spawn(async move |this, cx| {
//!     let item = cx.read_clipboard_item().await;
//!     match item.as_ref().and_then(|i| i.as_image()) {
//!         Some(img) => upload(img.bytes, img.mime), // a screenshot: an image block
//!         None => { /* item.and_then(|i| i.as_text()) */ }
//!     }
//! });
//! cx.write_clipboard(
//!     ClipboardItem::new()
//!         .text("A caption")
//!         .image(png, "image/png")
//!         .custom("application/x-notes-block", json),
//! );
//! ```
//!
//! Types are MIME types (`text/plain`, `text/html`, `image/png`,
//! `text/uri-list` for files). Backends translate them to the platform's
//! own names (UTIs on Apple platforms) and back.

use crate::AsyncApp;
use crate::app::App;
use crate::device::{DeviceCall, DeviceValue, PickedFile};

/// What is on the clipboard (or goes on it): the same content in several
/// types, the richest first. A type set twice keeps its last data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipboardItem {
    entries: Vec<(String, Vec<u8>)>,
}

/// An image read from the clipboard: [`ClipboardItem::as_image`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardImage {
    /// The encoded image (PNG unless the clipboard held another format).
    pub bytes: Vec<u8>,
    /// Its MIME type (`image/png`).
    pub mime: String,
}

impl ClipboardItem {
    /// Plain text.
    pub const TEXT: &'static str = "text/plain";
    /// HTML.
    pub const HTML: &'static str = "text/html";
    /// Rich text (RTF).
    pub const RTF: &'static str = "text/rtf";
    /// A PNG image.
    pub const PNG: &'static str = "image/png";
    /// Files, one `file://` URL per line.
    pub const URI_LIST: &'static str = "text/uri-list";

    /// An empty item.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds (or replaces) data of type `ty`.
    pub fn custom(mut self, ty: impl Into<String>, data: impl Into<Vec<u8>>) -> Self {
        self.set(ty.into(), data.into());
        self
    }

    /// Adds the plain text (`text/plain`).
    pub fn text(self, text: impl Into<String>) -> Self {
        self.custom(Self::TEXT, text.into())
    }

    /// Adds HTML (`text/html`); give a [`text`](Self::text) too for apps
    /// that only paste text.
    pub fn html(self, html: impl Into<String>) -> Self {
        self.custom(Self::HTML, html.into())
    }

    /// Adds an encoded image (`image/png`, `image/jpeg`…).
    pub fn image(self, bytes: impl Into<Vec<u8>>, mime: impl Into<String>) -> Self {
        self.custom(mime, bytes)
    }

    fn set(&mut self, ty: String, data: Vec<u8>) {
        let ty = ty.trim().to_ascii_lowercase();
        match self.entries.iter_mut().find(|(t, _)| *t == ty) {
            Some(e) => e.1 = data,
            None => self.entries.push((ty, data)),
        }
    }

    /// The types it holds, in order.
    pub fn types(&self) -> Vec<&str> {
        self.entries.iter().map(|(t, _)| t.as_str()).collect()
    }

    /// Whether it holds data of type `ty`.
    pub fn has(&self, ty: &str) -> bool {
        self.get(ty).is_some()
    }

    /// The data of type `ty`.
    pub fn get(&self, ty: &str) -> Option<&[u8]> {
        let ty = ty.trim().to_ascii_lowercase();
        self.entries
            .iter()
            .find(|(t, _)| *t == ty)
            .map(|(_, d)| d.as_slice())
    }

    /// Every type with its data, in order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.entries.iter().map(|(t, d)| (t.as_str(), d.as_slice()))
    }

    /// Whether it holds nothing.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The data of type `ty` as text (UTF-8, invalid bytes replaced).
    pub fn string(&self, ty: &str) -> Option<String> {
        self.get(ty)
            .map(|d| String::from_utf8_lossy(d).into_owned())
    }

    /// The plain text.
    pub fn as_text(&self) -> Option<String> {
        self.string(Self::TEXT)
    }

    /// The HTML.
    pub fn as_html(&self) -> Option<String> {
        self.string(Self::HTML)
    }

    /// The image, PNG first, else the first `image/*` type.
    pub fn as_image(&self) -> Option<ClipboardImage> {
        let (mime, bytes) = self
            .entries
            .iter()
            .find(|(t, _)| t == Self::PNG)
            .or_else(|| self.entries.iter().find(|(t, _)| t.starts_with("image/")))?;
        Some(ClipboardImage {
            bytes: bytes.clone(),
            mime: mime.clone(),
        })
    }

    /// The copied files' paths (`text/uri-list`'s `file://` URLs).
    pub fn files(&self) -> Vec<std::path::PathBuf> {
        self.string(Self::URI_LIST)
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| l.strip_prefix("file://"))
            .map(|p| std::path::PathBuf::from(percent_decode(p.trim_start_matches("localhost"))))
            .collect()
    }

    /// For backends: whether data of type `ty` travels as text (a `text/*`
    /// type holding UTF-8).
    pub fn is_text(ty: &str, data: &[u8]) -> bool {
        ty.starts_with("text/") && std::str::from_utf8(data).is_ok()
    }

    /// For backends: the item a platform answered as files (#246), each
    /// named by its type (or typed by its MIME type), read then forgotten.
    pub fn from_files(files: &[PickedFile]) -> Option<Self> {
        let mut item = ClipboardItem::new();
        for f in files {
            let ty = if f.name.contains('/') {
                &f.name
            } else {
                &f.mime
            };
            if let Ok(data) = f.read() {
                item.set(ty.clone(), data);
            }
        }
        (!item.is_empty()).then_some(item)
    }
}

/// For backends: standard base64 with padding, for data that travels as
/// text (the Swift bridge, the Android host).
#[doc(hidden)]
pub fn base64(bytes: &[u8]) -> String {
    crate::element::base64(bytes)
}

/// `%20` → ` ` in a `file://` URL's path.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let hex = |i: usize| b.get(i).and_then(|c| (*c as char).to_digit(16));
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (hex(i + 1), hex(i + 2))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl App {
    /// Puts `item` on the clipboard, replacing what was there (#246): every
    /// type it has, so each app pastes the richest one it knows. See
    /// [`ClipboardItem`] and the platform notes in
    /// [`read_clipboard_item`](AsyncApp::read_clipboard_item).
    pub fn write_clipboard(&mut self, item: ClipboardItem) {
        self.device_call(DeviceCall::WriteClipboard(item));
    }
}

impl AsyncApp {
    /// What the clipboard holds, in all its types (#246): a screenshot as
    /// `image/png`, HTML and text from a browser, files from the file
    /// manager (`text/uri-list`), another app's own types. `None` when it is
    /// empty or can't be read.
    ///
    /// - macOS: `NSPasteboard` (TIFF becomes PNG; an image file's content
    ///   is given as its image type).
    /// - iOS, iPadOS, visionOS: `UIPasteboard` (the system may ask the user
    ///   to allow pasting).
    /// - Web: `navigator.clipboard.read()` (the browser may ask), else the
    ///   text; the data is uploaded to the server like picked files. The
    ///   page writes `text/plain`, `text/html` and `image/png` (other
    ///   images are converted), and other types as the browser's custom
    ///   `web …` formats where it has them.
    /// - Android: the primary clip (text, HTML, and an image behind a
    ///   content URI).
    /// - Linux: the display's clipboard (GDK formats; images as PNG).
    /// - tvOS, watchOS: no clipboard, always `None`.
    pub async fn read_clipboard_item(&self) -> Option<ClipboardItem> {
        match self.device(DeviceCall::ReadClipboardItem).await {
            Ok(DeviceValue::Clipboard(item)) => (!item.is_empty()).then_some(item),
            Ok(DeviceValue::Files(files)) => ClipboardItem::from_files(&files),
            Ok(DeviceValue::Text(Some(t))) => Some(ClipboardItem::new().text(t)),
            _ => None,
        }
    }
}
