fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("watchos") {
        prism_apple_build::build_watch_host();
    }
}
