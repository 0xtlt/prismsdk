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
//! resize, split view); `on_resize` gets every size change.

use std::rc::Rc;

use crate::app::App;
use crate::style::Breakpoint;

type ResizeHandler = Rc<dyn Fn((f32, f32), &mut App)>;

pub(crate) struct Window {
    pub size: (f32, f32),
    pub platform: crate::platform::Platform,
    pub handlers: Vec<ResizeHandler>,
}

impl Default for Window {
    fn default() -> Self {
        // A phone until the platform says otherwise.
        Window {
            size: (390., 844.),
            platform: crate::platform::Platform::compiled(),
            handlers: Vec::new(),
        }
    }
}

impl App {
    /// Width and height of the window (CSS pixels on the web, points natively).
    pub fn viewport(&self) -> (f32, f32) {
        self.window.size
    }

    /// The window's size class.
    pub fn breakpoint(&self) -> Breakpoint {
        Breakpoint::for_width(self.window.size.0)
    }

    /// Runs `f` on every size change (crossing a breakpoint already
    /// re-renders everything).
    pub fn on_resize(&mut self, f: impl Fn((f32, f32), &mut App) + 'static) {
        self.window.handlers.push(Rc::new(f));
    }

    pub(crate) fn set_viewport(&mut self, size: (f32, f32)) {
        if self.window.size == size {
            return;
        }
        let before = self.breakpoint();
        self.window.size = size;
        if self.breakpoint() != before {
            self.refresh();
        }
        for handler in self.window.handlers.clone() {
            handler(size, self);
        }
    }
}
