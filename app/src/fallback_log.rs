//! Initializes the migration harness's fallback counter without linking a reference executable.

/// Ensures `rust-anything app replay` can report a zero fallback count.
pub fn report_fallbacks() {
    let Some(path) = std::env::var_os("RUST_ANYTHING_FALLBACK_LOG") else {
        return;
    };
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path);
}
