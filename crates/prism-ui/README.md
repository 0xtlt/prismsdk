# prism-ui

Prism's customizable component library: themed, accessible, overridable, with a native escape hatch.

Prism's component library: `button`, `switch`, `checkbox`, `text_field`,
`card`, `heading`, `badge`, `list_row`, `divider` and charts (line, bar, pie,
ring, stat, sparkline).

Three levels of customization: a `Theme` (colors, radius, type scale, light
and dark), per-instance props (`ui::button("Delete").danger().large()`), and
app-wide overrides of any component. Each control can defer to the platform's
native widget with `.native()`.

Apps use it as `prism::ui`.

Part of [Prism](https://github.com/0xtlt/prismsdk). Guide: [https://0xtlt.github.io/prismsdk/](https://0xtlt.github.io/prismsdk/) · API: [docs.rs/prismsdk-ui](https://docs.rs/prismsdk-ui)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
