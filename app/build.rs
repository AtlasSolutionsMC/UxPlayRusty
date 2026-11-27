use std::{env, fs, path::PathBuf, process::Command};

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
        embed_windows_icon();
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

fn embed_windows_icon() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("build output directory"));
    let icon = root.join("../assets/uxplayrusty.ico");
    let manifest = root.join("../assets/uxplayrusty.manifest");
    let resource = output.join("uxplay-icon.rc");
    let object = output.join("uxplay-icon.o");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed={}", manifest.display());
    fs::write(
        &resource,
        format!(
            "1 ICON \"{}\"\n1 24 \"{}\"\n",
            resource_path(&icon),
            resource_path(&manifest)
        ),
    )
    .expect("write Windows icon resource script");
    let status = Command::new("windres")
        .args([
            "-i",
            resource.to_str().expect("resource path"),
            "-O",
            "coff",
            "-o",
        ])
        .arg(&object)
        .status()
        .expect("run windres to embed the Windows app icon");
    assert!(
        status.success(),
        "windres failed to embed the Windows app icon"
    );
    println!("cargo:rustc-link-arg-bin=uxplay={}", object.display());
}

fn resource_path(path: &std::path::Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}
