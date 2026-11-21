# Linux (GTK 4)

`cargo run` on Linux opens a native GTK 4 window: `GtkLabel`, `GtkButton`, `GtkEntry`, `GtkSwitch`, `GtkProgressBar`, `GtkPicture` and `GtkScrolledWindow`. Navigation stacks get a header with a Back button, tabs a tab bar, and modals their own window.

- Layout is prism-native's flexbox: a small GTK layout manager puts each widget at its frame, so screens look like on macOS. Styles become generated CSS.
- Wrapping labels report their longest word as minimum width.
- Storage goes to `$XDG_DATA_HOME/<title>`; the language comes from `LANG`. `cx.device_info()` reports the distribution's `VERSION_ID` and the monitor (size in mm, refresh rate).
- Develop and test in Docker: `docker build -f docker/linux-dev.Dockerfile -t prism-linux-dev docker`, then run the app under `xvfb-run` with `PRISM_SNAPSHOT`/`PRISM_CLICK` (see CI).
- Accessibility labels use GTK's C API (`gtk_accessible_update_property_value`, available in every GTK 4); gtk-rs only binds it from 4.10.
- Location uses GeoClue2 over the system bus, asynchronously; `request_permission` starts a client, so the desktop's agent asks the user. Also available: the file chooser for photos, the clipboard, opening links with the default app.
- Notifications use `GNotification`, with buttons; the dev image's D-Bus monitor shows them. Alerts are modal windows. Context menus open on right-click or press and hold (`GtkPopoverMenu`). `set_app_menu` becomes the menu bar, with shortcuts.
- The tray uses StatusNotifierItem through `ksni`, checked against a test watcher. Also supported: swipes (touchpad), scroll events, refresh with F5 / Ctrl+R, lifecycle (focus, minimized) and background tasks while the app runs. Text size follows GNOME's scaling factor; reduced motion follows "animations off".
- Secrets use the Secret Service (D-Bus; `biometric` ones answer `Unavailable`, since no Linux store ties a secret to the user's presence), fingerprints fprintd, sign-in the default browser with a loopback redirect (see [Security](#security-secrets-biometrics-sign-in)).
- No desktop standard exists for motion, camera, haptics or a share sheet, so `capabilities` reports them as unavailable.
- No [web views](../guide/web-views.md) yet (WebKitGTK would add a system dependency): `web_view` draws an empty box and `capabilities().web_view` is false.
