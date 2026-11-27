//! Rust-owned mux renderer port of `renderers/mux_renderer.c`.
//!
//! The C unit keeps one renderer and codec selection in process globals. This
//! port keeps that state in an owned renderer instance so the application can
//! manage its lifetime explicitly while preserving the source pipeline,
//! naming, packet timestamp, and stop behavior.

#![forbid(unsafe_code)]

use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

const SECOND_IN_NSECS: u64 = 1_000_000_000;
const STOP_TIMEOUT: gst::ClockTime = gst::ClockTime::from_seconds(5);
const H264_CAPS: &str = "video/x-h264,stream-format=(string)byte-stream,alignment=(string)au";
const H265_CAPS: &str = "video/x-h265,stream-format=(string)byte-stream,alignment=(string)au";
const AAC_ELD_CAPS: &str = "audio/mpeg,mpegversion=(int)4,channels=(int)2,rate=(int)44100,stream-format=raw,codec_data=(buffer)f8e85000";
const ALAC_CAPS: &str = "audio/x-alac,mpegversion=(int)4,channels=(int)2,rate=(int)44100,stream-format=raw,codec_data=(buffer)00000024616c616300000000000001600010280a0e0200ff00000000000000000000ac44";

pub type LogCallback = Arc<dyn Fn(LogLevel, &str) + Send + Sync + 'static>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum LogLevel {
    Error = 3,
    Info = 6,
    Debug = 7,
}

/// Configuration accepted by the original `mux_renderer_init` call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MuxRendererConfig {
    pub output_filename: String,
    pub use_audio: bool,
    pub use_video: bool,
}

impl MuxRendererConfig {
    pub fn new(filename: impl Into<String>, use_audio: bool, use_video: bool) -> Self {
        Self {
            output_filename: filename.into(),
            use_audio,
            use_video,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PipelinePlan {
    pub filename: String,
    pub launch: String,
    pub video_caps: Option<&'static str>,
    pub audio_caps: Option<&'static str>,
    pub has_video_source: bool,
    pub has_audio_source: bool,
}

/// C-source filename and launch-string construction, including its ALAC/video
/// condition. `audio_is_alac` also suppresses the video input in the source.
pub fn pipeline_plan(
    filename: &str,
    file_count: i32,
    no_audio: bool,
    no_video: bool,
    audio_is_alac: bool,
    video_is_h265: bool,
) -> PipelinePlan {
    let mut output_name = format!("{filename}.{file_count}.");
    if !no_video && !audio_is_alac {
        output_name.push_str(if video_is_h265 { "H265." } else { "H264." });
    }
    if !no_audio {
        output_name.push_str(if audio_is_alac { "ALAC." } else { "AAC." });
    }
    output_name.push_str("mp4");

    let has_video_source = !no_video && !audio_is_alac;
    let has_audio_source = !no_audio;
    let mut launch = String::new();
    if has_video_source {
        launch.push_str("appsrc name=video_src format=time is-live=true ! queue ! ");
        launch.push_str(if video_is_h265 {
            "h265parse ! mux. "
        } else {
            "h264parse ! mux. "
        });
    }
    if has_audio_source {
        launch.push_str("appsrc name=audio_src format=time is-live=true ! queue ! ");
        if !audio_is_alac {
            launch.push_str("aacparse ! queue ! ");
        }
        launch.push_str("mux. ");
    }
    launch.push_str(
        "mp4mux name=mux fragment-duration=2000 fragment-mode=first-moov-then-finalise ! filesink name=filesink location=",
    );
    launch.push_str(&output_name);

    PipelinePlan {
        filename: output_name,
        launch,
        video_caps: has_video_source.then_some(if video_is_h265 { H265_CAPS } else { H264_CAPS }),
        audio_caps: has_audio_source.then_some(if audio_is_alac {
            ALAC_CAPS
        } else {
            AAC_ELD_CAPS
        }),
        has_video_source,
        has_audio_source,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PushOutcome {
    Disabled,
    NoPipeline,
    SourceAbsent,
    Submitted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StartOutcome {
    Started(PipelinePlan),
    AlreadyRunning,
    Ignored,
    ParseFailed(String),
}

#[derive(Debug)]
struct ActivePipeline {
    pipeline: Option<gst::Pipeline>,
    video_appsrc: Option<gst_app::AppSrc>,
    audio_appsrc: Option<gst_app::AppSrc>,
    filesink: Option<gst::Element>,
    bus: Option<gst::Bus>,
    base_time: u64,
    first_video_time: u64,
    first_audio_time: u64,
    audio_started: bool,
    is_alac: bool,
    is_h265: bool,
    plan: PipelinePlan,
}

/// One owned MP4 mux recording session.
pub struct MuxRenderer {
    output_filename: Option<String>,
    file_count: i32,
    no_audio: bool,
    no_video: bool,
    audio_is_alac: bool,
    video_is_h265: bool,
    renderer: Option<ActivePipeline>,
    logger: LogCallback,
}

impl MuxRenderer {
    /// Initialize GStreamer and capture the source configuration.
    pub fn new(config: MuxRendererConfig) -> Result<Self, String> {
        Self::new_with_logger(config, Arc::new(|_, _| {}))
    }

    pub fn new_with_logger(config: MuxRendererConfig, logger: LogCallback) -> Result<Self, String> {
        gst::init().map_err(|error| format!("GStreamer initialization failed: {error}"))?;
        let no_audio = !config.use_audio;
        let no_video = !config.use_video;
        if no_audio && no_video {
            log(
                &logger,
                LogLevel::Info,
                "both audio and video rendering are disabled: nothing to record: (not starting mux renderer)",
            );
        } else if no_audio {
            log(
                &logger,
                LogLevel::Info,
                "audio rendering is disabled: video only will be recorded",
            );
        } else if no_video {
            log(
                &logger,
                LogLevel::Info,
                "video rendering is disabled: audio only will be recorded",
            );
        }
        let output_filename = if no_audio && no_video {
            None
        } else {
            log(
                &logger,
                LogLevel::Info,
                &format!("Mux renderer initialized: {}", config.output_filename),
            );
            Some(config.output_filename)
        };

        Ok(Self {
            output_filename,
            file_count: 0,
            no_audio,
            no_video,
            audio_is_alac: false,
            video_is_h265: false,
            renderer: None,
            logger,
        })
    }

    /// Mirrors `mux_renderer_choose_audio_codec`.
    pub fn choose_audio_codec(&mut self, audio_ct: u8) -> StartOutcome {
        if self.no_audio {
            return StartOutcome::Ignored;
        }
        self.audio_is_alac = audio_ct == 2;
        if self
            .renderer
            .as_ref()
            .is_some_and(|renderer| renderer.is_alac != self.audio_is_alac)
        {
            log(
                &self.logger,
                LogLevel::Debug,
                "Audio codec changed, recreating mux renderer",
            );
            self.destroy();
        }
        if audio_ct == 2 {
            self.start()
        } else {
            StartOutcome::Ignored
        }
    }

    /// Mirrors `mux_renderer_choose_video_codec`, including the source's
    /// `destroy()` reset if a running recording changes codec.
    pub fn choose_video_codec(&mut self, is_h265: bool) -> StartOutcome {
        self.video_is_h265 = is_h265;
        if self.renderer.as_ref().is_some_and(|renderer| {
            renderer.pipeline.is_some() && renderer.is_h265 != self.video_is_h265
        }) {
            log(
                &self.logger,
                LogLevel::Debug,
                "Video codec changed, recreating mux renderer",
            );
            self.destroy();
        }
        log(
            &self.logger,
            LogLevel::Debug,
            &format!(
                "Mux renderer video codec: h265={}",
                if is_h265 { "true" } else { "false" }
            ),
        );
        self.start()
    }

    /// Push an encoded video access unit with its original NTP nanosecond time.
    pub fn push_video(&mut self, data: &[u8], ntp_time: u64) -> PushOutcome {
        if self.no_video {
            return PushOutcome::Disabled;
        }
        let Some(renderer) = self.renderer.as_mut() else {
            return PushOutcome::NoPipeline;
        };
        let Some(appsrc) = renderer.video_appsrc.as_ref() else {
            return PushOutcome::SourceAbsent;
        };
        let Some(mut buffer) = make_buffer(data) else {
            return PushOutcome::NoPipeline;
        };
        if renderer.base_time == u64::MAX {
            renderer.base_time = ntp_time;
            renderer.first_video_time = ntp_time;
        }
        let pts = ntp_time.wrapping_sub(renderer.base_time);
        set_buffer_times(&mut buffer, pts, None);
        let _ = appsrc.push_buffer(buffer);
        PushOutcome::Submitted
    }

    /// Push an encoded audio packet and, when it starts after video, prepend
    /// the source's zero-filled 44.1 kHz stereo silence buffer.
    pub fn push_audio(&mut self, data: &[u8], ntp_time: u64) -> PushOutcome {
        if self.no_audio {
            return PushOutcome::Disabled;
        }
        let Some(renderer) = self.renderer.as_mut() else {
            return PushOutcome::NoPipeline;
        };
        let Some(appsrc) = renderer.audio_appsrc.as_ref() else {
            return PushOutcome::SourceAbsent;
        };

        if !renderer.audio_started && renderer.first_video_time != u64::MAX {
            renderer.audio_started = true;
            renderer.first_audio_time = ntp_time;
            if renderer.first_audio_time > renderer.first_video_time {
                let silence_duration = renderer
                    .first_audio_time
                    .wrapping_sub(renderer.first_video_time);
                let samples = silence_duration.wrapping_mul(44_100) / SECOND_IN_NSECS;
                let silence_size = samples.wrapping_mul(4);
                if let Ok(silence_len) = usize::try_from(silence_size)
                    && let Some(mut silence) = make_silence(silence_len)
                {
                    set_buffer_times(&mut silence, 0, Some(silence_duration));
                    let _ = appsrc.push_buffer(silence);
                    log(
                        &self.logger,
                        LogLevel::Debug,
                        &format!(
                            "Inserted {:.2} seconds of silence before audio",
                            silence_duration as f64 / SECOND_IN_NSECS as f64
                        ),
                    );
                }
            }
        }

        let Some(mut buffer) = make_buffer(data) else {
            return PushOutcome::NoPipeline;
        };
        if renderer.base_time == u64::MAX {
            renderer.base_time = ntp_time;
        }
        let pts = ntp_time.wrapping_sub(renderer.base_time);
        set_buffer_times(&mut buffer, pts, None);
        let _ = appsrc.push_buffer(buffer);
        PushOutcome::Submitted
    }

    /// Finish muxing, wait up to the C source's five-second EOS/error timeout,
    /// release the GStreamer objects, and reset global codec selection.
    pub fn stop(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if renderer.pipeline.is_none() {
            return;
        }

        if let Some(video) = &renderer.video_appsrc {
            let _ = video.end_of_stream();
        }
        if let Some(audio) = &renderer.audio_appsrc {
            let _ = audio.end_of_stream();
        }
        if let Some(bus) = &renderer.bus {
            let _ = bus.timed_pop_filtered(
                STOP_TIMEOUT,
                &[gst::MessageType::Eos, gst::MessageType::Error],
            );
        }
        if let Some(pipeline) = &renderer.pipeline {
            let _ = pipeline.set_state(gst::State::Null);
        }
        renderer.video_appsrc = None;
        renderer.audio_appsrc = None;
        renderer.filesink = None;
        renderer.bus = None;
        renderer.pipeline = None;
        renderer.base_time = u64::MAX;
        log(&self.logger, LogLevel::Info, "Stopped recording");
        self.audio_is_alac = false;
        self.video_is_h265 = false;
    }

    pub fn destroy(&mut self) {
        self.stop();
        self.renderer = None;
    }

    pub fn current_plan(&self) -> Option<&PipelinePlan> {
        self.renderer.as_ref().map(|renderer| &renderer.plan)
    }

    fn start(&mut self) -> StartOutcome {
        if self
            .renderer
            .as_ref()
            .is_some_and(|renderer| renderer.pipeline.is_some())
        {
            log(
                &self.logger,
                LogLevel::Debug,
                "Mux renderer already running",
            );
            return StartOutcome::AlreadyRunning;
        }
        self.destroy();

        self.file_count = self.file_count.wrapping_add(1);
        let plan = pipeline_plan(
            self.output_filename.as_deref().unwrap_or("(NULL)"),
            self.file_count,
            self.no_audio,
            self.no_video,
            self.audio_is_alac,
            self.video_is_h265,
        );
        log(
            &self.logger,
            LogLevel::Debug,
            &format!("created Mux pipeline: {}", plan.launch),
        );

        // Keep the C-shaped launch string in the plan for differential tests,
        // but set the path through GObject. `gst_parse_launch` treats Windows
        // backslashes in an unquoted `location=` value as escapes.
        let location_suffix = format!(" location={}", plan.filename);
        let Some(runtime_launch) = plan.launch.strip_suffix(&location_suffix) else {
            let message = "mux pipeline is missing its filesink location".to_owned();
            log(&self.logger, LogLevel::Error, &message);
            self.renderer = Some(ActivePipeline::failed(
                plan,
                self.audio_is_alac,
                self.video_is_h265,
            ));
            return StartOutcome::ParseFailed(message);
        };
        let parsed = gst::parse::launch(runtime_launch);
        let element = match parsed {
            Ok(element) => element,
            Err(error) => {
                log(
                    &self.logger,
                    LogLevel::Error,
                    &format!("Mux pipeline error: {}", error),
                );
                self.renderer = Some(ActivePipeline::failed(
                    plan,
                    self.audio_is_alac,
                    self.video_is_h265,
                ));
                return StartOutcome::ParseFailed(error.to_string());
            }
        };
        let pipeline = match element.downcast::<gst::Pipeline>() {
            Ok(pipeline) => pipeline,
            Err(_) => {
                let message = "mux parse did not produce a GstPipeline".to_owned();
                log(&self.logger, LogLevel::Error, &message);
                self.renderer = Some(ActivePipeline::failed(
                    plan,
                    self.audio_is_alac,
                    self.video_is_h265,
                ));
                return StartOutcome::ParseFailed(message);
            }
        };

        let video_appsrc = match setup_appsrc(&pipeline, "video_src", plan.video_caps) {
            Ok(appsrc) => appsrc,
            Err(error) => {
                log(&self.logger, LogLevel::Error, &error);
                let _ = pipeline.set_state(gst::State::Null);
                self.renderer = Some(ActivePipeline::failed(
                    plan,
                    self.audio_is_alac,
                    self.video_is_h265,
                ));
                return StartOutcome::ParseFailed(error);
            }
        };
        let audio_appsrc = match setup_appsrc(&pipeline, "audio_src", plan.audio_caps) {
            Ok(appsrc) => appsrc,
            Err(error) => {
                log(&self.logger, LogLevel::Error, &error);
                let _ = pipeline.set_state(gst::State::Null);
                self.renderer = Some(ActivePipeline::failed(
                    plan,
                    self.audio_is_alac,
                    self.video_is_h265,
                ));
                return StartOutcome::ParseFailed(error);
            }
        };
        let Some(filesink) = pipeline.by_name("filesink") else {
            let message = "mux pipeline has no filesink".to_owned();
            log(&self.logger, LogLevel::Error, &message);
            let _ = pipeline.set_state(gst::State::Null);
            self.renderer = Some(ActivePipeline::failed(
                plan,
                self.audio_is_alac,
                self.video_is_h265,
            ));
            return StartOutcome::ParseFailed(message);
        };
        filesink.set_property("location", &plan.filename);
        let Some(bus) = pipeline.bus() else {
            let message = "mux pipeline has no bus".to_owned();
            log(&self.logger, LogLevel::Error, &message);
            let _ = pipeline.set_state(gst::State::Null);
            self.renderer = Some(ActivePipeline::failed(
                plan,
                self.audio_is_alac,
                self.video_is_h265,
            ));
            return StartOutcome::ParseFailed(message);
        };
        let is_alac = self.audio_is_alac;
        let is_h265 = self.video_is_h265;
        self.renderer = Some(ActivePipeline {
            pipeline: Some(pipeline.clone()),
            video_appsrc,
            audio_appsrc,
            filesink: Some(filesink),
            bus: Some(bus),
            base_time: u64::MAX,
            first_video_time: u64::MAX,
            first_audio_time: u64::MAX,
            audio_started: false,
            is_alac,
            is_h265,
            plan: plan.clone(),
        });
        let _ = pipeline.set_state(gst::State::Playing);
        log(
            &self.logger,
            LogLevel::Info,
            &format!("Started recording to: {}", plan.filename),
        );
        StartOutcome::Started(plan)
    }
}

impl Drop for MuxRenderer {
    fn drop(&mut self) {
        self.destroy();
    }
}

impl ActivePipeline {
    fn failed(plan: PipelinePlan, is_alac: bool, is_h265: bool) -> Self {
        // Keep the failed session's plan and codec values so codec changes
        // follow the same destroy/restart path as the source's allocated but
        // pipeline-less renderer.
        Self {
            pipeline: None,
            video_appsrc: None,
            audio_appsrc: None,
            filesink: None,
            bus: None,
            base_time: u64::MAX,
            first_video_time: u64::MAX,
            first_audio_time: u64::MAX,
            audio_started: false,
            is_alac,
            is_h265,
            plan,
        }
    }
}

fn setup_appsrc(
    pipeline: &gst::Pipeline,
    name: &str,
    caps_text: Option<&'static str>,
) -> Result<Option<gst_app::AppSrc>, String> {
    let Some(caps_text) = caps_text else {
        return Ok(None);
    };
    let Some(element) = pipeline.by_name(name) else {
        return Err(format!("mux pipeline has no {name} appsrc"));
    };
    let appsrc = element
        .downcast::<gst_app::AppSrc>()
        .map_err(|_| format!("mux pipeline element {name} is not appsrc"))?;
    let caps = gst::Caps::from_str(caps_text)
        .map_err(|error| format!("invalid mux caps for {name}: {error}"))?;
    appsrc.set_caps(Some(&caps));
    appsrc.set_property("is-live", true);
    appsrc.set_property("format", gst::Format::Time);
    Ok(Some(appsrc))
}

fn make_buffer(data: &[u8]) -> Option<gst::Buffer> {
    let mut buffer = gst::Buffer::with_size(data.len()).ok()?;
    let buffer_ref = buffer.get_mut()?;
    let mut map = buffer_ref.map_writable().ok()?;
    map.copy_from_slice(data);
    drop(map);
    Some(buffer)
}

fn make_silence(size: usize) -> Option<gst::Buffer> {
    let mut buffer = gst::Buffer::with_size(size).ok()?;
    {
        let buffer_ref = buffer.get_mut()?;
        let mut map = buffer_ref.map_writable().ok()?;
        map.as_mut_slice().fill(0);
    }
    Some(buffer)
}

fn set_buffer_times(buffer: &mut gst::Buffer, pts: u64, duration: Option<u64>) {
    let Some(buffer_ref) = buffer.get_mut() else {
        return;
    };
    let pts = gst::ClockTime::from_nseconds(pts);
    buffer_ref.set_pts(pts);
    buffer_ref.set_dts(pts);
    if let Some(duration) = duration {
        buffer_ref.set_duration(gst::ClockTime::from_nseconds(duration));
    }
}

fn log(logger: &LogCallback, level: LogLevel, message: &str) {
    logger(level, message);
}

/// The generated C-compatible entry points use this process-local state. The
/// Rust application should prefer an owned `MuxRenderer` instance.
static GLOBAL_RENDERER: OnceLock<Mutex<Option<MuxRenderer>>> = OnceLock::new();

fn global_renderer() -> &'static Mutex<Option<MuxRenderer>> {
    GLOBAL_RENDERER.get_or_init(|| Mutex::new(None))
}

pub fn install_global(renderer: MuxRenderer) {
    if let Ok(mut slot) = global_renderer().lock() {
        *slot = Some(renderer);
    }
}

pub fn mux_renderer_choose_audio_codec(audio_ct: std::ffi::c_uchar) {
    if let Ok(mut slot) = global_renderer().lock()
        && let Some(renderer) = slot.as_mut()
    {
        let _ = renderer.choose_audio_codec(audio_ct);
    }
}

pub fn mux_renderer_choose_video_codec(is_h265: bool) {
    if let Ok(mut slot) = global_renderer().lock()
        && let Some(renderer) = slot.as_mut()
    {
        let _ = renderer.choose_video_codec(is_h265);
    }
}

pub fn mux_renderer_stop() {
    if let Ok(mut slot) = global_renderer().lock()
        && let Some(renderer) = slot.as_mut()
    {
        renderer.stop();
    }
}

pub fn mux_renderer_destroy() {
    if let Ok(mut slot) = global_renderer().lock() {
        if let Some(renderer) = slot.as_mut() {
            renderer.destroy();
        }
        *slot = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h264_aac_pipeline_matches_source_string_and_names() {
        let plan = pipeline_plan("capture", 1, false, false, false, false);
        assert_eq!(plan.filename, "capture.1.H264.AAC.mp4");
        assert_eq!(
            plan.launch,
            "appsrc name=video_src format=time is-live=true ! queue ! h264parse ! mux. appsrc name=audio_src format=time is-live=true ! queue ! aacparse ! queue ! mux. mp4mux name=mux fragment-duration=2000 fragment-mode=first-moov-then-finalise ! filesink name=filesink location=capture.1.H264.AAC.mp4"
        );
        assert_eq!(plan.video_caps, Some(H264_CAPS));
        assert_eq!(plan.audio_caps, Some(AAC_ELD_CAPS));
        assert!(plan.has_video_source && plan.has_audio_source);
    }

    #[test]
    fn h265_video_only_uses_the_selected_parser_and_suffix() {
        let plan = pipeline_plan("/tmp/out", 3, true, false, false, true);
        assert_eq!(plan.filename, "/tmp/out.3.H265.mp4");
        assert!(plan.launch.contains("h265parse ! mux."));
        assert_eq!(plan.video_caps, Some(H265_CAPS));
        assert_eq!(plan.audio_caps, None);
        assert!(plan.has_video_source && !plan.has_audio_source);
    }

    #[test]
    fn alac_source_condition_suppresses_video_even_when_enabled() {
        let plan = pipeline_plan("capture", 2, false, false, true, true);
        assert_eq!(plan.filename, "capture.2.ALAC.mp4");
        assert!(!plan.has_video_source);
        assert!(plan.has_audio_source);
        assert_eq!(plan.video_caps, None);
        assert_eq!(plan.audio_caps, Some(ALAC_CAPS));
        assert!(!plan.launch.contains("video_src"));
        assert!(!plan.launch.contains("aacparse"));
    }

    #[test]
    fn disabled_audio_and_video_still_build_an_empty_mux_pipeline_on_start() {
        let plan = pipeline_plan("capture", 1, true, true, false, false);
        assert_eq!(plan.filename, "capture.1.mp4");
        assert_eq!(
            plan.launch,
            "mp4mux name=mux fragment-duration=2000 fragment-mode=first-moov-then-finalise ! filesink name=filesink location=capture.1.mp4"
        );
        assert!(!plan.has_video_source && !plan.has_audio_source);
    }

    #[test]
    fn codec_caps_keep_source_strings() {
        assert!(AAC_ELD_CAPS.ends_with("codec_data=(buffer)f8e85000"));
        assert!(ALAC_CAPS.ends_with("000000000000ac44"));
        assert!(H264_CAPS.contains("alignment=(string)au"));
        assert!(H265_CAPS.contains("stream-format=(string)byte-stream"));
    }

    #[test]
    fn c_clock_subtraction_wraps_for_earlier_packet_times() {
        assert_eq!(3_u64.wrapping_sub(5), u64::MAX - 1);
        assert_eq!(7_u64.wrapping_mul(44_100), 308_700);
    }

    #[test]
    fn appsrc_pts_dts_and_audio_leading_silence_match_the_c_oracle() {
        let trace = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut renderer = MuxRenderer::new(MuxRendererConfig::new("mux-timestamps", true, true))
            .expect("GStreamer init");
        assert!(matches!(
            renderer.choose_video_codec(false),
            StartOutcome::Started(_)
        ));

        let active = renderer.renderer.as_ref().expect("started pipeline");
        let video_pad = active
            .video_appsrc
            .as_ref()
            .expect("video appsrc")
            .static_pad("src")
            .expect("video appsrc source pad");
        let audio_pad = active
            .audio_appsrc
            .as_ref()
            .expect("audio appsrc")
            .static_pad("src")
            .expect("audio appsrc source pad");

        let video_trace = Arc::clone(&trace);
        video_pad
            .add_probe(gst::PadProbeType::BUFFER, move |_, info| {
                if let Some(buffer) = info.buffer() {
                    let line = format!(
                        "TRACE|V|{}|{}|{}",
                        trace_time(buffer.pts()),
                        trace_time(buffer.dts()),
                        trace_time(buffer.duration()),
                    );
                    video_trace
                        .lock()
                        .expect("test trace mutex is not poisoned")
                        .push(line);
                }
                gst::PadProbeReturn::Ok
            })
            .expect("install video trace probe");
        let audio_trace = Arc::clone(&trace);
        audio_pad
            .add_probe(gst::PadProbeType::BUFFER, move |_, info| {
                if let Some(buffer) = info.buffer() {
                    let line = format!(
                        "TRACE|A|{}|{}|{}",
                        trace_time(buffer.pts()),
                        trace_time(buffer.dts()),
                        trace_time(buffer.duration()),
                    );
                    audio_trace
                        .lock()
                        .expect("test trace mutex is not poisoned")
                        .push(line);
                }
                gst::PadProbeReturn::Ok
            })
            .expect("install audio trace probe");

        let video = [0, 0, 0, 1, 0x65, 0x88, 0x84];
        let audio = [0x21, 0x10, 0x56, 0xe5];
        assert_eq!(
            renderer.push_video(&video, 1_000_000_000),
            PushOutcome::Submitted
        );
        assert_eq!(
            renderer.push_audio(&audio, 1_500_000_000),
            PushOutcome::Submitted
        );
        assert_eq!(
            renderer.push_video(&video, 2_250_000_000),
            PushOutcome::Submitted
        );
        renderer.stop();

        let mut actual = trace
            .lock()
            .expect("test trace mutex is not poisoned")
            .clone();
        actual.sort();
        let mut expected = include_str!("../record_shims/mux_renderer_oracle.expected.txt")
            .lines()
            .filter(|line| line.starts_with("TRACE|"))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(actual, expected);

        renderer.destroy();
        let filename = "mux-timestamps.1.H264.AAC.mp4";
        if std::path::Path::new(filename).exists() {
            std::fs::remove_file(filename).expect("remove generated oracle artifact");
        }
    }

    fn trace_time(time: Option<gst::ClockTime>) -> String {
        time.map_or_else(|| "NONE".to_owned(), |time| time.nseconds().to_string())
    }
}
