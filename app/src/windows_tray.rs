//! Windows notification-area host for the normal no-argument application launch.

use crate::options::{Options, ParseOutcome};
use crate::server_runtime::{self, ServerControl};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, ExitCode};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
    STD_OUTPUT_HANDLE, SetStdHandle,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteValueW, RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{CREATE_NEW_CONSOLE, CreateMutexW, GetCurrentThreadId};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NIN_BALLOONUSERCLICK, NOTIFYICONDATAW, Shell_NotifyIconW, ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetMessageW, IDYES, LoadIconW, MB_ICONERROR, MB_OK, MB_YESNO, MF_SEPARATOR,
    MF_STRING, MSG, MessageBoxW, PostMessageW, PostThreadMessageW, RegisterClassW,
    SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TranslateMessage, WM_APP, WM_CLOSE,
    WM_CONTEXTMENU, WM_DESTROY, WM_ENDSESSION, WM_LBUTTONDBLCLK, WM_QUERYENDSESSION, WM_RBUTTONUP,
    WNDCLASSW,
};

const CALLBACK_MESSAGE: u32 = WM_APP + 10;
const COMMAND_MESSAGE: u32 = WM_APP + 11;
const THREAD_COMMAND_MESSAGE: u32 = WM_APP + 12;
const SERVER_EXIT_MESSAGE: u32 = WM_APP + 13;
const PAIRING_PIN_MESSAGE: u32 = WM_APP + 14;
const SERVER_TIMER_ID: usize = 1;
const ID_CONFIG: usize = 1001;
const ID_RESTART: usize = 1002;
const ID_DISCONNECT: usize = 1003;
const ID_EXIT: usize = 1004;
const ID_HIDE_TERMINAL: usize = 1005;
const ID_SHOW_TERMINAL: usize = 1006;
const ID_RESET_PAIRED: usize = 1007;
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const DEFAULT_IDLE_RESTART_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

static UI_THREAD_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static TASKBAR_CREATED_MESSAGE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static GUI_LOG_FILE: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);
static GUI_LOG_PATH: std::sync::Mutex<Option<(std::path::PathBuf, std::path::PathBuf)>> =
    std::sync::Mutex::new(None);
static NOTIFICATIONS_ENABLED: AtomicBool = AtomicBool::new(true);
static PENDING_PAIRING_PIN: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Send the generated PIN to the tray UI thread for a Windows notification.
pub fn notify_pairing_pin(pin: &str) {
    if !NOTIFICATIONS_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let thread_id = UI_THREAD_ID.load(Ordering::Acquire);
    if thread_id == 0 {
        return;
    }
    *PENDING_PAIRING_PIN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pin.to_owned());
    unsafe { PostThreadMessageW(thread_id, PAIRING_PIN_MESSAGE, 0, 0) };
}

pub fn attach_console_for_cli() {
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0
            && windows_sys::Win32::System::Console::GetConsoleWindow().is_null()
        {
            AllocConsole();
        }
    }
}

/// Attach the internal log viewer to the new console and bind its standard
/// streams to that console, even though uxplay.exe uses the Windows GUI subsystem.
pub fn attach_console_for_log_terminal() {
    attach_console_for_cli();
    let input_name = wide("CONIN$");
    let output_name = wide("CONOUT$");
    unsafe {
        let input = CreateFileW(
            input_name.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        if input != INVALID_HANDLE_VALUE {
            SetStdHandle(STD_INPUT_HANDLE, input);
        }
        let output = CreateFileW(
            output_name.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        if output != INVALID_HANDLE_VALUE {
            SetStdHandle(STD_OUTPUT_HANDLE, output);
            SetStdHandle(STD_ERROR_HANDLE, output);
        }
    }
}

/// Prefer the private runtime shipped beside the installed executable.
/// Development builds without this layout keep using the system GStreamer paths.
pub fn prepare_bundled_runtime() {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let Some(install_dir) = executable.parent() else {
        return;
    };
    let runtime_dir = install_dir.join("runtime");
    let plugin_dir = runtime_dir.join("plugins");
    let scanner = runtime_dir
        .join("libexec")
        .join("gstreamer-1.0")
        .join("gst-plugin-scanner.exe");
    if !plugin_dir.is_dir() || !scanner.is_file() {
        return;
    }

    let mut search_path = vec![install_dir.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        search_path.extend(std::env::split_paths(&existing));
    }
    if let Ok(search_path) = std::env::join_paths(search_path) {
        unsafe { std::env::set_var("PATH", search_path) };
    }
    let registry = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .map(|path| {
            path.join("UxPlay")
                .join("cache")
                .join("gstreamer-registry.bin")
        });
    if let Some(registry) = registry {
        if let Some(parent) = registry.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        unsafe { std::env::set_var("GST_REGISTRY", registry) };
    }
    unsafe {
        std::env::set_var("GST_PLUGIN_PATH_1_0", &plugin_dir);
        std::env::set_var("GST_PLUGIN_SYSTEM_PATH_1_0", &plugin_dir);
        std::env::set_var("GST_PLUGIN_SCANNER", scanner);
    }
}

pub fn initialize_gui_logging(configured_path: Option<&std::path::Path>) {
    let _ = gui_log_path(configured_path);
}

fn gui_log_path(configured_path: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
    let configured_path = match configured_path {
        Some(path) => path.to_owned(),
        None => std::path::PathBuf::from(std::env::var_os("APPDATA")?)
            .join("UxPlay")
            .join("logs")
            .join("uxplay.log"),
    };
    let mut active = GUI_LOG_PATH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((active_base, active_path)) = active.as_ref() {
        if active_base == &configured_path {
            return Some(active_path.clone());
        }
    }

    let directory = configured_path.parent()?;
    if std::fs::create_dir_all(directory).is_err() {
        return None;
    }
    let timestamp = local_log_timestamp();
    for suffix in 0..100_u32 {
        let log_path = timestamped_log_path(&configured_path, &timestamp, suffix);
        let file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&log_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        };
        let handle = std::os::windows::io::AsRawHandle::as_raw_handle(&file);
        unsafe {
            windows_sys::Win32::System::Console::SetStdHandle(
                windows_sys::Win32::System::Console::STD_OUTPUT_HANDLE,
                handle,
            );
            windows_sys::Win32::System::Console::SetStdHandle(
                windows_sys::Win32::System::Console::STD_ERROR_HANDLE,
                handle,
            );
        }
        *GUI_LOG_FILE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(file);
        *active = Some((configured_path, log_path.clone()));
        return Some(log_path);
    }
    None
}

fn timestamped_log_path(
    base_path: &std::path::Path,
    timestamp: &str,
    collision_suffix: u32,
) -> std::path::PathBuf {
    let stem = base_path
        .file_stem()
        .unwrap_or_else(|| std::ffi::OsStr::new("uxplay"))
        .to_string_lossy();
    let extension = base_path
        .extension()
        .unwrap_or_else(|| std::ffi::OsStr::new("log"))
        .to_string_lossy();
    let suffix = if collision_suffix == 0 {
        String::new()
    } else {
        format!("-{collision_suffix}")
    };
    base_path.with_file_name(format!("{stem}-{timestamp}{suffix}.{extension}"))
}

fn local_log_timestamp() -> String {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut local = SYSTEMTIME::default();
    // SAFETY: GetLocalTime initializes the writable SYSTEMTIME structure.
    unsafe { GetLocalTime(&mut local) };
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond
    )
}

pub fn show_error(text: &str) {
    message(text, true);
}

pub fn run(options: Options, argv: Vec<OsString>, show_terminal: bool) -> ExitCode {
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

    if let Ok((_, _, config)) = crate::windows_config::load_or_create() {
        NOTIFICATIONS_ENABLED.store(
            config.notifications_enabled.unwrap_or(true),
            Ordering::Release,
        );
        if let Err(error) = set_startup(config.startup_enabled.unwrap_or(false)) {
            message(&error, true);
        }
    }

    let control = Arc::new(ServerControl::new());
    let mut server = Some(start_server(options, Arc::clone(&control)));
    let mut terminal = TerminalViewer::default();
    if show_terminal {
        terminal.show();
    }
    let mut idle_since = Some(Instant::now());
    let mut idle_restart_interval = crate::windows_config::load_or_create()
        .ok()
        .and_then(|(_, _, config)| config.idle_restart_minutes)
        .map_or(DEFAULT_IDLE_RESTART_INTERVAL, |minutes| {
            Duration::from_secs(u64::from(minutes) * 60)
        });
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
                ID_HIDE_TERMINAL => terminal.hide(),
                ID_SHOW_TERMINAL => terminal.show(),
                ID_RESET_PAIRED => {
                    let prompt =
                        wide("Reset all paired AirPlay devices? UxPlay will restart the receiver.");
                    let title = wide("Reset paired devices");
                    if unsafe {
                        MessageBoxW(
                            hwnd,
                            prompt.as_ptr(),
                            title.as_ptr(),
                            MB_YESNO | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONQUESTION,
                        )
                    } == IDYES
                    {
                        match reset_paired_devices(&argv, &control, &mut server) {
                            Ok(()) => idle_since = Some(Instant::now()),
                            Err(error) => message(&error, true),
                        }
                    }
                }
                ID_RESTART => {
                    if let Some(next_options) = validate_restart(&argv) {
                        if let Ok((_, _, config)) = crate::windows_config::load_or_create() {
                            NOTIFICATIONS_ENABLED.store(
                                config.notifications_enabled.unwrap_or(true),
                                Ordering::Release,
                            );
                            if let Err(error) = set_startup(config.startup_enabled.unwrap_or(false))
                            {
                                message(&error, true);
                                continue;
                            }
                            idle_restart_interval = config
                                .idle_restart_minutes
                                .map_or(DEFAULT_IDLE_RESTART_INTERVAL, |minutes| {
                                    Duration::from_secs(u64::from(minutes) * 60)
                                });
                        }
                        initialize_gui_logging(crate::windows_config::user_log_path().as_deref());
                        control.request_exit();
                        join_server(&mut server);
                        server = Some(start_server(next_options, Arc::clone(&control)));
                        idle_since = Some(Instant::now());
                    }
                }
                ID_EXIT => keep_running = false,
                _ => {}
            }
            continue;
        }
        if msg.message == PAIRING_PIN_MESSAGE {
            if !terminal.is_open() {
                show_pairing_pin_notification(hwnd);
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
            } else if control.has_active_clients() {
                idle_since = None;
            } else if idle_restart_interval != Duration::ZERO {
                let idle_started = *idle_since.get_or_insert_with(Instant::now);
                if idle_started.elapsed() >= idle_restart_interval {
                    if let Some(next_options) = validate_restart(&argv) {
                        control.request_exit();
                        join_server(&mut server);
                        server = Some(start_server(next_options, Arc::clone(&control)));
                        idle_since = Some(Instant::now());
                    }
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
    terminal.hide();
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

/// Tail the log in a console child; Windows selects the configured terminal host.
pub fn run_log_terminal(path: &std::path::Path) -> ExitCode {
    let mut output = match std::fs::OpenOptions::new().write(true).open("CONOUT$") {
        Ok(output) => output,
        Err(error) => {
            eprintln!("UxPlay could not access the default terminal output: {error}");
            return ExitCode::from(1);
        }
    };
    let _ = writeln!(output, "UxPlay receiver log (close this window to hide)");
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("UxPlay could not open log file {}: {error}", path.display());
            return ExitCode::from(1);
        }
    };
    let mut reader = BufReader::new(file);
    match seek_to_recent_log_tail(&mut reader, 64 * 1024) {
        Ok(true) => {
            let _ = writeln!(
                output,
                "(Showing the last 64 KiB of the existing log; following new output.)"
            );
        }
        Ok(false) => {}
        Err(error) => {
            let _ = writeln!(output, "Could not seek to the end of the log: {error}");
            return ExitCode::from(1);
        }
    }
    let mut line = Vec::new();
    let mut pending_line = Vec::new();
    let mut pending_count = 0_u64;
    let mut last_repeat = Instant::now();
    let coalesce_window = Duration::from_millis(350);
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => {
                if pending_count > 0 && last_repeat.elapsed() >= coalesce_window {
                    if write_collapsed_log_line(&mut output, &pending_line, pending_count).is_err()
                    {
                        return ExitCode::SUCCESS;
                    }
                    pending_line.clear();
                    pending_count = 0;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(_) => {
                if pending_count == 0 {
                    pending_line.clone_from(&line);
                    pending_count = 1;
                } else if line == pending_line {
                    pending_count = pending_count.saturating_add(1);
                    last_repeat = Instant::now();
                } else {
                    if write_collapsed_log_line(&mut output, &pending_line, pending_count).is_err()
                    {
                        return ExitCode::SUCCESS;
                    }
                    pending_line.clone_from(&line);
                    pending_count = 1;
                }
            }
            Err(error) => {
                eprintln!("UxPlay could not read log file {}: {error}", path.display());
                return ExitCode::from(1);
            }
        }
    }
}

fn seek_to_recent_log_tail(
    reader: &mut (impl BufRead + Seek),
    max_bytes: u64,
) -> std::io::Result<bool> {
    let end = reader.seek(SeekFrom::End(0))?;
    if end <= max_bytes {
        reader.seek(SeekFrom::Start(0))?;
        return Ok(false);
    }

    reader.seek(SeekFrom::Start(end - max_bytes))?;
    let mut partial_line = Vec::new();
    reader.read_until(b'\n', &mut partial_line)?;
    Ok(true)
}

fn write_collapsed_log_line(
    output: &mut impl Write,
    line: &[u8],
    repeat_count: u64,
) -> std::io::Result<()> {
    if repeat_count > 1 {
        write!(output, "[x{repeat_count}] ")?;
    }
    output.write_all(line)?;
    if !line.ends_with(b"\n") {
        output.write_all(b"\r\n")?;
    }
    output.flush()
}

#[derive(Default)]
struct TerminalViewer {
    child: Option<Child>,
}

impl TerminalViewer {
    fn show(&mut self) {
        if self
            .child
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
        {
            return;
        }
        self.child = None;

        let Some(log_path) = gui_log_path(crate::windows_config::user_log_path().as_deref()) else {
            message("UxPlay could not determine its log file path.", true);
            return;
        };
        let executable = match std::env::current_exe() {
            Ok(path) => path,
            Err(error) => {
                message(
                    &format!("UxPlay could not locate its executable: {error}"),
                    true,
                );
                return;
            }
        };
        match Command::new(executable)
            .arg("--internal-tail-log")
            .arg(log_path)
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
        {
            Ok(child) => self.child = Some(child),
            Err(error) => message(
                &format!("UxPlay could not open its log terminal: {error}"),
                true,
            ),
        }
    }

    fn hide(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn is_open(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else {
            return false;
        };
        if matches!(child.try_wait(), Ok(None)) {
            true
        } else {
            self.child = None;
            false
        }
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
            } else if lparam as u32 == NIN_BALLOONUSERCLICK {
                post_command(ID_SHOW_TERMINAL);
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
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION if wparam != 0 => {
            unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
            0
        }
        WM_ENDSESSION => 0,
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
        append(menu, ID_HIDE_TERMINAL, "Hide Terminal");
        append(menu, ID_SHOW_TERMINAL, "Show Terminal");
        append(menu, ID_RESET_PAIRED, "Reset Paired Devices");
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

fn reset_paired_devices(
    argv: &[OsString],
    control: &Arc<ServerControl>,
    server: &mut Option<std::thread::JoinHandle<ExitCode>>,
) -> Result<(), String> {
    let (config_path, _, config) = crate::windows_config::load_or_create()?;
    let options = validate_restart(argv)
        .ok_or_else(|| "UxPlay could not reload its startup arguments.".to_owned())?;
    let registry_path = config.pairing_file.unwrap_or_else(|| {
        config_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("paired-devices.txt")
    });
    match std::fs::remove_file(&registry_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "could not reset paired-device registry {}: {error}",
                registry_path.display()
            ));
        }
    }
    control.request_exit();
    join_server(server);
    *server = Some(start_server(options, Arc::clone(control)));
    Ok(())
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
        hIcon: unsafe { LoadIconW(GetModuleHandleW(std::ptr::null()), 1usize as *const u16) },
        ..Default::default()
    };
    let tip = wide("UxPlay AirPlay Receiver");
    let tip_len = tip.len().min(icon.szTip.len());
    icon.szTip[..tip_len].copy_from_slice(&tip[..tip_len]);
    icon
}

fn show_pairing_pin_notification(hwnd: HWND) {
    let Some(pin) = PENDING_PAIRING_PIN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    else {
        return;
    };
    let mut notification = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_INFO,
        dwInfoFlags: NIIF_INFO,
        ..Default::default()
    };
    copy_notification_text(&mut notification.szInfoTitle, "UxPlay needs an AirPlay PIN");
    copy_notification_text(
        &mut notification.szInfo,
        &format!("Enter PIN {pin} on your Apple device. Click to open the log terminal."),
    );
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &notification) };
}

fn copy_notification_text(destination: &mut [u16], text: &str) {
    let encoded: Vec<u16> = text.encode_utf16().collect();
    let length = encoded.len().min(destination.len().saturating_sub(1));
    destination[..length].copy_from_slice(&encoded[..length]);
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
    let (editor, arguments) = config_editor_arguments(&path);
    let editor = wide(editor);
    let arguments = wide(&arguments);
    let result = unsafe {
        ShellExecuteW(
            hwnd,
            verb.as_ptr(),
            editor.as_ptr(),
            arguments.as_ptr(),
            std::ptr::null(),
            1,
        )
    };
    if (result as isize) <= 32 {
        message(
            "Windows could not open the UxPlay config file in Notepad.",
            true,
        );
    }
}

fn config_editor_arguments(path: &std::path::Path) -> (&'static str, String) {
    ("notepad.exe", format!("\"{}\"", path.display()))
}

fn set_startup(enabled: bool) -> Result<(), String> {
    let command = if enabled {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        Some(wide(&format!(
            "\"{}\" --start-hidden",
            executable.display()
        )))
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

#[cfg(test)]
mod tests {
    use super::{
        config_editor_arguments, seek_to_recent_log_tail, timestamped_log_path,
        write_collapsed_log_line,
    };
    use std::io::{BufRead, BufReader, Cursor};
    use std::path::Path;

    #[test]
    fn config_opens_in_notepad_without_a_toml_file_association() {
        let path = Path::new(r"C:\Users\Test User\AppData\Roaming\UxPlay\config.toml");

        let (editor, arguments) = config_editor_arguments(path);

        assert_eq!(editor, "notepad.exe");
        assert_eq!(
            arguments,
            r#""C:\Users\Test User\AppData\Roaming\UxPlay\config.toml""#
        );
    }

    #[test]
    fn repeated_log_lines_show_a_repeat_count() {
        let mut output = Vec::new();

        write_collapsed_log_line(&mut output, b"receiver is ready\r\n", 4).unwrap();

        assert_eq!(output, b"[x4] receiver is ready\r\n");
    }

    #[test]
    fn large_logs_start_at_a_recent_line_boundary() {
        let mut reader = BufReader::new(Cursor::new(b"old\nkeep1\nkeep2\n".to_vec()));

        assert!(seek_to_recent_log_tail(&mut reader, 10).unwrap());
        let mut line = Vec::new();
        reader.read_until(b'\n', &mut line).unwrap();

        assert_eq!(line, b"keep2\n");
    }

    #[test]
    fn log_sessions_get_a_timestamped_sibling_file() {
        let base = Path::new(r"C:\Users\Test User\AppData\Roaming\UxPlay\logs\uxplay.log");

        assert_eq!(
            timestamped_log_path(base, "2026-11-28_14-05-09", 0),
            Path::new(
                r"C:\Users\Test User\AppData\Roaming\UxPlay\logs\uxplay-2026-11-28_14-05-09.log"
            )
        );
        assert_eq!(
            timestamped_log_path(base, "2026-11-28_14-05-09", 1),
            Path::new(
                r"C:\Users\Test User\AppData\Roaming\UxPlay\logs\uxplay-2026-11-28_14-05-09-1.log"
            )
        );
    }
}
