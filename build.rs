fn main() {
    // The expanded CLI parser and dispatch need more than the MSVC executable
    // default 1 MiB, especially in debug builds. Match the Unix main-thread
    // budget without relying on a user's environment or CI-only RUSTFLAGS.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bin=cutcraft=/STACK:8388608");
    }
}
