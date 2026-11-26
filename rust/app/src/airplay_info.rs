//! Stateless portions of the HTTP `/server-info` and `/playback-info` handlers.
//!
//! The C handlers live in `lib/http_handlers.h`. `/server-info` depends on the
//! configured DNS-SD hardware address, which is passed in here. `/playback-info`
//! depends on the renderer callback; this module accepts its snapshot and
//! preserves the source's unavailable/finished response effects.

const PLIST_HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n";
const PLIST_FOOTER: &str = "</plist>\n";

pub const PLIST_CONTENT_TYPE: &[u8] = b"text/x-apple-plist+xml";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaybackSnapshot {
    pub duration: f64,
    pub position: f64,
    pub seek_start: f64,
    pub seek_duration: f64,
    pub rate: f32,
    pub ready_to_play: bool,
    pub playback_buffer_empty: bool,
    pub playback_buffer_full: bool,
    pub playback_likely_to_keep_up: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlaybackInfoReply {
    /// The C handler closes the connection and calls `video_reset` when the
    /// renderer says duration is exactly `-1.0`.
    Finished,
    /// The C handler sends the default empty 200 response when position is -1.
    Unavailable,
    /// A normal renderer snapshot produces a plist body and Content-Type.
    Available(Vec<u8>),
}

/// Format `/server-info`'s device identifier in the lowercase, colon-separated
/// form produced by `utils_hwaddr_airplay`.
pub fn airplay_hardware_address(raw: &[u8]) -> String {
    let mut result = String::with_capacity(raw.len().saturating_mul(3).saturating_sub(1));
    for (index, byte) in raw.iter().enumerate() {
        if index != 0 {
            result.push(':');
        }
        use std::fmt::Write as _;
        // Writing into a String is infallible.
        let _ = write!(result, "{byte:02x}");
    }
    result
}

/// Serialize the exact plist dictionary shape created by
/// `http_handler_server_info` in `lib/http_handlers.h`.
pub fn server_info_plist_xml(raw_hardware_address: &[u8]) -> Vec<u8> {
    let address = airplay_hardware_address(raw_hardware_address);
    let mut xml = String::from(PLIST_HEADER);
    xml.push_str("<dict>\n");
    append_integer(&mut xml, "features", 0x27f);
    append_string(&mut xml, "macAddress", &address);
    append_string(&mut xml, "model", "AppleTV3,2");
    append_string(&mut xml, "osBuildVersion", "12B435");
    append_string(&mut xml, "protovers", "1.0");
    append_string(&mut xml, "srcvers", "220.68");
    append_integer(&mut xml, "vv", 2);
    append_string(&mut xml, "deviceid", &address);
    xml.push_str("</dict>\n");
    xml.push_str(PLIST_FOOTER);
    xml.into_bytes()
}

/// Apply the source handler's callback-result rules and serialize its plist
/// when playback is available.
pub fn playback_info_reply(snapshot: PlaybackSnapshot) -> PlaybackInfoReply {
    if snapshot.duration == -1.0 {
        return PlaybackInfoReply::Finished;
    }
    if snapshot.position == -1.0 {
        return PlaybackInfoReply::Unavailable;
    }
    PlaybackInfoReply::Available(playback_info_plist_xml(snapshot))
}

/// Serialize the fields and one-element time ranges added by
/// `http_handler_playback_info` before calling `plist_to_xml`.
pub fn playback_info_plist_xml(snapshot: PlaybackSnapshot) -> Vec<u8> {
    let mut xml = String::from(PLIST_HEADER);
    xml.push_str("<dict>\n");
    append_real(&mut xml, "duration", snapshot.duration);
    append_real(&mut xml, "position", snapshot.position);
    append_real(&mut xml, "rate", f64::from(snapshot.rate));
    append_integer(&mut xml, "readyToPlay", u64::from(snapshot.ready_to_play));
    append_integer(
        &mut xml,
        "playbackBufferEmpty",
        u64::from(snapshot.playback_buffer_empty),
    );
    append_integer(
        &mut xml,
        "playbackBufferFull",
        u64::from(snapshot.playback_buffer_full),
    );
    append_integer(
        &mut xml,
        "playbackLikelyToKeepUp",
        u64::from(snapshot.playback_likely_to_keep_up),
    );
    append_time_ranges(
        &mut xml,
        "loadedTimeRanges",
        &[(snapshot.position, snapshot.duration - snapshot.position)],
    );
    append_time_ranges(
        &mut xml,
        "seekableTimeRanges",
        &[(snapshot.seek_start, snapshot.seek_duration)],
    );
    xml.push_str("</dict>\n");
    xml.push_str(PLIST_FOOTER);
    xml.into_bytes()
}

fn append_integer(xml: &mut String, key: &str, value: u64) {
    append_key(xml, key);
    xml.push_str(&format!("\t<integer>{value}</integer>\n"));
}

fn append_real(xml: &mut String, key: &str, value: f64) {
    append_key(xml, key);
    let value = format_real(value);
    xml.push_str(&format!("\t<real>{value}</real>\n"));
}

fn append_string(xml: &mut String, key: &str, value: &str) {
    append_key(xml, key);
    xml.push_str("\t<string>");
    append_escaped(xml, value);
    xml.push_str("</string>\n");
}

fn append_key(xml: &mut String, key: &str) {
    xml.push_str("\t<key>");
    append_escaped(xml, key);
    xml.push_str("</key>\n");
}

fn append_time_ranges(xml: &mut String, key: &str, ranges: &[(f64, f64)]) {
    append_key(xml, key);
    xml.push_str("\t<array>\n");
    for &(start, duration) in ranges {
        xml.push_str("\t\t<dict>\n");
        xml.push_str("\t\t\t<key>duration</key>\n");
        xml.push_str(&format!("\t\t\t<real>{}</real>\n", format_real(duration)));
        xml.push_str("\t\t\t<key>start</key>\n");
        xml.push_str(&format!("\t\t\t<real>{}</real>\n", format_real(start)));
        xml.push_str("\t\t</dict>\n");
    }
    xml.push_str("\t</array>\n");
}

fn append_escaped(xml: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => xml.push_str("&amp;"),
            '<' => xml.push_str("&lt;"),
            '>' => xml.push_str("&gt;"),
            '\'' => xml.push_str("&apos;"),
            '"' => xml.push_str("&quot;"),
            _ => xml.push(character),
        }
    }
}

fn format_real(value: f64) -> String {
    // libplist emits zero real values with a decimal suffix, while integral
    // nonzero reals such as 60.0 and 1.0 are written without one.
    if value == 0.0 {
        if value.is_sign_negative() {
            "-0.0".to_owned()
        } else {
            "0.0".to_owned()
        }
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const C_PLIST_ORACLE: &str =
        include_str!("../../../tools/record-shims/http_info_plist_oracle.expected.txt");

    fn oracle_section(name: &str) -> String {
        let normalized = C_PLIST_ORACLE.replace("\r\n", "\n");
        let section = normalized
            .split_once(&format!("--{name}--\n"))
            .expect("oracle section")
            .1;
        section
            .split_once("--")
            .map_or(section, |(body, _)| body)
            .to_owned()
    }

    fn normal_snapshot() -> PlaybackSnapshot {
        PlaybackSnapshot {
            duration: 60.0,
            position: 12.5,
            seek_start: 0.0,
            seek_duration: 60.0,
            rate: 1.0,
            ready_to_play: true,
            playback_buffer_empty: false,
            playback_buffer_full: false,
            playback_likely_to_keep_up: true,
        }
    }

    #[test]
    fn formats_hardware_address_as_lowercase_airplay_hex() {
        assert_eq!(
            airplay_hardware_address(&[0x02, 0x00, 0x00, 0xa5, 0xc4, 0xb8]),
            "02:00:00:a5:c4:b8"
        );
    }

    #[test]
    fn server_info_body_matches_libplist_c_oracle() {
        let body = server_info_plist_xml(&[0x02, 0x00, 0x00, 0xa5, 0xc4, 0xb8]);
        assert_eq!(
            String::from_utf8(body).unwrap(),
            oracle_section("server-info")
        );
        assert_eq!(PLIST_CONTENT_TYPE, b"text/x-apple-plist+xml");
    }

    #[test]
    fn playback_info_body_matches_libplist_c_oracle() {
        let PlaybackInfoReply::Available(body) = playback_info_reply(normal_snapshot()) else {
            panic!("ordinary playback snapshot should serialize")
        };
        assert_eq!(
            String::from_utf8(body).unwrap(),
            oracle_section("playback-info")
        );
    }

    #[test]
    fn playback_info_preserves_source_unavailable_effects() {
        let mut snapshot = normal_snapshot();
        snapshot.duration = -1.0;
        assert_eq!(playback_info_reply(snapshot), PlaybackInfoReply::Finished);

        snapshot.duration = 60.0;
        snapshot.position = -1.0;
        assert_eq!(
            playback_info_reply(snapshot),
            PlaybackInfoReply::Unavailable
        );
    }

    #[test]
    fn playback_xml_escapes_string_values() {
        let mut xml = String::new();
        append_string(&mut xml, "test&key", "<tag> \"quoted\" & 'quoted'");
        assert_eq!(
            xml,
            "\t<key>test&amp;key</key>\n\t<string>&lt;tag&gt; &quot;quoted&quot; &amp; &apos;quoted&apos;</string>\n"
        );
    }
}
