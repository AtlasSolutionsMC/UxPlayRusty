//! Rust-owned GStreamer implementation of `renderers/video_renderer.c`.
//!
//! The public `VideoRenderer` is instance based so the app can own renderer
//! lifetime explicitly. The C-compatible functions below retain the source's
//! process-global API for the incremental migration bridge.

#![forbid(unsafe_code)]
// Preserve the source's readable nested state branches during the C port.
#![allow(clippy::collapsible_if)]

use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};

use crate::x11_fix::{SearchResult, X11WindowFix};
use glib::{ControlFlow, MainLoop};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;

const SECOND_IN_NSECS: u64 = 1_000_000_000;
const PLAY_FLAG_DOWNLOAD: u32 = 1 << 7;
const PLAY_FLAG_BUFFERING: u32 = 1 << 8;

/// Source-compatible logger levels used by `lib/logger.h`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum LogLevel {
    Error = 3,
    Warning = 4,
    Info = 6,
    Debug = 7,
}

pub type LogCallback = Arc<dyn Fn(LogLevel, &str) + Send + Sync + 'static>;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VideoRendererKind {
    #[default]
    Auto,
    D3d11,
    Vulkan,
}

/// The six `videoflip_t` values accepted by the original renderer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(i32)]
pub enum VideoFlip {
    #[default]
    None = 0,
    Left = 1,
    Right = 2,
    Invert = 3,
    Vertical = 4,
    Horizontal = 5,
}

impl VideoFlip {
    pub const fn from_source_value(value: i32) -> Self {
        match value {
            1 => Self::Left,
            2 => Self::Right,
            3 => Self::Invert,
            4 => Self::Vertical,
            5 => Self::Horizontal,
            _ => Self::None,
        }
    }
}

/// Renderer settings captured by `video_renderer_init`.
#[derive(Clone, Debug)]
pub struct VideoRendererConfig {
    pub renderer: VideoRendererKind,
    pub server_name: String,
    pub flip: [VideoFlip; 2],
    pub parser: String,
    pub rtp_pipeline: String,
    pub decoder: String,
    pub low_latency: bool,
    pub converter: String,
    pub video_sink: String,
    pub video_sink_options: String,
    pub initial_fullscreen: bool,
    pub video_sync: bool,
    pub h265_support: bool,
    pub coverart_support: bool,
    pub playbin_version: u32,
    pub uri: Option<String>,
}

impl Default for VideoRendererConfig {
    fn default() -> Self {
        Self {
            renderer: VideoRendererKind::Auto,
            server_name: "UxPlay".to_owned(),
            flip: [VideoFlip::None; 2],
            parser: "h264parse".to_owned(),
            rtp_pipeline: String::new(),
            decoder: "avdec_h264".to_owned(),
            low_latency: false,
            converter: "videoconvert".to_owned(),
            video_sink: "autovideosink".to_owned(),
            video_sink_options: String::new(),
            initial_fullscreen: false,
            video_sync: true,
            h265_support: false,
            coverart_support: false,
            playbin_version: 3,
            uri: None,
        }
    }
}

/// Deterministic result from forwarding an encoded packet to appsrc.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderOutcome {
    Submitted,
    InvalidFrame,
    NoActiveRenderer,
    EmptyPacket,
    InvalidTimestamp(u64),
}

/// HLS state exposed to the C playback-info API.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaybackInfo {
    pub duration: f64,
    pub position: f64,
    pub seek_start: f64,
    pub seek_duration: f64,
    pub rate: f32,
    pub buffer_empty: bool,
    pub buffer_full: bool,
}

impl Default for PlaybackInfo {
    fn default() -> Self {
        Self {
            duration: 0.0,
            position: -1.0,
            seek_start: 0.0,
            seek_duration: 0.0,
            rate: 0.0,
            buffer_empty: false,
            buffer_full: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Codec {
    H264,
    H265,
    Hls,
    Jpeg,
}

impl Codec {
    const fn name(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::H265 => "h265",
            Self::Hls => "hls",
            Self::Jpeg => "jpeg",
        }
    }

    const fn caps(self) -> Option<&'static str> {
        match self {
            Self::H264 => {
                Some("video/x-h264,stream-format=(string)byte-stream,alignment=(string)au")
            }
            Self::H265 => {
                Some("video/x-h265,stream-format=(string)byte-stream,alignment=(string)au")
            }
            Self::Jpeg => Some("image/jpeg"),
            Self::Hls => None,
        }
    }
}

#[derive(Debug)]
struct PipelineEntry {
    codec: Codec,
    pipeline: gst::Pipeline,
    appsrc: Option<gst_app::AppSrc>,
    textsrc: Option<gst::Element>,
    bus: gst::Bus,
    eos: Arc<Mutex<bool>>,
    watch: Option<glib::Source>,
    watch_id: Option<u32>,
    discovered_sink: Arc<Mutex<Option<gst::Element>>>,
    uri: Option<String>,
}

#[derive(Debug)]
struct HlsState {
    requested_start_ns: i64,
    seek_start_ns: i64,
    seek_end_ns: i64,
    duration_ns: i64,
    seek_enabled: bool,
    buffer_empty: bool,
    buffer_full: bool,
    volume: f64,
}

impl Default for HlsState {
    fn default() -> Self {
        Self {
            requested_start_ns: 0,
            seek_start_ns: -1,
            seek_end_ns: -1,
            duration_ns: -1,
            seek_enabled: false,
            buffer_empty: true,
            buffer_full: false,
            volume: 1.0,
        }
    }
}

/// Owns the pipelines, appsrc feeds, HLS status, and bus watches for one
/// AirPlay client. It preserves the original renderer's codec pipeline order.
pub struct VideoRenderer {
    config: VideoRendererConfig,
    renderers: Vec<Option<PipelineEntry>>,
    type_h264: Option<usize>,
    type_h265: Option<usize>,
    type_hls: Option<usize>,
    type_jpeg: Option<usize>,
    active: Option<usize>,
    sync: bool,
    first_packet: bool,
    base_time_ns: u64,
    hls: Arc<Mutex<HlsState>>,
    external_window_handle: Arc<std::sync::atomic::AtomicUsize>,
    x11_window_fix: Arc<Mutex<Option<X11WindowFix>>>,
    logger: LogCallback,
}

impl VideoRenderer {
    /// Initialize GStreamer and construct the HLS or mirror-mode pipelines.
    pub fn new(config: VideoRendererConfig) -> Result<Self, String> {
        Self::new_with_logger(config, Arc::new(|_, _| {}))
    }

    /// Initialize with a callback that receives source logger levels/messages.
    pub fn new_with_logger(
        config: VideoRendererConfig,
        logger: LogCallback,
    ) -> Result<Self, String> {
        if config.renderer != VideoRendererKind::Vulkan {
            return Self::build(config, logger);
        }
        if config.uri.is_some() {
            log(
                &logger,
                LogLevel::Warning,
                "video_renderer_init: Vulkan mirror renderer does not support HLS; falling back to the configured video sink",
            );
            let mut fallback = config;
            fallback.renderer = VideoRendererKind::Auto;
            return Self::build(fallback, logger);
        }

        match Self::build(config.clone(), Arc::clone(&logger)) {
            Ok(renderer) => Ok(renderer),
            Err(error) => {
                log(
                    &logger,
                    LogLevel::Warning,
                    &format!(
                        "video_renderer_init: Vulkan initialization failed ({error}); falling back to the configured video sink"
                    ),
                );
                let mut fallback = config;
                fallback.renderer = VideoRendererKind::Auto;
                Self::build(fallback, logger)
            }
        }
    }

    fn build(mut config: VideoRendererConfig, logger: LogCallback) -> Result<Self, String> {
        gst::init().map_err(|error| format!("GStreamer initialization failed: {error}"))?;
        apply_renderer_mode(&mut config);
        let missing_d3d11 = d3d11_factories_missing(&config);
        if !missing_d3d11.is_empty() {
            log(
                &logger,
                LogLevel::Warning,
                &format!(
                    "video_renderer_init: D3D11 factories {} are unavailable; falling back to automatic selection for those components",
                    missing_d3d11.join(", ")
                ),
            );
            fallback_unavailable_d3d11_components(&mut config, &missing_d3d11);
        }
        if glib::application_name().as_deref() != Some(config.server_name.as_str()) {
            glib::set_application_name(&config.server_name);
        }
        let hls_video = config.uri.is_some();
        if config.renderer == VideoRendererKind::Vulkan && config.coverart_support {
            log(
                &logger,
                LogLevel::Warning,
                "video_renderer_init: Vulkan is used for H.264/H.265 mirror video only; JPEG cover art keeps the configured video sink",
            );
        }
        let hls_state = HlsState::default();
        let mut renderers = Vec::with_capacity(3);
        let mut type_h264 = None;
        let mut type_h265 = None;
        let mut type_hls = None;
        let mut type_jpeg = None;
        let mut sync = false;
        let external_window_handle = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let auto_sink = config.video_sink.contains("autovideosink")
            || config.video_sink.contains("fpsdisplaysink");
        let x11_candidate = auto_sink
            || config.video_sink.contains("ximagesink")
            || config.video_sink.contains("xvimagesink");
        let x11_window_fix = Arc::new(Mutex::new(if x11_candidate {
            let version = gst::version();
            X11WindowFix::open(
                &config.server_name,
                config.initial_fullscreen,
                (version.0, version.1) < (1, 20),
            )
        } else {
            None
        }));

        let clock = gst::SystemClock::obtain();
        clock.set_property("clock-type", gst::ClockType::Realtime);
        let codecs = if hls_video {
            type_hls = Some(0);
            vec![Codec::Hls]
        } else {
            type_h264 = Some(0);
            let mut codecs = vec![Codec::H264];
            if config.h265_support {
                type_h265 = Some(codecs.len());
                codecs.push(Codec::H265);
            }
            if config.coverart_support {
                type_jpeg = Some(codecs.len());
                codecs.push(Codec::Jpeg);
            }
            codecs
        };

        for (id, codec) in codecs.into_iter().enumerate() {
            let uri = if codec == Codec::Hls {
                config.uri.clone()
            } else {
                None
            };
            let (pipeline, appsrc, textsrc) = if codec == Codec::Hls {
                if !matches!(config.playbin_version, 2 | 3) {
                    return Err(format!(
                        "video_renderer_init: invalid playbin version {}",
                        config.playbin_version
                    ));
                }
                let (factory, name) = if config.playbin_version == 2 {
                    ("playbin", "hls-playbin2")
                } else {
                    ("playbin3", "hls-playbin3")
                };
                let pipeline = gst::ElementFactory::make(factory)
                    .name(name)
                    .build()
                    .map_err(|error| format!("could not create {factory}: {error}"))?
                    .downcast::<gst::Pipeline>()
                    .map_err(|_| format!("{factory} did not produce a GstPipeline"))?;
                if !auto_sink {
                    if let Some(video_sink) =
                        make_video_sink(&config.video_sink, &config.video_sink_options)?
                    {
                        pipeline.set_property("video-sink", video_sink);
                    } else {
                        log(
                            &logger,
                            LogLevel::Error,
                            "video_renderer_init: failed to create playbin_videosink",
                        );
                    }
                }
                let flags = pipeline.property_value("flags");
                let mut flag_nicks = glib::FlagsValue::from_value(&flags)
                    .map(|(_, values)| {
                        values
                            .into_iter()
                            .map(|value| value.nick())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if PLAY_FLAG_DOWNLOAD != 0 && !flag_nicks.contains(&"download") {
                    flag_nicks.push("download");
                }
                if PLAY_FLAG_BUFFERING != 0 && !flag_nicks.contains(&"buffering") {
                    flag_nicks.push("buffering");
                }
                if !flag_nicks.is_empty() {
                    pipeline.set_property_from_str("flags", &flag_nicks.join("+"));
                }
                pipeline.set_property("volume", hls_state.volume);
                (pipeline, None, None)
            } else {
                let launch = pipeline_description(&config, codec);
                log(
                    &logger,
                    LogLevel::Debug,
                    &format!("GStreamer video pipeline {}:\n\"{}\"", id + 1, launch),
                );
                let element = gst::parse::launch(&launch).map_err(|error| {
                    format!("GStreamer gst_parse_launch failed to create video pipeline {}: {error}; launch=[{launch}]", id + 1)
                })?;
                let pipeline = element.downcast::<gst::Pipeline>().map_err(|_| {
                    format!("video pipeline {} did not produce a GstPipeline", id + 1)
                })?;
                pipeline.use_clock(Some(&clock));
                let appsrc = pipeline
                    .by_name("video_source")
                    .ok_or_else(|| format!("video pipeline {} has no video_source", id + 1))?
                    .downcast::<gst_app::AppSrc>()
                    .map_err(|_| format!("video pipeline {} video_source is not appsrc", id + 1))?;
                let caps = gst::Caps::from_str(codec.caps().expect("mirror codec has caps"))
                    .map_err(|error| format!("invalid {} caps: {error}", codec.name()))?;
                appsrc.set_caps(Some(&caps));
                appsrc.set_stream_type(gst_app::AppStreamType::Stream);
                appsrc.set_property("is-live", true);
                appsrc.set_property("format", gst::Format::Time);
                let textsrc = if codec == Codec::Jpeg {
                    let overlay = pipeline.by_name("metadata_overlay").ok_or_else(|| {
                        "JPEG pipeline has no metadata_overlay element".to_owned()
                    })?;
                    overlay.set_property("text", "");
                    overlay.set_property("shaded-background", true);
                    overlay.set_property("font-desc", "Sans, 16");
                    Some(overlay)
                } else {
                    None
                };
                sync = config.video_sync && codec != Codec::Jpeg;
                (pipeline, Some(appsrc), textsrc)
            };

            let bus = pipeline
                .bus()
                .ok_or_else(|| format!("video renderer {} has no GStreamer bus", id + 1))?;
            let eos = Arc::new(Mutex::new(false));
            let discovered_sink = Arc::new(Mutex::new(None));
            install_window_handle_handler(&bus, Arc::clone(&external_window_handle));
            pipeline.set_state(gst::State::Ready).map_err(|error| {
                format!(
                    "Failed to initialize GStreamer video renderer {}: {error}",
                    id + 1
                )
            })?;
            let (change, state, _) = pipeline.state(gst::ClockTime::from_mseconds(100));
            if change != Ok(gst::StateChangeSuccess::Success) || state != gst::State::Ready {
                return Err(format!(
                    "Failed to initialize GStreamer video renderer {}: reached {state:?}",
                    id + 1
                ));
            }
            if codec == Codec::Hls {
                log(
                    &logger,
                    LogLevel::Info,
                    &format!(
                        "Will use GStreamer playbin version {} to play HLS streamed video",
                        config.playbin_version
                    ),
                );
            }
            renderers.push(Some(PipelineEntry {
                codec,
                pipeline,
                appsrc,
                textsrc,
                bus,
                eos,
                watch: None,
                watch_id: None,
                discovered_sink,
                uri,
            }));
        }

        let active = type_hls;
        Ok(Self {
            config,
            renderers,
            type_h264,
            type_h265,
            type_hls,
            type_jpeg,
            active,
            sync,
            first_packet: false,
            base_time_ns: u64::MAX,
            hls: Arc::new(Mutex::new(hls_state)),
            external_window_handle,
            x11_window_fix,
            logger,
        })
    }

    /// Exact source pipeline grammar for an H.264, H.265, or JPEG renderer.
    pub fn pipeline_description(&self, codec: &str) -> Option<String> {
        let codec = match codec {
            "h264" => Codec::H264,
            "h265" => Codec::H265,
            "jpeg" => Codec::Jpeg,
            _ => return None,
        };
        Some(pipeline_description(&self.config, codec))
    }

    /// Select the requested decoder pipeline. Returns -1 on unsupported or
    /// conflicting selection, matching `video_renderer_choose_codec_and_size`.
    pub fn choose_codec(&mut self, jpeg: bool, h265: bool) -> i32 {
        if self.type_hls.is_some() {
            return -1;
        }
        let selected = if jpeg {
            let Some(id) = self.type_jpeg else {
                return -1;
            };
            id
        } else if h265 {
            let Some(id) = self.type_h265 else {
                log(
                    &self.logger,
                    LogLevel::Error,
                    "video is h265 but the -h265 option was not used",
                );
                return -1;
            };
            id
        } else if let Some(id) = self.type_h264 {
            id
        } else {
            return -1;
        };
        if self.active == Some(selected) {
            return 0;
        }
        if self.active.is_some() {
            return -1;
        }
        let Some(entry) = self.renderers.get(selected).and_then(Option::as_ref) else {
            return -1;
        };
        let _ = entry.pipeline.set_state(gst::State::Paused);
        let _ = entry.pipeline.state(gst::ClockTime::from_seconds(1));
        if entry.pipeline.set_state(gst::State::Playing).is_err() {
            return -1;
        }
        if entry.pipeline.state(gst::ClockTime::from_mseconds(100)).0 == Err(gst::StateChangeError)
        {
            return -1;
        }
        if let Some(appsrc) = entry.appsrc.as_ref() {
            self.base_time_ns = appsrc.base_time().map_or(u64::MAX, |time| time.nseconds());
        }
        self.active = Some(selected);
        for index in 0..self.renderers.len() {
            if index != selected {
                self.destroy_instance(index);
            }
        }
        0
    }

    /// Return whether the source's selected renderer is paused.
    pub fn is_paused(&self) -> bool {
        self.active_entry()
            .map(|entry| entry.pipeline.current_state())
            == Some(gst::State::Paused)
    }

    /// Mirror the source's `video_renderer_start` state transition.
    pub fn start(&mut self) {
        if let Some(id) = self.type_hls {
            if let Some(entry) = self.renderers.get(id).and_then(Option::as_ref) {
                if let Some(uri) = entry.uri.as_deref() {
                    entry.pipeline.set_property("uri", uri);
                }
                let _ = entry.pipeline.set_state(gst::State::Paused);
                let _ = entry.pipeline.state(gst::ClockTime::from_seconds(1));
            }
            return;
        }
        self.active = None;
        self.first_packet = true;
    }

    pub fn pause(&self) {
        if let Some(entry) = self.active_entry() {
            if let Err(error) = entry.pipeline.set_state(gst::State::Paused) {
                log(
                    &self.logger,
                    LogLevel::Debug,
                    &format!("video renderer pause: {error}"),
                );
            }
        }
    }

    pub fn resume(&mut self) {
        let Some(entry) = self.active_entry() else {
            return;
        };
        let _ = entry.pipeline.set_state(gst::State::Playing);
        let (_, state, _) = entry.pipeline.state(gst::ClockTime::from_mseconds(100));
        log(
            &self.logger,
            LogLevel::Debug,
            &format!("video renderer resumed: state {state:?}"),
        );
        if let Some(appsrc) = entry.appsrc.as_ref() {
            self.base_time_ns = appsrc.base_time().map_or(u64::MAX, |time| time.nseconds());
        }
    }

    pub fn render_buffer(&mut self, data: &[u8], ntp_time_ns: u64) -> (u64, RenderOutcome) {
        let mut pts = ntp_time_ns;
        if self.sync {
            if pts >= self.base_time_ns {
                pts -= self.base_time_ns;
            } else {
                log(
                    &self.logger,
                    LogLevel::Debug,
                    &format!(
                        "*** invalid ntp_time < gst_video_pipeline_base_time\n{:8.6} ntp_time\n{:8.6} base_time",
                        ntp_time_ns as f64 / SECOND_IN_NSECS as f64,
                        self.base_time_ns as f64 / SECOND_IN_NSECS as f64
                    ),
                );
                return (
                    self.base_time_ns - pts,
                    RenderOutcome::InvalidTimestamp(self.base_time_ns - pts),
                );
            }
        }
        let Some(first) = data.first() else {
            return (0, RenderOutcome::EmptyPacket);
        };
        if *first != 0 {
            log(
                &self.logger,
                LogLevel::Error,
                "*** ERROR decryption of video packet failed ",
            );
            return (0, RenderOutcome::InvalidFrame);
        }
        if self.first_packet {
            log(
                &self.logger,
                LogLevel::Info,
                "Begin streaming to GStreamer video pipeline",
            );
            self.first_packet = false;
        }
        let Some(appsrc) = self.active_entry().and_then(|entry| entry.appsrc.as_ref()) else {
            log(&self.logger, LogLevel::Debug, "*** no video renderer found");
            return (0, RenderOutcome::NoActiveRenderer);
        };
        let mut buffer = match gst::Buffer::with_size(data.len()) {
            Ok(buffer) => buffer,
            Err(_) => return (0, RenderOutcome::EmptyPacket),
        };
        if let Some(buffer_ref) = buffer.get_mut() {
            if self.sync {
                buffer_ref.set_pts(Some(gst::ClockTime::from_nseconds(pts)));
            }
            if let Ok(mut map) = buffer_ref.map_writable() {
                map.as_mut_slice().copy_from_slice(data);
            }
        }
        let _ = appsrc.push_buffer(buffer);
        let use_x11 = self.active_entry().is_none_or(|entry| {
            entry
                .discovered_sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .is_none_or(element_uses_x11)
        });
        if let Some(fix) = self
            .x11_window_fix
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            match fix.search_after_frame(use_x11) {
                SearchResult::Waiting(attempt) => log(
                    &self.logger,
                    LogLevel::Debug,
                    &format!("Looking for X11 UxPlay Window, attempt {attempt}"),
                ),
                SearchResult::Found(_) => log(
                    &self.logger,
                    LogLevel::Info,
                    "\n*** X11 Windows: Use key F11 or (left Alt)+Enter to toggle full-screen mode\n",
                ),
                SearchResult::NotSearching => {}
            }
        }
        (0, RenderOutcome::Submitted)
    }

    pub fn display_jpeg(&self, data: &[u8]) {
        if self.type_jpeg.is_none() {
            return;
        }
        if let Some(entry) = self
            .active_entry()
            .filter(|entry| entry.codec == Codec::Jpeg)
        {
            if let (Some(appsrc), Ok(mut buffer)) =
                (entry.appsrc.as_ref(), gst::Buffer::with_size(data.len()))
            {
                if let Some(buffer_ref) = buffer.get_mut() {
                    if let Ok(mut map) = buffer_ref.map_writable() {
                        map.as_mut_slice().copy_from_slice(data);
                    }
                }
                let _ = appsrc.push_buffer(buffer);
            }
        }
    }

    pub fn set_track_metadata(
        &self,
        title: Option<&[u8]>,
        artist: Option<&[u8]>,
        _album: Option<&[u8]>,
    ) {
        let artist = artist.map(String::from_utf8_lossy);
        let title = title.map(String::from_utf8_lossy);
        let mut metadata = String::new();
        if let Some(artist) = artist.as_ref() {
            metadata.push_str(artist);
        }
        if artist.is_some() && title.is_some() {
            metadata.push_str(": ");
        }
        if let Some(title) = title.as_ref() {
            metadata.push('"');
            metadata.push_str(title);
            metadata.push('"');
        }
        let metadata = metadata.replace('&', "&amp;");
        if let Some(textsrc) = self.active_entry().and_then(|entry| entry.textsrc.as_ref()) {
            if artist.is_some() || title.is_some() {
                textsrc.set_property("text", metadata);
            }
        }
    }

    pub fn hls_ready(&self) {
        let Some(entry) = self
            .active_entry()
            .filter(|entry| entry.codec == Codec::Hls)
        else {
            return;
        };
        let _ = entry.pipeline.set_state(gst::State::Ready);
        let _ = entry.pipeline.state(gst::ClockTime::from_seconds(1));
    }

    pub fn stop(&self) {
        if let Some(entry) = self.active_entry() {
            if let Some(appsrc) = entry.appsrc.as_ref() {
                let _ = appsrc.end_of_stream();
            }
            let _ = entry.pipeline.set_state(gst::State::Null);
        }
    }

    pub fn flush(&self) {}

    pub fn set_start(&self, position_seconds: f32) {
        let mut hls = self
            .hls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        hls.requested_start_ns = (f64::from(position_seconds) * SECOND_IN_NSECS as f64) as i64;
    }

    pub fn seek(&self, position_seconds: f32) {
        let Some(entry) = self.active_entry() else {
            return;
        };
        let hls = self
            .hls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if hls.duration_ns < 2_000 {
            return;
        }
        let mut seek_ns = (f64::from(position_seconds) * SECOND_IN_NSECS as f64) as i64;
        seek_ns = seek_ns.max(1_000).min(hls.duration_ns - 1_000);
        println!(
            "SCRUB: seek to {position_seconds} secs = {seek_ns} ns, duration = {} ns",
            hls.duration_ns
        );
        let result = entry.pipeline.seek_simple(
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::ClockTime::from_nseconds(seek_ns as u64),
        );
        if result.is_ok() {
            println!("seek succeeded");
            let _ = entry.pipeline.set_state(gst::State::Playing);
        } else {
            println!("seek failed");
        }
    }

    pub fn set_hls_volume(&self, volume: f64) {
        let Some(entry) = self
            .active_entry()
            .filter(|entry| entry.codec == Codec::Hls)
        else {
            return;
        };
        let volume = volume.clamp(0.0, 10.0);
        self.hls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .volume = volume;
        entry.pipeline.set_property("volume", volume);
    }

    pub fn playback_info(&self) -> PlaybackInfo {
        let Some(entry) = self.active_entry() else {
            return PlaybackInfo::default();
        };
        let mut info = PlaybackInfo::default();
        let mut hls = self
            .hls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if hls.seek_enabled {
            info.seek_start = hls.seek_start_ns as f64 / SECOND_IN_NSECS as f64;
            info.seek_duration =
                (hls.seek_end_ns - hls.seek_start_ns) as f64 / SECOND_IN_NSECS as f64;
        }
        info.buffer_empty = hls.buffer_empty;
        info.buffer_full = hls.buffer_full;
        if entry.pipeline.current_state() == gst::State::Playing {
            info.rate = 1.0;
        }
        if hls.duration_ns < 0 {
            if let Some(duration) = entry.pipeline.query_duration::<gst::ClockTime>() {
                hls.duration_ns = duration.nseconds() as i64;
            } else {
                return info;
            }
        }
        info.duration = hls.duration_ns as f64 / SECOND_IN_NSECS as f64;
        if info.duration != 0.0 {
            info.position = entry
                .pipeline
                .query_position::<gst::ClockTime>()
                .map_or(-1.0, |position| position.seconds() as f64);
        }
        info
    }

    pub fn eos_watch(&self) -> bool {
        let Some(entry) = self
            .active_entry()
            .filter(|entry| entry.codec == Codec::Hls)
        else {
            return false;
        };
        let mut eos = entry
            .eos
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *eos)
    }

    pub fn take_hls_eos_uri(&self) -> Option<String> {
        let entry = self
            .active_entry()
            .filter(|entry| entry.codec == Codec::Hls)?;
        take_eos_uri_once(&entry.eos, entry.uri.as_deref())
    }

    pub fn waiting_for_x11_window(&self) -> bool {
        let Some(hls_id) = self.type_hls else {
            return false;
        };
        let use_x11 = self
            .renderers
            .get(hls_id)
            .and_then(Option::as_ref)
            .and_then(|entry| {
                entry
                    .discovered_sink
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                    .map(element_uses_x11)
            })
            .unwrap_or(true);
        self.x11_window_fix
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
            .is_some_and(|fix| fix.waiting_for_window(use_x11))
    }

    pub fn set_window_handle(&self, handle: usize) {
        self.external_window_handle
            .store(handle, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn listen(&mut self, id: usize, main_loop: &MainLoop) -> Result<u32, String> {
        let Some(entry) = self.renderers.get_mut(id).and_then(Option::as_mut) else {
            return Err(format!("video renderer id out of range: {id}"));
        };
        if let Some(source_id) = entry.watch_id {
            return Ok(source_id);
        }
        let bus = entry.bus.clone();
        let pipeline = entry.pipeline.clone();
        let appsrc = entry.appsrc.clone();
        let codec = entry.codec;
        let eos = Arc::clone(&entry.eos);
        let hls = Arc::clone(&self.hls);
        let discovered_sink = Arc::clone(&entry.discovered_sink);
        let x11_window_fix = Arc::clone(&self.x11_window_fix);
        let logger = Arc::clone(&self.logger);
        let loop_ = main_loop.clone();
        let source = bus.create_watch(None, glib::Priority::DEFAULT, move |bus, message| {
            handle_bus_message(
                bus,
                message,
                &pipeline,
                appsrc.as_ref(),
                codec,
                &eos,
                &hls,
                &discovered_sink,
                &x11_window_fix,
                &loop_,
                &logger,
            );
            ControlFlow::Continue
        });
        let source_id = source.attach(Some(&main_loop.context())).as_raw();
        entry.watch = Some(source);
        entry.watch_id = Some(source_id);
        Ok(source_id)
    }

    pub fn cycle(&mut self) -> i32 {
        let Some(id) = self.active else {
            return -1;
        };
        let Some(entry) = self.renderers.get(id).and_then(Option::as_ref) else {
            return -1;
        };
        if entry.codec != Codec::Jpeg {
            return -1;
        }
        for (state, first) in [(gst::State::Null, true), (gst::State::Playing, false)] {
            if first {
                self.stop();
            } else if entry.pipeline.set_state(state).is_err() {
                return -1;
            }
            let mut failures = 0;
            loop {
                let (change, current, _) = entry.pipeline.state(gst::ClockTime::from_seconds(1));
                if current == state {
                    break;
                }
                if change == Err(gst::StateChangeError) {
                    failures += 1;
                    if failures > 10 {
                        return -1;
                    }
                }
            }
        }
        0
    }

    pub fn destroy(&mut self) {
        self.stop();
        for index in 0..self.renderers.len() {
            self.destroy_instance(index);
        }
        self.active = None;
    }

    pub fn decoder_check(decoder: Option<&[u8]>) -> bool {
        if gst::init().is_err() {
            return false;
        }
        let Some(decoder) = decoder else {
            return false;
        };
        let decoder = String::from_utf8_lossy(decoder);
        gst::ElementFactory::find(decoder.as_ref()).is_some()
    }

    pub fn active_codec(&self) -> Option<&'static str> {
        self.active_entry().map(|entry| entry.codec.name())
    }

    fn active_entry(&self) -> Option<&PipelineEntry> {
        self.active
            .and_then(|id| self.renderers.get(id))
            .and_then(Option::as_ref)
    }

    fn destroy_instance(&mut self, id: usize) {
        let Some(mut entry) = self.renderers.get_mut(id).and_then(Option::take) else {
            return;
        };
        let state = entry.pipeline.current_state();
        if state != gst::State::Null {
            if self.type_hls.is_none() {
                if let Some(appsrc) = entry.appsrc.as_ref() {
                    let _ = appsrc.end_of_stream();
                }
            }
            let _ = entry.pipeline.set_state(gst::State::Null);
            let _ = entry.pipeline.state(gst::ClockTime::from_seconds(1));
        }
        if let Some(watch) = entry.watch.take() {
            watch.destroy();
        }
        entry.watch_id = None;
    }
}

impl Drop for VideoRenderer {
    fn drop(&mut self) {
        self.destroy();
    }
}

/// Convert flip/rotation enum pairs to the source's `videoflip` element text.
pub fn videoflip_description(flip: VideoFlip, rotation: VideoFlip) -> Option<&'static str> {
    use VideoFlip::{Horizontal as H, Invert as I, Left as L, Right as R, Vertical as V};
    match flip {
        I => match rotation {
            L => Some("GST_VIDEO_ORIENTATION_90R"),
            R => Some("GST_VIDEO_ORIENTATION_90L"),
            _ => Some("GST_VIDEO_ORIENTATION_180"),
        },
        H => match rotation {
            L => Some("GST_VIDEO_ORIENTATION_UL_LR"),
            R => Some("GST_VIDEO_ORIENTATION_UR_LL"),
            _ => Some("GST_VIDEO_ORIENTATION_HORIZ"),
        },
        V => match rotation {
            L => Some("GST_VIDEO_ORIENTATION_UR_LL"),
            R => Some("GST_VIDEO_ORIENTATION_UL_LR"),
            _ => Some("GST_VIDEO_ORIENTATION_VERT"),
        },
        _ => match rotation {
            L => Some("GST_VIDEO_ORIENTATION_90L"),
            R => Some("GST_VIDEO_ORIENTATION_90R"),
            _ => None,
        },
    }
}

fn pipeline_description(config: &VideoRendererConfig, codec: Codec) -> String {
    let mut config = config.clone();
    apply_renderer_mode(&mut config);
    let jpeg = codec == Codec::Jpeg;
    let rtp = !config.rtp_pipeline.is_empty();
    let mut launch = String::from("appsrc name=video_source ! ");
    if jpeg {
        launch.push_str("jpegdec ");
    } else {
        if config.low_latency && !rtp {
            launch.push_str("queue max-size-buffers=16 max-size-bytes=0 max-size-time=0 ! ");
        } else {
            launch.push_str("queue ! ");
        }
        launch.push_str(&config.parser);
        launch.push_str(" ! ");
        if !rtp {
            launch.push_str(&config.decoder);
            if config.low_latency {
                launch.push_str(
                    " ! queue max-size-buffers=2 max-size-bytes=0 max-size-time=0 leaky=downstream",
                );
            }
        } else {
            launch.push_str("rtph264pay ");
            launch.push_str(&config.rtp_pipeline);
        }
    }
    if !rtp || jpeg {
        launch.push_str(" ! ");
        if let Some(direction) = videoflip_description(config.flip[0], config.flip[1]) {
            launch.push_str("videoflip video-direction=");
            launch.push_str(direction);
            launch.push_str(" ! ");
        }
        launch.push_str(&config.converter);
        launch.push_str(" ! videoscale ! ");
        if jpeg {
            launch
                .push_str(" imagefreeze allow-replace=TRUE ! textoverlay name=metadata_overlay ! ");
        }
        if config.renderer == VideoRendererKind::Vulkan && !jpeg {
            launch.push_str(&format!(
                "vulkanupload ! vulkansink name=vulkansink_{}",
                codec.name()
            ));
        } else {
            launch.push_str(&config.video_sink);
            launch.push_str(" name=");
            launch.push_str(&config.video_sink);
            launch.push('_');
            launch.push_str(codec.name());
            launch.push_str(&config.video_sink_options);
        }
        launch.push_str(if config.video_sync && !jpeg {
            " sync=true"
        } else {
            " sync=false"
        });
    }
    match codec {
        Codec::H264 => launch.replace("h265", "h264"),
        Codec::H265 => launch.replace("h264", "h265"),
        Codec::Hls | Codec::Jpeg => launch,
    }
}

fn d3d11_factories_missing(config: &VideoRendererConfig) -> Vec<String> {
    if !config.rtp_pipeline.is_empty() {
        return Vec::new();
    }
    let mut factories = Vec::new();
    if config.decoder.contains("d3d11h264dec") || config.decoder.contains("d3d11h265dec") {
        factories.push(config.decoder.replace("h265", "h264"));
        if config.h265_support {
            factories.push(config.decoder.replace("h264", "h265"));
        }
    }
    if config.video_sink == "d3d11videosink" {
        factories.push(config.video_sink.clone());
    }
    factories
        .into_iter()
        .filter(|factory| gst::ElementFactory::find(factory).is_none())
        .collect()
}

fn fallback_unavailable_d3d11_components(
    config: &mut VideoRendererConfig,
    missing_factories: &[String],
) {
    if !missing_factories.is_empty() {
        config.renderer = VideoRendererKind::Auto;
    }
    if missing_factories
        .iter()
        .any(|factory| factory.starts_with("d3d11h"))
    {
        config.decoder = "decodebin".to_owned();
    }
    if missing_factories
        .iter()
        .any(|factory| factory == "d3d11videosink")
    {
        config.video_sink = "autovideosink".to_owned();
        config.video_sink_options.clear();
    }
}

fn apply_renderer_mode(config: &mut VideoRendererConfig) {
    if config.renderer != VideoRendererKind::D3d11 {
        return;
    }
    if matches!(config.decoder.as_str(), "decodebin" | "avdec_h264") {
        config.decoder = "d3d11h264dec".to_owned();
    }
    if config.video_sink == "autovideosink" {
        config.video_sink = "d3d11videosink".to_owned();
        config.video_sink_options.clear();
    }
}

fn make_video_sink(videosink: &str, options: &str) -> Result<Option<gst::Element>, String> {
    let sink = match gst::ElementFactory::make(videosink)
        .name("videosink")
        .build()
    {
        Ok(sink) => sink,
        Err(_) => return Ok(None),
    };
    for token in options
        .split('!')
        .next()
        .unwrap_or_default()
        .split(' ')
        .filter(|token| !token.is_empty())
    {
        if let Some((property, value)) = token.split_once('=') {
            println!("playbin_videosink property: \"{property}\" \"{value}\"");
            sink.set_property_from_str(property, value);
        }
    }
    Ok(Some(sink))
}

fn install_window_handle_handler(bus: &gst::Bus, handle: Arc<std::sync::atomic::AtomicUsize>) {
    bus.set_sync_handler(move |_bus, message| {
        if gst_video::is_video_overlay_prepare_window_handle_message(message) {
            let window_handle = handle.load(std::sync::atomic::Ordering::SeqCst);
            if window_handle != 0 {
                if let Some(source) = message
                    .src()
                    .and_then(|source| source.downcast_ref::<gst::Element>().cloned())
                {
                    crate::set_video_overlay_window_handle(&source, window_handle);
                }
            }
            gst::BusSyncReply::Drop
        } else {
            gst::BusSyncReply::Pass
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn handle_bus_message(
    bus: &gst::Bus,
    message: &gst::Message,
    pipeline: &gst::Pipeline,
    appsrc: Option<&gst_app::AppSrc>,
    codec: Codec,
    eos: &Arc<Mutex<bool>>,
    hls: &Arc<Mutex<HlsState>>,
    discovered_sink: &Arc<Mutex<Option<gst::Element>>>,
    x11_window_fix: &Arc<Mutex<Option<X11WindowFix>>>,
    main_loop: &MainLoop,
    logger: &LogCallback,
) {
    let is_hls = codec == Codec::Hls;
    if is_hls {
        let position = pipeline.query_position::<gst::ClockTime>();
        let duration = pipeline.query_duration::<gst::ClockTime>();
        {
            let mut state = hls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(position) = position {
                if state.requested_start_ns != 0
                    && position.nseconds() as i64 >= state.requested_start_ns
                {
                    state.requested_start_ns = 0;
                }
            }
            if state.duration_ns < 0 {
                if let Some(duration) = duration {
                    state.duration_ns = duration.nseconds() as i64;
                }
            }
        }
        hls_video_seek_to_start_position(pipeline, hls);
    }
    match message.view() {
        gst::MessageView::Element(_) => {
            if let Ok(gst_video::NavigationMessage::Event(event_message)) =
                gst_video::NavigationMessage::parse(message)
                && let Ok(event) = gst_video::NavigationEvent::parse(&event_message.event)
            {
                let key_event = match event {
                    gst_video::NavigationEvent::KeyPress { key, .. } => Some((key, true)),
                    gst_video::NavigationEvent::KeyRelease { key, .. } => Some((key, false)),
                    _ => None,
                };
                if let Some((key, pressed)) = key_event
                    && let Some(fix) = x11_window_fix
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .as_mut()
                {
                    fix.handle_key(&key, pressed);
                }
            }
        }
        gst::MessageView::DurationChanged(..) => {
            hls.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .duration_ns = -1;
        }
        gst::MessageView::Buffering(buffering) if is_hls => {
            let percent = buffering.percent();
            let mut state = hls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.buffer_empty = true;
            state.buffer_full = false;
            if percent > 0 {
                state.buffer_empty = false;
                if percent < 100 {
                    let _ = pipeline.set_state(gst::State::Paused);
                } else {
                    state.buffer_full = true;
                    let _ = pipeline.set_state(gst::State::Playing);
                }
            }
        }
        gst::MessageView::Error(error) => {
            let source = message
                .src()
                .map_or_else(String::new, |source| source.name().to_string());
            let message_text = error.error().to_string();
            log(
                logger,
                LogLevel::Info,
                &format!("GStreamer error (video): {source} {message_text}"),
            );
            if let Some(appsrc) = appsrc {
                let _ = appsrc.end_of_stream();
            }
            if !is_hls || message_text.contains("Output window was closed") {
                bus.set_flushing(true);
                let _ = pipeline.set_state(gst::State::Ready);
                main_loop.quit();
            }
        }
        gst::MessageView::Eos(..) => {
            log(logger, LogLevel::Info, "GStreamer: End-Of-Stream (video)");
            if is_hls {
                bus.set_flushing(true);
                let _ = pipeline.set_state(gst::State::Ready);
                *eos.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            }
        }
        gst::MessageView::StateChanged(state_changed) => {
            if is_hls {
                if message
                    .src()
                    .is_some_and(|source| source.name().contains("hls-playbin"))
                {
                    let new_state = state_changed.current();
                    if new_state != gst::State::Playing {
                        return;
                    }
                    {
                        let mut hls = hls
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let mut query = gst::query::Seeking::new(gst::Format::Time);
                        if pipeline.query(&mut query) {
                            let (seekable, start, end) = query.result();
                            hls.seek_enabled = seekable;
                            hls.seek_start_ns = start.value();
                            hls.seek_end_ns = end.value();
                            if seekable {
                                println!(
                                    "Seeking is ENABLED from {} ns to {} ns",
                                    hls.seek_start_ns, hls.seek_end_ns
                                );
                            } else {
                                println!("Seeking is DISABLED for this stream.");
                            }
                        } else {
                            eprintln!("Seeking query failed.");
                        }
                    }
                    hls_video_seek_to_start_position(pipeline, hls);
                }
            }
            let src_name = message
                .src()
                .map_or_else(String::new, |source| source.name().to_string());
            let is_video_sink = src_name.contains("sink") && !src_name.contains("audio");
            if (src_name.contains(codec.name()) || (is_hls && is_video_sink))
                && src_name.contains("sink")
            {
                let new_state = state_changed.current();
                let sink_not_seen = discovered_sink
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_none();
                if new_state >= gst::State::Ready && sink_not_seen {
                    if let Some(source) = message
                        .src()
                        .and_then(|source| source.downcast_ref::<gst::Element>().cloned())
                    {
                        *discovered_sink
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(source);
                    }
                }
            }
        }
        _ => {}
    }
}

fn hls_video_seek_to_start_position(pipeline: &gst::Pipeline, hls: &Arc<Mutex<HlsState>>) {
    let requested = {
        let state = hls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.requested_start_ns != 0
            && state.seek_enabled
            && state.requested_start_ns >= state.seek_start_ns
            && state.requested_start_ns <= state.seek_end_ns
        {
            Some(state.requested_start_ns)
        } else {
            None
        }
    };
    if let Some(requested) = requested {
        println!("***************** seek to hls_requested_start_position {requested} ns");
        if pipeline
            .seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::ClockTime::from_nseconds(requested as u64),
            )
            .is_ok()
        {
            hls.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .requested_start_ns = 0;
        } else {
            println!("*** seek to requested_start_position failed");
        }
    }
}

fn log(logger: &LogCallback, level: LogLevel, message: &str) {
    logger(level, message);
}

fn element_uses_x11(sink: &gst::Element) -> bool {
    sink.factory()
        .map(|factory| factory.name().to_string())
        .is_none_or(|name| sink_factory_uses_x11(&name))
}

fn sink_factory_uses_x11(name: &str) -> bool {
    name.contains("ximagesink")
        || name.contains("xvimagesink")
        || name.contains("autovideosink")
        || name.contains("fpsdisplaysink")
}

static GLOBAL_RENDERER: OnceLock<Mutex<Option<VideoRenderer>>> = OnceLock::new();
static GLOBAL_MAIN_LOOP: OnceLock<Mutex<Option<MainLoop>>> = OnceLock::new();

fn global_renderer() -> &'static Mutex<Option<VideoRenderer>> {
    GLOBAL_RENDERER.get_or_init(|| Mutex::new(None))
}

fn global_main_loop() -> &'static Mutex<Option<MainLoop>> {
    GLOBAL_MAIN_LOOP.get_or_init(|| Mutex::new(None))
}

fn replace_renderer<T>(slot: &mut Option<T>, replacement: Result<T, String>) -> Result<(), String> {
    *slot = Some(replacement?);
    Ok(())
}

fn take_eos_uri_once(eos: &Mutex<bool>, uri: Option<&str>) -> Option<String> {
    let mut eos = eos
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if std::mem::take(&mut *eos) {
        uri.map(str::to_owned)
    } else {
        None
    }
}

/// Initialize the bridge's process-global renderer.
pub fn video_renderer_init(config: VideoRendererConfig, logger: LogCallback) -> Result<(), String> {
    let renderer = VideoRenderer::new_with_logger(config, logger)?;
    renderer.set_window_handle(global_window_handle().load(std::sync::atomic::Ordering::SeqCst));
    let mut global = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *global = Some(renderer);
    Ok(())
}

pub fn video_renderer_pause() {
    if let Some(renderer) = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        renderer.pause();
    }
}

pub fn video_renderer_resume() {
    if let Some(renderer) = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
    {
        renderer.resume();
    }
}

pub fn video_renderer_start() {
    if let Some(renderer) = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
    {
        renderer.start();
    }
}

pub fn waiting_for_x11_window() -> bool {
    global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(VideoRenderer::waiting_for_x11_window)
}

pub fn video_renderer_cycle() -> i32 {
    global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
        .map_or(-1, VideoRenderer::cycle)
}

pub fn video_renderer_flush() {
    if let Some(renderer) = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        renderer.flush();
    }
}

pub fn video_renderer_hls_ready() {
    if let Some(renderer) = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        renderer.hls_ready();
    }
}

pub fn video_renderer_stop() {
    if let Some(renderer) = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        renderer.stop();
    }
}

pub fn video_renderer_set_track_metadata(
    title: Option<&[u8]>,
    artist: Option<&[u8]>,
    album: Option<&[u8]>,
) {
    if let Some(renderer) = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        renderer.set_track_metadata(title, artist, album);
    }
}

/// Device frames are unsupported by the original GStreamer renderer.
pub fn video_renderer_set_device_model(_model: Option<&[u8]>, _name: Option<&[u8]>) {}

pub fn video_renderer_render_buffer(data: &[u8], ntp_time_ns: u64) -> u64 {
    global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
        .map_or(0, |renderer| renderer.render_buffer(data, ntp_time_ns).0)
}

pub fn video_renderer_display_jpeg(data: &[u8]) {
    if let Some(renderer) = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        renderer.display_jpeg(data);
    }
}

pub fn video_renderer_choose_codec_and_size(is_jpeg: bool, is_h265: bool) -> i32 {
    global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
        .map_or(-1, |renderer| renderer.choose_codec(is_jpeg, is_h265))
}

pub fn video_get_playback_info() -> PlaybackInfo {
    global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .map_or_else(PlaybackInfo::default, VideoRenderer::playback_info)
}

pub fn video_renderer_has_active_renderer() -> bool {
    global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|renderer| renderer.active.is_some())
}

pub fn video_renderer_listen(loop_: &MainLoop, id: usize) -> Result<u32, String> {
    *global_main_loop()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(loop_.clone());
    global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
        .ok_or_else(|| "video renderer has not been initialized".to_owned())?
        .listen(id, loop_)
}

/// Rebuild the active renderer as a playbin pipeline and start the HLS URI.
pub fn video_renderer_play_hls(uri: &str, start_position_seconds: f32) -> Result<(), String> {
    let mut global = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let current_uri = global
        .as_ref()
        .ok_or_else(|| "video renderer has not been initialized".to_owned())?
        .config
        .uri
        .as_deref();
    if current_uri == Some(uri) {
        let current = global.as_mut().expect("renderer was just checked");
        current.set_start(start_position_seconds);
        current.start();
        return Ok(());
    }
    let current = global.as_ref().expect("renderer was just checked");
    let mut config = current.config.clone();
    config.uri = Some(uri.to_owned());
    let logger = Arc::clone(&current.logger);
    let handle = current
        .external_window_handle
        .load(std::sync::atomic::Ordering::SeqCst);
    let main_loop = global_main_loop()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let renderer = (|| {
        let mut renderer = VideoRenderer::new_with_logger(config, logger)?;
        renderer.set_window_handle(handle);
        renderer.set_start(start_position_seconds);
        if let Some(main_loop) = main_loop.as_ref() {
            renderer.listen(0, main_loop)?;
        }
        Ok(renderer)
    })();
    replace_renderer(&mut global, renderer)?;
    if let Some(renderer) = global.as_mut() {
        renderer.start();
    }
    Ok(())
}

/// Restore the mirror-mode codec pipelines after an HLS renderer reset.
pub fn video_renderer_restore_mirror() -> Result<(), String> {
    let mut global = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let current = global
        .as_ref()
        .ok_or_else(|| "video renderer has not been initialized".to_owned())?;
    let mut config = current.config.clone();
    config.uri = None;
    let logger = Arc::clone(&current.logger);
    let handle = current
        .external_window_handle
        .load(std::sync::atomic::Ordering::SeqCst);
    let main_loop = global_main_loop()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let renderer = (|| {
        let mut renderer = VideoRenderer::new_with_logger(config, logger)?;
        renderer.set_window_handle(handle);
        if let Some(main_loop) = main_loop.as_ref() {
            for id in 0..renderer.renderers.len() {
                renderer.listen(id, main_loop)?;
            }
        }
        Ok(renderer)
    })();
    replace_renderer(&mut global, renderer)?;
    if let Some(renderer) = global.as_mut() {
        renderer.start();
    }
    Ok(())
}

/// Rebuild and restart the currently configured renderer after a client reset.
pub fn video_renderer_reinitialize() -> Result<(), String> {
    let mut global = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let current = global
        .as_ref()
        .ok_or_else(|| "video renderer has not been initialized".to_owned())?;
    let config = current.config.clone();
    let logger = Arc::clone(&current.logger);
    let handle = current
        .external_window_handle
        .load(std::sync::atomic::Ordering::SeqCst);
    let main_loop = global_main_loop()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let renderer = (|| {
        let mut renderer = VideoRenderer::new_with_logger(config, logger)?;
        renderer.set_window_handle(handle);
        if let Some(main_loop) = main_loop.as_ref() {
            for id in 0..renderer.renderers.len() {
                renderer.listen(id, main_loop)?;
            }
        }
        Ok(renderer)
    })();
    replace_renderer(&mut global, renderer)?;
    if let Some(renderer) = global.as_mut() {
        renderer.start();
    }
    Ok(())
}

pub fn video_renderer_set_window_handle(handle: usize) {
    global_window_handle().store(handle, std::sync::atomic::Ordering::SeqCst);
    if let Some(renderer) = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        renderer.set_window_handle(handle);
    }
}

static GLOBAL_WINDOW_HANDLE: OnceLock<std::sync::atomic::AtomicUsize> = OnceLock::new();

fn global_window_handle() -> &'static std::sync::atomic::AtomicUsize {
    GLOBAL_WINDOW_HANDLE.get_or_init(|| std::sync::atomic::AtomicUsize::new(0))
}

pub fn video_renderer_is_paused() -> bool {
    global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(VideoRenderer::is_paused)
}

pub fn video_renderer_destroy() {
    let mut global = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(mut renderer) = global.take() {
        renderer.destroy();
    }
}

pub fn video_renderer_set_start(position: f32) {
    if let Some(renderer) = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        renderer.set_start(position);
    }
}

pub fn video_renderer_seek(position: f32) {
    if let Some(renderer) = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        renderer.seek(position);
    }
}

pub fn video_renderer_eos_watch() -> bool {
    global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(VideoRenderer::eos_watch)
}

/// Atomically consume the active HLS EOS flag and return the URI that ended.
pub fn video_renderer_take_hls_eos_uri() -> Option<String> {
    global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .and_then(VideoRenderer::take_hls_eos_uri)
}

pub fn video_renderer_hls_set_volume(volume: f64) {
    if let Some(renderer) = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        renderer.set_hls_volume(volume);
    }
}

pub fn gstreamer_decoder_check(decoder: Option<&[u8]>) -> bool {
    VideoRenderer::decoder_check(decoder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_and_explicit_x11_sinks_keep_window_lookup_enabled() {
        for name in [
            "ximagesink",
            "xvimagesink",
            "autovideosink",
            "fpsdisplaysink",
        ] {
            assert!(sink_factory_uses_x11(name), "{name}");
        }
        assert!(!sink_factory_uses_x11("waylandsink"));
        assert!(!sink_factory_uses_x11("d3d11videosink"));
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

    struct DropMarker(Arc<std::sync::atomic::AtomicBool>);

    impl Drop for DropMarker {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::Release);
        }
    }

    fn config() -> VideoRendererConfig {
        VideoRendererConfig {
            video_sink: "fakesink".to_owned(),
            ..VideoRendererConfig::default()
        }
    }

    #[test]
    fn failed_renderer_rebuild_keeps_the_previous_renderer_installed() {
        let previous_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut active = Some(DropMarker(Arc::clone(&previous_dropped)));

        assert!(replace_renderer(&mut active, Err("pipeline creation failed".to_owned())).is_err());
        assert!(!previous_dropped.load(std::sync::atomic::Ordering::Acquire));
        assert!(active.is_some());

        let replacement_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        replace_renderer(
            &mut active,
            Ok(DropMarker(Arc::clone(&replacement_dropped))),
        )
        .unwrap();
        assert!(previous_dropped.load(std::sync::atomic::Ordering::Acquire));
        assert!(!replacement_dropped.load(std::sync::atomic::Ordering::Acquire));
        drop(active);
        assert!(replacement_dropped.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn taking_hls_eos_returns_the_ended_uri_once() {
        let eos = Mutex::new(true);
        assert_eq!(
            take_eos_uri_once(&eos, Some("https://origin.example/ended.m3u8")),
            Some("https://origin.example/ended.m3u8".to_owned())
        );
        assert_eq!(
            take_eos_uri_once(&eos, Some("https://origin.example/ended.m3u8")),
            None
        );
    }

    #[test]
    fn standard_pipeline_matches_source_grammar() {
        let config = config();
        assert_eq!(
            pipeline_description(&config, Codec::H264),
            "appsrc name=video_source ! queue ! h264parse ! avdec_h264 ! videoconvert ! videoscale ! fakesink name=fakesink_h264 sync=true"
        );
    }

    #[test]
    fn jpeg_pipeline_keeps_overlay_and_unsynchronized_sink() {
        let config = config();
        assert_eq!(
            pipeline_description(&config, Codec::Jpeg),
            "appsrc name=video_source ! jpegdec  ! videoconvert ! videoscale !  imagefreeze allow-replace=TRUE ! textoverlay name=metadata_overlay ! fakesink name=fakesink_jpeg sync=false"
        );
    }

    #[test]
    fn rtp_pipeline_skips_rendering_sink_and_rewrites_codec_names() {
        let mut config = config();
        config.rtp_pipeline = "host=127.0.0.1 port=5004".to_owned();
        assert_eq!(
            pipeline_description(&config, Codec::H264),
            "appsrc name=video_source ! queue ! h264parse ! rtph264pay host=127.0.0.1 port=5004"
        );
        config.parser = "h264parse".to_owned();
        config.decoder = "avdec_h264".to_owned();
        assert_eq!(
            pipeline_description(&config, Codec::H265),
            "appsrc name=video_source ! queue ! h265parse ! rtph265pay host=127.0.0.1 port=5004"
        );
    }

    #[test]
    fn flip_and_rotation_mapping_matches_source_switch() {
        assert_eq!(
            videoflip_description(VideoFlip::Invert, VideoFlip::Left),
            Some("GST_VIDEO_ORIENTATION_90R")
        );
        assert_eq!(
            videoflip_description(VideoFlip::Horizontal, VideoFlip::Right),
            Some("GST_VIDEO_ORIENTATION_UR_LL")
        );
        assert_eq!(
            videoflip_description(VideoFlip::Vertical, VideoFlip::Left),
            Some("GST_VIDEO_ORIENTATION_UR_LL")
        );
        assert_eq!(
            videoflip_description(VideoFlip::None, VideoFlip::None),
            None
        );
    }

    #[test]
    fn low_latency_bounds_encoded_and_decoded_mirror_queues_only() {
        let config = VideoRendererConfig {
            low_latency: true,
            h265_support: true,
            video_sink: "fakesink".to_owned(),
            ..VideoRendererConfig::default()
        };

        let h264 = pipeline_description(&config, Codec::H264);
        assert!(h264.contains(
            "queue max-size-buffers=16 max-size-bytes=0 max-size-time=0 ! h264parse ! avdec_h264 ! queue max-size-buffers=2 max-size-bytes=0 max-size-time=0 leaky=downstream"
        ));

        let h265_config = VideoRendererConfig {
            decoder: "d3d11h264dec".to_owned(),
            ..config.clone()
        };
        let h264_hardware = pipeline_description(&h265_config, Codec::H264);
        assert!(h264_hardware.contains("h264parse ! d3d11h264dec !"));
        let h265 = pipeline_description(&h265_config, Codec::H265);
        assert!(h265.contains(
            "queue max-size-buffers=16 max-size-bytes=0 max-size-time=0 ! h265parse ! d3d11h265dec ! queue max-size-buffers=2 max-size-bytes=0 max-size-time=0 leaky=downstream"
        ));

        let jpeg = pipeline_description(&config, Codec::Jpeg);
        assert!(!jpeg.contains("max-size-buffers"));

        let rtp_config = VideoRendererConfig {
            rtp_pipeline: "pt=96 ! fakesink".to_owned(),
            ..config
        };
        let rtp = pipeline_description(&rtp_config, Codec::H264);
        assert!(!rtp.contains("max-size-buffers"));
    }

    #[test]
    fn vulkan_is_only_inserted_for_mirror_video() {
        let config = VideoRendererConfig {
            renderer: VideoRendererKind::Vulkan,
            h265_support: true,
            video_sink: "fakesink".to_owned(),
            ..VideoRendererConfig::default()
        };
        let h264 = pipeline_description(&config, Codec::H264);
        assert!(h264.contains("videoconvert ! videoscale ! vulkanupload ! vulkansink"));
        assert!(h264.contains("vulkansink_h264 sync=true"));

        let h265 = pipeline_description(&config, Codec::H265);
        assert!(h265.contains("h265parse ! avdec_h265"));
        assert!(
            h265.contains("vulkanupload ! vulkansink name=vulkansink_h265 sync=true"),
            "{h265}"
        );

        let jpeg = pipeline_description(&config, Codec::Jpeg);
        assert!(jpeg.contains("textoverlay name=metadata_overlay ! fakesink name=fakesink_jpeg"));
        assert!(!jpeg.contains("vulkanupload"));

        let rtp_config = VideoRendererConfig {
            rtp_pipeline: "pt=96 ! fakesink".to_owned(),
            ..config
        };
        let rtp = pipeline_description(&rtp_config, Codec::H264);
        assert!(!rtp.contains("vulkanupload"));
    }

    #[test]
    fn d3d11_renderer_kind_selects_hardware_components() {
        let config = VideoRendererConfig {
            renderer: VideoRendererKind::D3d11,
            h265_support: true,
            ..VideoRendererConfig::default()
        };

        let h264 = pipeline_description(&config, Codec::H264);
        assert!(h264.contains("h264parse ! d3d11h264dec"));
        assert!(h264.contains("d3d11videosink name=d3d11videosink_h264"));

        let h265 = pipeline_description(&config, Codec::H265);
        assert!(h265.contains("h265parse ! d3d11h265dec"));
        assert!(h265.contains("d3d11videosink name=d3d11videosink_h265"));
    }

    #[test]
    fn vulkan_renderer_initializes_or_logs_automatic_fallback() {
        let messages = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured = Arc::clone(&messages);
        let renderer = VideoRenderer::new_with_logger(
            VideoRendererConfig {
                renderer: VideoRendererKind::Vulkan,
                parser: "identity".to_owned(),
                decoder: "identity".to_owned(),
                converter: "videoconvert".to_owned(),
                video_sink: "fakesink".to_owned(),
                ..VideoRendererConfig::default()
            },
            Arc::new(move |_, message| {
                captured
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(message.to_owned());
            }),
        )
        .expect("automatic sink fallback should keep renderer initialization available");
        match renderer.config.renderer {
            VideoRendererKind::Vulkan => assert!(
                renderer
                    .pipeline_description("h264")
                    .unwrap()
                    .contains("vulkanupload ! vulkansink")
            ),
            VideoRendererKind::Auto => assert!(
                messages
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .any(|message| message.contains("Vulkan initialization failed"))
            ),
            VideoRendererKind::D3d11 => unreachable!("Vulkan cannot fall back to D3D11 silently"),
        }
    }

    #[test]
    fn unavailable_d3d11_components_fall_back_independently() {
        let mut decoder_config = VideoRendererConfig {
            decoder: "d3d11h264dec".to_owned(),
            video_sink: "d3d11videosink".to_owned(),
            video_sink_options: " sync=false".to_owned(),
            ..VideoRendererConfig::default()
        };
        fallback_unavailable_d3d11_components(&mut decoder_config, &["d3d11h265dec".to_owned()]);
        assert_eq!(decoder_config.decoder, "decodebin");
        assert_eq!(decoder_config.video_sink, "d3d11videosink");

        fallback_unavailable_d3d11_components(&mut decoder_config, &["d3d11videosink".to_owned()]);
        assert_eq!(decoder_config.video_sink, "autovideosink");
        assert!(decoder_config.video_sink_options.is_empty());
    }

    #[test]
    fn pipeline_descriptions_match_recorded_c_oracle() {
        let expected = include_str!("../record_shims/video_oracle_expected.txt")
            .trim_start_matches('\u{feff}')
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(3, '|');
                Some((
                    fields.next()?.to_owned(),
                    fields.next()?.parse::<usize>().ok()?,
                    fields.next()?.to_owned(),
                ))
            })
            .collect::<Vec<_>>();
        let cases = [
            {
                let config = VideoRendererConfig {
                    video_sink: "fakesink".to_owned(),
                    ..VideoRendererConfig::default()
                };
                ("h264-default", config, vec![Codec::H264])
            },
            {
                let config = VideoRendererConfig {
                    flip: [VideoFlip::Invert, VideoFlip::Left],
                    h265_support: true,
                    coverart_support: true,
                    video_sink: "fakesink".to_owned(),
                    ..VideoRendererConfig::default()
                };
                (
                    "mirror-h265-jpeg-flip",
                    config,
                    vec![Codec::H264, Codec::H265, Codec::Jpeg],
                )
            },
            {
                let config = VideoRendererConfig {
                    rtp_pipeline: "pt=96 ! fakesink".to_owned(),
                    h265_support: true,
                    coverart_support: true,
                    video_sync: false,
                    video_sink: "fakesink".to_owned(),
                    ..VideoRendererConfig::default()
                };
                (
                    "rtp-h265-jpeg",
                    config,
                    vec![Codec::H264, Codec::H265, Codec::Jpeg],
                )
            },
            {
                let config = VideoRendererConfig {
                    video_sink_options: " qos=true".to_owned(),
                    video_sink: "fakesink".to_owned(),
                    video_sync: false,
                    ..VideoRendererConfig::default()
                };
                ("sink-options", config, vec![Codec::H264])
            },
        ];

        for (case, config, codecs) in cases {
            let actual = codecs
                .iter()
                .map(|codec| pipeline_description(&config, *codec))
                .collect::<Vec<_>>();
            let recorded = expected
                .iter()
                .filter(|(name, _, _)| name == case)
                .collect::<Vec<_>>();
            assert_eq!(actual.len(), recorded.len(), "C oracle case {case}");
            for (index, launch) in actual.iter().enumerate() {
                let (_, oracle_index, oracle_launch) = recorded[index];
                assert_eq!(index + 1, *oracle_index, "pipeline index in case {case}");
                assert_eq!(launch, oracle_launch, "C pipeline mismatch in case {case}");
            }
        }
    }

    #[test]
    fn gstreamer_pipeline_lifecycle_and_codec_selection() {
        let config = VideoRendererConfig {
            parser: "identity".to_owned(),
            decoder: "identity".to_owned(),
            converter: "identity".to_owned(),
            video_sink: "fakesink".to_owned(),
            h265_support: true,
            ..VideoRendererConfig::default()
        };
        let mut renderer =
            VideoRenderer::new(config).expect("core GStreamer pipeline should build");
        assert_eq!(renderer.active_codec(), None);
        assert_eq!(renderer.choose_codec(false, false), 0);
        assert_eq!(renderer.active_codec(), Some("h264"));
        assert_eq!(renderer.choose_codec(false, true), -1);
        assert!(VideoRenderer::decoder_check(Some(b"identity")));
        assert!(!VideoRenderer::decoder_check(Some(
            b"uxplay-missing-decoder"
        )));
        renderer.destroy();
    }

    #[test]
    fn playbin_keeps_existing_flags_and_adds_download_and_buffering() {
        gst::init().expect("GStreamer must be initialized before querying factories");
        if gst::ElementFactory::find("playbin3").is_none() {
            return;
        }
        let config = VideoRendererConfig {
            video_sink: "fakesink".to_owned(),
            uri: Some("https://example.invalid/stream.m3u8".to_owned()),
            playbin_version: 3,
            ..VideoRendererConfig::default()
        };
        let mut renderer =
            VideoRenderer::new(config).expect("playbin3 should construct in this runtime");
        let pipeline = &renderer.renderers[0]
            .as_ref()
            .expect("HLS renderer")
            .pipeline;
        let flag_value = pipeline.property_value("flags");
        let (_, set_flags) =
            glib::FlagsValue::from_value(&flag_value).expect("playbin flags property");
        let combined_flags = set_flags
            .iter()
            .fold(0_u32, |flags, value| flags | value.value());
        let c_flags = include_str!("../record_shims/video_oracle_flags.txt")
            .trim()
            .parse::<u32>()
            .expect("recorded C playbin flags");
        assert_eq!(combined_flags, c_flags);
        let flags = set_flags
            .into_iter()
            .map(|value| value.nick())
            .collect::<Vec<_>>();
        assert!(flags.contains(&"download"));
        assert!(flags.contains(&"buffering"));
        assert!(flags.contains(&"video"));
        assert!(flags.contains(&"audio"));
        renderer.destroy();
    }

    #[test]
    fn playbin3_demuxes_local_hls_fixture_and_reports_eos_uri() {
        gst::init().expect("GStreamer must be initialized before querying factories");
        if [
            "playbin3",
            "hlsdemux2",
            "h264parse",
            "avdec_h264",
            "fakesink",
        ]
        .into_iter()
        .any(|factory| gst::ElementFactory::find(factory).is_none())
        {
            return;
        }

        let playlist = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join("hls")
            .join("playlist.m3u8");
        let uri = glib::filename_to_uri(&playlist, None)
            .expect("the HLS fixture path should convert to a file URI")
            .to_string();
        let config = VideoRendererConfig {
            uri: Some(uri.clone()),
            video_sink: "fakesink".to_owned(),
            ..VideoRendererConfig::default()
        };
        let mut renderer =
            VideoRenderer::new(config).expect("playbin3 should build for the HLS fixture");
        let main_loop = MainLoop::new(None, false);
        renderer
            .listen(0, &main_loop)
            .expect("the local HLS playbin bus watch should attach");
        let main_loop_worker = main_loop.clone();
        let worker = std::thread::spawn(move || main_loop_worker.run());
        renderer.start();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let ended_uri = loop {
            if let Some(uri) = renderer.take_hls_eos_uri() {
                break Some(uri);
            }
            if std::time::Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        main_loop.quit();
        worker.join().unwrap();
        renderer.destroy();
        assert_eq!(ended_uri.as_deref(), Some(uri.as_str()));
    }

    #[test]
    fn h264_appsrc_pipeline_decodes_fixture_idr_to_fakesink() {
        gst::init().expect("GStreamer must be initialized before querying factories");
        if ["h264parse", "avdec_h264", "videoconvert", "fakesink"]
            .into_iter()
            .any(|factory| gst::ElementFactory::find(factory).is_none())
        {
            return;
        }

        let h264_sample = include_bytes!("../testdata/hls/mirror-frame.h264");
        let nalus = annex_b_nalus(h264_sample);
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

        let config = VideoRendererConfig {
            video_sink: "fakesink".to_owned(),
            video_sink_options: " signal-handoffs=true".to_owned(),
            video_sync: false,
            ..VideoRendererConfig::default()
        };
        let mut renderer = VideoRenderer::new(config).expect("H.264 decoder pipeline");
        let h264_index = renderer.type_h264.expect("H.264 renderer should exist");
        let sink = renderer.renderers[h264_index]
            .as_ref()
            .expect("H.264 renderer pipeline")
            .pipeline
            .by_name("fakesink_h264")
            .expect("named fakesink");
        let handoffs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sink_handoffs = Arc::clone(&handoffs);
        let _handoff_handler = sink.connect("handoff", false, move |_| {
            sink_handoffs.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            None
        });

        assert_eq!(renderer.choose_codec(false, false), 0);
        let (_, outcome) = renderer.render_buffer(&encoded_frame, 0);
        assert_eq!(outcome, RenderOutcome::Submitted);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while handoffs.load(std::sync::atomic::Ordering::Acquire) == 0
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        renderer.destroy();
        assert!(
            handoffs.load(std::sync::atomic::Ordering::Acquire) > 0,
            "valid SPS/PPS/IDR input did not reach fakesink through appsrc, h264parse and avdec_h264"
        );
    }

    #[test]
    fn h265_appsrc_pipeline_decodes_fixture_idr_to_fakesink() {
        gst::init().expect("GStreamer must be initialized before querying factories");
        if ["h265parse", "avdec_h265", "videoconvert", "fakesink"]
            .into_iter()
            .any(|factory| gst::ElementFactory::find(factory).is_none())
        {
            return;
        }

        let encoded_frame = include_bytes!("../testdata/mirror/black-frame.hevc");
        let config = VideoRendererConfig {
            video_sink: "fakesink".to_owned(),
            video_sink_options: " signal-handoffs=true".to_owned(),
            video_sync: false,
            h265_support: true,
            ..VideoRendererConfig::default()
        };
        let mut renderer = VideoRenderer::new(config).expect("H.265 decoder pipeline");
        let h265_index = renderer.type_h265.expect("H.265 renderer should exist");
        let sink = renderer.renderers[h265_index]
            .as_ref()
            .expect("H.265 renderer pipeline")
            .pipeline
            .by_name("fakesink_h265")
            .expect("named H.265 fakesink");
        let handoffs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sink_handoffs = Arc::clone(&handoffs);
        let _handoff_handler = sink.connect("handoff", false, move |_| {
            sink_handoffs.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            None
        });

        assert_eq!(renderer.choose_codec(false, true), 0);
        let (_, outcome) = renderer.render_buffer(encoded_frame, 0);
        assert_eq!(outcome, RenderOutcome::Submitted);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while handoffs.load(std::sync::atomic::Ordering::Acquire) == 0
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        renderer.destroy();
        assert!(
            handoffs.load(std::sync::atomic::Ordering::Acquire) > 0,
            "valid HEVC fixture did not reach fakesink through appsrc, h265parse and avdec_h265"
        );
    }
}
