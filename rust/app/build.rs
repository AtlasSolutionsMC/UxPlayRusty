use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_LIBDIR");
    println!("cargo:rerun-if-env-changed=UXPLAY_SKIP_LIBPLIST_PROBE");

    let host = env::var("HOST").expect("Cargo sets HOST");
    let target = env::var("TARGET").expect("Cargo sets TARGET");
    if target.ends_with("-windows-gnu") {
        // Keep bcrypt after OpenSSL's static archives for its RAND provider.
        println!("cargo:rustc-link-arg=-lbcrypt");
    }
    if host != target && env::var_os("UXPLAY_SKIP_LIBPLIST_PROBE").is_some() {
        println!(
            "cargo:warning=Cross-target check skips native libplist probing; this is not a linkable build"
        );
        return;
    }

    pkg_config::Config::new()
        .atleast_version("2.0")
        .probe("libplist-2.0")
        .expect("libplist-2.0 development files are required to build uxplay");
}
