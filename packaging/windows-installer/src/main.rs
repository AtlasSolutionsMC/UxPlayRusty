#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("UxPlay Setup can only be built for Windows.");
    std::process::exit(1);
}

#[cfg(windows)]
mod windows_installer {
    use flate2::read::GzDecoder;
    use std::{
        env, fs,
        io::{Cursor, Read},
        os::windows::ffi::OsStrExt,
        path::{Path, PathBuf},
        process::Command,
    };
    use windows::{
        Win32::{
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize, IPersistFile,
            },
            UI::Shell::{IShellLinkW, ShellLink},
        },
        core::{Interface, PCWSTR},
    };
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNO, MessageBoxW,
    };
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};

    include!(concat!(env!("OUT_DIR"), "/payload.rs"));

    const PRODUCT: &str = "UxPlayRusty";
    const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{9AFA2E24-CA0D-44D7-B13C-22D3F5BA6935}_is1";
    const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

    pub fn main() {
        let args: Vec<String> = env::args().skip(1).collect();
        let result = if args.iter().any(|arg| arg == "--uninstall") {
            uninstall(&args)
        } else {
            install(&args)
        };
        if let Err(error) = result {
            if args.iter().any(|arg| arg == "--quiet") {
                if let Some(temp) = env::var_os("TEMP") {
                    let _ = fs::write(
                        PathBuf::from(temp).join("UxPlay-Setup-error.txt"),
                        error.to_string(),
                    );
                }
            } else {
                show_message(
                    &format!("UxPlay setup failed.\n\n{error}"),
                    "UxPlay Setup",
                    MB_OK | MB_ICONERROR,
                );
            }
            std::process::exit(1);
        }
    }

    fn install(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
        let default_directory = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or("LOCALAPPDATA is not available")?
            .join("Programs")
            .join("UxPlay");
        let install_directory = option_value(args, "--install-dir")
            .map(PathBuf::from)
            .unwrap_or(default_directory);
        let mut autostart = args.iter().any(|arg| arg == "--autostart");
        let mut desktop_shortcut = args.iter().any(|arg| arg == "--desktop-shortcut");
        let quiet = args.iter().any(|arg| arg == "--quiet");

        if !quiet {
            let message = format!(
                "Install UxPlay to this folder?\n\n{}",
                install_directory.display()
            );
            if !confirm(&message, "UxPlay Setup") {
                return Ok(());
            }
            if !args.iter().any(|arg| arg == "--autostart") {
                autostart = confirm("Start UxPlay when you sign in to Windows?", "UxPlay Setup");
            }
            if !args.iter().any(|arg| arg == "--desktop-shortcut") {
                desktop_shortcut = confirm("Create a desktop shortcut?", "UxPlay Setup");
            }
        }

        fs::create_dir_all(&install_directory)?;
        for (relative, bytes) in PAYLOAD {
            let destination = install_directory.join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut decoder = GzDecoder::new(Cursor::new(bytes));
            let mut contents = Vec::new();
            decoder.read_to_end(&mut contents)?;
            fs::write(destination, contents)?;
        }
        let setup_path = env::current_exe()?;
        let uninstaller = uninstaller_path()?;
        fs::create_dir_all(
            uninstaller
                .parent()
                .ok_or("uninstaller path has no parent")?,
        )?;
        fs::copy(setup_path, &uninstaller)?;
        update_startup_config(&autostart)?;
        set_startup_entry(&install_directory, autostart)?;
        create_shortcut(
            &start_menu_shortcut()?,
            &install_directory.join("uxplay.exe"),
        )?;
        if desktop_shortcut {
            if let Some(desktop) = env::var_os("USERPROFILE") {
                create_shortcut(
                    &PathBuf::from(desktop).join("Desktop").join("UxPlay.lnk"),
                    &install_directory.join("uxplay.exe"),
                )?;
            }
        }
        register_uninstaller(&install_directory, &uninstaller)?;
        if !quiet {
            show_message(
                "UxPlay was installed successfully.",
                "UxPlay Setup",
                MB_OK | MB_ICONINFORMATION,
            );
        }
        if !quiet {
            let _ = Command::new(install_directory.join("uxplay.exe")).spawn();
        }
        Ok(())
    }

    fn uninstall(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
        if !args.iter().any(|arg| arg == "--quiet")
            && !confirm("Uninstall UxPlay from this computer?", "UxPlay Uninstaller")
        {
            return Ok(());
        }
        let root = RegKey::predef(HKEY_CURRENT_USER);
        let uninstall = root.open_subkey(UNINSTALL_KEY)?;
        let install_directory = PathBuf::from(uninstall.get_value::<String, _>("InstallLocation")?);
        drop(uninstall);
        if let Ok(run) = root.open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_WRITE) {
            let _ = run.delete_value("UxPlay");
        }
        let _ = root.delete_subkey_all(UNINSTALL_KEY);
        if let Some(appdata) = env::var_os("APPDATA") {
            let _ = fs::remove_file(PathBuf::from(appdata).join("UxPlay").join("uxplay.ble"));
        }
        let _ = fs::remove_file(start_menu_shortcut()?);
        let _ = fs::remove_file(
            env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join("Desktop")
                .join("UxPlay.lnk"),
        );
        let _ = fs::remove_dir_all(&install_directory);
        let current = env::current_exe()?;
        let wide: Vec<u16> = current.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            MoveFileExW(wide.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT);
        }
        if !args.iter().any(|arg| arg == "--quiet") {
            show_message(
                "UxPlay has been uninstalled. User configuration was preserved.",
                "UxPlay Uninstaller",
                MB_OK | MB_ICONINFORMATION,
            );
        }
        Ok(())
    }

    fn update_startup_config(enabled: &bool) -> Result<(), Box<dyn std::error::Error>> {
        let appdata = env::var_os("APPDATA").ok_or("APPDATA is not available")?;
        let directory = PathBuf::from(appdata).join("UxPlay");
        fs::create_dir_all(&directory)?;
        let config = directory.join("config.toml");
        if !config.exists() {
            let compressed = PAYLOAD
                .iter()
                .find(|(name, _)| *name == "config.example.toml")
                .ok_or("config.example.toml is missing")?
                .1;
            let mut decoder = GzDecoder::new(Cursor::new(compressed));
            let mut contents = Vec::new();
            decoder.read_to_end(&mut contents)?;
            fs::write(&config, contents)?;
        }
        let mut text = fs::read_to_string(&config)?;
        let value = format!("enabled = {enabled}");
        let lines: Vec<&str> = text.lines().collect();
        let mut in_startup = false;
        let mut startup_found = false;
        let mut replaced = false;
        let mut output = Vec::new();
        for line in lines {
            if line.trim() == "[startup]" {
                in_startup = true;
                startup_found = true;
            } else if line.trim().starts_with('[') {
                if in_startup && !replaced {
                    output.push(value.as_str());
                    replaced = true;
                }
                in_startup = false;
            }
            if in_startup && line.trim_start().starts_with("enabled =") {
                output.push(value.as_str());
                replaced = true;
            } else {
                output.push(line);
            }
        }
        if !replaced {
            if startup_found {
                output.push(value.as_str());
            } else {
                output.extend(["", "[startup]", value.as_str()]);
            }
        }
        text = output.join("\n");
        fs::write(config, text)?;
        Ok(())
    }

    fn set_startup_entry(
        directory: &Path,
        enabled: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = RegKey::predef(HKEY_CURRENT_USER);
        let (run, _) = root.create_subkey(RUN_KEY)?;
        if enabled {
            run.set_value(
                "UxPlay",
                &format!("\"{}\"", directory.join("uxplay.exe").display()),
            )?;
        } else {
            let _ = run.delete_value("UxPlay");
        }
        Ok(())
    }

    fn register_uninstaller(
        directory: &Path,
        uninstaller: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = root.create_subkey(UNINSTALL_KEY)?;
        key.set_value("DisplayName", &PRODUCT)?;
        key.set_value("DisplayVersion", &PRODUCT_VERSION)?;
        key.set_value("InstallLocation", &directory.to_string_lossy().to_string())?;
        key.set_value(
            "UninstallString",
            &format!("\"{}\" --uninstall", uninstaller.display()),
        )?;
        key.set_value("NoModify", &1u32)?;
        key.set_value("NoRepair", &1u32)?;
        Ok(())
    }

    fn create_shortcut(link_path: &Path, target: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let directory = link_path
            .parent()
            .ok_or("shortcut has no parent directory")?;
        fs::create_dir_all(directory)?;
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
            let result = (|| -> windows::core::Result<()> {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                link.SetPath(&windows::core::HSTRING::from(
                    target.as_os_str().to_string_lossy().as_ref(),
                ))?;
                link.SetWorkingDirectory(&windows::core::HSTRING::from(
                    target
                        .parent()
                        .unwrap_or(Path::new("."))
                        .as_os_str()
                        .to_string_lossy()
                        .as_ref(),
                ))?;
                let persist: IPersistFile = link.cast()?;
                persist.Save(PCWSTR(wide_path(link_path).as_ptr()), true)?;
                Ok(())
            })();
            CoUninitialize();
            result?;
        }
        Ok(())
    }

    fn wide_path(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }
    fn start_menu_shortcut() -> Result<PathBuf, Box<dyn std::error::Error>> {
        Ok(
            PathBuf::from(env::var_os("APPDATA").ok_or("APPDATA is not available")?)
                .join("Microsoft\\Windows\\Start Menu\\Programs\\UxPlay\\UxPlay.lnk"),
        )
    }
    fn uninstaller_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
        Ok(
            PathBuf::from(env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is not available")?)
                .join("UxPlay")
                .join("uninstall.exe"),
        )
    }
    fn option_value<'a>(args: &'a [String], option: &str) -> Option<&'a str> {
        args.iter()
            .position(|arg| arg == option)
            .and_then(|index| args.get(index + 1))
            .map(String::as_str)
    }
    fn confirm(message: &str, title: &str) -> bool {
        show_message(message, title, MB_YESNO | MB_ICONQUESTION) == IDYES
    }
    fn show_message(message: &str, title: &str, flags: u32) -> i32 {
        let message: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
        let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                flags,
            )
        }
    }
}

#[cfg(windows)]
fn main() {
    windows_installer::main();
}
