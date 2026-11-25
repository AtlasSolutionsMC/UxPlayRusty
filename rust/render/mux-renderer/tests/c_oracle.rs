use std::sync::{Arc, Mutex};

use mux_renderer_rs::port::{
    LogLevel, MuxRenderer, MuxRendererConfig, StartOutcome, pipeline_plan,
};

fn run_scenario(name: &str, filename: &str) -> Vec<String> {
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let capture = Arc::clone(&lines);
    let logger = Arc::new(move |level: LogLevel, message: &str| {
        capture
            .lock()
            .expect("test logger mutex is not poisoned")
            .push(format!("{}|{message}", level as i32));
    });
    let config = match name {
        "h264-aac" | "codec-switch" => MuxRendererConfig::new(filename, true, true),
        "h265-video" => MuxRendererConfig::new(filename, false, true),
        "alac-video-config" => MuxRendererConfig::new(filename, true, true),
        "both-disabled" => MuxRendererConfig::new(filename, false, false),
        _ => panic!("unexpected test scenario"),
    };
    let mut renderer = MuxRenderer::new_with_logger(config, logger).expect("GStreamer init");
    match name {
        "h264-aac" => assert!(matches!(
            renderer.choose_video_codec(false),
            StartOutcome::Started(_)
        )),
        "h265-video" => assert!(matches!(
            renderer.choose_video_codec(true),
            StartOutcome::Started(_)
        )),
        "alac-video-config" => assert!(matches!(
            renderer.choose_audio_codec(2),
            StartOutcome::Started(_)
        )),
        "codec-switch" => {
            assert!(matches!(
                renderer.choose_video_codec(true),
                StartOutcome::Started(_)
            ));
            assert!(matches!(
                renderer.choose_audio_codec(2),
                StartOutcome::Started(_)
            ));
        }
        "both-disabled" => assert!(matches!(
            renderer.choose_video_codec(false),
            StartOutcome::Started(_)
        )),
        _ => unreachable!(),
    }
    renderer.stop();
    renderer.destroy();
    drop(renderer);
    lines
        .lock()
        .expect("test logger mutex is not poisoned")
        .clone()
}

#[test]
fn renderer_lifecycle_logs_match_the_unchanged_c_oracle() {
    let scenarios = [
        ("h264-aac", "mux-h264-aac"),
        ("h265-video", "mux-h265-video"),
        ("alac-video-config", "mux-alac-video-config"),
        ("codec-switch", "mux-codec-switch"),
        ("both-disabled", "mux-both-disabled"),
    ];
    let mut actual = Vec::new();
    for (name, filename) in scenarios {
        actual.push(format!("CASE|{name}"));
        actual.extend(run_scenario(name, filename));
    }

    let expected = include_str!("../record_shims/mux_renderer_oracle.expected.txt")
        .lines()
        .take_while(|line| *line != "CASE|timestamps")
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);

    for filename in [
        "mux-h264-aac.1.H264.AAC.mp4",
        "mux-h265-video.1.H265.mp4",
        "mux-alac-video-config.1.ALAC.mp4",
        "mux-codec-switch.1.H265.AAC.mp4",
        "mux-codec-switch.2.H264.AAC.mp4",
        "(NULL).1.mp4",
    ] {
        let path = std::path::Path::new(filename);
        if path.exists() {
            std::fs::remove_file(path).expect("remove generated oracle artifact");
        }
    }
}

#[test]
fn pipeline_plans_match_recorded_launch_and_caps_scenarios() {
    let plan = pipeline_plan("mux-codec-switch", 2, false, false, false, false);
    assert!(plan.launch.contains("h264parse ! mux."));
    assert!(plan.launch.contains("aacparse ! queue ! mux."));
    assert_eq!(plan.filename, "mux-codec-switch.2.H264.AAC.mp4");
}
