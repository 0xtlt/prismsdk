//! Videos played in place with the platform's controls (#207).
//!
//! ```
//! use prism_core::prelude::*;
//! use prism_core::video;
//!
//! struct Clip;
//!
//! impl Render for Clip {
//!     fn render(&mut self, _cx: &mut Context<Self>) -> impl IntoElement {
//!         video("https://example.com/talk.mp4")
//!             .header("Authorization", "Bearer s3cret")
//!             .poster("https://example.com/talk.jpg")
//!             .w_full()
//!     }
//! }
//! ```
//!
//! | | Player | Headers |
//! |---|---|---|
//! | Web | `<video controls playsinline>` | through the server (`<base>/_prism/media/<id>`, ranges included) |
//! | macOS | `AVPlayerView` (inline controls) | `AVURLAssetHTTPHeaderFieldsKey` |
//! | iOS, iPadOS, tvOS, visionOS | `AVPlayerViewController`'s view | `AVURLAssetHTTPHeaderFieldsKey` |
//! | Android | `VideoView` with a `MediaController` | `setVideoURI(uri, headers)` |
//! | Linux | `GtkVideo` (GStreamer) | not sent: use a URL the player can open |
//! | Windows | `MediaPlayerElement` (transport controls) | range requests of its own (`MediaSource::CreateFromStream`); HLS and DASH: `AdaptiveMediaSource` with an `HttpClient` |
//!
//! Its height follows the video's own ratio once the platform knows it,
//! like a [`picture`](crate::picture()): give it a width (`w_full()`),
//! an `aspect_ratio` or a height to fix it. watchOS draws nothing.

use crate::a11y::{Accessibility, Accessible};
use crate::element::{Element, FluentBuilder, Handler, IntoElement, NodeKind, VNode};
use crate::style::{Style, Styled};

/// A video ([`video`]): its source (with [`header`](Video::header)s) and
/// how it plays.
pub struct Video {
    node: VNode,
    config: VideoConfig,
}

/// The playback reached the end: [`Video::on_ended`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EndedEvent;

/// How a video plays (its node's config): tokens `controls`, `autoplay`,
/// `loop`, `muted`, then `poster=<url>` last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoConfig {
    /// The platform's controls (play / pause, scrubber, volume, full
    /// screen, picture in picture where it has them).
    pub controls: bool,
    /// Starts playing as soon as it can (browsers allow it only muted).
    pub autoplay: bool,
    /// Starts over at the end.
    pub looping: bool,
    /// Without sound.
    pub muted: bool,
    /// A still shown before playback (a URL or an app asset).
    pub poster: Option<String>,
}

impl Default for VideoConfig {
    fn default() -> Self {
        VideoConfig {
            controls: true,
            autoplay: false,
            looping: false,
            muted: false,
            poster: None,
        }
    }
}

impl VideoConfig {
    /// Space-separated tokens, the poster last.
    pub fn encode(&self) -> String {
        let mut t: Vec<String> = Vec::new();
        for (on, name) in [
            (self.controls, "controls"),
            (self.autoplay, "autoplay"),
            (self.looping, "loop"),
            (self.muted, "muted"),
        ] {
            if on {
                t.push(name.into());
            }
        }
        if let Some(p) = &self.poster {
            t.push(format!("poster={p}"));
        }
        t.join(" ")
    }

    /// Parses [`encode`](Self::encode)'s output.
    pub fn decode(s: &str) -> Self {
        let (flags, poster) = match s.split_once("poster=") {
            Some((flags, p)) => (flags, Some(p.trim().to_string()).filter(|p| !p.is_empty())),
            None => (s, None),
        };
        let has = |t: &str| flags.split_whitespace().any(|x| x == t);
        VideoConfig {
            controls: has("controls"),
            autoplay: has("autoplay"),
            looping: has("loop"),
            muted: has("muted"),
            poster,
        }
    }
}

/// A video played in place with the platform's own controls (#207):
/// `src` is an `https://` URL, a `file://` URL or an app asset. See the
/// [module docs](self) for each backend.
pub fn video(src: impl Into<String>) -> Video {
    let mut node = VNode::new(NodeKind::Video);
    node.text = src.into();
    Video {
        node,
        config: VideoConfig::default(),
    }
}

impl Video {
    /// A header sent with every request of a remote video (a bearer
    /// token), as [`Picture::header`](crate::Picture::header): on the web
    /// the server fetches it, so the header never reaches the browser.
    /// Ignored for assets and files.
    pub fn header(mut self, name: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        let clean = |s: &str| s.replace(['\r', '\n'], "");
        let (name, value) = (clean(name.as_ref()), clean(value.as_ref()));
        let remote =
            self.node.text.starts_with("http://") || self.node.text.starts_with("https://");
        if remote && !name.trim().is_empty() {
            self.node.text.push('\n');
            self.node.text.push_str(name.trim());
            self.node.text.push_str(": ");
            self.node.text.push_str(value.trim());
        }
        self
    }

    /// The platform's controls (default: shown).
    pub fn controls(mut self, shown: bool) -> Self {
        self.config.controls = shown;
        self
    }

    /// Plays as soon as it can (browsers only allow it muted).
    pub fn autoplay(mut self, on: bool) -> Self {
        self.config.autoplay = on;
        self
    }

    /// Starts over at the end.
    pub fn looping(mut self, on: bool) -> Self {
        self.config.looping = on;
        self
    }

    /// Without sound.
    pub fn muted(mut self, on: bool) -> Self {
        self.config.muted = on;
        self
    }

    /// A still shown before playback (a URL or an app asset).
    pub fn poster(mut self, src: impl Into<String>) -> Self {
        let p = src.into().replace(['\r', '\n', ' '], "");
        self.config.poster = Some(p).filter(|p| !p.is_empty());
        self
    }

    /// Called when the playback reaches the end (not while looping).
    pub fn on_ended(mut self, handler: Handler<EndedEvent>) -> Self {
        self.node.on_ended = Some(handler);
        self
    }

    /// Describes the video for screen readers.
    pub fn alt(mut self, alt: impl Into<String>) -> Self {
        self.node.a11y.label = Some(alt.into());
        self
    }

    /// Stable identity among siblings: a keyed video keeps its playback
    /// when its siblings move.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.node.key = Some(key.into());
        self
    }

    /// A name for the element (`.id`).
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.node.id = Some(id.into());
        self
    }
}

impl Styled for Video {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.node.style
    }
}

impl Accessible for Video {
    fn a11y_mut(&mut self) -> &mut Accessibility {
        &mut self.node.a11y
    }
}

impl FluentBuilder for Video {}

impl IntoElement for Video {
    fn into_element(self) -> Element {
        let Video { mut node, config } = self;
        node.config = config.encode();
        Element::Node(Box::new(node))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configs_round_trip_with_the_poster_last() {
        assert_eq!(VideoConfig::default().encode(), "controls");
        let c = VideoConfig {
            controls: false,
            autoplay: true,
            looping: true,
            muted: true,
            poster: Some("https://x.y/a b?c=1".replace(' ', "")),
        };
        assert_eq!(c.encode(), "autoplay loop muted poster=https://x.y/ab?c=1");
        assert_eq!(VideoConfig::decode(&c.encode()), c);
        assert_eq!(VideoConfig::decode("controls"), VideoConfig::default());
    }
}
