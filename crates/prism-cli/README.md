# prism-cli

The `prism` command: create, run, build and live-restart Prism apps.

The `prism` command: create, run, build and live-restart Prism apps.

```sh
cargo install prism-cli
prism new my-app && cd my-app
prism dev web            # http://localhost:3000, restarts on save
prism dev ios            # iOS simulator
prism dev android        # the running emulator or a USB phone
prism build macos        # release .app with widgets and icon
prism devices            # simulators, paired iPhones, Android devices
```

The bundling scripts and the platform sources (Swift, Java) are embedded in
the binary, so projects don't need a Prism checkout. Projects are configured
by a `Prism.toml` next to their `Cargo.toml`.

Part of [Prism](https://github.com/0xtlt/prism). Guide: [https://0xtlt.github.io/prism/](https://0xtlt.github.io/prism/) · Command reference: [the `prism` command](https://0xtlt.github.io/prism/cli.html)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
