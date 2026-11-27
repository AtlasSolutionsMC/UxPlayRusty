fn main() {
    let is_windows_gnu = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu");
    if is_windows_gnu {
        // OpenSSL's Windows-GNU random provider calls BCryptGenRandom.
        println!("cargo:rustc-link-arg=-lbcrypt");
    }
}
