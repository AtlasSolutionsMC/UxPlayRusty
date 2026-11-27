use std::{
    collections::{HashMap, HashSet, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const PLUGINS: &[&str] = &[
    "libgstapp.dll",
    "libgstcoreelements.dll",
    "libgsttypefindfunctions.dll",
    "libgstplayback.dll",
    "libgstlibav.dll",
    "libgstautodetect.dll",
    "libgstvideoparsersbad.dll",
    "libgstvideoconvertscale.dll",
    "libgstaudioconvert.dll",
    "libgstaudioresample.dll",
    "libgstvolume.dll",
    "libgstlevel.dll",
    "libgstwasapi.dll",
    "libgstd3d11.dll",
    "libgstvulkan.dll",
    "libgsthls.dll",
    "libgstsoup.dll",
    "libgstrtp.dll",
    "libgstudp.dll",
    "libgstisomp4.dll",
    "libgstmpegtsdemux.dll",
    "libgstsubparse.dll",
    "libgstjpeg.dll",
    "libgstvideorate.dll",
    "libgsttcp.dll",
];

fn main() {
    if let Err(error) = run() {
        eprintln!("UxPlay Windows tools failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("package-release") => package_release(PathBuf::from(args.next().ok_or("pass the MSYS2 root")?)),
        Some("smoke-runtime") => smoke_runtime(PathBuf::from(args.next().ok_or("pass the stage directory")?), PathBuf::from(args.next().ok_or("pass the MSYS2 root")?)),
        Some("test-install") => test_install(PathBuf::from(args.next().ok_or("pass Setup.exe")?)),
        Some("test-winrt-beacon-shutdown") => test_beacon_shutdown(PathBuf::from(args.next().ok_or("pass uxplay-beacon.exe")?)),
        _ => Err("usage: uxplay-windows-tools <package-release MSYS2_ROOT | smoke-runtime STAGE_DIR MSYS2_ROOT | test-install Setup.exe | test-winrt-beacon-shutdown BEACON_EXE>".into()),
    }
}

fn package_release(msys_root: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let root = repository_root()?;
    let msys_root = msys_root.canonicalize()?;
    let ucrt = msys_root.join("ucrt64");
    let bin = ucrt.join("bin");
    let plugins = ucrt.join("lib/gstreamer-1.0");
    let scanner = ucrt.join("libexec/gstreamer-1.0/gst-plugin-scanner.exe");
    let objdump = bin.join("objdump.exe");
    let pacman = msys_root.join("usr/bin/pacman.exe");
    for required in [&bin, &plugins, &scanner, &objdump, &pacman] {
        if !required.exists() {
            return Err(format!("required MSYS2 file is missing: {}", required.display()).into());
        }
    }
    let output = root.join("dist/windows");
    fs::create_dir_all(&output)?;
    let stage = output.join("stage.next");
    if stage.exists() {
        fs::remove_dir_all(&stage)?;
    }
    let stage_plugins = stage.join("runtime/plugins");
    let stage_scanner = stage.join("runtime/libexec/gstreamer-1.0");
    fs::create_dir_all(&stage_plugins)?;
    fs::create_dir_all(&stage_scanner)?;

    let app_version = manifest_version(&root.join("app/Cargo.toml"))?;
    let beacon_version = manifest_version(&root.join("beacon/Cargo.toml"))?;
    if app_version != beacon_version {
        return Err("app and beacon versions do not match".into());
    }
    let mut build = msys_cargo(&msys_root, &root);
    build
        .env("RUSTUP_TOOLCHAIN", "stable-x86_64-pc-windows-gnu")
        .env("CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER", "/ucrt64/bin/gcc")
        .env("PKG_CONFIG_PATH", "/ucrt64/lib/pkgconfig:/ucrt64/share/pkgconfig");
    run_command(
        build
            .args(["build"])
            .args(["--manifest-path", "app/Cargo.toml", "--release"]),
    )?;
    let mut build = msys_cargo(&msys_root, &root);
    build
        .env("RUSTUP_TOOLCHAIN", "stable-x86_64-pc-windows-gnu")
        .env("CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER", "/ucrt64/bin/gcc")
        .env("PKG_CONFIG_PATH", "/ucrt64/lib/pkgconfig:/ucrt64/share/pkgconfig");
    run_command(
        build
            .args(["build"])
            .args(["--manifest-path", "beacon/Cargo.toml", "--release"]),
    )?;

    for (source, name) in [
        (root.join("app/target/release/uxplay.exe"), "uxplay.exe"),
        (
            root.join("beacon/target/release/uxplay-beacon.exe"),
            "uxplay-beacon.exe",
        ),
        (
            root.join("app/assets/config.example.toml"),
            "config.example.toml",
        ),
        (root.join("README.md"), "README.md"),
        (root.join("LICENSE"), "LICENSE"),
        (
            scanner.clone(),
            "runtime/libexec/gstreamer-1.0/gst-plugin-scanner.exe",
        ),
    ] {
        fs::copy(source, stage.join(name))?;
    }

    let mut plugin_paths = Vec::new();
    for name in PLUGINS {
        let source = plugins.join(name);
        if !source.exists() {
            return Err(format!("required GStreamer plugin is missing: {name}").into());
        }
        fs::copy(&source, stage_plugins.join(name))?;
        plugin_paths.push(source);
    }
    let mut dll_index = HashMap::new();
    for entry in fs::read_dir(&bin)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
        {
            if let Some(name) = path.file_name() {
                dll_index.insert(name.to_string_lossy().to_ascii_lowercase(), path);
            }
        }
    }
    let mut pending = VecDeque::from([
        stage.join("uxplay.exe"),
        stage.join("uxplay-beacon.exe"),
        scanner,
    ]);
    pending.extend(plugin_paths);
    let mut visited = HashSet::new();
    let mut bundled = HashSet::new();
    let system32 =
        PathBuf::from(env::var_os("WINDIR").ok_or("WINDIR is unavailable")?).join("System32");
    while let Some(current) = pending.pop_front() {
        if !visited.insert(current.clone()) {
            continue;
        }
        for name in imported_dlls(&objdump, &current)? {
            if name.to_ascii_lowercase().starts_with("api-ms-win-") || system32.join(&name).exists()
            {
                continue;
            }
            let source = dll_index.get(&name.to_ascii_lowercase()).ok_or_else(|| {
                format!("unresolved DLL {name} imported by {}", current.display())
            })?;
            if bundled.insert(name.to_ascii_lowercase()) {
                fs::copy(source, stage.join(&name))?;
                pending.push_back(source.clone());
            }
        }
    }
    copy_tree_if_present(
        &ucrt.join("share/gstreamer-1.0"),
        &stage.join("runtime/share/gstreamer-1.0"),
    )?;
    copy_tree_if_present(
        &ucrt.join("share/glib-2.0/schemas"),
        &stage.join("runtime/share/glib-2.0/schemas"),
    )?;
    copy_tree_if_present(&ucrt.join("etc/ssl"), &stage.join("runtime/ssl"))?;
    write_runtime_notices(&pacman, &ucrt, &stage, &plugins, &dll_index, &bundled)?;

    let version = app_version;
    let mut setup = msys_cargo(&msys_root, &root);
    setup
        .env("RUSTUP_TOOLCHAIN", "stable-x86_64-pc-windows-gnu")
        .env("CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER", "/ucrt64/bin/gcc")
        .env("UXPLAY_STAGE_DIR", &stage)
        .env("UXPLAY_VERSION", &version)
        .env(
            "RUSTFLAGS",
            "-C target-feature=+crt-static -C link-arg=-static",
        );
    run_command(setup.args(["build"]).args([
        "--manifest-path",
        "packaging/windows-installer/Cargo.toml",
        "--release",
    ]))?;
    fs::copy(
        root.join("packaging/windows-installer/target/release/Setup.exe"),
        output.join("Setup.exe"),
    )?;
    let published_stage = output.join("stage");
    if published_stage.exists() {
        fs::remove_dir_all(&published_stage)?;
    }
    fs::rename(&stage, &published_stage)?;
    println!("Created {}", output.join("Setup.exe").display());
    Ok(())
}

fn msys_cargo(msys_root: &Path, root: &Path) -> Command {
    let cargo = env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join("cargo.exe"))
        .find(|candidate| candidate.is_file())
        .expect("cargo.exe must be available on PATH");
    let mut command = Command::new(msys_root.join("usr/bin/bash.exe"));
    command
        .current_dir(root)
        .env("UXPLAY_CARGO_EXE", cargo)
        .env("UXPLAY_REPO_ROOT", root)
        .args([
            "-lc",
            "export MSYSTEM=UCRT64; export MSYS2_PATH_TYPE=inherit; export MINGW_PREFIX=/ucrt64; export CHERE_INVOKING=1; export PATH=/ucrt64/bin:$PATH; cd \"$(cygpath -u \"$UXPLAY_REPO_ROOT\")\" || exit; cargo_exe=$(cygpath -u \"$UXPLAY_CARGO_EXE\"); exec \"$cargo_exe\" \"$@\"",
            "uxplay-msys-cargo",
        ]);
    command
}

fn smoke_runtime(stage: PathBuf, msys_root: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let stage = stage.canonicalize()?;
    let temp = env::temp_dir().join(format!("uxplay-runtime-smoke-{}", std::process::id()));
    if temp.exists() {
        fs::remove_dir_all(&temp)?;
    }
    fs::create_dir_all(&temp)?;
    let result = (|| {
        for entry in fs::read_dir(&stage)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
            {
                fs::copy(&path, temp.join(path.file_name().unwrap()))?;
            }
        }
        let ucrt = msys_root.join("ucrt64");
        fs::copy(
            ucrt.join("bin/gst-inspect-1.0.exe"),
            temp.join("gst-inspect-1.0.exe"),
        )?;
        copy_tree(&stage.join("runtime"), &temp.join("runtime"))?;
        let system_path = format!(
            "{};{};{}",
            temp.display(),
            env::var("WINDIR")? + "\\System32",
            env::var("WINDIR")?
        );
        let inspect = temp.join("gst-inspect-1.0.exe");
        for factory in [
            "appsrc",
            "decodebin",
            "avdec_aac",
            "avdec_alac",
            "h264parse",
            "h265parse",
            "audioconvert",
            "audioresample",
            "volume",
            "level",
            "autoaudiosink",
            "wasapisink",
            "videoconvert",
            "videoscale",
            "autovideosink",
            "playbin3",
            "hlsdemux",
        ] {
            let mut check = Command::new(&inspect);
            check
                .arg(factory)
                .env("PATH", &system_path)
                .env("GST_PLUGIN_SYSTEM_PATH_1_0", temp.join("runtime/plugins"))
                .env("GST_PLUGIN_PATH_1_0", temp.join("runtime/plugins"))
                .env(
                    "GST_PLUGIN_SCANNER",
                    temp.join("runtime/libexec/gstreamer-1.0/gst-plugin-scanner.exe"),
                )
                .env("GST_REGISTRY", temp.join("registry.bin"));
            run_command(&mut check)?;
            println!("PASS {factory}");
        }
        for (plugin, filename) in [("d3d11", "libgstd3d11.dll"), ("vulkan", "libgstvulkan.dll")] {
            if !temp.join("runtime/plugins").join(filename).exists() {
                return Err(format!("bundled plugin is missing: {filename}").into());
            }
            if plugin == "d3d11" {
                let mut check = Command::new(&inspect);
                check
                    .args(["--plugin", plugin])
                    .env("PATH", &system_path)
                    .env("GST_PLUGIN_SYSTEM_PATH_1_0", temp.join("runtime/plugins"))
                    .env("GST_PLUGIN_PATH_1_0", temp.join("runtime/plugins"))
                    .env(
                        "GST_PLUGIN_SCANNER",
                        temp.join("runtime/libexec/gstreamer-1.0/gst-plugin-scanner.exe"),
                    )
                    .env("GST_REGISTRY", temp.join("registry.bin"));
                run_command(&mut check)?;
            }
            println!("PASS {filename}");
        }
        let mut help = Command::new(stage.join("uxplay.exe"));
        help.arg("-h").env(
            "PATH",
            format!(
                "{};{};{}",
                stage.display(),
                env::var("WINDIR")? + "\\System32",
                env::var("WINDIR")?
            ),
        );
        run_command(&mut help)?;
        println!("PASS uxplay -h without MSYS2 PATH");
        Ok::<(), Box<dyn std::error::Error>>(())
    })();
    let _ = fs::remove_dir_all(&temp);
    result
}

fn test_install(setup: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    let setup = setup.canonicalize()?;
    let base = env::temp_dir().join(format!("uxplay-install-smoke-{}", std::process::id()));
    let install = base.join("install");
    let roaming = base.join("roaming");
    let local = base.join("local");
    let profile = base.join("profile");
    fs::create_dir_all(&roaming)?;
    fs::create_dir_all(&local)?;
    fs::create_dir_all(&profile)?;
    let environment = |command: &mut Command| {
        command
            .env("APPDATA", &roaming)
            .env("LOCALAPPDATA", &local)
            .env("USERPROFILE", &profile);
    };
    let result = (|| {
        let mut installer = Command::new(&setup);
        installer
            .args(["--quiet", "--install-dir"])
            .arg(&install)
            .arg("--autostart");
        environment(&mut installer);
        run_command(&mut installer)?;
        for required in [
            install.join("uxplay.exe"),
            local.join("UxPlay/uninstall.exe"),
            roaming.join("Microsoft/Windows/Start Menu/Programs/UxPlay/UxPlay.lnk"),
        ] {
            if !required.exists() {
                return Err(format!("installer did not create {}", required.display()).into());
            }
        }
        let config = fs::read_to_string(roaming.join("UxPlay/config.toml"))?;
        if !config.lines().any(|line| line.trim() == "enabled = true") {
            return Err("startup setting was not written to config.toml".into());
        }
        let root = RegKey::predef(HKEY_CURRENT_USER);
        let run = root.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")?;
        let startup: String = run.get_value("UxPlay")?;
        if !startup.contains("uxplay.exe") {
            return Err("Run registry entry does not point to uxplay.exe".into());
        }
        let uninstall_key = root.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{9AFA2E24-CA0D-44D7-B13C-22D3F5BA6935}_is1")?;
        let display_name: String = uninstall_key.get_value("DisplayName")?;
        if display_name != "UxPlayRusty" {
            return Err("uninstall registration has an unexpected display name".into());
        }
        drop(uninstall_key);
        let mut help = Command::new(install.join("uxplay.exe"));
        help.arg("-h").env(
            "PATH",
            format!(
                "{};{}\\System32;{}",
                install.display(),
                env::var("WINDIR")?,
                env::var("WINDIR")?
            ),
        );
        environment(&mut help);
        run_command(&mut help)?;
        let uninstaller = local.join("UxPlay/uninstall.exe");
        let mut uninstall = Command::new(&uninstaller);
        uninstall.args(["--uninstall", "--quiet"]);
        environment(&mut uninstall);
        run_command(&mut uninstall)?;
        if install.exists() {
            return Err("installation directory remains after uninstall".into());
        }
        let _ = fs::remove_file(&uninstaller);
        if run.get_value::<String, _>("UxPlay").is_ok() {
            return Err("startup Run value remains after uninstall".into());
        }
        if root.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{9AFA2E24-CA0D-44D7-B13C-22D3F5BA6935}_is1").is_ok() {
            return Err("uninstall registration remains after uninstall".into());
        }
        println!("PASS Rust Setup install, startup registration, shortcut and uninstall");
        Ok::<(), Box<dyn std::error::Error>>(())
    })();
    let _ = fs::remove_dir_all(base);
    result
}

fn test_beacon_shutdown(beacon: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let beacon = beacon.canonicalize()?;
    let scratch = env::temp_dir().join(format!("uxplay-winrt-smoke-{}", std::process::id()));
    fs::create_dir_all(&scratch)?;
    let handoff = scratch.join("handoff.ble");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&7101u16.to_ne_bytes());
    bytes.extend_from_slice(&std::process::id().to_ne_bytes());
    let owner_name = env::current_exe()?
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    bytes.extend_from_slice(owner_name.as_bytes());
    fs::write(&handoff, bytes)?;
    let output = scratch.join("beacon.log");
    let stderr = fs::File::create(&output)?;
    let mut process = Command::new(&beacon)
        .args(["--ipv4", "192.0.2.18", "--path"])
        .arg(&handoff)
        .arg("--exit-when-stopped")
        .stdout(Stdio::from(stderr.try_clone()?))
        .stderr(Stdio::from(stderr))
        .spawn()?;
    let start = std::time::Instant::now();
    let result = (|| {
        loop {
            if fs::read_to_string(&output)
                .unwrap_or_default()
                .contains("registered")
            {
                break;
            }
            if let Some(status) = process.try_wait()? {
                return Err(format!("beacon exited before advertising: {status}").into());
            }
            if start.elapsed() > std::time::Duration::from_secs(20) {
                return Err("beacon startup timed out".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        fs::remove_file(&handoff)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
        loop {
            if let Some(status) = process.try_wait()? {
                if !status.success() {
                    return Err(format!("beacon exited unsuccessfully: {status}").into());
                }
                break;
            }
            if std::time::Instant::now() > deadline {
                return Err("beacon did not stop after handoff removal".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        println!("PASS WinRT beacon shutdown after handoff removal");
        Ok::<(), Box<dyn std::error::Error>>(())
    })();
    if process.try_wait()?.is_none() {
        let _ = process.kill();
        let _ = process.wait();
    }
    let _ = fs::remove_dir_all(scratch);
    result
}

fn repository_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut path = env::current_dir()?;
    loop {
        if path.join("app/Cargo.toml").exists() && path.join(".git").exists() {
            return Ok(path);
        }
        if !path.pop() {
            return Err("could not locate repository root".into());
        }
    }
}
fn manifest_version(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let text = fs::read_to_string(path)?;
    for line in text.lines() {
        if let Some(value) = line.trim().strip_prefix("version = ") {
            return Ok(value.trim_matches('"').to_owned());
        }
    }
    Err(format!("package version not found in {}", path.display()).into())
}
fn run_command(command: &mut Command) -> Result<(), Box<dyn std::error::Error>> {
    let status = command.stdout(Stdio::null()).status()?;
    if !status.success() {
        return Err(format!("command failed with {status}: {command:?}").into());
    }
    Ok(())
}
fn imported_dlls(objdump: &Path, binary: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let output = Command::new(objdump).arg("-p").arg(binary).output()?;
    if !output.status.success() {
        return Err(format!("objdump failed for {}", binary.display()).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            line.split_once("DLL Name:")
                .map(|(_, name)| name.trim().to_owned())
        })
        .collect())
}
fn copy_tree_if_present(
    source: &Path,
    destination: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if source.exists() {
        copy_tree(source, destination)?;
    }
    Ok(())
}
fn copy_tree(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
fn write_runtime_notices(
    pacman: &Path,
    ucrt: &Path,
    stage: &Path,
    plugins: &Path,
    dll_index: &HashMap<String, PathBuf>,
    bundled: &HashSet<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut files: Vec<PathBuf> = PLUGINS.iter().map(|name| plugins.join(name)).collect();
    files.extend(
        bundled
            .iter()
            .filter_map(|name| dll_index.get(name).cloned()),
    );
    files.sort();
    files.dedup();
    let mut owners = HashSet::new();
    for file in files {
        let relative = file.strip_prefix(ucrt)?;
        let msys_path = format!("/ucrt64/{}", relative.to_string_lossy().replace('\\', "/"));
        let output = Command::new(pacman).args(["-Qoq", &msys_path]).output()?;
        if !output.status.success() {
            return Err(format!("cannot find MSYS2 package owner for {}", file.display()).into());
        }
        owners.insert(String::from_utf8(output.stdout)?.trim().to_owned());
    }
    let mut package_info = String::new();
    for owner in &owners {
        let output = Command::new(pacman).args(["-Qi", owner]).output()?;
        if !output.status.success() {
            return Err(format!("cannot read package metadata for {owner}").into());
        }
        package_info.push_str(&String::from_utf8_lossy(&output.stdout));
        package_info.push('\n');
    }
    let notice_root = stage.join("third-party-licenses");
    fs::create_dir_all(&notice_root)?;
    let license_root = ucrt.join("share/licenses");
    for owner in &owners {
        let short = owner
            .strip_prefix("mingw-w64-ucrt-x86_64-")
            .unwrap_or(owner);
        let candidate = [
            license_root.join(owner),
            license_root.join(short),
            ucrt.join("../usr/share/licenses").join(short),
        ]
        .into_iter()
        .find(|path| path.exists());
        let destination = notice_root.join(owner);
        if let Some(license) = candidate {
            copy_tree(&license, &destination)?;
        } else {
            fs::create_dir_all(&destination)?;
            fs::write(
                destination.join("LICENSE-REFERENCE.txt"),
                format!(
                    "Package: {owner}\nThe installed MSYS2 package did not provide a separate license text.\nSee THIRD-PARTY-NOTICES.txt for package SPDX and upstream metadata.\n"
                ),
            )?;
        }
    }
    fs::write(
        stage.join("THIRD-PARTY-NOTICES.txt"),
        format!(
            "UxPlayRusty includes runtime DLLs and GStreamer plugins from these MSYS2 UCRT64 packages.\nPackage versions, upstream URLs and SPDX identifiers follow.\n\n{package_info}"
        ),
    )?;
    Ok(())
}
