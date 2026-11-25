//! Rust-owned audio renderer port of `renderers/audio_renderer.c`.
//!
//! This module keeps the two formats enabled by the source's `NFORMATS = 2`:
//! AAC-ELD (`ct = 8`) and ALAC (`ct = 2`). It deliberately uses GStreamer for
//! parsing, clocks, buffers, state changes, plugin discovery, and bus watches.

#![forbid(unsafe_code)]

use std::io::Write;
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};

use glib::{ControlFlow, MainLoop};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

const SECOND_IN_NSECS: u64 = 1_000_000_000;
const REQUIRED_PLUGINS: [&str; 5] = ["app", "libav", "playback", "autodetect", "videoparsersbad"];
const AAC_DECODER: &str = "avdec_aac";
const ALAC_DECODER: &str = "avdec_alac";

const LPCM_CAPS: &str =
    "audio/x-raw,rate=(int)44100,channels=(int)2,format=S16LE,layout=interleaved";
const ALAC_CAPS: &str = "audio/x-alac,mpegversion=(int)4,channels=(int)2,rate=(int)44100,stream-format=raw,codec_data=(buffer)00000024616c616300000000000001600010280a0e0200ff00000000000000000000ac44";
const AAC_LC_CAPS: &str = "audio/mpeg,mpegversion=(int)4,channels=(int)2,rate=(int)44100,stream-format=raw,codec_data=(buffer)1210";
const AAC_ELD_CAPS: &str = "audio/mpeg,mpegversion=(int)4,channels=(int)2,rate=(int)44100,stream-format=raw,codec_data=(buffer)f8e85000";

/// Source-compatible logger severity values from `lib/logger.h`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum LogLevel {
    Error = 3,
    Info = 6,
    Debug = 7,
}

pub type LogCallback = Arc<dyn Fn(LogLevel, &str) + Send + Sync + 'static>;

/// Renderer settings captured by the original `audio_renderer_init` call.
#[derive(Clone, Debug)]
pub struct AudioRendererConfig {
    /// GStreamer sink description, appended verbatim to the pipeline grammar.
    pub audio_sink: String,
    pub audio_sync: bool,
    pub video_sync: bool,
    /// When non-empty, this is appended after `rtpL16pay` as GStreamer syntax.
    pub rtp_pipeline: String,
}

impl AudioRendererConfig {
    pub fn new(audio_sink: impl Into<String>, audio_sync: bool, video_sync: bool) -> Self {
        Self {
            audio_sink: audio_sink.into(),
            audio_sync,
            video_sync,
            rtp_pipeline: String::new(),
        }
    }

    pub fn with_rtp_pipeline(mut self, pipeline: impl Into<String>) -> Self {
        self.rtp_pipeline = pipeline.into();
        self
    }
}

/// AirPlay audio compression types recognized by the source renderer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum AudioFormat {
    Pcm = 1,
    Alac = 2,
    AacLc = 4,
    AacEld = 8,
}

impl AudioFormat {
    pub const fn from_ct(ct: u8) -> Option<Self> {
        match ct {
            1 => Some(Self::Pcm),
            2 => Some(Self::Alac),
            4 => Some(Self::AacLc),
            8 => Some(Self::AacEld),
            _ => None,
        }
    }

    pub const fn ct(self) -> u8 {
        self as u8
    }

    const fn label(self) -> &'static str {
        match self {
            Self::AacEld => "AAC-ELD 44100/2",
            Self::Alac => "ALAC 44100/16/2",
            Self::AacLc => "AAC-LC 44100/2",
            Self::Pcm => "PCM 44100/16/2 S16LE",
        }
    }

    const fn caps(self) -> &'static str {
        match self {
            Self::AacEld => AAC_ELD_CAPS,
            Self::Alac => ALAC_CAPS,
            Self::AacLc => AAC_LC_CAPS,
            Self::Pcm => LPCM_CAPS,
        }
    }

    const fn decoder(self) -> Option<&'static str> {
        match self {
            Self::AacEld | Self::AacLc => Some(AAC_DECODER),
            Self::Alac => Some(ALAC_DECODER),
            Self::Pcm => None,
        }
    }
}

/// Deterministic result of handing an AirPlay packet to the renderer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderOutcome {
    Disabled,
    NoActivePipeline,
    EmptyPacket,
    InvalidTimestamp,
    InvalidFrame,
    Submitted,
}

/// A renderer start result, including the unsupported-type behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartOutcome {
    Started(AudioFormat),
    Unsupported(u8),
}

#[derive(Debug)]
struct AudioPipeline {
    format: AudioFormat,
    pipeline: gst::Pipeline,
    appsrc: gst_app::AppSrc,
    volume: Option<gst::Element>,
    bus: gst::Bus,
}

#[derive(Clone, Debug)]
struct ActivePipeline {
    pipeline: gst::Pipeline,
    appsrc: gst_app::AppSrc,
}

/// Per-client GStreamer audio state.
///
/// The C implementation stores these values in process globals. Owning them in
/// an instance makes the same lifecycle usable from the pure Rust app while
/// preserving one live pipeline per enabled audio format.
pub struct AudioRenderer {
    formats: Vec<AudioPipeline>,
    active_index: Option<usize>,
    active_for_bus_callback: Arc<Mutex<Option<ActivePipeline>>>,
    render_audio: bool,
    sync: bool,
    base_time_ns: u64,
    async_audio: bool,
    video_sync: bool,
    audio_rtp: bool,
    volume_level: f64,
    logger: LogCallback,
}

impl AudioRenderer {
    /// Initialize GStreamer and construct AAC-ELD plus ALAC pipelines.
    pub fn new(config: AudioRendererConfig) -> Result<Self, String> {
        Self::new_with_logger(config, Arc::new(|_, _| {}))
    }

    /// Initialize with a callback that receives the source logger levels/messages.
    pub fn new_with_logger(
        config: AudioRendererConfig,
        logger: LogCallback,
    ) -> Result<Self, String> {
        gst::init().map_err(|error| format!("GStreamer initialization failed: {error}"))?;
        if !check_plugins() {
            return Err("required GStreamer plugins are unavailable".to_owned());
        }

        let aac = check_plugin_feature(AAC_DECODER);
        let alac = check_plugin_feature(ALAC_DECODER);
        let audio_rtp = !config.rtp_pipeline.is_empty();
        let (async_audio, video_sync) =
            selected_sync_options(audio_rtp, config.audio_sync, config.video_sync);
        if audio_rtp {
            source_println(format!(
                "*** Audio RTP mode enabled: sending to {}",
                config.rtp_pipeline
            ));
        }

        let clock = gst::SystemClock::obtain();
        clock.set_property("clock-type", gst::ClockType::Realtime);

        let mut formats = Vec::with_capacity(2);
        for format in [AudioFormat::AacEld, AudioFormat::Alac] {
            let decoder_available = match format {
                AudioFormat::AacEld => aac,
                AudioFormat::Alac => alac,
                AudioFormat::AacLc | AudioFormat::Pcm => false,
            };
            let launch = pipeline_description(
                format,
                decoder_available,
                &config.audio_sink,
                config.audio_sync,
                config.video_sync,
                &config.rtp_pipeline,
            );
            let element = gst::parse::launch(&launch).map_err(|error| {
                format!(
                    "gst_parse_launch error (audio {}): {error}",
                    formats.len() + 1
                )
            })?;
            let pipeline = element.downcast::<gst::Pipeline>().map_err(|_| {
                format!(
                    "audio pipeline {} did not produce a GstPipeline",
                    formats.len() + 1
                )
            })?;
            pipeline.use_clock(Some(&clock));
            let bus = pipeline.bus().ok_or_else(|| {
                format!("audio pipeline {} has no GStreamer bus", formats.len() + 1)
            })?;
            let source = pipeline
                .by_name("audio_source")
                .ok_or_else(|| {
                    format!(
                        "audio pipeline {} has no audio_source appsrc",
                        formats.len() + 1
                    )
                })?
                .downcast::<gst_app::AppSrc>()
                .map_err(|_| {
                    format!(
                        "audio pipeline {} audio_source is not appsrc",
                        formats.len() + 1
                    )
                })?;
            let volume = pipeline.by_name("volume");
            if let Some(element) = &volume {
                element.set_property("volume", 1.0_f64);
            }

            let caps = gst::Caps::from_str(format.caps())
                .map_err(|error| format!("invalid caps for {}: {error}", format.label()))?;
            source.set_caps(Some(&caps));
            source.set_stream_type(gst_app::AppStreamType::Stream);
            source.set_property("is-live", true);
            source.set_property("format", gst::Format::Time);

            log(
                &logger,
                LogLevel::Debug,
                &format!("Audio format {}: {}", formats.len() + 1, format.label()),
            );
            log(
                &logger,
                LogLevel::Debug,
                &format!(
                    "GStreamer audio pipeline {}: \"{}\"",
                    formats.len() + 1,
                    launch
                ),
            );
            formats.push(AudioPipeline {
                format,
                pipeline,
                appsrc: source,
                volume,
                bus,
            });
        }

        Ok(Self {
            formats,
            active_index: None,
            active_for_bus_callback: Arc::new(Mutex::new(None)),
            render_audio: false,
            sync: false,
            base_time_ns: u64::MAX,
            async_audio,
            video_sync,
            audio_rtp,
            volume_level: 1.0,
            logger,
        })
    }

    /// Start a format while retaining the source's pipeline switch ordering.
    pub fn start(&mut self, ct: u8) -> StartOutcome {
        self.render_audio = false;
        let Some(format) = AudioFormat::from_ct(ct) else {
            log(
                &self.logger,
                LogLevel::Error,
                &format!("unknown audio compression type ct = {ct}"),
            );
            return StartOutcome::Unsupported(ct);
        };
        let Some(id) = self.formats.iter().position(|item| item.format == format) else {
            // AAC-LC and PCM have known caps in the source but are disabled by
            // NFORMATS = 2, so their renderer slots do not exist.
            log(
                &self.logger,
                LogLevel::Error,
                &format!("unknown audio compression type ct = {ct}"),
            );
            return StartOutcome::Unsupported(ct);
        };

        match format {
            AudioFormat::AacEld => {
                self.render_audio = gst::ElementFactory::find(AAC_DECODER).is_some();
                self.sync = self.video_sync;
                if !self.render_audio {
                    log(
                        &self.logger,
                        LogLevel::Info,
                        "*** GStreamer libav plugin feature avdec_aac is missing, cannot decode AAC audio",
                    );
                }
            }
            AudioFormat::Alac => {
                self.render_audio = gst::ElementFactory::find(ALAC_DECODER).is_some();
                self.sync = self.async_audio;
                if !self.render_audio {
                    log(
                        &self.logger,
                        LogLevel::Info,
                        "*** GStreamer libav plugin feature avdec_alac is missing, cannot decode ALAC audio",
                    );
                }
            }
            AudioFormat::AacLc | AudioFormat::Pcm => {
                self.render_audio = false;
                return StartOutcome::Unsupported(ct);
            }
        }

        if let Some(previous) = self.active_index {
            if self.formats[previous].format != format {
                let old = &self.formats[previous];
                let _ = old.appsrc.end_of_stream();
                let _ = old.pipeline.set_state(gst::State::Null);
                log(
                    &self.logger,
                    LogLevel::Info,
                    &format!("changed audio connection, format {}", format.label()),
                );
                self.activate(id);
            }
        } else {
            log(
                &self.logger,
                LogLevel::Info,
                &format!("start audio connection, format {}", format.label()),
            );
            self.activate(id);
        }
        StartOutcome::Started(format)
    }

    fn activate(&mut self, id: usize) {
        let selected = &self.formats[id];
        let _ = selected.pipeline.set_state(gst::State::Playing);
        self.base_time_ns = selected
            .appsrc
            .base_time()
            .map_or(u64::MAX, gst::ClockTime::nseconds);
        self.active_index = Some(id);
        let handle = ActivePipeline {
            pipeline: selected.pipeline.clone(),
            appsrc: selected.appsrc.clone(),
        };
        *self
            .active_for_bus_callback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(handle);
    }

    /// Submit one compressed AirPlay packet. `seqnum` is intentionally unused,
    /// matching the source callback; NTP time becomes PTS only for sync mode.
    pub fn render_buffer(&self, data: &[u8], _seqnum: u16, ntp_time: u64) -> RenderOutcome {
        if !self.render_audio {
            return RenderOutcome::Disabled;
        }

        let pts = match adjusted_pts(ntp_time, self.base_time_ns, self.sync) {
            Ok(pts) => pts,
            Err(()) => {
                log(
                    &self.logger,
                    LogLevel::Error,
                    &format!(
                        "*** invalid ntp_time < gst_audio_pipeline_base_time\n{:8.6} ntp_time\n{:8.6} base_time",
                        ntp_time as f64 / SECOND_IN_NSECS as f64,
                        self.base_time_ns as f64 / SECOND_IN_NSECS as f64,
                    ),
                );
                return RenderOutcome::InvalidTimestamp;
            }
        };

        let Some(id) = self.active_index else {
            return RenderOutcome::NoActivePipeline;
        };
        if data.is_empty() {
            // The C path compares the length pointer to null and then reads
            // data[0] even for a zero-sized buffer (undefined behavior). Treat
            // a valid empty slice as a safely dropped packet.
            return RenderOutcome::EmptyPacket;
        }

        let format = self.formats[id].format;
        if !is_valid_frame(format, data[0]) {
            log(
                &self.logger,
                LogLevel::Error,
                &format!(
                    "*** ERROR invalid  audio frame (compression_type {}) skipped ",
                    format.ct()
                ),
            );
            log(
                &self.logger,
                LogLevel::Error,
                &format!(
                    "***       first byte of invalid frame was  0x{:2.2x} ",
                    data[0]
                ),
            );
            return RenderOutcome::InvalidFrame;
        }

        let mut buffer = gst::Buffer::from_mut_slice(data.to_vec());
        if self.sync
            && let Some(buffer) = buffer.get_mut()
        {
            buffer.set_pts(gst::ClockTime::from_nseconds(pts));
        }
        // The source ignores GstFlowReturn. Preserve its callback contract by
        // returning Submitted for a valid frame even if downstream has stopped.
        let _ = self.formats[id].appsrc.push_buffer(buffer);
        RenderOutcome::Submitted
    }

    /// End the current appsrc stream, set its pipeline to NULL, and clear only
    /// the active renderer, as `audio_renderer_stop` does.
    pub fn stop(&mut self) {
        if let Some(id) = self.active_index.take() {
            let selected = &self.formats[id];
            let _ = selected.appsrc.end_of_stream();
            let _ = selected.pipeline.set_state(gst::State::Null);
            *self
                .active_for_bus_callback
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
    }

    /// Apply the C source clamp, but only after a format has become active.
    pub fn set_volume(&mut self, volume: f64) {
        if self.active_index.is_none() {
            return;
        }
        self.volume_level = clamp_volume(volume);
        for renderer in &self.formats {
            if let Some(element) = &renderer.volume {
                element.set_property("volume", self.volume_level);
            }
        }
    }

    /// The source implementation is intentionally empty.
    pub fn flush(&mut self) {}

    /// Stop playback, then release every bus, element, and pipeline reference.
    pub fn destroy(&mut self) {
        self.stop();
        self.formats.clear();
    }

    /// Install the source's watch on a caller-supplied GLib main loop.
    pub fn listen(&self, loop_: &MainLoop, id: usize) -> Result<gst::bus::BusWatchGuard, String> {
        let selected = self
            .formats
            .get(id)
            .ok_or_else(|| format!("audio renderer id out of range: {id}"))?;
        let bus = selected.bus.clone();
        let active = Arc::clone(&self.active_for_bus_callback);
        let logger = Arc::clone(&self.logger);
        let main_loop = loop_.clone();
        bus.add_watch(move |bus, message| {
            use gst::MessageView;

            match message.view() {
                MessageView::Error(error) => {
                    log(
                        &logger,
                        LogLevel::Info,
                        &format!(
                            "GStreamer error (audio): {} {}",
                            message
                                .src()
                                .map(|source| source.name())
                                .unwrap_or_default(),
                            error.error(),
                        ),
                    );
                    let current = active
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    if let Some(current) = current {
                        let _ = current.appsrc.end_of_stream();
                        bus.set_flushing(true);
                        let _ = current.pipeline.set_state(gst::State::Ready);
                    }
                    main_loop.quit();
                }
                MessageView::Eos(..) => {
                    log(&logger, LogLevel::Info, "GStreamer: End-Of-Stream (audio)");
                }
                MessageView::Element(..) => {
                    // The level element emits many routine messages.
                }
                _ => {
                    log(
                        &logger,
                        LogLevel::Debug,
                        &format!(
                            "GStreamer unhandled audio bus message: src = {} type = {}",
                            message
                                .src()
                                .map(|source| source.name())
                                .unwrap_or_default(),
                            message_type_name(message.type_()),
                        ),
                    );
                }
            }
            ControlFlow::Continue
        })
        .map_err(|error| format!("could not add audio bus watch: {error}"))
    }

    pub fn is_rtp(&self) -> bool {
        self.audio_rtp
    }

    pub fn active_format(&self) -> Option<AudioFormat> {
        self.active_index.map(|id| self.formats[id].format)
    }

    pub fn volume_level(&self) -> f64 {
        self.volume_level
    }

    /// Pipeline descriptions retained for deterministic inspection and tests.
    pub fn pipeline_descriptions(&self, config: &AudioRendererConfig) -> Vec<String> {
        self.formats
            .iter()
            .map(|item| {
                let decoder = item
                    .format
                    .decoder()
                    .is_some_and(|name| gst::ElementFactory::find(name).is_some());
                pipeline_description(
                    item.format,
                    decoder,
                    &config.audio_sink,
                    config.audio_sync,
                    config.video_sync,
                    &config.rtp_pipeline,
                )
            })
            .collect()
    }
}

impl Drop for AudioRenderer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Initializes GStreamer and checks the same five plugin modules as C.
pub fn gstreamer_init() -> bool {
    if let Err(error) = gst::init() {
        source_println(format!("GStreamer initialization failed: {error}"));
        return false;
    }
    check_plugins()
}

fn source_print(text: &str) {
    #[cfg(windows)]
    let text = text.replace('\n', "\r\n");
    #[cfg(not(windows))]
    let text = text.to_owned();
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
}

fn source_println(message: impl std::fmt::Display) {
    source_print(&format!("{message}\n"));
}

fn check_plugins() -> bool {
    let registry = gst::Registry::get();
    let mut complete = true;
    for (needed, package) in REQUIRED_PLUGINS.iter().zip([
        "plugins-base",
        "libav",
        "plugins-base",
        "plugins-good",
        "plugins-bad",
    ]) {
        if registry.find_plugin(needed).is_none() {
            source_print(&format!(
                "Required gstreamer plugin '{needed}' not found\nMissing plugin is contained in  '[GStreamer 1.x]-{package}'\n"
            ));
            complete = false;
        }
    }
    if !complete {
        source_print(
            "\nif the plugin is installed, but not found, your gstreamer registry may have been corrupted.\nto rebuild it when gstreamer next starts, clear your gstreamer cache with:\n\"rm -rf ~/.cache/gstreamer-1.0\"\n\n",
        );
    }
    complete
}

fn check_plugin_feature(feature: &str) -> bool {
    if gst::ElementFactory::find(feature).is_some() {
        return true;
    }
    source_print(&format!(
        "Required gstreamer libav plugin feature '{feature}' not found:\n\nThis may be missing because the FFmpeg package used by GStreamer-1.x-libav is incomplete.\n(Some distributions provide an incomplete FFmpeg due to License or Patent issues:\nin such cases a complete version for that distribution is usually made available elsewhere)\n"
    ));
    source_print(
        "\nif the plugin feature is installed, but not found, your gstreamer registry may have been corrupted.\nto rebuild it when gstreamer next starts, clear your gstreamer cache with:\n\"rm -rf ~/.cache/gstreamer-1.0\"\n\n",
    );
    false
}

fn pipeline_description(
    format: AudioFormat,
    decoder_available: bool,
    audio_sink: &str,
    audio_sync: bool,
    video_sync: bool,
    rtp_pipeline: &str,
) -> String {
    let mut launch = String::from("appsrc name=audio_source ! queue ! ");
    if decoder_available && let Some(decoder) = format.decoder() {
        launch.push_str(decoder);
        launch.push_str(" ! ");
    }
    launch.push_str("audioconvert ! audioresample quality=10 ! volume name=volume ! ");
    if !rtp_pipeline.is_empty() {
        launch
            .push_str("audioconvert ! audio/x-raw,format=S16BE,rate=44100,channels=2 ! rtpL16pay ");
        launch.push_str(rtp_pipeline);
    } else {
        launch.push_str("level ! ");
        launch.push_str(audio_sink);
        let sync = if format == AudioFormat::Alac {
            audio_sync
        } else {
            video_sync
        };
        launch.push_str(if sync { " sync=true" } else { " sync=false" });
    }
    launch
}

fn selected_sync_options(audio_rtp: bool, audio_sync: bool, video_sync: bool) -> (bool, bool) {
    if audio_rtp {
        (false, false)
    } else {
        (audio_sync, video_sync)
    }
}

fn is_valid_frame(format: AudioFormat, first_byte: u8) -> bool {
    match format {
        AudioFormat::AacEld => matches!(first_byte, 0x8c..=0x8e | 0x80..=0x82),
        AudioFormat::Alac => first_byte == 0x20,
        AudioFormat::AacLc => first_byte == 0xff,
        AudioFormat::Pcm => true,
    }
}

fn adjusted_pts(ntp_time: u64, base_time_ns: u64, sync: bool) -> Result<u64, ()> {
    if !sync {
        return Ok(ntp_time);
    }
    ntp_time.checked_sub(base_time_ns).ok_or(())
}

fn clamp_volume(volume: f64) -> f64 {
    volume.clamp(0.0, 10.0)
}

fn log(logger: &LogCallback, level: LogLevel, message: &str) {
    logger(level, message);
}

fn message_type_name(message_type: gst::MessageType) -> String {
    match message_type {
        gst::MessageType::Eos => "eos".to_owned(),
        gst::MessageType::Error => "error".to_owned(),
        gst::MessageType::Warning => "warning".to_owned(),
        gst::MessageType::Info => "info".to_owned(),
        gst::MessageType::Tag => "tag".to_owned(),
        gst::MessageType::Buffering => "buffering".to_owned(),
        gst::MessageType::StateChanged => "state-changed".to_owned(),
        gst::MessageType::StateDirty => "state-dirty".to_owned(),
        gst::MessageType::StepDone => "step-done".to_owned(),
        gst::MessageType::ClockProvide => "clock-provide".to_owned(),
        gst::MessageType::ClockLost => "clock-lost".to_owned(),
        gst::MessageType::NewClock => "new-clock".to_owned(),
        gst::MessageType::StructureChange => "structure-change".to_owned(),
        gst::MessageType::StreamStatus => "stream-status".to_owned(),
        gst::MessageType::Application => "application".to_owned(),
        gst::MessageType::Element => "element".to_owned(),
        gst::MessageType::SegmentStart => "segment-start".to_owned(),
        gst::MessageType::SegmentDone => "segment-done".to_owned(),
        other => format!("{other:?}").to_lowercase().replace('_', "-"),
    }
}

static GLOBAL_RENDERER: OnceLock<Mutex<Option<AudioRenderer>>> = OnceLock::new();

fn global_renderer() -> &'static Mutex<Option<AudioRenderer>> {
    GLOBAL_RENDERER.get_or_init(|| Mutex::new(None))
}

/// Initialize the process-global renderer, corresponding to the C globals.
pub fn audio_renderer_init(config: AudioRendererConfig, logger: LogCallback) -> Result<(), String> {
    let renderer = AudioRenderer::new_with_logger(config, logger)?;
    let mut global = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *global = Some(renderer);
    Ok(())
}

/// Start a format on the process-global renderer if it has been initialized.
pub fn audio_renderer_start(ct: u8) -> Option<StartOutcome> {
    let mut global = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    global.as_mut().map(|renderer| renderer.start(ct))
}

/// Render a packet through the process-global renderer.
pub fn audio_renderer_render_buffer(
    data: &[u8],
    seqnum: u16,
    ntp_time: u64,
) -> Option<RenderOutcome> {
    let global = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    global
        .as_ref()
        .map(|renderer| renderer.render_buffer(data, seqnum, ntp_time))
}

/// Install one watch on a process-global renderer pipeline.
pub fn audio_renderer_listen(
    loop_: &MainLoop,
    id: usize,
) -> Result<Option<gst::bus::BusWatchGuard>, String> {
    let global = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    global
        .as_ref()
        .map(|renderer| renderer.listen(loop_, id))
        .transpose()
}

pub fn audio_renderer_stop() {
    let mut global = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(renderer) = global.as_mut() {
        renderer.stop();
    }
}

pub fn audio_renderer_set_volume(volume: f64) {
    let mut global = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(renderer) = global.as_mut() {
        renderer.set_volume(volume);
    }
}

pub fn audio_renderer_flush() {
    let mut global = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(renderer) = global.as_mut() {
        renderer.flush();
    }
}

pub fn audio_renderer_destroy() {
    let mut global = global_renderer()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(mut renderer) = global.take() {
        renderer.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_enables_only_the_first_two_formats() {
        assert_eq!(
            [AudioFormat::AacEld, AudioFormat::Alac].map(AudioFormat::ct),
            [8, 2]
        );
    }

    #[test]
    fn local_pipeline_descriptions_match_source_construction() {
        let aac = pipeline_description(AudioFormat::AacEld, true, "fakesink", false, true, "");
        let alac = pipeline_description(AudioFormat::Alac, true, "fakesink", true, false, "");
        assert_eq!(
            aac,
            "appsrc name=audio_source ! queue ! avdec_aac ! audioconvert ! audioresample quality=10 ! volume name=volume ! level ! fakesink sync=true"
        );
        assert_eq!(
            alac,
            "appsrc name=audio_source ! queue ! avdec_alac ! audioconvert ! audioresample quality=10 ! volume name=volume ! level ! fakesink sync=true"
        );
    }

    #[test]
    fn missing_decoder_and_unsynchronized_paths_match_source() {
        assert_eq!(
            pipeline_description(AudioFormat::AacEld, false, "fakesink", false, false, ""),
            "appsrc name=audio_source ! queue ! audioconvert ! audioresample quality=10 ! volume name=volume ! level ! fakesink sync=false"
        );
        assert_eq!(
            pipeline_description(AudioFormat::Alac, false, "fakesink", false, true, ""),
            "appsrc name=audio_source ! queue ! audioconvert ! audioresample quality=10 ! volume name=volume ! level ! fakesink sync=false"
        );
    }

    #[test]
    fn rtp_pipeline_is_appended_without_retokenizing() {
        assert_eq!(
            pipeline_description(
                AudioFormat::AacEld,
                true,
                "ignored-sink",
                true,
                true,
                "host=127.0.0.1 port=5004",
            ),
            "appsrc name=audio_source ! queue ! avdec_aac ! audioconvert ! audioresample quality=10 ! volume name=volume ! audioconvert ! audio/x-raw,format=S16BE,rate=44100,channels=2 ! rtpL16pay host=127.0.0.1 port=5004"
        );
    }

    #[test]
    fn rtp_initialization_keeps_source_sync_defaults_false() {
        assert_eq!(selected_sync_options(true, true, true), (false, false));
        assert_eq!(selected_sync_options(false, true, false), (true, false));
    }

    #[test]
    fn compressed_frame_headers_match_source_rules() {
        for byte in [0x8c, 0x8d, 0x8e, 0x80, 0x81, 0x82] {
            assert!(is_valid_frame(AudioFormat::AacEld, byte));
        }
        assert!(!is_valid_frame(AudioFormat::AacEld, 0xff));
        assert!(is_valid_frame(AudioFormat::Alac, 0x20));
        assert!(!is_valid_frame(AudioFormat::Alac, 0x21));
        assert!(is_valid_frame(AudioFormat::AacLc, 0xff));
        assert!(!is_valid_frame(AudioFormat::AacLc, 0x20));
        assert!(is_valid_frame(AudioFormat::Pcm, 0));
    }

    #[test]
    fn source_volume_clamp_keeps_nan_comparison_behavior() {
        assert_eq!(clamp_volume(-1.0), 0.0);
        assert_eq!(clamp_volume(5.0), 5.0);
        assert_eq!(clamp_volume(100.0), 10.0);
        assert!(clamp_volume(f64::NAN).is_nan());
    }

    #[test]
    fn source_caps_match_airplay_format_metadata() {
        assert!(LPCM_CAPS.contains("format=S16LE"));
        assert!(ALAC_CAPS.ends_with("0000ac44"));
        assert!(AAC_LC_CAPS.ends_with("codec_data=(buffer)1210"));
        assert!(AAC_ELD_CAPS.ends_with("codec_data=(buffer)f8e85000"));
    }

    #[test]
    fn rtp_mode_does_not_add_local_level_or_sink() {
        let launch = pipeline_description(
            AudioFormat::Alac,
            false,
            "fakesink",
            true,
            true,
            "host=127.0.0.1 port=5004",
        );
        assert!(launch.contains("audio/x-raw,format=S16BE,rate=44100,channels=2"));
        assert!(!launch.contains("level"));
        assert!(!launch.contains("fakesink"));
    }

    #[test]
    fn timestamp_is_relative_to_base_time_only_for_sync_formats() {
        assert_eq!(adjusted_pts(1_500, 500, true), Ok(1_000));
        assert_eq!(adjusted_pts(1_500, 500, false), Ok(1_500));
        assert_eq!(adjusted_pts(499, 500, true), Err(()));
    }

    #[test]
    fn native_pipeline_lifecycle_runs_through_fakesink() {
        let config = AudioRendererConfig::new("fakesink", false, false);
        let mut renderer = AudioRenderer::new(config).expect("native GStreamer is required");
        let loop_ = MainLoop::new(None, false);
        let _watch = renderer
            .listen(&loop_, 0)
            .expect("audio bus watch should attach");
        assert_eq!(
            renderer.start(8),
            StartOutcome::Started(AudioFormat::AacEld)
        );
        assert_eq!(renderer.active_format(), Some(AudioFormat::AacEld));
        renderer.set_volume(3.5);
        assert_eq!(renderer.volume_level(), 3.5);
        assert_eq!(
            renderer.render_buffer(&[], 7, 0),
            RenderOutcome::EmptyPacket
        );
        assert_eq!(
            renderer.render_buffer(&[0xff, 0], 7, 0),
            RenderOutcome::InvalidFrame
        );
        if gst::ElementFactory::find(AAC_DECODER).is_some() {
            assert_eq!(
                renderer.render_buffer(&[0x8c, 0], 7, 0),
                RenderOutcome::Submitted
            );
        }
        assert_eq!(renderer.start(2), StartOutcome::Started(AudioFormat::Alac));
        assert_eq!(renderer.active_format(), Some(AudioFormat::Alac));
        if gst::ElementFactory::find(ALAC_DECODER).is_some() {
            assert_eq!(
                renderer.render_buffer(&[0x20, 0], 7, 0),
                RenderOutcome::Submitted
            );
        }
        assert_eq!(renderer.start(4), StartOutcome::Unsupported(4));
        renderer.stop();
        assert_eq!(renderer.active_format(), None);
    }

    #[test]
    fn valid_alac_fixture_decodes_through_appsrc_to_fakesink() {
        gst::init().expect("GStreamer must be initialized before querying factories");
        if [
            "appsrc",
            "queue",
            "avdec_alac",
            "audioconvert",
            "audioresample",
            "volume",
            "level",
            "fakesink",
        ]
        .into_iter()
        .any(|factory| gst::ElementFactory::find(factory).is_none())
        {
            return;
        }

        let config = AudioRendererConfig::new(
            "fakesink name=fakesink_alac signal-handoffs=true",
            false,
            false,
        );
        let mut renderer = AudioRenderer::new(config).expect("ALAC decoder pipeline");
        let sink = renderer.formats[1]
            .pipeline
            .by_name("fakesink_alac")
            .expect("named ALAC fakesink");
        let handoffs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sink_handoffs = Arc::clone(&handoffs);
        let _handoff_handler = sink.connect("handoff", false, move |_| {
            sink_handoffs.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            None
        });

        assert_eq!(
            renderer.start(AudioFormat::Alac.ct()),
            StartOutcome::Started(AudioFormat::Alac)
        );
        let packet = include_bytes!("../testdata/alac-frame-352.alac");
        assert_eq!(packet.first().map(|byte| byte & 0xf0), Some(0x20));
        assert_eq!(
            renderer.render_buffer(packet, 17, 0),
            RenderOutcome::Submitted
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while handoffs.load(std::sync::atomic::Ordering::Acquire) == 0
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        renderer.destroy();
        assert!(
            handoffs.load(std::sync::atomic::Ordering::Acquire) > 0,
            "valid ALAC input did not reach fakesink through appsrc and avdec_alac"
        );
    }
}
