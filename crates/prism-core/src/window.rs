//! The window (or browser viewport) size, for responsive layouts.
//!
//! ```ignore
//! // Structure: a sidebar on large screens, a single column on phones.
//! if cx.breakpoint() >= Breakpoint::Expanded { two_columns() } else { one_column() }
//! // Styles: tailwind-like rules, from a width up.
//! div().flex_col().gap(12.).md(|s| s.flex_row().gap(24.))
//! ```
//!
//! Everything re-renders when the window crosses a breakpoint (rotation,
//! resize, split view) or a width an `.at_width(…)` rule uses;
//! `on_resize` gets every size change. The breakpoints themselves move with
//! [`App::set_breakpoints`] (`Application::breakpoints([480., 900., 1280.])`).

use std::rc::Rc;

use crate::app::App;
use crate::style::Breakpoint;

type ResizeHandler = Rc<dyn Fn((f32, f32), &mut App)>;

pub(crate) struct Window {
    pub size: (f32, f32),
    pub platform: crate::platform::Platform,
    pub handlers: Vec<ResizeHandler>,
    /// Smallest widths of `Medium`, `Expanded` and `Large`.
    pub breakpoints: [f32; 3],
    /// Window widths `.at_width(…)` rules switch at (re-render when crossed).
    pub thresholds: Vec<f32>,
    // --- Safe area (#28) ---
    /// The whole window, system bars included (`size` is what they leave).
    pub full: (f32, f32),
    /// What the system's bars, cutouts and side bars cover of the window.
    pub safe_area: crate::style::Edges,
}

impl Default for Window {
    fn default() -> Self {
        // A phone until the platform says otherwise.
        Window {
            size: (390., 844.),
            platform: crate::platform::Platform::compiled(),
            handlers: Vec::new(),
            breakpoints: Breakpoint::DEFAULT_WIDTHS,
            thresholds: Vec::new(),
            full: (390., 844.),
            safe_area: crate::style::Edges::default(),
        }
    }
}

impl App {
    /// Width and height of the area the root screen lays out in: the window
    /// minus what the system's bars, cutouts and side bars cover (see
    /// [`safe_area`](Self::safe_area)), in CSS pixels on the web and points
    /// natively. Breakpoints, `.md()`… and `.at_width(…)` resolve against
    /// this width.
    pub fn viewport(&self) -> (f32, f32) {
        self.window.size
    }

    /// The whole window, system bars included: [`viewport`](Self::viewport)
    /// plus [`safe_area`](Self::safe_area).
    pub fn window_size(&self) -> (f32, f32) {
        self.window.full
    }

    /// What the system covers on each edge of the window: status bar, home
    /// indicator, notch and cutouts, a foldable's or iPad's side bars, the
    /// tab bar beside the content, the browser's `env(safe-area-inset-*)`.
    /// Updated on rotation, fold, split view and resize (`on_resize` runs).
    pub fn safe_area(&self) -> crate::style::Edges {
        self.window.safe_area
    }

    /// The window's size class.
    pub fn breakpoint(&self) -> Breakpoint {
        Breakpoint::for_width_in(self.window.size.0, self.window.breakpoints)
    }

    /// Runs `f` on every size change (crossing a breakpoint already
    /// re-renders everything).
    pub fn on_resize(&mut self, f: impl Fn((f32, f32), &mut App) + 'static) {
        self.window.handlers.push(Rc::new(f));
    }

    pub(crate) fn set_viewport(&mut self, size: (f32, f32)) {
        self.set_window(size, crate::style::Edges::default());
    }

    /// The window is `full` large and the system covers `safe_area` of it.
    pub(crate) fn set_window(&mut self, full: (f32, f32), safe_area: crate::style::Edges) {
        let size = (
            (full.0 - safe_area.left - safe_area.right).max(0.),
            (full.1 - safe_area.top - safe_area.bottom).max(0.),
        );
        if (self.window.full, self.window.safe_area, self.window.size) == (full, safe_area, size) {
            return;
        }
        let before = (self.breakpoint(), self.window.band(self.window.size.0));
        // Renders that read `safe_area()` see the new insets.
        let moved_insets = self.window.safe_area != safe_area;
        self.window.full = full;
        self.window.safe_area = safe_area;
        self.window.size = size;
        if moved_insets || (self.breakpoint(), self.window.band(size.0)) != before {
            self.refresh();
        }
        for handler in self.window.handlers.clone() {
            handler(size, self);
        }
    }
}

impl Window {
    /// How many `.at_width(…)` thresholds a window `width` wide has passed.
    pub(crate) fn band(&self, width: f32) -> usize {
        self.thresholds.iter().filter(|t| width >= **t).count()
    }

    /// Remembers the window widths `style`'s `.at_width(…)` rules switch at.
    pub(crate) fn watch_widths(&mut self, style: &crate::style::Style) {
        for w in style.window_width_thresholds() {
            if !self.thresholds.contains(&w) {
                self.thresholds.push(w);
            }
        }
    }
}

impl App {
    /// Moves the size classes: the smallest widths of `Medium`, `Expanded`
    /// and `Large` (600, 1024 and 1440 by default), in points. `.md()`,
    /// `.lg()`, `.xl()` and [`breakpoint`](Self::breakpoint) follow them,
    /// and the app re-renders when the window crosses one. Usually set with
    /// `Application::breakpoints`.
    pub fn set_breakpoints(&mut self, widths: [f32; 3]) {
        if self.window.breakpoints == widths {
            return;
        }
        self.window.breakpoints = widths;
        self.refresh();
    }

    /// The smallest widths of `Medium`, `Expanded` and `Large`, in points.
    pub fn breakpoints(&self) -> [f32; 3] {
        self.window.breakpoints
    }
}
