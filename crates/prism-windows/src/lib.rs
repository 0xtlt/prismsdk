//! # prism-windows
//!
//! Win32 + Direct2D/DirectWrite backend (in progress): the retained scene,
//! key names and the Fluent palette, which have no Win32 types and are
//! tested on every host.

// Theme colors and control metrics: no Win32, tested everywhere.
mod fluent;
// Key names and icon glyphs: no Win32, tested everywhere.
mod keys;
// The retained tree: no Win32, tested everywhere.
mod scene;
