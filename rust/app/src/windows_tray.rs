//! Windows notification-area host for the normal no-argument application launch.

use crate::options::{Options, ParseOutcome};
use crate::server_runtime::{self, ServerControl};
use std::ffi::OsString;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::process::ExitCode;
use std::sync::Arc;
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole, GetConsoleWindow, SetStdHandle,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteValueW, RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentThreadId};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
    ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetMessageW, LoadIconW, MB_ICONERROR, MB_OK, MF_SEPARATOR, MF_STRING, MSG,
    MessageBoxW, PostMessageW, PostThreadMessageW, RegisterClassW, SetForegroundWindow,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TranslateMessage, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY,
    WM_LBUTTONDBLCLK, WM_RBUTTONUP, WNDCLASSW,
};

const CALLBACK_MESSAGE: u32 = WM_APP + 10;
const COMMAND_MESSAGE: u32 = WM_APP + 11;
const THREAD_COMMAND_MESSAGE: u32 = WM_APP + 12;
const SERVER_EXIT_MESSAGE: u32 = WM_APP + 13;
const SERVER_TIMER_ID: usize = 1;
const ID_CONFIG: usize = 1001;
const ID_RESTART: usize = 1002;
const ID_DISCONNECT: usize = 1003;
const ID_EXIT: usize = 1004;
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

static UI_THREAD_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static TASKBAR_CREATED_MESSAGE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static GUI_LOG_FILE: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);

pub fn attach_console_for_cli() {
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 && GetConsoleWindow().is_null() {
            AllocConsole();
        }
    }
}

pub fn initialize_gui_logging(configured_path: Option<&std::path::Path>) {
    let Some(appdata) = std::env::var_os("APPDATA") else {
        return;
    };
    let base = std::path::PathBuf::from(appdata).join("UxPlay");
    let log_path = configured_path.map_or_else(
        || base.join("logs").join("uxplay.log"),
        std::path::Path::to_owned,
    );
    let Some(directory) = log_path.parent() else {
        return;
    };
    if std::fs::create_dir_all(directory).is_err() {
        return;
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    else {
        return;
    };
    let handle = std::os::windows::io::AsRawHandle::as_raw_handle(&file);
    unsafe {
        SetStdHandle(
            windows_sys::Win32::System::Console::STD_OUTPUT_HANDLE,
            handle,
        );
        SetStdHandle(
            windows_sys::Win32::System::Console::STD_ERROR_HANDLE,
            handle,
        );
    }
    *GUI_LOG_FILE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(file);
}

pub fn show_error(text: &str) {
    message(text, true);
}

pub fn run(options: Options, argv: Vec<OsString>) -> ExitCode {
    UI_THREAD_ID.store(
        unsafe { GetCurrentThreadId() },
        std::sync::atomic::Ordering::Release,
    );
    let mutex_name = wide("Local\\UxPlayRusty.TrayHost");
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
    if mutex.is_null() {
        message("UxPlay could not create its single-instance lock.", true);
        return ExitCode::from(1);
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        message("UxPlay is already running in the notification area.", false);
        unsafe { windows_sys::Win32::Foundation::CloseHandle(mutex) };
        return ExitCode::SUCCESS;
    }

    let Some(hwnd) = create_tray_window() else {
        message("UxPlay could not create its notification-area icon.", true);
        unsafe { windows_sys::Win32::Foundation::CloseHandle(mutex) };
        return ExitCode::from(1);
    };

    if let Ok((_, _, config)) = crate::windows_config::load_or_create()
        && let Err(error) = set_startup(config.startup_enabled.unwrap_or(false))
    {
        message(&error, true);
    }

    let control = Arc::new(ServerControl::new());
    let mut server = Some(start_server(options, Arc::clone(&control)));
    let mut msg = unsafe { std::mem::zeroed::<MSG>() };
    let mut keep_running = true;
    while keep_running {
        let status = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if status <= 0 {
            break;
        }
        if msg.message == THREAD_COMMAND_MESSAGE {
            match msg.wParam {
                ID_CONFIG => open_config(hwnd),
                ID_DISCONNECT => control.request_disconnect(),
                ID_RESTART => {
                    if let Some(next_options) = validate_restart(&argv) {
                        if let Ok((_, _, config)) = crate::windows_config::load_or_create()
                            && let Err(error) = set_startup(config.startup_enabled.unwrap_or(false))
                        {
                            message(&error, true);
                            continue;
                        }
                        initialize_gui_logging(crate::windows_config::user_log_path().as_deref());
                        control.request_exit();
                        join_server(&mut server);
                        server = Some(start_server(next_options, Arc::clone(&control)));
                    }
                }
                ID_EXIT => keep_running = false,
                _ => {}
            }
            continue;
        }
        if msg.message == SERVER_EXIT_MESSAGE {
            if server
                .as_ref()
                .is_some_and(std::thread::JoinHandle::is_finished)
            {
                let status = server.take().and_then(|server| server.join().ok());
                if let Some(status) = status {
                    message(
                        &format!(
                            "UxPlay server stopped with exit status {status:?}.
Use Restart from the tray menu to try again."
                        ),
                        status != ExitCode::SUCCESS,
                    );
                }
            }
            continue;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    control.request_exit();
    join_server(&mut server);
    let icon = icon_data(hwnd);
    unsafe {
        Shell_NotifyIconW(NIM_DELETE, &icon);
        DestroyWindow(hwnd);
        windows_sys::Win32::Foundation::CloseHandle(mutex);
    }
    ExitCode::SUCCESS
}

fn start_server(
    options: Options,
    control: Arc<ServerControl>,
) -> std::thread::JoinHandle<ExitCode> {
    control.reset();
    std::thread::spawn(move || server_runtime::run_with_control(options, control))
}

fn join_server(server: &mut Option<std::thread::JoinHandle<ExitCode>>) {
    if let Some(server) = server.take() {
        let _ = server.join();
    }
}

fn validate_restart(argv: &[OsString]) -> Option<Options> {
    match crate::parse_startup_options(argv) {
        Some((ParseOutcome::Ready(success), config_error)) if config_error.is_empty() => {
            Some(success.options)
        }
        Some((ParseOutcome::Ready(_), config_error)) => {
            message(&config_error, true);
            None
        }
        Some((ParseOutcome::Exit(exit), _)) => {
            message(&format!("{}{}", exit.stdout, exit.stderr), true);
            None
        }
        None => {
            message("UxPlay could not reload its startup arguments.", true);
            None
        }
    }
}

fn create_tray_window() -> Option<HWND> {
    let class_name = wide("UxPlayRustyTrayHost");
    let title = wide("UxPlay");
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 && GetLastError() != 1410 {
            return None;
        }
        TASKBAR_CREATED_MESSAGE.store(
            windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW(
                wide("TaskbarCreated").as_ptr(),
            ),
            std::sync::atomic::Ordering::Release,
        );
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return None;
        }
        let icon = icon_data(hwnd);
        if Shell_NotifyIconW(NIM_ADD, &icon) == 0 {
            DestroyWindow(hwnd);
            return None;
        }
        windows_sys::Win32::UI::WindowsAndMessaging::SetTimer(hwnd, SERVER_TIMER_ID, 1500, None);
        Some(hwnd)
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message_id: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message_id == TASKBAR_CREATED_MESSAGE.load(std::sync::atomic::Ordering::Acquire)
        && message_id != 0
    {
        let icon = icon_data(hwnd);
        unsafe { Shell_NotifyIconW(NIM_ADD, &icon) };
        return 0;
    }
    match message_id {
        CALLBACK_MESSAGE => {
            if lparam as u32 == WM_RBUTTONUP || lparam as u32 == WM_CONTEXTMENU {
                show_menu(hwnd);
            } else if lparam as u32 == WM_LBUTTONDBLCLK {
                post_command(ID_CONFIG);
            }
            0
        }
        COMMAND_MESSAGE => {
            post_command(wparam);
            0
        }
        windows_sys::Win32::UI::WindowsAndMessaging::WM_TIMER if wparam == SERVER_TIMER_ID => {
            let thread_id = UI_THREAD_ID.load(std::sync::atomic::Ordering::Acquire);
            if thread_id != 0 {
                unsafe { PostThreadMessageW(thread_id, SERVER_EXIT_MESSAGE, 0, 0) };
            }
            0
        }
        WM_CLOSE => {
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_DESTROY => {
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message_id, wparam, lparam) },
    }
}

fn show_menu(hwnd: HWND) {
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        append(menu, ID_CONFIG, "Config");
        append(menu, ID_RESTART, "Restart");
        append(menu, ID_DISCONNECT, "Disconnect");
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        append(menu, ID_EXIT, "Exit");
        let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point);
        SetForegroundWindow(hwnd);
        let command = windows_sys::Win32::UI::WindowsAndMessaging::TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            hwnd,
            std::ptr::null(),
        );
        DestroyMenu(menu);
        if command != 0 {
            PostMessageW(hwnd, COMMAND_MESSAGE, command as usize, 0);
        }
    }
}

unsafe fn append(menu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU, id: usize, label: &str) {
    let label = wide(label);
    unsafe { AppendMenuW(menu, MF_STRING, id, label.as_ptr()) };
}

fn post_command(command: WPARAM) {
    let thread_id = UI_THREAD_ID.load(std::sync::atomic::Ordering::Acquire);
    if thread_id != 0 {
        unsafe { PostThreadMessageW(thread_id, THREAD_COMMAND_MESSAGE, command, 0) };
    }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut icon = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: CALLBACK_MESSAGE,
        hIcon: unsafe { LoadIconW(std::ptr::null_mut(), 32512usize as *const u16) },
        ..Default::default()
    };
    let tip = wide("UxPlay AirPlay Receiver");
    let tip_len = tip.len().min(icon.szTip.len());
    icon.szTip[..tip_len].copy_from_slice(&tip[..tip_len]);
    icon
}

fn open_config(hwnd: HWND) {
    let path = match crate::windows_config::config_path() {
        Ok(path) => path,
        Err(error) => {
            message(&error, true);
            return;
        }
    };
    let verb = wide("open");
    let path = wide(&path.to_string_lossy());
    let result = unsafe {
        ShellExecuteW(
            hwnd,
            verb.as_ptr(),
            path.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    };
    if (result as isize) <= 32 {
        message("Windows could not open the UxPlay config file.", true);
    }
}

fn set_startup(enabled: bool) -> Result<(), String> {
    let command = if enabled {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        Some(wide(&format!("\"{}\"", executable.display())))
    } else {
        None
    };
    let key_path = wide(RUN_KEY);
    let value_name = wide("UxPlay");
    let mut key = std::ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key_path.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    if status != 0 {
        return Err(format!(
            "could not open the current-user startup key (Windows error {status})"
        ));
    }
    let status = if enabled {
        let command = command
            .as_ref()
            .expect("enabled startup command is prepared");
        unsafe {
            RegSetValueExW(
                key,
                value_name.as_ptr(),
                0,
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * size_of::<u16>()) as u32,
            )
        }
    } else {
        let status = unsafe { RegDeleteValueW(key, value_name.as_ptr()) };
        if status == 2 { 0 } else { status }
    };
    unsafe { RegCloseKey(key) };
    if status == 0 {
        Ok(())
    } else {
        Err(format!(
            "could not update UxPlay startup registration (Windows error {status})"
        ))
    }
}

fn message(text: &str, is_error: bool) {
    let text = wide(text);
    let title = wide("UxPlay");
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | if is_error { MB_ICONERROR } else { 0 },
        );
    }
}

fn wide(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
