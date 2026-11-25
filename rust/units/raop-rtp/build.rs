fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu")
    {
        // OpenSSL's Windows random provider uses BCryptGenRandom.
        println!("cargo:rustc-link-arg=-lbcrypt");
    }
}
