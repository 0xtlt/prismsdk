//! Videos (`video()`, #207): WinUI's `MediaPlayerElement` over a
//! `Windows.Media.Playback.MediaPlayer`, with the platform's transport
//! controls (play / pause, scrubber, volume, full window, cast), as
//! `AVPlayerView` and `GtkVideo` are on macOS and Linux.
//!
//! The source is a remote URL, a file or an app asset (copied out of the
//! binary when it is compiled in, `prism_native::images::media_url`); the
//! poster a picture shown until playback starts. A remote video with
//! request headers (`Video::header`) is read through `stream.rs` (HTTP
//! ranges with the headers), or an `AdaptiveMediaSource` whose
//! `HttpClient` sends them for HLS and DASH. The player's events come
//! on a thread of the media stack: they are brought back to the UI thread
//! through its dispatcher queue: the end of the playback
//! (`UiEvent::Ended`, not while looping) and the video's natural size (the
//! node lays out again).

use std::cell::RefCell;
use std::collections::HashMap;

use prism_core::{NodeId, UiEvent, VideoConfig};
use windows::Foundation::{TypedEventHandler, Uri};
use windows::Media::Core::MediaSource;
use windows::Media::Playback::MediaPlayer;
use windows::Media::Streaming::Adaptive::{AdaptiveMediaSource, AdaptiveMediaSourceCreationStatus};
use windows_core::{IInspectable, Interface, Result};

use crate::imp::{defer, dispatch};
use crate::widgets::Win;
use crate::winui::bindings::Microsoft::UI::Dispatching::{DispatcherQueue, DispatcherQueueHandler};
use crate::winui::h;
use crate::winui::xaml::Controls::MediaPlayerElement;
use crate::winui::xaml::Media::Imaging::{BitmapImage, WriteableBitmap};
use crate::winui::xaml::Media::{ImageSource, Stretch};

thread_local! {
    /// The natural size of each video once its stream opened (pixels,
    /// one per DIP like pictures).
    static NATURAL: RefCell<HashMap<NodeId, (f32, f32)>> = RefCell::new(HashMap::new());
}

/// A video node's element and player.
pub struct Video {
    pub element: MediaPlayerElement,
    player: MediaPlayer,
    /// The source and poster shown (`set_text`, the config's poster).
    src: RefCell<String>,
    poster: RefCell<Option<String>>,
}

impl Drop for Video {
    /// The node is gone: the playback stops, the stream closes.
    fn drop(&mut self) {
        let _ = self.player.Pause();
        let _ = self.player.Close();
    }
}

impl Video {
    pub fn new(id: NodeId) -> Result<Video> {
        let element = MediaPlayerElement::new()?;
        element.SetStretch(Stretch::Uniform)?;
        element.SetAreTransportControlsEnabled(true)?;
        let player = MediaPlayer::new()?;
        player.SetAutoPlay(false)?;
        let queue = DispatcherQueue::GetForCurrentThread()?;
        // On the media thread: back to the UI thread.
        let q = queue.clone();
        player.MediaEnded(&TypedEventHandler::<MediaPlayer, IInspectable>::new(
            move |p, _| {
                let looping = p
                    .as_ref()
                    .and_then(|p| p.IsLoopingEnabled().ok())
                    .unwrap_or(false);
                if !looping {
                    let _ = q.TryEnqueue(&DispatcherQueueHandler::new(move || {
                        dispatch(UiEvent::Ended(id));
                        Ok(())
                    }));
                }
                Ok(())
            },
        ))?;
        player.MediaOpened(&TypedEventHandler::<MediaPlayer, IInspectable>::new(
            move |p, _| {
                let Some(session) = p.as_ref().and_then(|p| p.PlaybackSession().ok()) else {
                    return Ok(());
                };
                let (w, hh) = (
                    session.NaturalVideoWidth().unwrap_or(0),
                    session.NaturalVideoHeight().unwrap_or(0),
                );
                if w > 0 && hh > 0 {
                    let _ = queue.TryEnqueue(&DispatcherQueueHandler::new(move || {
                        natural_size(id, (w as f32, hh as f32));
                        Ok(())
                    }));
                }
                Ok(())
            },
        ))?;
        element.SetMediaPlayer(&player)?;
        Ok(Video {
            element,
            player,
            src: RefCell::new(String::new()),
            poster: RefCell::new(None),
        })
    }

    /// The controls, autoplay, looping, sound.
    pub fn configure(&self, config: &str) -> Result<()> {
        let c = VideoConfig::decode(config);
        self.element.SetAreTransportControlsEnabled(c.controls)?;
        self.player.SetIsLoopingEnabled(c.looping)?;
        self.player.SetIsMuted(c.muted)?;
        self.player.SetAutoPlay(c.autoplay)?;
        *self.poster.borrow_mut() = c.poster;
        Ok(())
    }
}

/// Opens remote video `url` of node `id` with its request `headers`,
/// then plays it if the node still shows `src` (its text).
fn open_with_headers(
    id: NodeId,
    src: String,
    url: String,
    headers: Vec<(String, String)>,
) -> Result<()> {
    let queue = DispatcherQueue::GetForCurrentThread()?;
    // Back on the UI thread: the node may show another video by now.
    let play = move |source: MediaSource| {
        let src = src.clone();
        let _ = queue.TryEnqueue(&DispatcherQueueHandler::new(move || {
            crate::imp::with_state(|s| {
                if let Some(crate::controls::Content::Video(v)) =
                    s.renderer.widgets.views.get(&id).map(|v| &v.content)
                    && *v.src.borrow() == src
                {
                    let _ = v.player.SetSource(&source);
                }
            });
            Ok(())
        }));
    };
    if crate::stream::adaptive(&url) {
        // HLS, DASH: the manifest and every segment through this client.
        let client = windows::Web::Http::HttpClient::new()?;
        let defaults = client.DefaultRequestHeaders()?;
        for (k, v) in &headers {
            defaults.TryAppendWithoutValidation(&h(k), &h(v))?;
        }
        let op = AdaptiveMediaSource::CreateFromUriWithDownloaderAsync(
            &Uri::CreateUri(&h(&url))?,
            &client,
        )?;
        op.SetCompleted(&windows_future::AsyncOperationCompletedHandler::<
            windows::Media::Streaming::Adaptive::AdaptiveMediaSourceCreationResult,
        >::new(move |op, _| {
            let Some(op) = op.as_ref() else { return Ok(()) };
            let r = op.GetResults()?;
            if r.Status()? != AdaptiveMediaSourceCreationStatus::Success {
                crate::win::log!(
                    "video {id}: the adaptive stream did not open ({:?})",
                    r.Status()?
                );
                return Ok(());
            }
            play(MediaSource::CreateFromAdaptiveMediaSource(
                &r.MediaSource()?,
            )?);
            Ok(())
        }))?;
        return Ok(());
    }
    let refs: Vec<(String, String)> = headers;
    std::thread::spawn(move || {
        let pairs: Vec<(&str, &str)> = refs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        match crate::stream::Remote::open(&url, &pairs) {
            Ok((remote, kind)) => {
                let source =
                    MediaSource::CreateFromStream(&crate::stream::Stream::open(remote), &h(&kind));
                match source {
                    Ok(s) => play(s),
                    Err(e) => crate::win::log!("video {id}: {e}"),
                }
            }
            Err(e) => {
                // Never the headers: they hold tokens.
                crate::win::log!("video {id}: {e}; played without its headers");
                if let Ok(s) = Uri::CreateUri(&h(&url)).and_then(|u| MediaSource::CreateFromUri(&u))
                {
                    play(s);
                }
            }
        }
    });
    Ok(())
}

/// The stream of node `id` knows its size: it lays out again.
fn natural_size(id: NodeId, size: (f32, f32)) {
    NATURAL.with(|n| n.borrow_mut().insert(id, size));
    defer(Box::new(move |s| {
        s.renderer.widgets.measurer.forget(id);
        s.renderer.mark_all_dirty();
        crate::imp::relayout(s);
    }));
}

/// A video's natural size within `max_width`: its stream's, scaled down
/// to fit, else 16:9.
pub fn measure(id: NodeId, max_width: Option<f32>) -> (f32, f32) {
    match NATURAL.with(|n| n.borrow().get(&id).copied()) {
        Some((w, h)) => {
            let shown = max_width.map_or(w, |m| m.min(w));
            (shown, shown * h / w)
        }
        None => {
            let w = max_width.unwrap_or(320.);
            (w, w * 9. / 16.)
        }
    }
}

/// The node is gone.
pub fn forget(id: NodeId) {
    NATURAL.with(|n| n.borrow_mut().remove(&id));
}

impl Win {
    /// Video node `id` shows its source (`text`) and poster: called after
    /// its text or config changed.
    pub fn show_video(&mut self, id: NodeId) {
        let Some(text) = self.scene.get(id).map(|n| n.text.clone()) else {
            return;
        };
        let (src, poster) = match self.views.get(&id).map(|v| &v.content) {
            Some(crate::controls::Content::Video(v)) => {
                (v.src.borrow().clone(), v.poster.borrow().clone())
            }
            _ => return,
        };
        let r = (|| -> Result<()> {
            let poster = match poster {
                Some(p) => self.poster(&p)?,
                None => None,
            };
            let Some(crate::controls::Content::Video(video)) =
                self.views.get(&id).map(|v| &v.content)
            else {
                return Ok(());
            };
            if src != text {
                *video.src.borrow_mut() = text.clone();
                match prism_native::images::media_url(&text, &self.images.sources) {
                    Some(url) => {
                        let (plain, headers) = prism_core::picture_source(&url);
                        if headers.is_empty() || !prism_native::images::is_remote(plain) {
                            let source = MediaSource::CreateFromUri(&Uri::CreateUri(&h(plain))?)?;
                            video.player.SetSource(&source)?;
                        } else {
                            video.player.SetSource(
                                None::<&windows::Media::Playback::IMediaPlaybackSource>,
                            )?;
                            let headers: Vec<(String, String)> = headers
                                .iter()
                                .map(|(k, v)| (k.to_string(), v.to_string()))
                                .collect();
                            open_with_headers(id, text.clone(), plain.to_string(), headers)?;
                        }
                    }
                    None => video
                        .player
                        .SetSource(None::<&windows::Media::Playback::IMediaPlaybackSource>)?,
                }
            }
            video.element.SetPosterSource(poster.as_ref())?;
            Ok(())
        })();
        if let Err(e) = r {
            crate::win::log!("video {id}: {e}");
        }
    }

    /// A poster: a remote picture loads by itself (`BitmapImage`), an app
    /// asset or a file decodes now (WIC, like pictures).
    fn poster(&mut self, src: &str) -> Result<Option<ImageSource>> {
        let url = prism_core::picture_source(src).0;
        if prism_native::images::is_remote(url) {
            let b = BitmapImage::new()?;
            b.SetUriSource(&Uri::CreateUri(&h(url))?)?;
            return Ok(Some(b.cast()?));
        }
        let wic = self.wic.clone();
        self.images.request(&wic, src, self.hwnd);
        let Some((_, bitmap)) = self.images.bitmap(src) else {
            return Ok(None);
        };
        Ok(Some(writeable(&bitmap)?.cast()?))
    }
}

/// A WIC bitmap as a `WriteableBitmap` (its pixels copied).
fn writeable(bitmap: &windows::Win32::Graphics::Imaging::IWICBitmap) -> Result<WriteableBitmap> {
    use windows::Win32::System::WinRT::IBufferByteAccess;
    unsafe {
        let (mut w, mut hh) = (0u32, 0u32);
        bitmap.GetSize(&mut w, &mut hh)?;
        let wb = WriteableBitmap::CreateInstanceWithDimensions(w as i32, hh as i32)?;
        let buffer = wb.PixelBuffer()?;
        let access: IBufferByteAccess = buffer.cast()?;
        let len = buffer.Capacity()?;
        let dst = std::slice::from_raw_parts_mut(access.Buffer()?, len as usize);
        bitmap.CopyPixels(std::ptr::null(), w * 4, dst)?;
        buffer.SetLength(len)?;
        wb.Invalidate()?;
        Ok(wb)
    }
}
