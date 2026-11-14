fn main() {
    // locales/*.json → typed `strings::…` functions, checked against English.
    prism_build::translations("locales")
        .fallback("en")
        .generate();
}
