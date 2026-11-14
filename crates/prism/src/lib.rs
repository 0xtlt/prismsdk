//! # Prism
//!
//! A GPUI-flavoured, React-Native-style UI framework: describe your UI once in
//! Rust, then ship it as
//!
//! - a **web server** (`--features web`): server-rendered HTML/CSS + a tiny JS
//!   client receiving DOM patches. No WASM.
//! - a **native app** (`--features native`, default): real AppKit views on
//!   macOS, real UIKit views on iOS.
//!
//! Backends are separate crates behind the [`Backend`](prism_core::Backend)
//! patch protocol; adding Android, Windows or Linux means adding a crate that
//! implements `prism_native::Widgets`, plus one line in [`Application::run`].
//!
//! ```no_run
//! use prism::prelude::*;
//!
//! struct Counter { count: i32 }
//!
//! impl Render for Counter {
//!     fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
//!         div().p(24.).gap(12.)
//!             .child(text(format!("Count: {}", self.count)).text_size(32.))
//!             // Listeners re-render their entity automatically.
//!             .child(button("+1").on_click(cx.listener(|this, _, _| this.count += 1)))
//!     }
//! }
//!
//! fn main() {
//!     prism::Application::new().title("Counter").run(|cx| cx.new_entity(|_| Counter { count: 0 }));
//! }
//! ```

pub use prism_core::*;

/// The component library: themed, accessible, overridable, native on demand.
pub use prism_ui as ui;

use std::sync::Arc;

/// Which backend [`Application::run`] will use for this build.
pub const BACKEND: &str = if cfg!(feature = "web") {
    "web"
} else if cfg!(all(feature = "native", target_os = "macos")) {
    "macos"
} else if cfg!(all(feature = "native", target_os = "ios")) {
    "ios"
} else {
    "none"
};

/// Entry point: configure, then [`run`](Application::run).
#[derive(Clone, Debug, Default)]
pub struct Application {
    config: AppConfig,
}

impl Application {
    pub fn new() -> Self {
        Self::default()
    }

    /// Window title (native) or page `<title>` (web).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.config.title = title.into();
        self
    }

    /// Initial window size in points (desktop only).
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.config.width = width;
        self.config.height = height;
        self
    }

    /// Web server port (default 3000, `PRISM_PORT` overrides).
    pub fn port(mut self, port: u16) -> Self {
        self.config.port = port;
        self
    }

    /// Starts the backend selected at compile time. `init` creates the root
    /// entity; it runs once per window, or once per browser session on the web.
    pub fn run<V: Render>(self, init: impl Fn(&mut App) -> Entity<V> + Send + Sync + 'static) {
        let init: AppInit = Arc::new(move |app: &mut App| {
            let root = init(app);
            app.set_root(root);
        });
        let config = self.config;

        // Build tooling: `PRISM_DESCRIBE=widgets` prints the registered widgets
        // (used to generate the WidgetKit extension) and exits.
        if std::env::var("PRISM_DESCRIBE").as_deref() == Ok("widgets") {
            let runtime = Runtime::new(&init);
            println!("{}", runtime.app.describe_widgets());
            return;
        }

        #[cfg(feature = "web")]
        {
            prism_web::run(config, init);
        }
        #[cfg(all(not(feature = "web"), feature = "native", target_os = "macos"))]
        {
            prism_macos::run(config, init);
        }
        #[cfg(all(not(feature = "web"), feature = "native", target_os = "ios"))]
        {
            prism_ios::run(config, init);
        }
        #[cfg(not(any(
            feature = "web",
            all(feature = "native", any(target_os = "macos", target_os = "ios"))
        )))]
        {
            let _ = (config, init);
            panic!(
                "prism: no backend for this target. Enable `web`, or build `native` for macOS/iOS."
            );
        }
    }
}

pub mod prelude {
    pub use crate::Application;
    pub use prism_core::prelude::*;
}
