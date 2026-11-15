fn main() {
    // The app binary's link settings (Apple: Swift's rpath, deployment target).
    prism_build::link_app();
    // locales/*.json → typed `strings::…` functions, checked against English.
    prism_build::translations("locales")
        .fallback("en")
        .generate();
}
