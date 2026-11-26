//! The app run as its own widget extension (#331): an app whose widgets
//! refresh without it (`Widget::refresh`) ships its binary in the `.appex`
//! too, with the extension's Swift linked in (the `prism_widget_extension`
//! cfg, set by `prism-apple-build`). There, `run` hands the process to
//! WidgetKit instead of starting UIKit, and the timeline providers run the
//! hooks in a runtime without a window
//! (`prism_native::apple::widget_extension`).

use prism_core::{AppConfig, AppInit, Capabilities, Runtime, RuntimeOptions};

/// Starts WidgetKit with the app's widget code. Never returns.
pub(crate) fn run(_config: AppConfig, init: AppInit) {
    #[cfg(prism_widget_extension)]
    prism_native::apple::widget_extension::run(move || runtime(&init), linked::done);
    #[cfg(not(prism_widget_extension))]
    {
        let _ = (runtime, init);
        eprintln!(
            "prism-ios: this binary runs as a widget extension but has no extension code: \
             bundle the app with `prism run` / `prism build`"
        );
        std::process::exit(1);
    }
}

/// A runtime without a window: the app's storage (the App Group, shared
/// with the app), environment and formats; it draws only the widgets its
/// refresh hooks updated.
fn runtime(init: &AppInit) -> Runtime {
    let data_dir = crate::imp::data_dir();
    let mut runtime = Runtime::with_options(
        init,
        RuntimeOptions {
            path: "/".into(),
            capabilities: Capabilities {
                widgets: true,
                ..Capabilities::default()
            },
            storage: Some(Box::new(prism_core::FileStorage::new(data_dir.clone()))),
            wake_ui: Some(prism_native::apple::widget_extension::wake_ui()),
            environment: crate::imp::ios_environment(),
            formatter: Some(Box::new(prism_native::apple::FoundationFormatter)),
            // iCloud key-value storage is the app's (its entitlement).
            cloud_storage: None,
            viewport: None,
            platform: None,
            data_dir: Some(data_dir),
            shared_data_dir: None,
            device_info: None,
        },
    );
    runtime.app.set_widgets_refresh_only(true);
    runtime
        .app
        .set_widget_configurations_source(prism_native::apple::widget_configurations);
    runtime
}

#[cfg(prism_widget_extension)]
mod linked {
    unsafe extern "C" {
        /// `PrismWidgetKit.swift`: the timeline of `token` can be built.
        fn prism_widget_refresh_done(token: u64);
    }

    pub(super) fn done(token: u64) {
        unsafe { prism_widget_refresh_done(token) }
    }
}
