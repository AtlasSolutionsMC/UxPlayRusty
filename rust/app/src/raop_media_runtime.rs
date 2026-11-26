//! RAOP timing, RTP, mirroring, and GStreamer runtime adapters.

use crate::logger::Logger;
use crate::ntp_math::TimeProtocol;
use crate::options::{Options, VideoFlip as OptionFlip, VideoRotation};
use crate::raop_ntp_runtime::{NtpLogCallback, RaopNtpRuntime};
use crate::raop_parameters::{ParameterEffect, VolumeTarget};
use audio_renderer_rs::port::{
    AudioRendererConfig, audio_renderer_destroy, audio_renderer_flush, audio_renderer_init,
    audio_renderer_listen, audio_renderer_render_buffer, audio_renderer_set_volume,
    audio_renderer_stop, gstreamer_init as audio_renderer_gstreamer_init,
};
use glib::MainLoop;
use gstreamer::bus::BusWatchGuard;
use mux_renderer_rs::port::{MuxRenderer, MuxRendererConfig};
use std::fs::{File, OpenOptions};
use std::io;
use std::io::Write;
use std::net::{IpAddr, TcpListener, UdpSocket};
use std::str::FromStr;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
#[cfg(all(unix, not(target_os = "macos"), feature = "dbus-screensaver"))]
use std::time::Duration;
use uxplay_raop_rtp::{
    AudioCallbacks, AudioFrame, AudioPorts, LogCallback as RtpLogCallback, RaopRtpSession,
    StartState, TimingClock, UdpSocketBinder,
};
use uxplay_raop_rtp_mirror::runtime::{
    MirrorLogLevel, MirrorReceiver, MirrorRuntimeCallbacks, TcpListenerBinder,
};
use uxplay_raop_rtp_mirror::{MirrorEvent, NtpClock as MirrorNtpClock, VideoCodec};
use uxplay_raop_session::{SessionEffect, TimingProtocol, VideoResetReason};
use video_renderer_rs::port::{
    VideoFlip as RendererFlip, VideoRendererConfig, video_renderer_choose_codec_and_size,
    video_renderer_destroy, video_renderer_display_jpeg, video_renderer_flush, video_renderer_init,
    video_renderer_listen, video_renderer_pause, video_renderer_reinitialize,
    video_renderer_render_buffer, video_renderer_restore_mirror, video_renderer_resume,
    video_renderer_set_track_metadata, video_renderer_start, video_renderer_stop,
};

#[cfg(test)]
pub(crate) static VIDEO_RENDERER_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Keep GStreamer's main loop and bus watches alive while the protocol server runs.
pub struct RendererEventLoop {
    main_loop: MainLoop,
    worker: Option<JoinHandle<()>>,
    _audio_watches: Vec<BusWatchGuard>,
    _video_watch_ids: Vec<u32>,
}

impl RendererEventLoop {
    pub fn start(options: &Options, logger: &Arc<Logger>) -> io::Result<Self> {
        if !audio_renderer_gstreamer_init() {
            return Err(io::Error::other(
                "required GStreamer plugins are unavailable",
            ));
        }
        let use_audio = options.use_audio && options.audio_sink != "0";
        if use_audio {
            let config = AudioRendererConfig::new(
                options.audio_sink.clone(),
                options.audio_sync,
                options.video_sync,
            )
            .with_rtp_pipeline(options.audio_rtp_pipeline.clone().unwrap_or_default());
            audio_renderer_init(config, audio_renderer_log_callback(logger)).map_err(|error| {
                io::Error::other(format!("audio renderer startup failed: {error}"))
            })?;
        }

        let use_video = options.video_sink != "0";
        if use_video {
            let config = video_renderer_config(options);
            video_renderer_init(config, video_renderer_log_callback(logger)).map_err(|error| {
                io::Error::other(format!("video renderer startup failed: {error}"))
            })?;
            video_renderer_start();
        }

        let main_loop = MainLoop::new(None, false);
        let mut audio_watches = Vec::new();
        if use_audio {
            for id in 0..2 {
                if let Some(watch) = audio_renderer_listen(&main_loop, id).map_err(|error| {
                    io::Error::other(format!("audio renderer bus watch failed: {error}"))
                })? {
                    audio_watches.push(watch);
                }
            }
        }

        let video_renderer_count = if use_video {
            1 + usize::from(options.h265_support) + usize::from(options.render_cover_art)
        } else {
            0
        };
        let mut video_watch_ids = Vec::with_capacity(video_renderer_count);
        for id in 0..video_renderer_count {
            let source_id = video_renderer_listen(&main_loop, id).map_err(|error| {
                io::Error::other(format!("video renderer bus watch failed: {error}"))
            })?;
            video_watch_ids.push(source_id);
        }

        let loop_for_worker = main_loop.clone();
        let worker = thread::Builder::new()
            .name("uxplay-gstreamer-main-loop".to_owned())
            .spawn(move || loop_for_worker.run())?;
        Ok(Self {
            main_loop,
            worker: Some(worker),
            _audio_watches: audio_watches,
            _video_watch_ids: video_watch_ids,
        })
    }

    pub fn stop(&mut self) {
        self.main_loop.quit();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        audio_renderer_destroy();
        video_renderer_destroy();
    }
}

impl Drop for RendererEventLoop {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod renderer_event_loop_tests {
    use super::*;

    #[test]
    fn disabled_video_skips_video_renderer_initialization_and_bus_watches() {
        let _renderer_guard = VIDEO_RENDERER_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let options = Options {
            use_audio: false,
            audio_sink: "0".into(),
            video_sink: "0".into(),
            h265_support: true,
            render_cover_art: true,
            ..Options::default()
        };
        let logger = Arc::new(Logger::new());

        let renderer_loop = RendererEventLoop::start(&options, &logger)
            .expect("disabling media outputs must still start the server event loop");
        assert_eq!(video_renderer_choose_codec_and_size(false, false), -1);
        std::thread::sleep(std::time::Duration::from_millis(100));
        drop(renderer_loop);
    }
}

/// Process options consumed by RAOP callbacks.
#[derive(Clone)]
pub struct SessionMediaConfig {
    pub use_audio: bool,
    pub use_video: bool,
    pub close_window_on_client_stop: bool,
    pub nofreeze: bool,
    pub peer_to_peer: bool,
    pub render_cover_art: bool,
    pub cover_art_filename: Option<String>,
    pub metadata_filename: Option<String>,
    pub dacp_filename: Option<String>,
    pub taper_volume: bool,
    pub db_range: (f64, f64),
    pub audio_delay_alac_ns: i64,
    pub audio_delay_aac_ns: i64,
    pub current_volume_db: Arc<Mutex<f32>>,
    pub outputs: Arc<Mutex<MediaOutputRuntime>>,
    pub logger: Option<Arc<Logger>>,
}

impl Default for SessionMediaConfig {
    fn default() -> Self {
        Self {
            use_audio: true,
            use_video: true,
            close_window_on_client_stop: cfg!(not(target_os = "macos")),
            nofreeze: false,
            peer_to_peer: false,
            render_cover_art: false,
            cover_art_filename: None,
            metadata_filename: None,
            dacp_filename: None,
            taper_volume: false,
            db_range: (-30.0, 0.0),
            audio_delay_alac_ns: 0,
            audio_delay_aac_ns: 0,
            current_volume_db: Arc::new(Mutex::new(0.0)),
            outputs: Arc::new(Mutex::new(MediaOutputRuntime::default())),
            logger: None,
        }
    }
}

/// Shared process outputs for `-admp`, `-vdmp`, and `-mp4`.
///
/// Source callbacks are process-global, so this state is shared by all
/// protocol connections while RTP frames retain their connection-local timing.
#[derive(Default)]
pub struct MediaOutputRuntime {
    mux: Option<MuxRenderer>,
    dump_audio: bool,
    audio_dump_limit: Option<u32>,
    audio_dump_filename: String,
    audio_dump_file: Option<File>,
    audio_dump_count: u32,
    audio_dump_file_count: u32,
    audio_type: u8,
    previous_audio_type: u8,
    dump_video: bool,
    video_dump_limit: Option<u32>,
    video_dump_filename: String,
    video_dump_file: Option<File>,
    video_dump_count: u32,
    video_dump_file_count: u32,
    active_connections: usize,
    missed_feedback_seconds: u32,
    remote_clock_offset: u64,
    screensaver: ScreensaverInhibitor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedbackWatchdogTick {
    Idle,
    Active(u32),
    Warning(u32),
    TimedOut(u32),
}

impl MediaOutputRuntime {
    #[cfg(test)]
    pub fn new(options: &Options) -> io::Result<Self> {
        Self::new_with_mux_logger(options, None, None)
    }

    pub fn new_with_logger(options: &Options, logger: &Arc<Logger>) -> io::Result<Self> {
        Self::new_with_mux_logger(
            options,
            Some(mux_renderer_log_callback(logger)),
            Some(Arc::clone(logger)),
        )
    }

    fn new_with_mux_logger(
        options: &Options,
        logger: Option<mux_renderer_rs::port::LogCallback>,
        screen_logger: Option<Arc<Logger>>,
    ) -> io::Result<Self> {
        let mux = if options.mux_to_file {
            let config = MuxRendererConfig::new(
                options.mux_filename.clone(),
                options.use_audio && options.audio_sink != "0",
                options.video_sink != "0",
            );
            Some(
                match logger {
                    Some(logger) => MuxRenderer::new_with_logger(config, logger),
                    None => MuxRenderer::new(config),
                }
                .map_err(io::Error::other)?,
            )
        } else {
            None
        };
        Ok(Self {
            mux,
            dump_audio: options.dump_audio && options.audio_sink != "0",
            audio_dump_limit: options.audio_dump_limit,
            audio_dump_filename: options.audio_dump_filename.clone(),
            dump_video: options.dump_video,
            video_dump_limit: options.video_dump_limit,
            video_dump_filename: options.video_dump_filename.clone(),
            screensaver: screen_logger.map_or_else(ScreensaverInhibitor::default, |logger| {
                ScreensaverInhibitor::start(
                    options.screensaver_inhibit,
                    options.video_sink != "0",
                    logger,
                )
            }),
            ..Self::default()
        })
    }

    fn connection_opened(&mut self) {
        self.active_connections = self.active_connections.saturating_add(1);
    }

    fn connection_closed(&mut self, use_audio: bool, dacp_filename: Option<&str>) -> bool {
        self.active_connections = self.active_connections.saturating_sub(1);
        let last_connection = self.active_connections == 0;
        if last_connection {
            self.reset_remote_clock_offset();
            if use_audio {
                audio_renderer_stop();
            }
            if let Some(path) = dacp_filename {
                let _ = std::fs::remove_file(path);
            }
            if let Some(mux) = self.mux.as_mut() {
                mux.stop();
            }
        }
        last_connection
    }

    /// Clear the process-wide wall-clock mapping used by the source audio/video callbacks.
    pub fn reset_remote_clock_offset(&mut self) {
        self.remote_clock_offset = 0;
    }

    fn renderer_timestamp(&mut self, remote_time_ns: u64, local_time_ns: u64) -> u64 {
        if self.remote_clock_offset == 0 {
            let local_time_ns = if local_time_ns == 0 {
                RaopNtpRuntime::local_time_ns()
            } else {
                local_time_ns
            };
            self.remote_clock_offset = local_time_ns.wrapping_sub(remote_time_ns);
        }
        remote_time_ns.wrapping_add(self.remote_clock_offset)
    }

    fn adjust_remote_clock_offset(&mut self, mismatch_ns: u64) {
        self.remote_clock_offset = self.remote_clock_offset.wrapping_add(mismatch_ns);
    }

    pub fn note_client_feedback(&mut self) {
        self.missed_feedback_seconds = 0;
    }

    pub fn feedback_watchdog_tick(&mut self, limit_seconds: u32) -> FeedbackWatchdogTick {
        if self.active_connections == 0 {
            self.missed_feedback_seconds = 0;
            return FeedbackWatchdogTick::Idle;
        }

        let missed = self.missed_feedback_seconds;
        if limit_seconds != 0 && missed > limit_seconds {
            return FeedbackWatchdogTick::TimedOut(missed);
        }
        self.missed_feedback_seconds = missed.saturating_add(1);
        if missed > 2 {
            FeedbackWatchdogTick::Warning(missed)
        } else {
            FeedbackWatchdogTick::Active(missed)
        }
    }

    pub fn select_audio_codec(&mut self, codec_type: u8) {
        let audio_type = match codec_type {
            2 => 0x20,
            8 => 0x80,
            _ => 0x10,
        };
        if self.audio_type != audio_type {
            self.audio_dump_file.take();
        }
        self.audio_type = audio_type;
        if let Some(mux) = self.mux.as_mut() {
            let _ = mux.choose_audio_codec(codec_type);
        }
    }

    fn process_audio(&mut self, codec_type: u8, data: &[u8], remote_time_ns: u64) {
        if self.audio_type == 0 {
            self.select_audio_codec(codec_type);
        }
        if self.dump_audio {
            self.write_audio_dump(data);
        }
        if let Some(mux) = self.mux.as_mut() {
            let _ = mux.push_audio(data, remote_time_ns);
        }
    }

    fn write_audio_dump(&mut self, data: &[u8]) {
        if self.audio_dump_file.is_none() && self.audio_type != self.previous_audio_type {
            self.previous_audio_type = self.audio_type;
            self.audio_dump_file_count = self.audio_dump_file_count.saturating_add(1);
            self.audio_dump_count = 0;
            let extension = match self.audio_type {
                0x20 => "alac",
                0x80 => "aac",
                _ => "aud",
            };
            let filename = format!(
                "{}.{}.{}",
                self.audio_dump_filename, self.audio_dump_file_count, extension
            );
            self.audio_dump_file = open_dump_file(&filename, "audio frames");
        }
        let Some(file) = self.audio_dump_file.as_mut() else {
            return;
        };
        if let Err(error) = file.write_all(data) {
            eprintln!("UxPlay Rust: failed to write audio dump: {error}");
            self.audio_dump_file.take();
            return;
        }
        if let Some(limit) = self.audio_dump_limit.filter(|limit| *limit != 0) {
            self.audio_dump_count = self.audio_dump_count.saturating_add(1);
            if self.audio_dump_count >= limit {
                self.audio_dump_file.take();
            }
        }
    }

    fn select_video_codec(&mut self, is_h265: bool) {
        if let Some(mux) = self.mux.as_mut() {
            let _ = mux.choose_video_codec(is_h265);
        }
    }

    fn process_video(&mut self, data: &[u8], remote_time_ns: u64) {
        if self.dump_video {
            self.write_video_dump(data);
        }
        if let Some(mux) = self.mux.as_mut() {
            let _ = mux.push_video(data, remote_time_ns);
        }
    }

    fn write_video_dump(&mut self, data: &[u8]) {
        if self.video_dump_limit.is_some_and(|limit| limit > 0)
            && self.video_dump_file.is_some()
            && data.get(4).is_some_and(|byte| byte & 0x1f == 0x07)
        {
            if let Some(file) = self.video_dump_file.as_mut() {
                let _ = file.write_all(&[0, 0, 0, 1]);
            }
            self.video_dump_file.take();
            self.video_dump_count = 0;
        }
        if self.video_dump_file.is_none() {
            let filename = if self.video_dump_limit.is_some_and(|limit| limit > 0) {
                self.video_dump_file_count = self.video_dump_file_count.saturating_add(1);
                format!(
                    "{}.{}.h264",
                    self.video_dump_filename, self.video_dump_file_count
                )
            } else {
                format!("{}.h264", self.video_dump_filename)
            };
            self.video_dump_file = open_dump_file(&filename, "H.264 frames");
        }
        let Some(file) = self.video_dump_file.as_mut() else {
            return;
        };
        if self.video_dump_limit.is_none_or(|limit| limit == 0) {
            let _ = file.write_all(data);
        } else if self.video_dump_count < self.video_dump_limit.unwrap_or_default() {
            self.video_dump_count = self.video_dump_count.saturating_add(1);
            let _ = file.write_all(data);
        }
    }

    pub fn shutdown(&mut self) {
        self.audio_dump_file.take();
        if let Some(file) = self.video_dump_file.as_mut() {
            let _ = file.write_all(&[0, 0, 0, 1]);
        }
        self.video_dump_file.take();
        if let Some(mux) = self.mux.as_mut() {
            mux.destroy();
        }
        self.mux.take();
        self.screensaver.shutdown();
    }

    fn mirror_video_running(&mut self, running: bool) {
        self.screensaver.mirror_video_running(running);
    }

    pub(crate) fn observe_hls_position(&mut self, position: f64) {
        self.screensaver.observe_hls_position(position);
    }
}

#[cfg(any(
    test,
    all(unix, not(target_os = "macos"), feature = "dbus-screensaver")
))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ScreensaverService {
    name: &'static str,
    path: &'static str,
    interface: &'static str,
}

#[cfg(any(
    test,
    all(unix, not(target_os = "macos"), feature = "dbus-screensaver")
))]
const FREEDESKTOP_SCREENSAVER: ScreensaverService = ScreensaverService {
    name: "org.freedesktop.ScreenSaver",
    path: "/org/freedesktop/ScreenSaver",
    interface: "org.freedesktop.ScreenSaver",
};

#[cfg(any(
    test,
    all(unix, not(target_os = "macos"), feature = "dbus-screensaver")
))]
fn screensaver_service_for_desktop(desktop: &str) -> ScreensaverService {
    if desktop.contains("XFCE") {
        ScreensaverService {
            name: "org.xfce.ScreenSaver",
            path: "/org/xfce/ScreenSaver",
            interface: "org.xfce.ScreenSaver",
        }
    } else if desktop.contains("MATE") {
        ScreensaverService {
            name: "org.mate.ScreenSaver",
            path: "/org/mate/ScreenSaver",
            interface: "org.mate.ScreenSaver",
        }
    } else {
        FREEDESKTOP_SCREENSAVER
    }
}

enum ScreensaverRequest {
    #[cfg(any(
        test,
        all(unix, not(target_os = "macos"), feature = "dbus-screensaver")
    ))]
    SetInhibited(bool),
    Shutdown,
}

/// Mirrors the C `dbus_last_message` and HLS-position state while keeping D-Bus calls
/// off mirror and HTTP callback threads.
struct ScreensaverInhibitor {
    mode: u32,
    inhibited: bool,
    previous_hls_position: f64,
    requests: Option<Sender<ScreensaverRequest>>,
    worker: Option<JoinHandle<()>>,
}

impl ScreensaverInhibitor {
    fn start(mode: u32, use_video: bool, logger: Arc<Logger>) -> Self {
        if mode == 0 {
            return Self::default();
        }
        if !use_video {
            logger.log(
                crate::logger::INFO,
                format!("-scrsv = {mode} will be ignored, as no video will be rendered"),
            );
            return Self::default();
        }

        #[cfg(all(unix, not(target_os = "macos"), feature = "dbus-screensaver"))]
        {
            use dbus::blocking::Connection;

            let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
            let service = screensaver_service_for_desktop(&desktop);
            logger.log(
                crate::logger::DEBUG,
                format!("Desktop Environment:  {desktop}"),
            );

            let (requests, receiver) = std::sync::mpsc::channel();
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
            let worker_logger = Arc::clone(&logger);
            let worker = match thread::Builder::new()
                .name("uxplay-dbus-screensaver".to_owned())
                .spawn(move || {
                    let connection = match Connection::new_session() {
                        Ok(connection) => connection,
                        Err(error) => {
                            let _ = ready_tx.send(Err(error.to_string()));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(()));
                    run_screensaver_worker(connection, service, mode, receiver, worker_logger);
                }) {
                Ok(worker) => worker,
                Err(_) => {
                    logger.log(
                        crate::logger::INFO,
                        "D-Bus session not found: screensaver inhibition option (\"-scrsv\") will not be active",
                    );
                    return Self::default();
                }
            };

            match ready_rx.recv() {
                Ok(Ok(())) => {}
                Ok(Err(_)) | Err(_) => {
                    let _ = worker.join();
                    logger.log(
                        crate::logger::INFO,
                        "D-Bus session not found: screensaver inhibition option (\"-scrsv\") will not be active",
                    );
                    return Self::default();
                }
            }

            logger.log(crate::logger::DEBUG, "D-Bus session support is available");
            logger.log(
                crate::logger::INFO,
                format!(
                    "Will attempt to use {} (D-Bus screensaver inhibition) {}",
                    service.name,
                    if mode == 1 {
                        "while displaying mirrored or streamed video"
                    } else {
                        "always"
                    }
                ),
            );

            let mut inhibitor = Self {
                mode,
                inhibited: false,
                previous_hls_position: 0.0,
                requests: Some(requests),
                worker: Some(worker),
            };
            if mode == 2 {
                inhibitor.set_inhibited(true);
            }
            return inhibitor;
        }

        #[cfg(not(all(unix, not(target_os = "macos"), feature = "dbus-screensaver")))]
        {
            let _ = (mode, logger);
            Self::default()
        }
    }

    fn set_inhibited(&mut self, inhibited: bool) {
        if self.inhibited == inhibited {
            return;
        }
        self.inhibited = inhibited;
        #[cfg(any(
            test,
            all(unix, not(target_os = "macos"), feature = "dbus-screensaver")
        ))]
        if let Some(requests) = &self.requests {
            let _ = requests.send(ScreensaverRequest::SetInhibited(inhibited));
        }
    }

    fn mirror_video_running(&mut self, running: bool) {
        if self.mode == 1 {
            self.set_inhibited(running);
        }
    }

    fn observe_hls_position(&mut self, position: f64) {
        if self.mode == 1 {
            let next = hls_inhibit_state(self.inhibited, self.previous_hls_position, position);
            self.set_inhibited(next);
            self.previous_hls_position = position;
        }
    }

    fn shutdown(&mut self) {
        if let Some(requests) = self.requests.take() {
            let _ = requests.send(ScreensaverRequest::Shutdown);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.inhibited = false;
        self.mode = 0;
    }
}

impl Default for ScreensaverInhibitor {
    fn default() -> Self {
        Self {
            mode: 0,
            inhibited: false,
            previous_hls_position: 0.0,
            requests: None,
            worker: None,
        }
    }
}

impl Drop for ScreensaverInhibitor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn hls_inhibit_state(inhibited: bool, previous_position: f64, position: f64) -> bool {
    if position > previous_position {
        true
    } else if position == previous_position {
        false
    } else {
        inhibited
    }
}

#[cfg(all(unix, not(target_os = "macos"), feature = "dbus-screensaver"))]
fn run_screensaver_worker(
    connection: dbus::blocking::Connection,
    service: ScreensaverService,
    mode: u32,
    requests: std::sync::mpsc::Receiver<ScreensaverRequest>,
    logger: Arc<Logger>,
) {
    let mut cookie = None;
    let mut inhibited = false;
    while let Ok(request) = requests.recv() {
        match request {
            ScreensaverRequest::SetInhibited(true) if !inhibited => {
                let reason = if mode == 1 {
                    "actively receiving video"
                } else {
                    "mirroring client: inhibit always"
                };
                let proxy =
                    connection.with_proxy(service.name, service.path, Duration::from_secs(25));
                let result: Result<(u32,), _> =
                    proxy.method_call(service.interface, "Inhibit", ("UxPlay", reason));
                if let Ok((new_cookie,)) = result {
                    cookie = Some(new_cookie);
                    logger.log(
                        crate::logger::DEBUG,
                        format!("screen_saver: got D-Bus cookie {new_cookie}"),
                    );
                }
                inhibited = true;
            }
            ScreensaverRequest::SetInhibited(false) if inhibited => {
                if let Some(cookie_value) = cookie {
                    logger.log(
                        crate::logger::DEBUG,
                        format!("screen_saver: releasing D-Bus cookie {cookie_value}"),
                    );
                    if send_uninhibit(&connection, service, cookie_value) {
                        cookie = None;
                    }
                }
                inhibited = false;
            }
            ScreensaverRequest::SetInhibited(_) => {}
            ScreensaverRequest::Shutdown => {
                if inhibited && let Some(cookie_value) = cookie {
                    let _ = send_uninhibit(&connection, service, cookie_value);
                }
                break;
            }
        }
    }
}

#[cfg(all(unix, not(target_os = "macos"), feature = "dbus-screensaver"))]
fn send_uninhibit(
    connection: &dbus::blocking::Connection,
    service: ScreensaverService,
    cookie: u32,
) -> bool {
    use dbus::channel::Sender;

    let Ok(message) =
        dbus::Message::new_method_call(service.name, service.path, service.interface, "UnInhibit")
    else {
        return false;
    };
    let sent = connection.channel().send(message.append1(cookie)).is_ok();
    if sent {
        connection.channel().flush();
    }
    sent
}

fn open_dump_file(filename: &str, description: &str) -> Option<File> {
    match OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(filename)
    {
        Ok(file) => Some(file),
        Err(error) => {
            eprintln!("UxPlay Rust: could not open {filename} for {description}: {error}");
            None
        }
    }
}

/// Resources that belong to one persistent RTSP connection.
pub struct SessionMedia {
    config: SessionMediaConfig,
    timing: Option<Arc<Mutex<RaopNtpRuntime>>>,
    timing_local_port: Option<u16>,
    audio: Option<RaopRtpSession>,
    audio_ports: Option<AudioPorts>,
    mirror: Option<MirrorReceiver>,
    mirror_data_port: Option<u16>,
    remote_control_id: Option<(String, String)>,
    track_metadata: Arc<Mutex<crate::dmap_metadata::TrackMetadata>>,
    output_connection_open: bool,
}

impl Default for SessionMedia {
    fn default() -> Self {
        Self::new(SessionMediaConfig::default())
    }
}

impl SessionMedia {
    pub fn new(config: SessionMediaConfig) -> Self {
        lock(&config.outputs).connection_opened();
        Self {
            config,
            timing: None,
            timing_local_port: None,
            audio: None,
            audio_ports: None,
            mirror: None,
            mirror_data_port: None,
            remote_control_id: None,
            track_metadata: Arc::new(Mutex::new(crate::dmap_metadata::TrackMetadata::default())),
            output_connection_open: true,
        }
    }

    pub fn apply(&mut self, effect: SessionEffect) -> io::Result<()> {
        match effect {
            SessionEffect::SetRemoteControlId {
                dacp_id,
                active_remote,
            } => {
                self.remote_control_id = Some((dacp_id.clone(), active_remote.clone()));
                if let Some(audio) = &self.audio {
                    audio.set_remote_control_id(Some(&dacp_id), Some(&active_remote));
                }
            }
            SessionEffect::StartTiming {
                remote_address,
                remote_port,
                local_port,
                protocol,
            } => {
                if let Some(previous) = self.timing.take() {
                    lock(&previous).stop();
                }
                let remote_address_length = address_length(&remote_address)?;
                let logger = ntp_logger_callback(&self.config.logger);
                let mut runtime = RaopNtpRuntime::new(
                    logger,
                    &remote_address,
                    remote_address_length,
                    remote_port,
                    time_protocol(protocol),
                )?;
                let mut local_port = local_port;
                runtime.start_with_peer_to_peer(&mut local_port, self.config.peer_to_peer)?;
                self.timing_local_port = Some(local_port);
                self.timing = Some(Arc::new(Mutex::new(runtime)));
            }
            SessionEffect::InitializeAudio {
                remote_address,
                key,
                iv,
            } => {
                if let Some(mut previous) = self.audio.take() {
                    previous.stop();
                }
                self.audio_ports = None;
                let timing = self.timing_clock()?;
                let callbacks: Arc<dyn AudioCallbacks> = Arc::new(RendererAudioCallbacks {
                    config: self.config.clone(),
                    track_metadata: Arc::clone(&self.track_metadata),
                });
                let logger = rtp_logger_callback(&self.config.logger);
                let debug_data_enabled = self
                    .config
                    .logger
                    .as_ref()
                    .is_some_and(|logger| logger.level() >= crate::logger::DEBUG_DATA);
                let mut audio = RaopRtpSession::new(
                    logger,
                    callbacks,
                    timing,
                    &remote_address,
                    address_length(&remote_address)?,
                    &key,
                    &iv,
                )?;
                audio.set_debug_data_enabled(debug_data_enabled);
                let peer_to_peer = self.config.peer_to_peer;
                let binder: UdpSocketBinder = Arc::new(move |address, port| {
                    bind_session_udp_socket(address, port, peer_to_peer)
                });
                audio.set_udp_socket_binder(binder);
                if let Some((dacp_id, active_remote)) = &self.remote_control_id {
                    audio.set_remote_control_id(Some(dacp_id), Some(active_remote));
                }
                self.audio = Some(audio);
            }
            SessionEffect::InitializeMirror { config } => {
                if let Some(mut previous) = self.mirror.take() {
                    previous.stop()?;
                }
                self.mirror_data_port = None;
                let timing = self.timing.as_ref().cloned().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotConnected,
                        "timing session is not initialized",
                    )
                })?;
                let clock: Arc<Mutex<Box<dyn MirrorNtpClock + Send>>> =
                    Arc::new(Mutex::new(Box::new(MirrorTimingClock(timing))));
                let callbacks: Arc<dyn MirrorRuntimeCallbacks> =
                    Arc::new(RendererMirrorCallbacks {
                        outputs: Arc::clone(&self.config.outputs),
                        logger: self.config.logger.clone(),
                        use_video: self.config.use_video,
                        nofreeze: self.config.nofreeze,
                    });
                let mut mirror = config.create_receiver(clock, callbacks)?;
                let peer_to_peer = self.config.peer_to_peer;
                let binder: TcpListenerBinder = Arc::new(move |address, port| {
                    bind_mirror_tcp_listener(address, port, peer_to_peer)
                });
                mirror.set_listener_binder(binder);
                self.mirror = Some(mirror);
            }
            SessionEffect::ClearHlsVideoData => video_renderer_flush(),
            SessionEffect::InitializeMirrorAes {
                stream_connection_id,
            } => {
                let mirror = self.mirror.as_mut().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotConnected,
                        "mirror receiver is not initialized",
                    )
                })?;
                mirror.init_aes(stream_connection_id);
            }
            SessionEffect::StartMirror {
                requested_port,
                show_client_fps_data,
            } => {
                let mirror = self.mirror.as_mut().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotConnected,
                        "mirror receiver is not initialized",
                    )
                })?;
                self.mirror_data_port =
                    Some(mirror.start(requested_port, show_client_fps_data != 0)?);
            }
            SessionEffect::StartAudio {
                ports,
                format,
                sample_rate,
                ..
            } => {
                let audio = self.audio.as_mut().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotConnected,
                        "audio receiver is not initialized",
                    )
                })?;
                match audio.start_audio(ports, format.codec_type, sample_rate)? {
                    StartState::Started(actual_ports) => self.audio_ports = Some(actual_ports),
                    StartState::AlreadyRunning => {}
                }
            }
            SessionEffect::FlushAudio {
                next_sequence_number,
            } => {
                if let Some(audio) = &self.audio {
                    audio.flush(next_sequence_number);
                }
            }
            SessionEffect::StopAudio => {
                if let Some(audio) = &mut self.audio {
                    audio.stop();
                }
                self.audio_ports = None;
            }
            SessionEffect::StopCoverArtRendering if self.config.render_cover_art => {
                video_renderer_stop();
            }
            SessionEffect::StopCoverArtRendering => {}
            SessionEffect::StopMirror => {
                if let Some(mirror) = &mut self.mirror {
                    mirror.stop()?;
                }
                video_renderer_stop();
            }
            SessionEffect::DestroyAudio => {
                if let Some(mut audio) = self.audio.take() {
                    audio.stop();
                }
                self.audio_ports = None;
            }
            SessionEffect::DestroyMirror => {
                if let Some(mut mirror) = self.mirror.take() {
                    mirror.stop()?;
                }
                self.mirror_data_port = None;
            }
            SessionEffect::VideoReset(reason) => {
                lock(&self.config.outputs).reset_remote_clock_offset();
                if self.config.use_video {
                    reset_video(reason, self.config.close_window_on_client_stop)?;
                }
            }
            SessionEffect::ReportClientRequest { .. }
            | SessionEffect::RegisterClient { .. }
            | SessionEffect::GetAudioFormat { .. } => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "session effect must be handled by the protocol callback adapter",
                ));
            }
        }
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(mut audio) = self.audio.take() {
            audio.stop();
        }
        if let Some(mut mirror) = self.mirror.take() {
            let _ = mirror.stop();
        }
        if let Some(timing) = self.timing.take() {
            lock(&timing).stop();
        }
        video_renderer_flush();
        self.audio_ports = None;
        self.mirror_data_port = None;
        self.timing_local_port = None;
        if self.output_connection_open {
            lock(&self.config.outputs)
                .connection_closed(self.config.use_audio, self.config.dacp_filename.as_deref());
            self.output_connection_open = false;
        }
    }

    /// Stop only the running RAOP worker services when the C server identifies a
    /// separate AirPlay video connection. The session objects stay installed so a
    /// later RAOP SETUP can replace and restart them on the same TCP connection.
    pub fn stop_services_for_airplay(&mut self) -> io::Result<()> {
        let connection = std::ptr::from_ref(self);
        let mut first_error = None;

        if self.mirror.is_some() {
            if let Some(logger) = &self.config.logger {
                logger.log(
                    crate::logger::DEBUG,
                    format!(
                        "New AirPlay connection: stopping RAOP mirror service on RAOP connection {connection:p}"
                    ),
                );
            }
            if let Some(mirror) = &mut self.mirror
                && let Err(error) = mirror.stop()
            {
                first_error = Some(error);
            }
        }

        if self.audio.is_some() {
            if let Some(logger) = &self.config.logger {
                logger.log(
                    crate::logger::DEBUG,
                    format!(
                        "New AirPlay connection: stopping RAOP audio service on RAOP connection {connection:p}"
                    ),
                );
            }
            if let Some(audio) = &mut self.audio {
                audio.stop();
            }

            // The reference guards its NTP stop with the RAOP RTP pointer check.
            if let Some(logger) = &self.config.logger {
                logger.log(
                    crate::logger::DEBUG,
                    format!(
                        "New AirPlay connection: stopping NTP time service on RAOP connection {connection:p}"
                    ),
                );
            }
            if let Some(timing) = &self.timing {
                lock(timing).stop();
            }
        }

        self.audio_ports = None;
        self.mirror_data_port = None;
        self.timing_local_port = None;

        if let Some(error) = first_error {
            Err(error)
        } else {
            Ok(())
        }
    }

    pub fn parameter_state(&self) -> (bool, bool) {
        (
            self.audio.is_some(),
            self.audio.as_ref().is_some_and(RaopRtpSession::is_running),
        )
    }

    pub fn apply_parameter(&self, effect: ParameterEffect) -> io::Result<()> {
        let audio = self.audio.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "RAOP audio session is not initialized",
            )
        })?;
        match effect {
            ParameterEffect::Volume {
                target: VolumeTarget::RaopRtp,
                value,
            } => audio.set_volume(value),
            ParameterEffect::Volume {
                target: VolumeTarget::HlsRenderer,
                ..
            } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "HLS volume is handled by the process callback",
                ));
            }
            ParameterEffect::Progress {
                start,
                current,
                end,
            } => audio.set_progress(start, current, end),
            ParameterEffect::CoverArt { data, .. } => audio.set_coverart(&data),
            ParameterEffect::Metadata(data) => audio.set_metadata(&data),
        }
        Ok(())
    }

    pub fn setup_ports(&self) -> (Option<u16>, Option<AudioPorts>, Option<u16>) {
        (
            self.timing_local_port,
            self.audio_ports,
            self.mirror_data_port,
        )
    }

    fn timing_clock(&self) -> io::Result<Arc<dyn TimingClock>> {
        self.timing
            .as_ref()
            .map(|timing| Arc::new(TimingClockAdapter(Arc::clone(timing))) as Arc<dyn TimingClock>)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotConnected,
                    "timing session is not initialized",
                )
            })
    }
}

impl Drop for SessionMedia {
    fn drop(&mut self) {
        self.stop();
    }
}

struct TimingClockAdapter(Arc<Mutex<RaopNtpRuntime>>);

impl TimingClock for TimingClockAdapter {
    fn local_time_ns(&self) -> u64 {
        RaopNtpRuntime::local_time_ns()
    }

    fn adjust_remote_timestamp_offset(&self, timestamp: u64, add_epoch_seconds: bool) -> u64 {
        lock(&self.0).adjust_remote_timestamp_offset(timestamp, add_epoch_seconds)
    }

    fn timestamp_to_nanoseconds(&self, timestamp: u64) -> u64 {
        lock(&self.0).timestamp_to_nanoseconds(timestamp)
    }

    fn convert_remote_time_ns(&self, remote_time_ns: u64) -> u64 {
        lock(&self.0).convert_remote_time_ns(remote_time_ns)
    }

    fn video_arrival_offset(&self) -> u64 {
        lock(&self.0).video_arrival_offset()
    }
}

struct MirrorTimingClock(Arc<Mutex<RaopNtpRuntime>>);

impl MirrorNtpClock for MirrorTimingClock {
    fn adjust_remote_timestamp_offset(&mut self, wire_ntp: u64, include_epoch: bool) -> u64 {
        lock(&self.0).adjust_remote_timestamp_offset(wire_ntp, include_epoch)
    }

    fn timestamp_to_nanoseconds(&self, timestamp: u64) -> u64 {
        lock(&self.0).timestamp_to_nanoseconds(timestamp)
    }

    fn local_time_nanoseconds(&self) -> u64 {
        RaopNtpRuntime::local_time_ns()
    }

    fn convert_remote_time(&self, remote_nanoseconds: u64) -> u64 {
        lock(&self.0).convert_remote_time_ns(remote_nanoseconds)
    }

    fn set_video_arrival_offset(&mut self, offset_nanoseconds: u64) {
        lock(&self.0).set_video_arrival_offset(offset_nanoseconds);
    }
}

struct RendererAudioCallbacks {
    config: SessionMediaConfig,
    track_metadata: Arc<Mutex<crate::dmap_metadata::TrackMetadata>>,
}

impl AudioCallbacks for RendererAudioCallbacks {
    fn process_audio(&self, frame: AudioFrame) {
        let mut outputs = lock(&self.config.outputs);
        outputs.process_audio(frame.codec_type, &frame.data, frame.remote_time_ns);
        if self.config.use_audio {
            let timestamp = outputs.renderer_timestamp(frame.remote_time_ns, frame.local_time_ns);
            audio_renderer_render_buffer(
                &frame.data,
                frame.sequence_number,
                audio_timestamp_with_option_delay(
                    frame.codec_type,
                    timestamp,
                    self.config.audio_delay_alac_ns,
                    self.config.audio_delay_aac_ns,
                ),
            );
        }
    }

    fn flush(&self) {
        audio_renderer_flush();
    }

    fn set_volume(&self, volume: f32) {
        if self.config.use_audio {
            let volume = source_volume_db(volume);
            *lock(&self.config.current_volume_db) = volume;
            let output_volume =
                source_volume_to_gstreamer(volume, self.config.db_range, self.config.taper_volume);
            audio_renderer_set_volume(output_volume);
            video_renderer_rs::port::video_renderer_hls_set_volume(output_volume);
        }
    }

    fn set_metadata(&self, metadata: Vec<u8>) {
        println!("====================Audio Metadata==================");
        let Ok(update) = crate::dmap_metadata::parse(&metadata) else {
            eprintln!("UxPlay Rust: received invalid RAOP DMAP metadata");
            return;
        };
        if !update.text.is_empty() {
            print!("{}", String::from_utf8_lossy(&update.text));
            let _ = std::io::stdout().flush();
        }
        if let Some(path) = self.config.metadata_filename.as_deref() {
            let mut bytes = update.text.clone();
            bytes.push(0);
            write_output(path, &bytes, "metadata");
        }
        let mut current = lock(&self.track_metadata);
        if update.title.is_some() {
            current.title = update.title;
        }
        if update.artist.is_some() {
            current.artist = update.artist;
        }
        if update.album.is_some() {
            current.album = update.album;
        }
        if self.config.render_cover_art {
            video_renderer_set_track_metadata(
                current.title.as_deref(),
                current.artist.as_deref(),
                current.album.as_deref(),
            );
        }
    }

    fn set_coverart(&self, coverart: Vec<u8>) {
        if let Some(path) = self.config.cover_art_filename.as_deref() {
            write_output(path, &coverart, "cover art");
        } else if self.config.render_cover_art {
            video_renderer_choose_codec_and_size(true, false);
            video_renderer_display_jpeg(&coverart);
        }
    }

    fn remote_control_id(&self, dacp_id: String, active_remote_header: String) {
        if let Some(path) = self.config.dacp_filename.as_deref() {
            let contents = format!("{dacp_id}\n{active_remote_header}\n");
            write_output(path, contents.as_bytes(), "DACP");
        }
    }

    fn set_progress(&self, start: u32, current: u32, end: u32) {
        if current < start || current > end {
            return;
        }
        let duration = u64::from(end - start) / 44_100;
        let position = u64::from(current - start) / 44_100;
        let remaining = duration.saturating_sub(position);
        print!(
            "audio progress (min:sec): {:3}:{:02}; remaining: {:3}:{:02}; track length {}:{:02}\r",
            position / 60,
            position % 60,
            remaining / 60,
            remaining % 60,
            duration / 60,
            duration % 60
        );
        let _ = std::io::stdout().flush();
    }
}

pub fn initialize_audio_output_files(config: &SessionMediaConfig) {
    if let Some(path) = config.cover_art_filename.as_deref() {
        write_output(path, EMPTY_COVER_ART, "cover art");
    }
    if let Some(path) = config.metadata_filename.as_deref() {
        write_output(path, b"no data\n\0", "metadata");
    }
}

const EMPTY_COVER_ART: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x01, 0x03, 0x00, 0x00, 0x00, 0x25, 0xdb, 0x56,
    0xca, 0x00, 0x00, 0x00, 0x03, 0x50, 0x4c, 0x54, 0x45, 0x00, 0x00, 0x00, 0xa7, 0x7a, 0x3d, 0xda,
    0x00, 0x00, 0x00, 0x01, 0x74, 0x52, 0x4e, 0x53, 0x00, 0x40, 0xe6, 0xd8, 0x66, 0x00, 0x00, 0x00,
    0x0a, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0x60, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xe2,
    0x21, 0xbc, 0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn write_output(path: &str, bytes: &[u8], description: &str) {
    if let Err(error) = std::fs::write(path, bytes) {
        eprintln!("UxPlay Rust: failed to write {description} file {path}: {error}");
    }
}

pub fn source_volume_to_gstreamer(volume: f32, db_range: (f64, f64), taper: bool) -> f64 {
    if volume == -144.0 {
        return 0.0;
    }
    let clamped = volume.clamp(-30.0, 0.0);
    let fraction = f64::from((30.0 + clamped) / 30.0).clamp(0.0, 1.0);
    if fraction == 0.0 {
        return 0.0;
    }
    let flat_db = db_range.0 + (db_range.1 - db_range.0) * fraction;
    let db = if taper {
        (db_range.1 + 10.0 * (fraction.log10() / 2.0_f64.log10())).max(flat_db)
    } else {
        flat_db
    };
    10.0_f64.powf(0.05 * db)
}

pub fn source_volume_db(volume: f32) -> f32 {
    if volume == -144.0 {
        volume
    } else {
        volume.clamp(-30.0, 0.0)
    }
}

fn audio_renderer_log_callback(logger: &Arc<Logger>) -> audio_renderer_rs::port::LogCallback {
    let logger = Arc::clone(logger);
    Arc::new(move |level, message| logger.log(level as i32, message))
}

fn video_renderer_log_callback(logger: &Arc<Logger>) -> video_renderer_rs::port::LogCallback {
    let logger = Arc::clone(logger);
    Arc::new(move |level, message| logger.log(level as i32, message))
}

fn mux_renderer_log_callback(logger: &Arc<Logger>) -> mux_renderer_rs::port::LogCallback {
    let logger = Arc::clone(logger);
    Arc::new(move |level, message| logger.log(level as i32, message))
}

struct RendererMirrorCallbacks {
    outputs: Arc<Mutex<MediaOutputRuntime>>,
    logger: Option<Arc<Logger>>,
    use_video: bool,
    nofreeze: bool,
}

impl MirrorRuntimeCallbacks for RendererMirrorCallbacks {
    fn log(&self, level: MirrorLogLevel, message: &str) {
        if let Some(logger) = &self.logger {
            logger.log(level as i32, message);
        }
    }

    fn event(&self, event: MirrorEvent) -> bool {
        match event {
            MirrorEvent::CodecConfigured { codec, .. } => {
                lock(&self.outputs).select_video_codec(codec == VideoCodec::H265);
                !self.use_video
                    || video_renderer_choose_codec_and_size(false, codec == VideoCodec::H265) == 0
            }
            MirrorEvent::VideoFrame(frame) => {
                let mut outputs = lock(&self.outputs);
                outputs.process_video(&frame.data, frame.ntp_time_remote);
                if self.use_video {
                    render_video_with_clock(
                        &mut outputs,
                        self.logger.as_deref(),
                        &frame.data,
                        frame.ntp_time_remote,
                        frame.ntp_time_local,
                        video_renderer_render_buffer,
                    );
                }
                true
            }
            MirrorEvent::VideoPaused => {
                if self.use_video {
                    video_renderer_pause();
                }
                true
            }
            MirrorEvent::VideoResumed => {
                if self.use_video {
                    video_renderer_resume();
                }
                true
            }
            MirrorEvent::UnsupportedCodec
            | MirrorEvent::MalformedParameterSets(_)
            | MirrorEvent::CodecChanged { .. }
            | MirrorEvent::InvalidVideoPayload => false,
            MirrorEvent::ClientStreamingReport(report) => {
                if let Some(logger) = &self.logger
                    && let Ok(value) = plist::Value::from_reader(std::io::Cursor::new(report))
                {
                    let mut xml = Vec::new();
                    if value.to_writer_xml(&mut xml).is_ok() {
                        logger.log_bytes(crate::logger::INFO, &xml);
                    }
                }
                true
            }
            MirrorEvent::OldProtocolHeartbeat | MirrorEvent::UnexpectedPacketType(_) => true,
        }
    }

    fn mirror_video_running(&self, running: bool) {
        lock(&self.outputs).mirror_video_running(running);
    }

    fn connection_reset(&self, _reason: u8) {
        if should_stop_video_on_connection_reset(self.nofreeze) {
            video_renderer_stop();
        }
    }

    fn video_reset(&self) {
        video_renderer_stop();
    }
}

fn render_video_with_clock(
    outputs: &mut MediaOutputRuntime,
    logger: Option<&Logger>,
    data: &[u8],
    remote_time_ns: u64,
    local_time_ns: u64,
    mut render: impl FnMut(&[u8], u64) -> u64,
) {
    let mut timestamp = outputs.renderer_timestamp(remote_time_ns, local_time_ns);
    for _ in 0..10 {
        let mismatch_ns = render(data, timestamp);
        if mismatch_ns == 0 {
            break;
        }
        if let Some(logger) = logger {
            logger.log(
                crate::logger::INFO,
                format!(
                    "adjust timestamps by {:8.6} secs",
                    mismatch_ns as f64 / 1_000_000_000.0
                ),
            );
        }
        outputs.adjust_remote_clock_offset(mismatch_ns);
        timestamp = timestamp.wrapping_add(mismatch_ns);
    }
}

fn reset_video(reason: VideoResetReason, close_window_on_client_stop: bool) -> io::Result<()> {
    video_renderer_stop();
    match video_reset_action(reason, close_window_on_client_stop) {
        VideoResetAction::Keep => {}
        VideoResetAction::Reinitialize => {
            video_renderer_reinitialize().map_err(io::Error::other)?;
        }
        VideoResetAction::RestoreMirror => {
            video_renderer_restore_mirror().map_err(io::Error::other)?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VideoResetAction {
    Keep,
    Reinitialize,
    RestoreMirror,
}

fn video_reset_action(
    reason: VideoResetReason,
    close_window_on_client_stop: bool,
) -> VideoResetAction {
    match reason {
        VideoResetReason::RtpToHlsTeardown => VideoResetAction::Reinitialize,
        VideoResetReason::RtpShutdown if close_window_on_client_stop => {
            VideoResetAction::Reinitialize
        }
        VideoResetReason::RtpShutdown => VideoResetAction::Keep,
        VideoResetReason::HlsShutdown => VideoResetAction::RestoreMirror,
    }
}

fn should_stop_video_on_connection_reset(nofreeze: bool) -> bool {
    nofreeze
}

fn renderer_flip(flip: OptionFlip) -> RendererFlip {
    match flip {
        OptionFlip::None => RendererFlip::None,
        OptionFlip::Horizontal => RendererFlip::Horizontal,
        OptionFlip::Vertical => RendererFlip::Vertical,
        OptionFlip::Invert => RendererFlip::Invert,
    }
}

fn renderer_rotation(rotation: VideoRotation) -> RendererFlip {
    match rotation {
        VideoRotation::None => RendererFlip::None,
        VideoRotation::Left => RendererFlip::Left,
        VideoRotation::Right => RendererFlip::Right,
    }
}

fn video_renderer_config(options: &Options) -> VideoRendererConfig {
    let use_video = options.video_sink != "0";
    let mut parser = options.video_parser.clone();
    let mut converter = options.video_converter.clone();
    if use_video && options.bt709_fix {
        parser.push_str(" ! capssetter caps=\"video/x-h264, colorimetry=bt709\"");
    }
    if use_video && options.srgb_fix {
        converter = format!("{converter} ! video/x-raw,colorimetry=sRGB,format=RGBx ! {converter}");
    }
    VideoRendererConfig {
        server_name: options.server_name.clone(),
        flip: [
            renderer_flip(options.video_flip),
            renderer_rotation(options.video_rotation),
        ],
        parser,
        rtp_pipeline: options.video_rtp_pipeline.clone().unwrap_or_default(),
        decoder: options.video_decoder.clone(),
        converter,
        video_sink: options.video_sink.clone(),
        video_sink_options: options.video_sink_options.clone(),
        initial_fullscreen: options.fullscreen,
        video_sync: options.video_sync,
        h265_support: options.h265_support,
        coverart_support: options.render_cover_art,
        playbin_version: options.playbin_version,
        uri: None,
    }
}

fn audio_timestamp_with_option_delay(
    codec_type: u8,
    local_timestamp_ns: u64,
    alac_delay_ns: i64,
    aac_delay_ns: i64,
) -> u64 {
    let delay_ns = match codec_type {
        2 => alac_delay_ns,
        4 | 8 => aac_delay_ns,
        _ => 0,
    };
    local_timestamp_ns.wrapping_add(delay_ns as u64)
}

fn address_length(address: &str) -> io::Result<usize> {
    match IpAddr::from_str(address) {
        Ok(address) => Ok(if address.is_ipv4() { 4 } else { 16 }),
        Err(_) if crate::netutils::scoped_ipv6_socket_addr(address, 0).is_ok() => Ok(16),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "remote address is not numeric",
        )),
    }
}

#[cfg(test)]
mod address_tests {
    use super::address_length;

    #[test]
    fn link_local_ipv6_address_keeps_numeric_scope_as_ipv6() {
        assert_eq!(address_length("fe80::1%7").unwrap(), 16);
        assert!(address_length("fe80::1%en0").is_err());
    }
}

fn bind_session_udp_socket(
    remote_address: IpAddr,
    requested_port: u16,
    peer_to_peer: bool,
) -> io::Result<(UdpSocket, u16)> {
    let family = if remote_address.is_ipv4() {
        crate::netutils::AddressFamily::Ipv4
    } else {
        crate::netutils::AddressFamily::Ipv6
    };
    let mut local_port = requested_port;
    let socket = crate::netutils::bind_socket(
        &mut local_port,
        family,
        crate::netutils::Transport::Udp,
        peer_to_peer,
    )?;
    Ok((socket.into(), local_port))
}

fn bind_mirror_tcp_listener(
    remote_address: IpAddr,
    requested_port: u16,
    peer_to_peer: bool,
) -> io::Result<TcpListener> {
    let family = if remote_address.is_ipv4() {
        crate::netutils::AddressFamily::Ipv4
    } else {
        crate::netutils::AddressFamily::Ipv6
    };
    let mut local_port = requested_port;
    let socket = crate::netutils::bind_socket(
        &mut local_port,
        family,
        crate::netutils::Transport::Tcp,
        peer_to_peer,
    )?;
    socket.listen(1)?;
    Ok(socket.into())
}

fn time_protocol(protocol: TimingProtocol) -> TimeProtocol {
    match protocol {
        TimingProtocol::Ntp => TimeProtocol::Ntp,
        TimingProtocol::None => TimeProtocol::None,
        TimingProtocol::Other => TimeProtocol::Other,
        TimingProtocol::Unspecified => TimeProtocol::Unspecified,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn ntp_logger_callback(logger: &Option<Arc<Logger>>) -> Option<NtpLogCallback> {
    logger.as_ref().map(|logger| {
        let logger = Arc::clone(logger);
        Arc::new(move |level, message: &str| logger.log(level, message)) as NtpLogCallback
    })
}

fn rtp_logger_callback(logger: &Option<Arc<Logger>>) -> Option<RtpLogCallback> {
    logger.as_ref().map(|logger| {
        let logger = Arc::clone(logger);
        Arc::new(move |level, message: &str| logger.log(level, message)) as RtpLogCallback
    })
}

#[cfg(test)]
mod volume_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_prefix(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("uxplay-{label}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn encrypted_alac_rtp_reaches_output_sink_and_finalizes_mp4() {
        let _renderer_guard = VIDEO_RENDERER_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gstreamer::init().expect("GStreamer is required for the audio renderer test");
        if [
            "appsrc",
            "queue",
            "avdec_alac",
            "audioconvert",
            "audioresample",
            "volume",
            "level",
            "udpsink",
            "mp4mux",
        ]
        .into_iter()
        .any(|factory| gstreamer::ElementFactory::find(factory).is_none())
        {
            return;
        }
        use uxplay_crypto::port::{AesContext, AesMode, Direction};

        struct RendererCleanup;
        impl Drop for RendererCleanup {
            fn drop(&mut self) {
                audio_renderer_destroy();
            }
        }

        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let port = receiver.local_addr().unwrap().port();
        let logger = Arc::new(Logger::new());
        audio_renderer_init(
            AudioRendererConfig::new(format!("udpsink host=127.0.0.1 port={port}"), false, false),
            audio_renderer_log_callback(&logger),
        )
        .expect("audio renderer should build with the loopback sink");
        let _cleanup = RendererCleanup;
        assert_eq!(
            audio_renderer_rs::port::audio_renderer_start(2),
            Some(audio_renderer_rs::port::StartOutcome::Started(
                audio_renderer_rs::port::AudioFormat::Alac
            ))
        );

        let dump_prefix = temporary_prefix("audio-renderer-rtp");
        let mux_prefix = temporary_prefix("audio-renderer-mp4");
        let output_options = Options {
            mux_to_file: true,
            mux_filename: mux_prefix.to_string_lossy().into_owned(),
            video_sink: "0".into(),
            ..Options::default()
        };
        let mut output_runtime = MediaOutputRuntime::new(&output_options)
            .expect("app output runtime should initialize the MP4 muxer");
        output_runtime.dump_audio = true;
        output_runtime.audio_dump_limit = Some(1);
        output_runtime.audio_dump_filename = dump_prefix.to_string_lossy().into_owned();
        output_runtime.connection_opened();
        let outputs = Arc::new(Mutex::new(output_runtime));
        let callbacks: Arc<dyn AudioCallbacks> = Arc::new(RendererAudioCallbacks {
            config: SessionMediaConfig {
                use_audio: true,
                outputs: Arc::clone(&outputs),
                ..SessionMediaConfig::default()
            },
            track_metadata: Arc::new(Mutex::new(crate::dmap_metadata::TrackMetadata::default())),
        });

        struct FixedClock;
        impl TimingClock for FixedClock {
            fn local_time_ns(&self) -> u64 {
                2_000_000_000
            }

            fn adjust_remote_timestamp_offset(
                &self,
                timestamp: u64,
                _add_epoch_seconds: bool,
            ) -> u64 {
                timestamp
            }

            fn timestamp_to_nanoseconds(&self, timestamp: u64) -> u64 {
                timestamp
            }

            fn convert_remote_time_ns(&self, remote_time_ns: u64) -> u64 {
                remote_time_ns
            }

            fn video_arrival_offset(&self) -> u64 {
                0
            }
        }

        let key = [0x19; 16];
        let iv = [0xa7; 16];
        let mut receiver_session = RaopRtpSession::new(
            None,
            callbacks,
            Arc::new(FixedClock),
            "127.0.0.1",
            4,
            &key,
            &iv,
        )
        .unwrap();
        let StartState::Started(ports) = receiver_session
            .start_audio(
                AudioPorts {
                    control_remote: 0,
                    control_local: 0,
                    data_local: 0,
                },
                2,
                44_100,
            )
            .unwrap()
        else {
            panic!("RAOP audio receiver should bind loopback UDP ports")
        };

        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut sync_packet = [0; 20];
        sync_packet[0] = 0x80;
        sync_packet[1] = 0xd4;
        sync_packet[2..4].copy_from_slice(&4_u16.to_be_bytes());
        sync_packet[4..8].copy_from_slice(&352_u32.to_be_bytes());
        sync_packet[8..16].copy_from_slice(&2_000_000_000_u64.to_be_bytes());
        sync_packet[16..20].copy_from_slice(&704_u32.to_be_bytes());
        sender
            .send_to(&sync_packet, ("127.0.0.1", ports.control_local))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));

        let clear_frame =
            include_bytes!("../../render/audio-renderer/testdata/alac-frame-352.alac");
        let encrypted_length = clear_frame.len() / 16 * 16;
        let mut encryptor = AesContext::new(AesMode::Cbc, Direction::Encrypt, &key, &iv).unwrap();
        let mut encrypted_payload = encryptor
            .cbc_crypt(&clear_frame[..encrypted_length], Direction::Encrypt)
            .unwrap();
        encrypted_payload.extend_from_slice(&clear_frame[encrypted_length..]);
        for sequence in [29_u16, 30] {
            let mut audio_packet = vec![0x80, 0x60];
            audio_packet.extend_from_slice(&sequence.to_be_bytes());
            audio_packet.extend_from_slice(&352_u32.to_be_bytes());
            audio_packet.extend_from_slice(&[0; 4]);
            audio_packet.extend_from_slice(&encrypted_payload);
            sender
                .send_to(&audio_packet, ("127.0.0.1", ports.data_local))
                .unwrap();
        }

        let mut decoded_pcm = [0; 4096];
        let length = receiver.recv(&mut decoded_pcm).unwrap();
        assert!(
            length >= 128,
            "ALAC output sink received only {length} bytes"
        );
        assert_eq!(length % 4, 0, "decoded output is stereo PCM frames");
        assert_ne!(&decoded_pcm[..length], clear_frame);
        let dump_path = std::path::PathBuf::from(format!("{}.1.alac", dump_prefix.display()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !dump_path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(std::fs::read(&dump_path).unwrap(), clear_frame);
        receiver_session.stop();
        audio_renderer_stop();
        lock(&outputs).connection_closed(false, None);
        let mp4_path = std::path::PathBuf::from(format!("{}.1.ALAC.mp4", mux_prefix.display()));
        let mp4 = std::fs::read(&mp4_path).expect("audio callback should finalize its MP4 output");
        assert!(mp4.len() > 64, "finalized MP4 is only {} bytes", mp4.len());
        for atom in [b"ftyp".as_slice(), b"moov".as_slice(), b"mdat".as_slice()] {
            assert!(
                mp4.windows(atom.len()).any(|window| window == atom),
                "finalized MP4 does not contain the {} atom",
                String::from_utf8_lossy(atom)
            );
        }
        std::fs::remove_file(dump_path).unwrap();
        std::fs::remove_file(mp4_path).unwrap();
    }

    fn annex_b_nalus(data: &[u8]) -> Vec<&[u8]> {
        let mut start_codes = Vec::new();
        let mut offset = 0;
        while offset + 3 <= data.len() {
            if offset + 4 <= data.len() && data[offset..offset + 4] == [0, 0, 0, 1] {
                start_codes.push((offset, 4));
                offset += 4;
            } else if data[offset..offset + 3] == [0, 0, 1] {
                start_codes.push((offset, 3));
                offset += 3;
            } else {
                offset += 1;
            }
        }
        start_codes
            .iter()
            .enumerate()
            .filter_map(|(index, &(start, prefix_len))| {
                let payload_start = start + prefix_len;
                let payload_end = start_codes
                    .get(index + 1)
                    .map_or(data.len(), |(next_start, _)| *next_start);
                (payload_start < payload_end).then_some(&data[payload_start..payload_end])
            })
            .collect()
    }

    #[test]
    fn h264_mirror_callback_finalizes_video_only_mp4() {
        let _renderer_guard = VIDEO_RENDERER_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gstreamer::init().expect("GStreamer is required for the video mux test");
        if ["appsrc", "queue", "h264parse", "mp4mux"]
            .into_iter()
            .any(|factory| gstreamer::ElementFactory::find(factory).is_none())
        {
            return;
        }

        let mux_prefix = temporary_prefix("mirror-video-mp4");
        let options = Options {
            use_audio: false,
            audio_sink: "0".into(),
            video_sink: "fakesink".into(),
            mux_to_file: true,
            mux_filename: mux_prefix.to_string_lossy().into_owned(),
            ..Options::default()
        };
        let mut output_runtime =
            MediaOutputRuntime::new(&options).expect("app output runtime should initialize mux");
        output_runtime.connection_opened();
        let outputs = Arc::new(Mutex::new(output_runtime));
        let callbacks = RendererMirrorCallbacks {
            outputs: Arc::clone(&outputs),
            logger: None,
            use_video: false,
            nofreeze: false,
        };

        assert!(callbacks.event(MirrorEvent::CodecConfigured {
            codec: VideoCodec::H264,
            dimensions: uxplay_raop_rtp_mirror::VideoDimensions {
                width_source: 0,
                height_source: 0,
                x_offset: 0,
                y_offset: 0,
                width: 0,
                height: 0,
            },
        }));
        let fixture = include_bytes!("../../render/video-renderer/testdata/hls/mirror-frame.h264");
        let nalus = annex_b_nalus(fixture);
        let sps = nalus
            .iter()
            .copied()
            .find(|nalu| nalu.first().is_some_and(|byte| byte & 0x1f == 7))
            .expect("H.264 fixture SPS");
        let pps = nalus
            .iter()
            .copied()
            .find(|nalu| nalu.first().is_some_and(|byte| byte & 0x1f == 8))
            .expect("H.264 fixture PPS");
        let idr = nalus
            .iter()
            .copied()
            .find(|nalu| nalu.first().is_some_and(|byte| byte & 0x1f == 5))
            .expect("H.264 fixture IDR frame");
        let mut encoded_frame = Vec::new();
        for nalu in [sps, pps, idr] {
            encoded_frame.extend_from_slice(&[0, 0, 0, 1]);
            encoded_frame.extend_from_slice(nalu);
        }
        assert!(callbacks.event(MirrorEvent::VideoFrame(
            uxplay_raop_rtp_mirror::VideoFrame {
                codec: VideoCodec::H264,
                data: encoded_frame,
                nal_count: 3,
                ntp_time_local: 2_000_000_000,
                ntp_time_remote: 2_000_000_000,
            }
        )));
        lock(&outputs).connection_closed(false, None);

        let mp4_path = std::path::PathBuf::from(format!("{}.1.H264.mp4", mux_prefix.display()));
        let mp4 = std::fs::read(&mp4_path).expect("mirror callback should finalize video MP4");
        assert!(
            mp4.len() > 64,
            "finalized video MP4 is only {} bytes",
            mp4.len()
        );
        for atom in [b"ftyp".as_slice(), b"moov".as_slice(), b"mdat".as_slice()] {
            assert!(
                mp4.windows(atom.len()).any(|window| window == atom),
                "finalized video MP4 does not contain the {} atom",
                String::from_utf8_lossy(atom)
            );
        }
        std::fs::remove_file(mp4_path).unwrap();
    }

    #[test]
    fn screensaver_mode_one_tracks_mirror_start_and_stop_once() {
        let (requests, received) = std::sync::mpsc::channel();
        let mut inhibitor = ScreensaverInhibitor {
            mode: 1,
            inhibited: false,
            previous_hls_position: 0.0,
            requests: Some(requests),
            worker: None,
        };

        inhibitor.mirror_video_running(true);
        inhibitor.mirror_video_running(true);
        inhibitor.mirror_video_running(false);

        assert!(matches!(
            received.recv().unwrap(),
            ScreensaverRequest::SetInhibited(true)
        ));
        assert!(matches!(
            received.recv().unwrap(),
            ScreensaverRequest::SetInhibited(false)
        ));
        inhibitor.shutdown();
        assert!(matches!(
            received.recv().unwrap(),
            ScreensaverRequest::Shutdown
        ));
    }

    #[test]
    fn screensaver_mode_one_tracks_hls_progress_and_stalls() {
        let (requests, received) = std::sync::mpsc::channel();
        let mut inhibitor = ScreensaverInhibitor {
            mode: 1,
            inhibited: false,
            previous_hls_position: 0.0,
            requests: Some(requests),
            worker: None,
        };

        inhibitor.observe_hls_position(0.0);
        inhibitor.observe_hls_position(1.0);
        inhibitor.observe_hls_position(2.0);
        inhibitor.observe_hls_position(2.0);

        assert!(matches!(
            received.recv().unwrap(),
            ScreensaverRequest::SetInhibited(true)
        ));
        assert!(matches!(
            received.recv().unwrap(),
            ScreensaverRequest::SetInhibited(false)
        ));
        assert!(received.try_recv().is_err());
        inhibitor.shutdown();
    }

    #[test]
    fn screensaver_service_selection_matches_source_desktop_substrings() {
        assert_eq!(
            screensaver_service_for_desktop("XFCE:GNOME"),
            ScreensaverService {
                name: "org.xfce.ScreenSaver",
                path: "/org/xfce/ScreenSaver",
                interface: "org.xfce.ScreenSaver",
            }
        );
        assert_eq!(
            screensaver_service_for_desktop("MATE"),
            ScreensaverService {
                name: "org.mate.ScreenSaver",
                path: "/org/mate/ScreenSaver",
                interface: "org.mate.ScreenSaver",
            }
        );
        assert_eq!(
            screensaver_service_for_desktop("GNOME"),
            FREEDESKTOP_SCREENSAVER
        );
    }

    #[test]
    fn timing_and_audio_runtime_callbacks_reach_the_source_logger() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_for_callback = Arc::clone(&events);
        let logger = Arc::new(Logger::new());
        logger.set_level(crate::logger::DEBUG);
        logger.set_callback(Some(Arc::new(move |level, message| {
            lock(&events_for_callback).push((level, message.to_vec()));
        })));
        let configured = Some(logger);

        ntp_logger_callback(&configured).unwrap()(crate::logger::DEBUG, "timing thread");
        rtp_logger_callback(&configured).unwrap()(crate::logger::ERR, "audio receive failed");

        assert_eq!(
            *lock(&events),
            [
                (crate::logger::DEBUG, b"timing thread".to_vec()),
                (crate::logger::ERR, b"audio receive failed".to_vec()),
            ]
        );
    }

    #[test]
    fn mirror_runtime_logs_keep_source_levels_and_streaming_report_xml() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_for_callback = Arc::clone(&events);
        let logger = Arc::new(Logger::new());
        logger.set_level(crate::logger::DEBUG_DATA);
        logger.set_callback(Some(Arc::new(move |level, message| {
            lock(&events_for_callback).push((level, message.to_vec()));
        })));
        let callbacks = RendererMirrorCallbacks {
            outputs: Arc::new(Mutex::new(MediaOutputRuntime::default())),
            logger: Some(Arc::clone(&logger)),
            use_video: false,
            nofreeze: false,
        };
        let mut report = plist::Dictionary::new();
        report.insert("fps".to_owned(), plist::Value::Integer(60.into()));
        let report = plist::Value::Dictionary(report);
        let mut binary = Vec::new();
        report.to_writer_binary(&mut binary).unwrap();
        let mut expected_xml = Vec::new();
        report.to_writer_xml(&mut expected_xml).unwrap();

        callbacks.log(MirrorLogLevel::DebugData, "mirror packet timing");
        assert!(callbacks.event(MirrorEvent::ClientStreamingReport(binary)));

        assert_eq!(
            *lock(&events),
            [
                (crate::logger::DEBUG_DATA, b"mirror packet timing".to_vec()),
                (crate::logger::INFO, expected_xml),
            ]
        );
    }

    #[test]
    fn renderer_callbacks_share_the_source_logger_and_severity_values() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_for_callback = Arc::clone(&events);
        let logger = Arc::new(Logger::new());
        logger.set_level(crate::logger::DEBUG);
        logger.set_callback(Some(Arc::new(move |level, message| {
            lock(&events_for_callback).push((level, message.to_vec()));
        })));

        audio_renderer_log_callback(&logger)(
            audio_renderer_rs::port::LogLevel::Error,
            "audio pipeline failed",
        );
        video_renderer_log_callback(&logger)(
            video_renderer_rs::port::LogLevel::Warning,
            "video sink warning",
        );
        mux_renderer_log_callback(&logger)(
            mux_renderer_rs::port::LogLevel::Info,
            "recording started",
        );

        assert_eq!(
            *lock(&events),
            [
                (crate::logger::ERR, b"audio pipeline failed".to_vec()),
                (crate::logger::WARNING, b"video sink warning".to_vec()),
                (crate::logger::INFO, b"recording started".to_vec()),
            ]
        );
    }

    #[test]
    fn maps_airplay_db_to_configured_gstreamer_range_and_taper() {
        assert_eq!(source_volume_to_gstreamer(-144.0, (-30.0, 0.0), false), 0.0);
        assert_eq!(source_volume_to_gstreamer(-30.0, (-30.0, 0.0), false), 0.0);
        assert_eq!(source_volume_to_gstreamer(0.0, (-30.0, 0.0), false), 1.0);
        let flat = source_volume_to_gstreamer(-15.0, (-30.0, 0.0), false);
        assert!((flat - 10.0_f64.powf(-0.75)).abs() < 1e-12);
        let custom = source_volume_to_gstreamer(-15.0, (-24.0, -3.0), false);
        assert!((custom - 10.0_f64.powf(-0.675)).abs() < 1e-12);
        let tapered = source_volume_to_gstreamer(-15.0, (-30.0, 0.0), true);
        assert!((tapered - 10.0_f64.powf(-0.5)).abs() < 1e-12);
    }

    #[test]
    fn nofreeze_controls_whether_a_mirror_disconnect_stops_video() {
        assert!(!should_stop_video_on_connection_reset(false));
        assert!(should_stop_video_on_connection_reset(true));
    }

    #[test]
    fn nc_controls_renderer_reinitialization_after_client_stop() {
        assert_eq!(
            video_reset_action(VideoResetReason::RtpShutdown, true),
            VideoResetAction::Reinitialize
        );
        assert_eq!(
            video_reset_action(VideoResetReason::RtpShutdown, false),
            VideoResetAction::Keep
        );
        assert_eq!(
            video_reset_action(VideoResetReason::RtpToHlsTeardown, false),
            VideoResetAction::Reinitialize
        );
        assert_eq!(
            video_reset_action(VideoResetReason::HlsShutdown, false),
            VideoResetAction::RestoreMirror
        );
    }

    #[test]
    fn audio_dump_rotates_by_negotiated_codec_and_obeys_frame_limit() {
        let prefix = temporary_prefix("audio");
        let mut outputs = MediaOutputRuntime {
            dump_audio: true,
            audio_dump_limit: Some(1),
            audio_dump_filename: prefix.to_string_lossy().into_owned(),
            ..MediaOutputRuntime::default()
        };
        outputs.select_audio_codec(2);
        outputs.process_audio(2, b"alac-frame", 1);
        outputs.process_audio(2, b"over-limit", 2);
        outputs.select_audio_codec(8);
        outputs.process_audio(8, b"aac-frame", 3);
        outputs.shutdown();

        let alac = std::path::PathBuf::from(format!("{}.1.alac", prefix.display()));
        let aac = std::path::PathBuf::from(format!("{}.2.aac", prefix.display()));
        assert_eq!(std::fs::read(&alac).unwrap(), b"alac-frame");
        assert_eq!(std::fs::read(&aac).unwrap(), b"aac-frame");
        let _ = std::fs::remove_file(alac);
        let _ = std::fs::remove_file(aac);
    }

    #[test]
    fn video_dump_rolls_on_h264_sps_and_closes_with_annex_b_marker() {
        let prefix = temporary_prefix("video");
        let mut outputs = MediaOutputRuntime {
            dump_video: true,
            video_dump_limit: Some(1),
            video_dump_filename: prefix.to_string_lossy().into_owned(),
            ..MediaOutputRuntime::default()
        };
        let frame = [0, 0, 0, 1, 0x65, 0xaa];
        let sps = [0, 0, 0, 1, 0x67, 0xbb];
        outputs.process_video(&frame, 1);
        outputs.process_video(&sps, 2);
        outputs.shutdown();

        let first = std::path::PathBuf::from(format!("{}.1.h264", prefix.display()));
        let second = std::path::PathBuf::from(format!("{}.2.h264", prefix.display()));
        assert_eq!(
            std::fs::read(&first).unwrap(),
            [frame.as_slice(), &[0, 0, 0, 1]].concat()
        );
        assert_eq!(
            std::fs::read(&second).unwrap(),
            [sps.as_slice(), &[0, 0, 0, 1]].concat()
        );
        let _ = std::fs::remove_file(first);
        let _ = std::fs::remove_file(second);
    }

    #[test]
    fn disabled_audio_sink_disables_audio_dump_output() {
        let options = Options {
            dump_audio: true,
            audio_sink: "0".into(),
            ..Options::default()
        };
        let outputs = MediaOutputRuntime::new(&options).unwrap();
        assert!(!outputs.dump_audio);
    }

    #[test]
    fn app_supplied_peer_to_peer_binders_keep_the_socket_family_and_selected_port() {
        let peer_to_peer = cfg!(all(target_os = "macos", feature = "apple-p2p"));
        let (audio_socket, audio_port) =
            bind_session_udp_socket(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0, peer_to_peer)
                .unwrap();
        assert_ne!(audio_port, 0);
        assert_eq!(audio_socket.local_addr().unwrap().port(), audio_port);
        assert!(audio_socket.local_addr().unwrap().is_ipv4());

        let mirror_listener =
            bind_mirror_tcp_listener(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0, peer_to_peer)
                .unwrap();
        assert!(mirror_listener.local_addr().unwrap().is_ipv4());
        assert_ne!(mirror_listener.local_addr().unwrap().port(), 0);
    }

    #[test]
    fn video_color_options_match_source_pipeline_fragments() {
        let options = Options {
            bt709_fix: true,
            srgb_fix: true,
            ..Options::default()
        };
        let config = video_renderer_config(&options);
        assert_eq!(
            config.parser,
            "h264parse ! capssetter caps=\"video/x-h264, colorimetry=bt709\""
        );
        assert_eq!(
            config.converter,
            "videoconvert ! video/x-raw,colorimetry=sRGB,format=RGBx ! videoconvert"
        );

        let disabled = video_renderer_config(&Options {
            video_sink: "0".into(),
            ..options
        });
        assert_eq!(disabled.parser, "h264parse");
        assert_eq!(disabled.converter, "videoconvert");
    }

    #[test]
    fn audio_timestamp_delay_uses_codec_specific_signed_offset() {
        assert_eq!(audio_timestamp_with_option_delay(2, 100, 25, -7), 125);
        assert_eq!(audio_timestamp_with_option_delay(4, 100, 25, -7), 93);
        assert_eq!(audio_timestamp_with_option_delay(8, 100, 25, -7), 93);
        assert_eq!(audio_timestamp_with_option_delay(9, 100, 25, -7), 100);
        assert_eq!(audio_timestamp_with_option_delay(2, 3, -5, 0), u64::MAX - 1);
    }

    #[test]
    fn audio_and_video_share_the_source_clock_offset_and_pts_correction() {
        let mut outputs = MediaOutputRuntime::default();
        assert_eq!(outputs.renderer_timestamp(100, 110), 110);
        assert_eq!(outputs.renderer_timestamp(200, 250), 210);

        let mut submitted = Vec::new();
        let mut mismatches = [19, 0].into_iter();
        render_video_with_clock(
            &mut outputs,
            None,
            &[0, 0, 0, 1],
            300,
            350,
            |_, timestamp| {
                submitted.push(timestamp);
                mismatches.next().unwrap_or(0)
            },
        );
        assert_eq!(submitted, [310, 329]);
        assert_eq!(outputs.renderer_timestamp(400, 470), 429);
    }

    #[test]
    fn raop_video_reset_clears_the_shared_clock_even_when_video_is_disabled() {
        let outputs = Arc::new(Mutex::new(MediaOutputRuntime::default()));
        lock(&outputs).remote_clock_offset = 123;
        let config = SessionMediaConfig {
            use_video: false,
            outputs: Arc::clone(&outputs),
            ..SessionMediaConfig::default()
        };
        let mut media = SessionMedia::new(config);

        media
            .apply(SessionEffect::VideoReset(VideoResetReason::RtpShutdown))
            .unwrap();

        assert_eq!(lock(&outputs).remote_clock_offset, 0);
        media.stop();
    }

    #[test]
    fn feedback_watchdog_resets_while_idle_and_expires_after_source_limit() {
        let mut outputs = MediaOutputRuntime::default();
        assert_eq!(
            outputs.feedback_watchdog_tick(4),
            FeedbackWatchdogTick::Idle
        );
        outputs.connection_opened();
        assert_eq!(
            outputs.feedback_watchdog_tick(4),
            FeedbackWatchdogTick::Active(0)
        );
        assert_eq!(
            outputs.feedback_watchdog_tick(4),
            FeedbackWatchdogTick::Active(1)
        );
        assert_eq!(
            outputs.feedback_watchdog_tick(4),
            FeedbackWatchdogTick::Active(2)
        );
        assert_eq!(
            outputs.feedback_watchdog_tick(4),
            FeedbackWatchdogTick::Warning(3)
        );
        assert_eq!(
            outputs.feedback_watchdog_tick(4),
            FeedbackWatchdogTick::Warning(4)
        );
        assert_eq!(
            outputs.feedback_watchdog_tick(4),
            FeedbackWatchdogTick::TimedOut(5)
        );

        outputs.note_client_feedback();
        assert_eq!(
            outputs.feedback_watchdog_tick(0),
            FeedbackWatchdogTick::Active(0)
        );
        outputs.connection_closed(false, None);
        assert_eq!(
            outputs.feedback_watchdog_tick(2),
            FeedbackWatchdogTick::Idle
        );
    }

    #[test]
    fn connection_close_signals_global_audio_cleanup_only_for_last_peer() {
        let mut outputs = MediaOutputRuntime::default();
        outputs.connection_opened();
        outputs.connection_opened();
        outputs.remote_clock_offset = 123;

        assert!(!outputs.connection_closed(false, None));
        assert_eq!(outputs.remote_clock_offset, 123);
        assert!(outputs.connection_closed(false, None));
        assert_eq!(outputs.remote_clock_offset, 0);

        outputs.connection_opened();
        assert!(outputs.connection_closed(false, None));
    }

    #[test]
    fn dacp_export_is_removed_only_when_the_last_connection_closes() {
        let path = temporary_prefix("dacp-cleanup");
        let outputs = Arc::new(Mutex::new(MediaOutputRuntime::default()));
        let config = SessionMediaConfig {
            dacp_filename: Some(path.to_string_lossy().into_owned()),
            outputs,
            ..SessionMediaConfig::default()
        };
        let mut first = SessionMedia::new(config.clone());
        let mut second = SessionMedia::new(config);
        std::fs::write(&path, b"client-dacp\n42\n").unwrap();

        first.stop();
        assert!(
            path.exists(),
            "the export belongs to the remaining connection"
        );
        second.stop();
        assert!(
            !path.exists(),
            "the final connection removes the DACP export"
        );
    }

    #[test]
    fn audio_teardown_keeps_global_renderer_until_last_connection_closes() {
        let _renderer_guard = VIDEO_RENDERER_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gstreamer::init().expect("GStreamer is required for audio renderer lifecycle test");
        if ["appsrc", "queue", "avdec_alac", "audioconvert", "volume"]
            .into_iter()
            .any(|factory| gstreamer::ElementFactory::find(factory).is_none())
        {
            return;
        }

        struct RendererCleanup;
        impl Drop for RendererCleanup {
            fn drop(&mut self) {
                audio_renderer_destroy();
            }
        }

        let logger = Arc::new(Logger::new());
        audio_renderer_init(
            AudioRendererConfig::new("fakesink sync=false", false, false),
            audio_renderer_log_callback(&logger),
        )
        .expect("audio renderer should initialize for lifecycle test");
        let _cleanup = RendererCleanup;
        assert_eq!(
            audio_renderer_rs::port::audio_renderer_start(2),
            Some(audio_renderer_rs::port::StartOutcome::Started(
                audio_renderer_rs::port::AudioFormat::Alac
            ))
        );

        let outputs = Arc::new(Mutex::new(MediaOutputRuntime::default()));
        let config = SessionMediaConfig {
            use_audio: true,
            outputs,
            ..SessionMediaConfig::default()
        };
        let mut first = SessionMedia::new(config.clone());
        let mut second = SessionMedia::new(config);
        first.apply(SessionEffect::StopAudio).unwrap();
        first.stop();

        let frame = include_bytes!("../../render/audio-renderer/testdata/alac-frame-352.alac");
        assert_eq!(
            audio_renderer_render_buffer(frame, 0, 0),
            Some(audio_renderer_rs::port::RenderOutcome::Submitted),
            "closing one RAOP connection must leave audio active for other clients"
        );

        second.stop();
        assert_eq!(
            audio_renderer_render_buffer(frame, 0, 0),
            Some(audio_renderer_rs::port::RenderOutcome::NoActivePipeline),
            "the last connection close stops the process-global audio renderer"
        );
    }
}
