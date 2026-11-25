use plist::Value;
use std::io::Cursor;
use uxplay_airplay_http_handlers::{
    HandlerResponse, HlsPlan, extract_media_playlist_uris, fairplay_setup2, filter_master_playlist,
    finish_hls, get_property, plan_hls, reverse, set_property,
};

const FIXTURES: &str = include_str!("../fixtures/c-oracles.tsv");

fn fixture(name: &str) -> (&'static str, &'static str) {
    FIXTURES
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .find_map(|line| {
            let mut columns = line.splitn(4, '|');
            let found_name = columns.next()?;
            let _input = columns.next()?;
            let source = columns.next()?;
            let behavior = columns.next()?;
            (found_name == name).then_some((source, behavior))
        })
        .unwrap_or_else(|| panic!("missing C oracle fixture {name}"))
}

#[test]
fn deterministic_handler_results_match_source_oracle_fixtures() {
    let (_, expected) = fixture("set_property_known");
    let known = set_property("/setProperty?actionAtItemEnd");
    assert!(expected.contains("default status"));
    assert_eq!(known.status, None);
    assert_eq!(known.headers[0].0, "Content-Type");
    assert_eq!(known.headers[0].1, "text/x-apple-plist+xml");
    let plist = Value::from_reader_xml(Cursor::new(known.body)).unwrap();
    assert_eq!(
        plist.as_dictionary().unwrap()["errorCode"].as_signed_integer(),
        Some(0)
    );

    let (_, expected) = fixture("set_property_unknown");
    let unknown = set_property("/setProperty?unknown");
    assert!(expected.contains("Content-Length=0"));
    assert_eq!(unknown.status, None);
    assert_eq!(unknown.headers, [("Content-Length".into(), "0".into())]);

    let (_, expected) = fixture("get_property");
    assert!(expected.contains("unchanged"));
    assert_eq!(
        get_property("/getProperty?anything"),
        HandlerResponse::default()
    );
}

#[test]
fn response_reinitialization_and_reverse_count_match_source_oracles() {
    let (_, expected) = fixture("fp_setup2");
    let fp = fairplay_setup2();
    assert!(expected.contains("no headers"));
    assert_eq!(fp.status, Some((421, "Misdirected Request")));
    assert!(fp.headers.is_empty());

    let (_, expected) = fixture("reverse_first");
    let first = reverse(1);
    assert!(expected.contains("101 Switching Protocols"));
    assert_eq!(first.response.status, Some((101, "Switching Protocols")));
    assert_eq!(
        first.response.headers,
        [
            ("Connection".into(), "Upgrade".into()),
            ("Upgrade".into(), "PTTH/1.0".into()),
        ]
    );

    let (_, expected) = fixture("reverse_second");
    let second = reverse(2);
    assert!(expected.contains("default response unchanged"));
    assert_eq!(second.response, HandlerResponse::default());
}

#[test]
fn hls_response_mutations_match_source_oracles() {
    let (_, expected) = fixture("hls_no_video");
    let no_video = plan_hls(None, false, None, "/master.m3u8");
    assert!(expected.contains("404 Not Found"));
    assert_eq!(
        finish_hls(&no_video, None, "date"),
        HandlerResponse {
            status: Some((404, "Not Found")),
            ..HandlerResponse::default()
        }
    );

    let (_, expected) = fixture("hls_missing_playlist");
    assert!(expected.contains("CORS headers and Date"));
    let missing_playlist = plan_hls(Some(0), false, None, "/master.m3u8");
    assert_eq!(
        finish_hls(&missing_playlist, None, "date"),
        HandlerResponse {
            status: Some((404, "Not Found")),
            headers: vec![
                ("Access-Control-Allow-Headers".into(), "Content-type".into()),
                ("Access-Control-Allow-Origin".into(), "*".into()),
                ("Date".into(), "date".into()),
            ],
            ..HandlerResponse::default()
        }
    );

    let (_, expected) = fixture("hls_master");
    let master = plan_hls(Some(0), true, None, "/master.m3u8");
    assert!(expected.contains("CORS headers"));
    assert_eq!(master, HlsPlan::FetchMaster);
    let response = finish_hls(&master, Some(b"#EXTM3U\n".to_vec()), "date");
    assert_eq!(response.status, None);
    assert!(response.headers.iter().any(|(name, value)| {
        name == "Content-Type" && value == "application/x-mpegURL; charset=utf-8"
    }));
    assert!(
        response
            .headers
            .iter()
            .any(|(name, value)| name == "Access-Control-Allow-Origin" && value == "*")
    );
}

#[test]
fn custom_master_profile_matches_source_derived_filter_fixture() {
    let (_, expected) = fixture("filter_master_avc_profile");
    let master = b"#EXTM3U\n#EXT-X-STREAM-INF:RESOLUTION=1920x1080,CODECS=\"avc1.640028\"\nhigh.m3u8\n#EXT-X-STREAM-INF:RESOLUTION=1280x720,CODECS=\"avc1.640028\"\nlow.m3u8\n";
    assert_eq!(
        filter_master_playlist(master, "AVC:720"),
        expected.replace("\\n", "\n").as_bytes()
    );
}

#[test]
fn incomplete_trailing_master_uri_matches_source_table_failure() {
    let (_, expected) = fixture("master_uri_trailing_incomplete");
    assert!(expected.contains("num_uri=0"));
    let master = b"#EXTM3U\nhttps://upstream/video/valid.m3u8\nhttps://upstream/video/incomplete\n";
    assert!(extract_media_playlist_uris(master, b"https://upstream/video").is_empty());
}

#[test]
fn fixture_provenance_is_present_for_every_oracle() {
    let fixtures: Vec<_> = FIXTURES
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .collect();
    assert_eq!(fixtures.len(), 11);
    for line in fixtures {
        let columns: Vec<_> = line.splitn(4, '|').collect();
        assert_eq!(columns.len(), 4);
        assert!(
            columns[2].starts_with("http_handlers.h:")
                || columns[2].starts_with("airplay_video.c:")
        );
        assert!(!columns[3].is_empty());
    }
}
