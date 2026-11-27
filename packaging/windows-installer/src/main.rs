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
    use windows_sys::Win32::Networking::WinHttp::{
        INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, WINHTTP_ADDREQ_FLAG_ADD,
        WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
        WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen,
        WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData,
        WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
    };
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};
    use windows_sys::Win32::Graphics::Gdi::UpdateWindow;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::Shell::{
        BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW, SHBrowseForFolderW,
        SHGetPathFromIDListW, ShellExecuteW,
    };
    use windows_sys::Win32::UI::Controls::{BST_CHECKED, CheckDlgButton};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BM_GETCHECK, DialogBoxParamW, EndDialog, GetDlgItemTextW, ICON_BIG,
        ICON_SMALL, IDYES, IMAGE_ICON, LR_DEFAULTCOLOR, MB_ICONERROR, MB_ICONINFORMATION,
        MB_ICONQUESTION, MB_OK, MB_YESNO, MessageBoxW, STM_SETIMAGE, SendDlgItemMessageW,
        SetDlgItemTextW, WM_CLOSE, WM_COMMAND, WM_INITDIALOG, WM_SETICON,
    };
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};

    include!(concat!(env!("OUT_DIR"), "/payload.rs"));

    const PRODUCT: &str = "UxPlayRusty";
    const IDD_INSTALLER: u16 = 101;
    const IDD_FIXER: u16 = 102;
    const IDC_LOGO: i32 = 1001;
    const IDC_INSTALL_PATH: i32 = 1002;
    const IDC_BROWSE: usize = 1003;
    const IDC_AUTOSTART: usize = 1004;
    const IDC_DESKTOP_SHORTCUT: usize = 1005;
    const IDC_FIXER_STATUS: i32 = 1006;
    const IDC_REPORT: usize = 1007;
    const IDC_RESET: usize = 1008;
    const IDC_CHECK_UPDATE: usize = 1009;
    const IDC_UNINSTALL: usize = 1010;
    const REPORT_URL: &str = "https://github.com/AtlasSolutionsMC/UxPlayRusty/issues/new";
    const RELEASE_URL: &str = "https://github.com/AtlasSolutionsMC/UxPlayRusty/releases/latest";
    const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{9AFA2E24-CA0D-44D7-B13C-22D3F5BA6935}_is1";
    const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

    struct InstallOptions {
        directory: PathBuf,
        autostart: bool,
        desktop_shortcut: bool,
    }

    struct DialogDefaults {
        path: Vec<u16>,
        autostart: bool,
        desktop_shortcut: bool,
    }

    static INSTALL_OPTIONS: std::sync::Mutex<Option<InstallOptions>> = std::sync::Mutex::new(None);
    static INSTALL_DIALOG_DEFAULTS: std::sync::Mutex<Option<DialogDefaults>> =
        std::sync::Mutex::new(None);

    pub fn main() {
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let args: Vec<String> = env::args().skip(1).collect();
        let result = if args.iter().any(|arg| arg == "--uninstall") {
            uninstall(&args)
        } else if !args.iter().any(|arg| arg == "--quiet") && installed_directory().is_some() {
            show_fixer_dialog()
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
        let mut install_directory = option_value(args, "--install-dir")
            .map(PathBuf::from)
            .unwrap_or(default_directory);
        let mut autostart = args.iter().any(|arg| arg == "--autostart");
        let mut desktop_shortcut = args.iter().any(|arg| arg == "--desktop-shortcut");
        let quiet = args.iter().any(|arg| arg == "--quiet");

        if !quiet {
            let Some(options) =
                show_install_dialog(&install_directory, autostart, desktop_shortcut)?
            else {
                return Ok(());
            };
            install_directory = options.directory;
            autostart = options.autostart;
            desktop_shortcut = options.desktop_shortcut;
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
        let installed_uninstaller = uninstaller_path()?;
        if current
            .to_string_lossy()
            .eq_ignore_ascii_case(&installed_uninstaller.to_string_lossy())
        {
            let wide: Vec<u16> = current.as_os_str().encode_wide().chain(Some(0)).collect();
            unsafe {
                MoveFileExW(wide.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT);
            }
        } else {
            let _ = fs::remove_file(installed_uninstaller);
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
                link.SetIconLocation(
                    &windows::core::HSTRING::from(target.as_os_str().to_string_lossy().as_ref()),
                    0,
                )?;
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

    fn installed_directory() -> Option<PathBuf> {
        let root = RegKey::predef(HKEY_CURRENT_USER);
        let uninstall = root.open_subkey(UNINSTALL_KEY).ok()?;
        let directory = PathBuf::from(uninstall.get_value::<String, _>("InstallLocation").ok()?);
        directory.join("uxplay.exe").is_file().then_some(directory)
    }

    fn installed_version() -> Option<String> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(UNINSTALL_KEY)
            .ok()?
            .get_value("DisplayVersion")
            .ok()
    }

    fn reset_user_settings() -> Result<(), Box<dyn std::error::Error>> {
        let directory = installed_directory().ok_or("UxPlay is not installed")?;
        let appdata = env::var_os("APPDATA").ok_or("APPDATA is not available")?;
        let config = PathBuf::from(appdata).join("UxPlay/config.toml");
        let defaults = PAYLOAD
            .iter()
            .find(|(name, _)| *name == "config.example.toml")
            .ok_or("config.example.toml is missing from the installer")?
            .1;
        let mut decoder = GzDecoder::new(Cursor::new(defaults));
        let mut contents = Vec::new();
        decoder.read_to_end(&mut contents)?;
        if let Some(parent) = config.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(config, contents)?;
        set_startup_entry(&directory, false)?;
        Ok(())
    }

    fn set_dialog_text(dialog: windows_sys::Win32::Foundation::HWND, id: i32, text: &str) {
        let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        unsafe { SetDlgItemTextW(dialog, id, text.as_ptr()) };
    }

    fn open_url(dialog: windows_sys::Win32::Foundation::HWND, url: &str) {
        let operation: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
        let url: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
        unsafe {
            ShellExecuteW(
                dialog,
                operation.as_ptr(),
                url.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            );
        }
    }

    struct WinHttpHandle(*mut std::ffi::c_void);

    impl Drop for WinHttpHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }

    fn latest_release_version() -> Result<String, Box<dyn std::error::Error>> {
        let agent = wide("UxPlayRusty updater");
        let server = wide("api.github.com");
        let path = wide("/repos/AtlasSolutionsMC/UxPlayRusty/releases/latest");
        let session = unsafe {
            WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
                std::ptr::null(),
                std::ptr::null(),
                0,
            )
        };
        if session.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let session = WinHttpHandle(session);
        unsafe { WinHttpSetTimeouts(session.0, 4000, 5000, 5000, 5000) };
        let connection =
            unsafe { WinHttpConnect(session.0, server.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0) };
        if connection.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let connection = WinHttpHandle(connection);
        let method = wide("GET");
        let request = unsafe {
            WinHttpOpenRequest(
                connection.0,
                method.as_ptr(),
                path.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            )
        };
        if request.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let request = WinHttpHandle(request);
        let headers = wide("Accept: application/vnd.github+json\r\nUser-Agent: UxPlayRusty\r\n");
        let headers_len = headers.len().saturating_sub(1) as u32;
        if unsafe {
            WinHttpAddRequestHeaders(
                request.0,
                headers.as_ptr(),
                headers_len,
                WINHTTP_ADDREQ_FLAG_ADD,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        if unsafe { WinHttpSendRequest(request.0, std::ptr::null(), 0, std::ptr::null(), 0, 0, 0) }
            == 0
            || unsafe { WinHttpReceiveResponse(request.0, std::ptr::null_mut()) } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut status = 0u32;
        let mut status_size = std::mem::size_of::<u32>() as u32;
        if unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                std::ptr::null(),
                (&mut status as *mut u32).cast(),
                &mut status_size,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        if status != 200 {
            if status == 404 {
                return Err("No GitHub release has been published yet".into());
            }
            return Err(format!("GitHub returned HTTP {status}").into());
        }
        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            if unsafe { WinHttpQueryDataAvailable(request.0, &mut available) } == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if available == 0 {
                break;
            }
            if body.len().saturating_add(available as usize) > 1024 * 1024 {
                return Err("GitHub release response is unexpectedly large".into());
            }
            let offset = body.len();
            body.resize(offset + available as usize, 0);
            let mut read = 0u32;
            if unsafe {
                WinHttpReadData(
                    request.0,
                    body[offset..].as_mut_ptr().cast(),
                    available,
                    &mut read,
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            body.truncate(offset + read as usize);
        }
        let body = String::from_utf8_lossy(&body);
        let tag = json_string_field(&body, "tag_name")
            .ok_or("GitHub response did not include a release tag")?;
        Ok(normalized_version(&tag).to_owned())
    }

    fn normalized_version(version: &str) -> &str {
        version.trim_start_matches(|character| character == 'v' || character == 'V')
    }

    fn is_newer_version(candidate: &str, installed: &str) -> bool {
        let parse = |version: &str| -> Option<[u64; 3]> {
            let core = normalized_version(version)
                .split(|character| character == '-' || character == '+')
                .next()?;
            let mut components = core.split('.');
            Some([
                components.next()?.parse().ok()?,
                components.next()?.parse().ok()?,
                components.next()?.parse().ok()?,
            ])
        };
        match (parse(candidate), parse(installed)) {
            (Some(candidate), Some(installed)) if candidate != installed => candidate > installed,
            (Some(_), Some(_)) => installed.contains('-') && !candidate.contains('-'),
            _ => normalized_version(candidate) != normalized_version(installed),
        }
    }

    fn json_string_field(json: &str, name: &str) -> Option<String> {
        let marker = format!("\"{name}\"");
        let value = json.get(json.find(&marker)? + marker.len()..)?;
        let value = value.get(value.find(':')? + 1..)?.trim_start();
        let value = value.strip_prefix('"')?;
        let end = value.find('"')?;
        Some(value[..end].to_owned())
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    fn show_install_dialog(
        directory: &Path,
        autostart: bool,
        desktop_shortcut: bool,
    ) -> Result<Option<InstallOptions>, Box<dyn std::error::Error>> {
        *INSTALL_OPTIONS.lock().unwrap() = None;
        *INSTALL_DIALOG_DEFAULTS.lock().unwrap() = Some(DialogDefaults {
            path: wide_path(directory),
            autostart,
            desktop_shortcut,
        });
        let module = unsafe { GetModuleHandleW(std::ptr::null()) };
        let result = unsafe {
            DialogBoxParamW(
                module,
                IDD_INSTALLER as usize as *const u16,
                std::ptr::null_mut(),
                Some(install_dialog_proc),
                0,
            )
        };
        if result == 1 {
            Ok(INSTALL_OPTIONS.lock().unwrap().take())
        } else if result == 2 {
            Ok(None)
        } else {
            Err(std::io::Error::last_os_error().into())
        }
    }

    fn show_fixer_dialog() -> Result<(), Box<dyn std::error::Error>> {
        let module = unsafe { GetModuleHandleW(std::ptr::null()) };
        let result = unsafe {
            DialogBoxParamW(
                module,
                IDD_FIXER as usize as *const u16,
                std::ptr::null_mut(),
                Some(install_dialog_proc),
                1,
            )
        };
        if result == -1 {
            Err(std::io::Error::last_os_error().into())
        } else {
            Ok(())
        }
    }

    unsafe extern "system" fn install_dialog_proc(
        dialog: windows_sys::Win32::Foundation::HWND,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        match message {
            WM_INITDIALOG => {
                let module = unsafe { GetModuleHandleW(std::ptr::null()) };
                let icon = unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::LoadImageW(
                        module,
                        1usize as *const u16,
                        IMAGE_ICON,
                        64,
                        64,
                        LR_DEFAULTCOLOR,
                    )
                };
                if !icon.is_null() {
                    let small_icon = unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::LoadIconW(
                            module,
                            1usize as *const u16,
                        )
                    };
                    unsafe {
                        SendDlgItemMessageW(
                            dialog,
                            IDC_LOGO,
                            STM_SETIMAGE,
                            IMAGE_ICON as usize,
                            icon as isize,
                        );
                        windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW(
                            dialog,
                            WM_SETICON,
                            ICON_BIG as usize,
                            icon as isize,
                        );
                        windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW(
                            dialog,
                            WM_SETICON,
                            ICON_SMALL as usize,
                            small_icon as isize,
                        );
                    }
                }
                if lparam == 1 {
                    let version = installed_version().unwrap_or_else(|| PRODUCT_VERSION.to_owned());
                    set_dialog_text(
                        dialog,
                        IDC_FIXER_STATUS,
                        &format!("Installed version: {version}"),
                    );
                    return 1;
                }
                let defaults = INSTALL_DIALOG_DEFAULTS.lock().unwrap();
                unsafe {
                    let defaults = defaults.as_ref().expect("dialog defaults are initialized");
                    SetDlgItemTextW(dialog, IDC_INSTALL_PATH, defaults.path.as_ptr());
                    CheckDlgButton(
                        dialog,
                        IDC_AUTOSTART as i32,
                        if defaults.autostart { BST_CHECKED } else { 0 },
                    );
                    CheckDlgButton(
                        dialog,
                        IDC_DESKTOP_SHORTCUT as i32,
                        if defaults.desktop_shortcut {
                            BST_CHECKED
                        } else {
                            0
                        },
                    );
                }
                1
            }
            WM_COMMAND if (wparam & 0xffff) == IDC_BROWSE => {
                browse_for_directory(dialog);
                1
            }
            WM_COMMAND if (wparam & 0xffff) == 1 => {
                let mut path = vec![0u16; 32768];
                let length = unsafe {
                    GetDlgItemTextW(
                        dialog,
                        IDC_INSTALL_PATH,
                        path.as_mut_ptr(),
                        path.len() as i32,
                    )
                } as usize;
                path.truncate(length);
                let directory = PathBuf::from(String::from_utf16_lossy(&path));
                if !directory.as_os_str().is_empty() {
                    let options = InstallOptions {
                        directory,
                        autostart: unsafe {
                            SendDlgItemMessageW(dialog, IDC_AUTOSTART as i32, BM_GETCHECK, 0, 0)
                                as u32
                                == BST_CHECKED
                        },
                        desktop_shortcut: unsafe {
                            SendDlgItemMessageW(
                                dialog,
                                IDC_DESKTOP_SHORTCUT as i32,
                                BM_GETCHECK,
                                0,
                                0,
                            ) as u32
                                == BST_CHECKED
                        },
                    };
                    *INSTALL_OPTIONS.lock().unwrap() = Some(options);
                    unsafe { EndDialog(dialog, 1) };
                }
                1
            }
            WM_COMMAND if (wparam & 0xffff) == 2 => {
                unsafe { EndDialog(dialog, 2) };
                1
            }
            WM_COMMAND if (wparam & 0xffff) == IDC_REPORT => {
                open_url(dialog, REPORT_URL);
                1
            }
            WM_COMMAND if (wparam & 0xffff) == IDC_RESET => {
                if confirm(
                    "Restore default settings? Existing AirPlay pairings will be kept.",
                    "UxPlay Fixer",
                ) {
                    match reset_user_settings() {
                        Ok(()) => set_dialog_text(
                            dialog,
                            IDC_FIXER_STATUS,
                            "Settings restored to defaults.",
                        ),
                        Err(error) => set_dialog_text(
                            dialog,
                            IDC_FIXER_STATUS,
                            &format!("Could not reset settings: {error}"),
                        ),
                    }
                }
                1
            }
            WM_COMMAND if (wparam & 0xffff) == IDC_CHECK_UPDATE => {
                set_dialog_text(
                    dialog,
                    IDC_FIXER_STATUS,
                    "Checking GitHub for the latest release...",
                );
                unsafe { UpdateWindow(dialog) };
                match latest_release_version() {
                    Ok(version) => {
                        let current =
                            installed_version().unwrap_or_else(|| PRODUCT_VERSION.to_owned());
                        if normalized_version(&version) == normalized_version(&current) {
                            set_dialog_text(
                                dialog,
                                IDC_FIXER_STATUS,
                                &format!("UxPlay is up to date ({version})."),
                            );
                        } else if is_newer_version(&version, &current) {
                            set_dialog_text(
                                dialog,
                                IDC_FIXER_STATUS,
                                &format!(
                                    "New release {version} is available. Opening the release page..."
                                ),
                            );
                            open_url(dialog, RELEASE_URL);
                        } else {
                            set_dialog_text(
                                dialog,
                                IDC_FIXER_STATUS,
                                &format!(
                                    "Installed version {current} is newer than latest release {version}."
                                ),
                            );
                        }
                    }
                    Err(error) => set_dialog_text(
                        dialog,
                        IDC_FIXER_STATUS,
                        &format!("Could not check for updates: {error}"),
                    ),
                }
                1
            }
            WM_COMMAND if (wparam & 0xffff) == IDC_UNINSTALL => {
                if confirm(
                    "Uninstall UxPlay? Your settings and AirPlay pairings will be kept.",
                    "UxPlay Fixer",
                ) {
                    let args = ["--uninstall".to_owned(), "--quiet".to_owned()];
                    match uninstall(&args) {
                        Ok(()) => {
                            unsafe { EndDialog(dialog, 2) };
                        }
                        Err(error) => set_dialog_text(
                            dialog,
                            IDC_FIXER_STATUS,
                            &format!("Could not uninstall UxPlay: {error}"),
                        ),
                    };
                }
                1
            }
            WM_CLOSE => {
                unsafe { EndDialog(dialog, 2) };
                1
            }
            _ => 0,
        }
    }

    fn browse_for_directory(dialog: windows_sys::Win32::Foundation::HWND) {
        let title: Vec<u16> = "Select the installation folder"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut display_name = vec![0u16; 260];
        let browse = BROWSEINFOW {
            hwndOwner: dialog,
            pidlRoot: std::ptr::null_mut(),
            pszDisplayName: display_name.as_mut_ptr(),
            lpszTitle: title.as_ptr(),
            ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
            lpfn: None,
            lParam: 0,
            iImage: 0,
        };
        let selected = unsafe { SHBrowseForFolderW(&browse) };
        if selected.is_null() {
            return;
        }
        let mut path = vec![0u16; 32768];
        let found = unsafe { SHGetPathFromIDListW(selected, path.as_mut_ptr()) } != 0;
        unsafe { CoTaskMemFree(selected.cast()) };
        if found {
            unsafe { SetDlgItemTextW(dialog, IDC_INSTALL_PATH, path.as_ptr()) };
        }
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
