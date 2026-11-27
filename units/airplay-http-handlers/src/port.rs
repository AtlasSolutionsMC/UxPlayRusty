use plist::Value;
use std::io::Cursor;

const PROPERTY_CONTENT_TYPE: &str = "text/x-apple-plist+xml";
const HLS_CONTENT_TYPE: &str = "application/x-mpegURL; charset=utf-8";
const BINARY_PLIST_MARKER: &[u8] = b"apple-binary-plist";

/// The changes a C handler makes to the response initialized by `raop.c`.
/// `status: None` means the handler leaves the default response untouched.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HandlerResponse {
    pub status: Option<(u16, &'static str)>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub disconnect: bool,
}

/// Result of `PUT /setProperty?...`.
pub fn set_property(url: &str) -> HandlerResponse {
    set_property_bytes(url.as_bytes())
}

/// Byte-oriented form matching the C handler's NUL-terminated URL access.
pub fn set_property_bytes(url: &[u8]) -> HandlerResponse {
    let url = c_string_prefix_bytes(url);
    let property = url
        .strip_prefix(b"/setProperty?")
        .unwrap_or_else(|| url.get(b"/setProperty?".len()..).unwrap_or_default());
    if is_known_unhandled_property(property) {
        let mut response = HandlerResponse::default();
        response
            .headers
            .push(("Content-Type".to_owned(), PROPERTY_CONTENT_TYPE.to_owned()));
        response.body = property_error_plist();
        response
    } else {
        HandlerResponse {
            headers: vec![("Content-Length".to_owned(), "0".to_owned())],
            ..HandlerResponse::default()
        }
    }
}

fn is_known_unhandled_property(property: &[u8]) -> bool {
    matches!(
        property,
        b"actionAtItemEnd"
            | b"selectedMediaArray"
            | b"reverseEndTime"
            | b"forwardEndTime"
            | b"interstitialEvents"
            | b"textMarkupArray"
            | b"isInterestedInDateRange"
    )
}

fn c_string_prefix_bytes(value: &[u8]) -> &[u8] {
    let end = value
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(value.len());
    &value[..end]
}

fn property_error_plist() -> Vec<u8> {
    let mut root = plist::Dictionary::new();
    root.insert("errorCode".to_owned(), Value::Integer(0.into()));
    let mut xml = Vec::new();
    // A one-entry plist dictionary is always serializable.
    Value::Dictionary(root)
        .to_writer_xml(&mut xml)
        .expect("in-memory property error plist serialization cannot fail");
    xml
}

/// The source GET handler only logs the requested property and leaves the
/// default response unchanged.
pub fn get_property(_url: &str) -> HandlerResponse {
    HandlerResponse::default()
}

/// The source adds Content-Type and then reinitializes the response to 421.
/// `http_response_init` clears the already-added header, so the wire response
/// has no Content-Type header.
pub fn fairplay_setup2() -> HandlerResponse {
    HandlerResponse {
        status: Some((421, "Misdirected Request")),
        ..HandlerResponse::default()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReverseEffect {
    MarkThisConnectionAsEvent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReversePlan {
    pub response: HandlerResponse,
    pub effect: ReverseEffect,
}

/// `event_connections_after_mark` is the connection count returned by
/// `httpd_count_connection_type` after this connection has been marked EVENT.
pub fn reverse(event_connections_after_mark: usize) -> ReversePlan {
    let response = if event_connections_after_mark == 1 {
        HandlerResponse {
            status: Some((101, "Switching Protocols")),
            headers: vec![
                ("Connection".to_owned(), "Upgrade".to_owned()),
                ("Upgrade".to_owned(), "PTTH/1.0".to_owned()),
            ],
            ..HandlerResponse::default()
        }
    } else {
        HandlerResponse::default()
    };
    ReversePlan {
        response,
        effect: ReverseEffect::MarkThisConnectionAsEvent,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionPlan {
    /// Callback-dependent mutation: clear current_video, invoke
    /// on_video_playlist_remove, and save its returned resume position while
    /// retaining the playlist for a future UUID reuse.
    RemovePlaylist {
        uuid: String,
    },
    /// `playlistInsert` is recognized but remains an unimplemented no-op in C.
    InsertPlaylist {
        uuid: Option<String>,
    },
    /// Apply the playlist to HLS storage, then either send the next FCUP request
    /// or invoke on_video_play once all media playlists have arrived.
    FcupPlaylist {
        url: String,
        is_master: bool,
        playlist: Vec<u8>,
    },
    /// Unknown action types are logged and otherwise leave the response alone.
    IgnoreUnknown {
        action_type: String,
    },
    BadRequest,
}

/// Parse and validate the state-independent portion of `POST /action`.
/// The C handler checks a case-sensitive substring in the complete header
/// block, exact session-ID equality, and a binary plist dictionary.
pub fn action(
    current_video_session_id: Option<&str>,
    session_header: Option<&str>,
    full_header_block: &[u8],
    body: &[u8],
) -> ActionPlan {
    let (Some(current_session), Some(session)) = (current_video_session_id, session_header) else {
        return ActionPlan::BadRequest;
    };
    if current_session != session
        || !contains_bytes(full_header_block, BINARY_PLIST_MARKER)
        || !body.starts_with(b"bplist00")
        || body.is_empty()
    {
        return ActionPlan::BadRequest;
    }

    let Ok(root) = Value::from_reader(Cursor::new(body)) else {
        return ActionPlan::BadRequest;
    };
    let Some(root) = root.as_dictionary() else {
        return ActionPlan::BadRequest;
    };
    let Some(action_type) = root.get("type").and_then(Value::as_string) else {
        return ActionPlan::BadRequest;
    };
    let action_type = c_string_prefix(action_type);
    let Some(params) = root.get("params").and_then(Value::as_dictionary) else {
        return ActionPlan::BadRequest;
    };

    match action_type {
        "playlistRemove" => {
            let Some(item) = params.get("item").and_then(Value::as_dictionary) else {
                return ActionPlan::BadRequest;
            };
            let Some(uuid) = item.get("uuid").and_then(Value::as_string) else {
                return ActionPlan::BadRequest;
            };
            ActionPlan::RemovePlaylist {
                uuid: c_string_prefix(uuid).to_owned(),
            }
        }
        "playlistInsert" => {
            let Some(item) = params.get("item").and_then(Value::as_dictionary) else {
                return ActionPlan::BadRequest;
            };
            ActionPlan::InsertPlaylist {
                uuid: item
                    .get("uuid")
                    .and_then(Value::as_string)
                    .map(|value| c_string_prefix(value).to_owned()),
            }
        }
        "unhandledURLResponse" => {
            let Some(url) = params.get("FCUP_Response_URL").and_then(Value::as_string) else {
                return ActionPlan::BadRequest;
            };
            let Some(data) = params.get("FCUP_Response_Data").and_then(Value::as_data) else {
                return ActionPlan::BadRequest;
            };
            if data.is_empty() {
                // The C handler reads data[-1] for an empty data value. Treat
                // this undefined case as invalid input instead of reproducing UB.
                return ActionPlan::BadRequest;
            }
            let mut playlist = data.to_vec();
            if playlist.last() != Some(&b'\n') {
                playlist.push(b'\n');
            }
            let c_len = playlist
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(playlist.len());
            playlist.truncate(c_len);
            if !playlist.starts_with(b"#EXTM3U\n") {
                return ActionPlan::BadRequest;
            }
            let url = c_string_prefix(url).to_owned();
            ActionPlan::FcupPlaylist {
                is_master: url.contains("/master.m3u8"),
                url,
                playlist,
            }
        }
        _ => ActionPlan::IgnoreUnknown {
            action_type: c_string_prefix(action_type).to_owned(),
        },
    }
}

/// Resolve the handler's playlist UUID check into an explicit state effect.
pub fn resolve_playlist_remove(
    action: &ActionPlan,
    current_video_uuid: Option<&str>,
) -> ActionEffect {
    match action {
        ActionPlan::RemovePlaylist { uuid } if current_video_uuid == Some(uuid.as_str()) => {
            ActionEffect::RemoveCurrentPlaylist {
                uuid: uuid.clone(),
                preserve_playlist_for_resume: true,
                persist_callback_resume_position: true,
            }
        }
        ActionPlan::RemovePlaylist { uuid } => {
            ActionEffect::IgnoreNonCurrentRemoval { uuid: uuid.clone() }
        }
        _ => ActionEffect::NoEffect,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionEffect {
    RemoveCurrentPlaylist {
        uuid: String,
        preserve_playlist_for_resume: bool,
        persist_callback_resume_position: bool,
    },
    IgnoreNonCurrentRemoval {
        uuid: String,
    },
    NoEffect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlayRequest {
    pub session_id: String,
    pub playback_uuid: String,
    pub content_location: Option<String>,
    pub start_position_seconds: f32,
    pub client_proc_name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlayParseError {
    MissingSessionId,
    UnsupportedContentType,
    InvalidBinaryPlist,
    MissingUuid,
}

/// Parse the binary-plist envelope from `POST /play` without assuming any
/// playlist-store state. Content-Location is optional here because the source
/// returns early when the UUID names a reusable stored playlist.
pub fn parse_play(
    session_header: Option<&str>,
    full_header_block: &[u8],
    body: &[u8],
) -> Result<PlayRequest, PlayParseError> {
    let session_id = session_header.ok_or(PlayParseError::MissingSessionId)?;
    if body.is_empty()
        || !contains_bytes(full_header_block, b"x-apple-binary-plist")
        || !body.starts_with(b"bplist00")
    {
        return Err(PlayParseError::UnsupportedContentType);
    }
    let root =
        Value::from_reader(Cursor::new(body)).map_err(|_| PlayParseError::InvalidBinaryPlist)?;
    let root = root
        .as_dictionary()
        .ok_or(PlayParseError::InvalidBinaryPlist)?;
    let playback_uuid = root
        .get("uuid")
        .and_then(Value::as_string)
        .ok_or(PlayParseError::MissingUuid)?;
    let start_position_seconds = root
        .get("Start-Position-Seconds")
        .and_then(plist_number)
        .unwrap_or(0.0) as f32;
    Ok(PlayRequest {
        session_id: c_string_prefix(session_id).to_owned(),
        playback_uuid: c_string_prefix(playback_uuid).to_owned(),
        content_location: root
            .get("Content-Location")
            .and_then(Value::as_string)
            .map(|value| c_string_prefix(value).to_owned()),
        start_position_seconds,
        client_proc_name: root
            .get("clientProcName")
            .and_then(Value::as_string)
            .map(|value| c_string_prefix(value).to_owned()),
    })
}

fn plist_number(value: &Value) -> Option<f64> {
    value
        .as_real()
        .or_else(|| value.as_signed_integer().map(|number| number as f64))
        .or_else(|| value.as_unsigned_integer().map(|number| number as f64))
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReusablePlaylist {
    pub id: usize,
    pub playback_location: String,
    pub resume_position_seconds: f32,
    pub start_position_seconds: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlaybackSource {
    DirectUrl {
        location: String,
    },
    Hls {
        original_location: String,
        playback_location: String,
        uri_prefix: String,
        client_proc_name: Option<String>,
        warn_unsupported_client: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlayPlan {
    ReusePlaylist {
        id: usize,
        location: String,
        resume_position_seconds: f32,
        set_session_id: String,
    },
    CreatePlaylist {
        playback_uuid: String,
        set_session_id: String,
        start_position_seconds: f32,
        source: PlaybackSource,
    },
    BadRequest,
}

/// Choose the source handler's direct URL, local-HLS, or stored-playlist path.
/// Playlist allocation/cleanup and renderer/FCUP callbacks stay typed effects.
pub fn plan_play(
    request: &PlayRequest,
    reusable: Option<&ReusablePlaylist>,
    local_uri_prefix: &str,
) -> PlayPlan {
    if let Some(video) = reusable {
        return PlayPlan::ReusePlaylist {
            id: video.id,
            location: video.playback_location.clone(),
            resume_position_seconds: video
                .resume_position_seconds
                .max(video.start_position_seconds),
            set_session_id: request.session_id.clone(),
        };
    }
    let Some(location) = request.content_location.as_deref() else {
        return PlayPlan::BadRequest;
    };
    if location.starts_with("http://") || location.starts_with("https://") {
        return PlayPlan::CreatePlaylist {
            playback_uuid: request.playback_uuid.clone(),
            set_session_id: request.session_id.clone(),
            start_position_seconds: request.start_position_seconds,
            source: PlaybackSource::DirectUrl {
                location: location.to_owned(),
            },
        };
    }
    let Some(suffix_start) = location.find("/master.m3u8") else {
        return PlayPlan::BadRequest;
    };
    let suffix = &location[suffix_start..];
    let supported = request
        .client_proc_name
        .as_deref()
        .is_none_or(|name| "YouTube;".contains(name));
    PlayPlan::CreatePlaylist {
        playback_uuid: request.playback_uuid.clone(),
        set_session_id: request.session_id.clone(),
        start_position_seconds: request.start_position_seconds,
        source: PlaybackSource::Hls {
            original_location: location.to_owned(),
            playback_location: format!("{local_uri_prefix}{suffix}"),
            uri_prefix: location[..suffix_start].to_owned(),
            client_proc_name: request.client_proc_name.clone(),
            warn_unsupported_client: !supported,
        },
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HlsPlan {
    MissingCurrentVideo,
    DeclineUpgrade,
    MissingPlaylist,
    FetchMaster,
    FetchMedia {
        uri: String,
        expand_condensed_uris: bool,
    },
}

pub fn plan_hls(
    current_video_index: Option<usize>,
    current_slot_has_playlist: bool,
    upgrade_header: Option<&str>,
    url: &str,
) -> HlsPlan {
    if current_video_index.is_none() {
        return HlsPlan::MissingCurrentVideo;
    }
    if upgrade_header.is_some() {
        return HlsPlan::DeclineUpgrade;
    }
    if !current_slot_has_playlist {
        return HlsPlan::MissingPlaylist;
    }
    if url == "/master.m3u8" {
        HlsPlan::FetchMaster
    } else {
        HlsPlan::FetchMedia {
            uri: c_string_prefix(url).to_owned(),
            expand_condensed_uris: true,
        }
    }
}

/// Finish a master/media fetch after callback-dependent playlist lookup and,
/// for media, `adjust_yt_condensed_playlist` have completed.
pub fn finish_hls(plan: &HlsPlan, body: Option<Vec<u8>>, date: &str) -> HandlerResponse {
    match plan {
        HlsPlan::MissingCurrentVideo => HandlerResponse {
            status: Some((404, "Not Found")),
            ..HandlerResponse::default()
        },
        HlsPlan::DeclineUpgrade => HandlerResponse::default(),
        HlsPlan::MissingPlaylist | HlsPlan::FetchMaster | HlsPlan::FetchMedia { .. } => {
            let mut response = HandlerResponse {
                headers: vec![
                    (
                        "Access-Control-Allow-Headers".to_owned(),
                        "Content-type".to_owned(),
                    ),
                    ("Access-Control-Allow-Origin".to_owned(), "*".to_owned()),
                    ("Date".to_owned(), date.to_owned()),
                ],
                ..HandlerResponse::default()
            };
            if let Some(body) = body.filter(|body| !body.is_empty()) {
                response
                    .headers
                    .push(("Content-Type".to_owned(), HLS_CONTENT_TYPE.to_owned()));
                response.body = body;
            } else {
                response.status = Some((404, "Not Found"));
            }
            response
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MediaPlaylistStats {
    pub chunks: usize,
    pub duration_seconds: f32,
    pub endlist: bool,
}

/// Find the upstream media-playlist URLs referenced by the master playlist.
///
/// The C source scans for the configured URI prefix, then takes each match
/// through the next `m3u8`. Keep that byte-based behavior, including query text.
pub fn extract_media_playlist_uris(master: &[u8], upstream_prefix: &[u8]) -> Vec<Vec<u8>> {
    if upstream_prefix.is_empty() {
        return Vec::new();
    }
    let mut uris = Vec::new();
    let mut cursor = 0;
    while let Some(start) = find_bytes(master, upstream_prefix, cursor) {
        // C create_media_uri_table() aborts the entire table when any later
        // prefix has no following m3u8. Its caller initializes num_uri to zero
        // and leaves it there when this function returns 1.
        let Some(end) = find_bytes(&master[start..], b"m3u8", 0) else {
            return Vec::new();
        };
        let uri_end = start + end + 4;
        uris.push(master[start..uri_end].to_vec());
        cursor = uri_end;
    }
    uris
}

/// Replace every occurrence of the upstream prefix in a master playlist.
pub fn rewrite_master_playlist(
    master: &[u8],
    upstream_prefix: &[u8],
    local_prefix: &[u8],
) -> Vec<u8> {
    if upstream_prefix.is_empty() {
        return master.to_vec();
    }
    let mut rewritten = Vec::with_capacity(master.len());
    let mut cursor = 0;
    while let Some(start) = find_bytes(master, upstream_prefix, cursor) {
        rewritten.extend_from_slice(&master[cursor..start]);
        rewritten.extend_from_slice(local_prefix);
        cursor = start + upstream_prefix.len();
    }
    rewritten.extend_from_slice(&master[cursor..]);
    rewritten
}

/// Count EXTINF entries, sum their C `strtof` prefixes, and detect ENDLIST.
pub fn analyze_media_playlist(playlist: &[u8]) -> MediaPlaylistStats {
    let mut stats = MediaPlaylistStats::default();
    let mut cursor = 0;
    let mut last_number_end = None;
    while let Some(tag) = find_bytes(playlist, b"#EXTINF:", cursor) {
        let value_start = tag + b"#EXTINF:".len();
        let (duration, consumed) = parse_float_prefix(&playlist[value_start..]);
        stats.duration_seconds += duration;
        stats.chunks += 1;
        let value_end = value_start + consumed;
        last_number_end = Some(value_end);
        cursor = value_end;
    }
    if let Some(cursor) = last_number_end {
        stats.endlist = find_bytes(playlist, b"#EXT-X-ENDLIST", cursor).is_some();
    }
    stats
}

/// Expand the source's `#YT-EXT-CONDENSED-URL` segment convention.
///
/// Invalid or incomplete condensed headers return `None`; ordinary playlists
/// are copied unchanged. A nonpositive chunk count also returns a copy.
pub fn expand_condensed_media_playlist(playlist: &[u8], chunks: usize) -> Option<Vec<u8>> {
    let first_chunk = find_bytes(playlist, b"#EXTINF", 0);
    let Some(first_chunk) = first_chunk else {
        return (find_bytes(playlist, b"#YT-EXT-CONDENSED-URL", 0).is_none())
            .then(|| playlist.to_vec());
    };
    let Some(tag) =
        find_bytes(playlist, b"#YT-EXT-CONDENSED-URL", 0).filter(|tag| *tag < first_chunk)
    else {
        return Some(playlist.to_vec());
    };
    if chunks == 0 {
        return Some(playlist.to_vec());
    }
    let header = &playlist[tag..first_chunk];
    let base_uri = attribute(header, b"BASE-URI=")?;
    let params = attribute(header, b"PARAMS=")?;
    let prefix = attribute(header, b"PREFIX=")?;
    if base_uri.is_empty() || params.is_empty() || prefix.is_empty() {
        return None;
    }
    let params = params.split(|byte| *byte == b',').collect::<Vec<_>>();
    let mut output = Vec::with_capacity(playlist.len() + chunks * (base_uri.len() + params.len()));
    output.extend_from_slice(&playlist[..first_chunk]);
    let mut cursor = first_chunk;
    for chunk in 0..chunks {
        let start = find_bytes(playlist, prefix, cursor)?;
        output.extend_from_slice(&playlist[cursor..start]);
        output.extend_from_slice(base_uri);
        output.push(b'/');
        cursor = start + prefix.len();
        for (index, param) in params.iter().enumerate() {
            output.extend_from_slice(param);
            output.push(b'/');
            let end = if index + 1 < params.len() {
                find_bytes(playlist, b"/", cursor)? + 1
            } else if chunk + 1 < chunks {
                find_bytes(playlist, b"#EXTINF", cursor)?
            } else {
                playlist.len()
            };
            output.extend_from_slice(&playlist[cursor..end]);
            cursor = end;
        }
    }
    output.extend_from_slice(&playlist[cursor..]);
    Some(output)
}

/// Select audio/subtitle renditions and rewrite DEFAULT/AUTOSELECT flags.
pub fn select_master_playlist_language(
    master: &[u8],
    requested_audio: &str,
    requested_subtitles: &str,
    system_languages: &str,
) -> Vec<u8> {
    let mut lines = master
        .split_inclusive(|byte| *byte == b'\n')
        .map(MasterPlaylistLine::parse)
        .collect::<Vec<_>>();
    let requested_audio = split_languages(requested_audio);
    let requested_subtitles = split_languages(requested_subtitles);
    let system_languages = split_languages(system_languages);
    let mut audio_selected = false;
    let mut audio_default_selected = false;
    let mut subtitle_selected = false;
    let mut subtitles_requested = !requested_subtitles.is_empty();
    let mut subtitles_enabled = false;
    let mut listed_audio = true;
    let mut listed_subtitles = true;

    for iteration in 0..4 {
        let (language_preferences, kind, autoselect) = match iteration {
            0 if !requested_audio.is_empty() => (&requested_audio, b'a', false),
            1 if !audio_selected && listed_audio => (&system_languages, b'a', true),
            2 if (!requested_audio.is_empty() || !requested_subtitles.is_empty()) => {
                let preferences = if requested_subtitles.is_empty() {
                    &requested_audio
                } else {
                    &requested_subtitles
                };
                (preferences, b's', false)
            }
            3 if !subtitle_selected && listed_subtitles => {
                subtitles_requested = false;
                (&system_languages, b's', true)
            }
            _ => continue,
        };
        let (available, default_language) = available_languages(&lines, kind, autoselect);
        if available.is_empty() {
            if kind == b'a' && iteration == 0 {
                listed_audio = false;
            } else if kind == b's' && iteration == 2 {
                listed_subtitles = false;
            }
            continue;
        }
        let selected = if autoselect {
            match_language(language_preferences, &available).or_else(|| {
                (kind == b'a')
                    .then_some(default_language.as_deref())
                    .flatten()
            })
        } else {
            strict_match_language(language_preferences, &available)
        };
        let Some(selected) = selected else {
            continue;
        };
        match iteration {
            0 | 1 => {
                audio_selected = true;
                if iteration == 1
                    && !language_preferences.iter().any(|preferred| {
                        match_language(std::slice::from_ref(preferred), &available).is_some()
                    })
                {
                    audio_default_selected = true;
                    subtitles_enabled = true;
                }
            }
            _ => subtitle_selected = true,
        }
        for line in &mut lines {
            if line.kind == Some(kind)
                && line
                    .language
                    .as_deref()
                    .is_some_and(|language| language == selected)
            {
                line.delete = false;
            } else if line.kind == Some(kind) && line.language.is_some() {
                line.delete = true;
            }
        }
    }
    if subtitle_selected {
        subtitles_enabled = subtitles_requested || !audio_selected || audio_default_selected;
    }

    let mut output = Vec::with_capacity(master.len());
    for line in lines {
        if line.delete {
            continue;
        }
        if matches!(line.kind, Some(b'a' | b's')) {
            let enabled = line.kind == Some(b'a') || subtitles_enabled;
            let normalized = remove_default_autoselect(&line.bytes);
            output.extend_from_slice(&normalized);
            output.push(b',');
            output.extend_from_slice(if enabled {
                b"DEFAULT=YES,AUTOSELECT=YES\n"
            } else {
                b"DEFAULT=NO,AUTOSELECT=NO\n"
            });
        } else {
            output.extend_from_slice(&line.bytes);
        }
    }
    output
}

/// Remove master-playlist variants that exceed a per-codec custom profile.
///
/// This follows `filter_master_playlist` from `lib/airplay_video.c`: each
/// `EXT-X-STREAM-INF` line is evaluated with its following URI line, while
/// other nonempty lines are retained.
pub fn filter_master_playlist(master: &[u8], custom_profile: &str) -> Vec<u8> {
    if custom_profile.is_empty() {
        return master.to_vec();
    }
    let mut output = Vec::with_capacity(master.len());
    let mut pending_info: Option<Vec<u8>> = None;
    for source_line in master.split(|byte| *byte == b'\n') {
        let line = source_line.strip_suffix(b"\r").unwrap_or(source_line);
        if line.starts_with(b"#EXT-X-STREAM-INF:") {
            pending_info = Some(line.to_vec());
            continue;
        }
        if let Some(info) = pending_info.take() {
            if keep_profile_stream(&info, custom_profile.as_bytes()) {
                output.extend_from_slice(&info);
                output.push(b'\n');
                if !line.is_empty() {
                    output.extend_from_slice(line);
                    output.push(b'\n');
                }
            }
        } else if !line.is_empty() {
            output.extend_from_slice(line);
            output.push(b'\n');
        }
    }
    output
}

fn keep_profile_stream(info_line: &[u8], custom_profile: &[u8]) -> bool {
    let height = profile_attribute(info_line, b"RESOLUTION=", 31)
        .and_then(|resolution| {
            resolution
                .iter()
                .position(|byte| *byte == b'x')
                .map(|x| &resolution[x + 1..])
        })
        .map_or(0, c_atoi_prefix);
    let codec = profile_attribute(info_line, b"CODECS=", 63).unwrap_or_default();
    let fps = if let Some(frame_rate) = profile_attribute(info_line, b"FRAME-RATE=", 15) {
        parse_float_prefix(frame_rate).0
    } else {
        deduce_h264_fps(codec)
    };
    let profile_codec = if codec.starts_with(b"avc1") {
        "AVC"
    } else if codec.starts_with(b"hvc1") || codec.starts_with(b"hev1") {
        "HEVC"
    } else if codec.starts_with(b"vp09") {
        "VP9"
    } else if codec.starts_with(b"av01") {
        "AV1"
    } else {
        return false;
    };
    if fps > 60.0 {
        return false;
    }
    let fps_bucket = if fps <= 30.0 { 30 } else { 60 };
    let (max_30, max_60) =
        profile_max_heights(profile_codec.as_bytes(), custom_profile).unwrap_or((2160, 2160));
    !(max_30 >= max_60 && ((fps_bucket == 60 && height > max_60) || height > max_30))
}

fn profile_attribute<'a>(line: &'a [u8], name: &[u8], max_len: usize) -> Option<&'a [u8]> {
    let mut cursor = 0;
    while let Some(found) = find_bytes(line, name, cursor) {
        if found == 0 || matches!(line[found - 1], b',' | b':') {
            let mut start = found + name.len();
            let quoted = line.get(start) == Some(&b'\"');
            if quoted {
                start += 1;
            }
            let end = line[start..]
                .iter()
                .position(|byte| {
                    *byte == b'\n'
                        || *byte == b'\r'
                        || (quoted && *byte == b'\"')
                        || (!quoted && *byte == b',')
                })
                .map_or(line.len(), |offset| start + offset);
            let value = &line[start..end.min(start.saturating_add(max_len))];
            return (!value.is_empty()).then_some(value);
        }
        cursor = found + 1;
    }
    None
}

fn c_atoi_prefix(input: &[u8]) -> i32 {
    parse_c_integer_prefix(input).0
}

fn parse_c_integer_prefix(input: &[u8]) -> (i32, usize) {
    let mut cursor = input
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(input.len());
    let start = cursor;
    let negative = input.get(cursor) == Some(&b'-');
    if input
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b'+' | b'-'))
    {
        cursor += 1;
    }
    let digits_start = cursor;
    let mut value = 0_i64;
    while input.get(cursor).is_some_and(u8::is_ascii_digit) {
        value = value
            .saturating_mul(10)
            .saturating_add(i64::from(input[cursor] - b'0'));
        cursor += 1;
    }
    if cursor == digits_start {
        return (0, start);
    }
    let signed = if negative { -value } else { value };
    (
        signed.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        cursor,
    )
}

fn profile_max_heights(codec: &[u8], profile: &[u8]) -> Option<(i32, i32)> {
    let start = find_bytes(profile, codec, 0)? + codec.len();
    if profile.get(start) != Some(&b':') {
        return None;
    }
    let (max_30, consumed) = parse_c_integer_prefix(&profile[start + 1..]);
    if max_30 < 0 {
        return None;
    }
    let mut max_60 = max_30;
    let next = start + 1 + consumed;
    if profile.get(next) == Some(&b',') {
        max_60 = parse_c_integer_prefix(&profile[next + 1..]).0;
        if max_60 > max_30 || max_60 < 0 {
            return None;
        }
    }
    Some((max_30, max_60))
}

fn deduce_h264_fps(codec: &[u8]) -> f32 {
    let mut level = 0_u32;
    if codec.starts_with(b"avc1") && codec.len() >= 11 {
        for byte in &codec[9..] {
            let digit = match byte {
                b'0'..=b'9' => u32::from(byte - b'0'),
                b'a'..=b'f' => u32::from(byte - b'a') + 10,
                b'A'..=b'F' => u32::from(byte - b'A') + 10,
                _ => break,
            };
            level = level.saturating_mul(16).saturating_add(digit);
        }
    }
    if level >= 0x2a { 60.0 } else { 30.0 }
}

#[derive(Clone, Debug)]
struct MasterPlaylistLine {
    bytes: Vec<u8>,
    kind: Option<u8>,
    language: Option<Vec<u8>>,
    delete: bool,
}

impl MasterPlaylistLine {
    fn parse(bytes: &[u8]) -> Self {
        let line = bytes.strip_suffix(b"\n").unwrap_or(bytes);
        let type_value = attribute_unquoted(line, b"TYPE=");
        let kind = match type_value {
            Some(b"AUDIO") => Some(b'a'),
            Some(b"SUBTITLES") => Some(b's'),
            Some(b"VIDEO") => Some(b'v'),
            Some(b"CLOSED-CAPTIONS") => Some(b'c'),
            _ => None,
        };
        let language = attribute(line, b"LANGUAGE=").map(<[u8]>::to_vec);
        Self {
            bytes: bytes.to_vec(),
            kind,
            language,
            delete: false,
        }
    }
}

fn split_languages(value: &str) -> Vec<Vec<u8>> {
    value
        .split(':')
        .filter(|language| !language.is_empty())
        .map(|language| language.as_bytes().to_vec())
        .collect()
}

fn available_languages(
    lines: &[MasterPlaylistLine],
    kind: u8,
    autoselect: bool,
) -> (Vec<Vec<u8>>, Option<Vec<u8>>) {
    let mut available = Vec::new();
    let mut default_language = None;
    for line in lines.iter().filter(|line| line.kind == Some(kind)) {
        let Some(language) = line.language.as_ref() else {
            continue;
        };
        if autoselect && contains_bytes(&line.bytes, b"AUTOSELECT=NO") {
            continue;
        }
        if !available.contains(language) {
            available.push(language.clone());
            if contains_bytes(&line.bytes, b"DEFAULT=YES") {
                default_language = Some(language.clone());
            }
        }
    }
    (available, default_language)
}

fn strict_match_language<'a>(preferred: &[Vec<u8>], available: &'a [Vec<u8>]) -> Option<&'a [u8]> {
    preferred
        .iter()
        .find_map(|preferred| {
            available
                .iter()
                .find(|available| language_prefix_match(preferred, available))
        })
        .map(Vec::as_slice)
}

fn match_language<'a>(preferred: &[Vec<u8>], available: &'a [Vec<u8>]) -> Option<&'a [u8]> {
    preferred.iter().find_map(|preference| {
        let mut candidate = preference.clone();
        loop {
            if let Some(found) = available
                .iter()
                .find(|language| language_prefix_match(&candidate, language))
            {
                return Some(found.as_slice());
            }
            let Some(separator) = candidate.iter().rposition(|byte| *byte == b'-') else {
                break;
            };
            candidate.truncate(separator);
        }
        None
    })
}

fn language_prefix_match(preferred: &[u8], available: &[u8]) -> bool {
    if preferred.is_empty() {
        return false;
    }
    let common = preferred.len().min(available.len());
    if !preferred[..common]
        .iter()
        .zip(&available[..common])
        .all(|(left, right)| normalize_language_byte(*left) == normalize_language_byte(*right))
    {
        return false;
    }
    if preferred.len() == available.len() {
        return true;
    }
    preferred.len() < available.len() && normalize_language_byte(available[preferred.len()]) == b'-'
}

fn normalize_language_byte(byte: u8) -> u8 {
    match byte {
        b'A'..=b'Z' => byte + (b'a' - b'A'),
        b'_' => b'-',
        _ => byte,
    }
}

fn attribute_unquoted<'a>(line: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let start = find_bytes(line, name, 0)? + name.len();
    let end = line[start..]
        .iter()
        .position(|byte| matches!(byte, b',' | b'\r'))
        .map_or(line.len(), |offset| start + offset);
    Some(&line[start..end])
}

fn remove_default_autoselect(line: &[u8]) -> Vec<u8> {
    const TOKENS: [&[u8]; 4] = [
        b"DEFAULT=YES",
        b"DEFAULT=NO",
        b"AUTOSELECT=YES",
        b"AUTOSELECT=NO",
    ];
    let line_end = line
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(line.len());
    let mut output = Vec::with_capacity(line_end);
    let mut cursor = 0;
    while cursor < line_end {
        if let Some(token) = TOKENS
            .iter()
            .find(|token| line[cursor..line_end].starts_with(token))
        {
            cursor += token.len();
            if line.get(cursor) == Some(&b',') {
                cursor += 1;
            }
        } else {
            output.push(line[cursor]);
            cursor += 1;
        }
    }
    output
}

fn find_bytes(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from + offset)
}

fn attribute<'a>(line: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let start = find_bytes(line, name, 0)? + name.len();
    if line.get(start) != Some(&b'"') {
        return None;
    }
    let value_start = start + 1;
    let end = find_bytes(line, b"\"", value_start)?;
    Some(&line[value_start..end])
}

fn parse_float_prefix(input: &[u8]) -> (f32, usize) {
    let start = input
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(input.len());
    let mut end = start;
    if input
        .get(end)
        .is_some_and(|byte| matches!(byte, b'+' | b'-'))
    {
        end += 1;
    }
    let mut digits = 0;
    while input.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
        digits += 1;
    }
    if input.get(end) == Some(&b'.') {
        end += 1;
        while input.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return (0.0, start);
    }
    if input
        .get(end)
        .is_some_and(|byte| matches!(byte, b'e' | b'E'))
    {
        let exponent = end;
        end += 1;
        if input
            .get(end)
            .is_some_and(|byte| matches!(byte, b'+' | b'-'))
        {
            end += 1;
        }
        let exponent_start = end;
        while input.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        if exponent_start == end {
            end = exponent;
        }
    }
    let value = std::str::from_utf8(&input[start..end])
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0.0);
    (value, end)
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn c_string_prefix(value: &str) -> &str {
    let end = value.find('\0').unwrap_or(value.len());
    &value[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary_plist(root: Value) -> Vec<u8> {
        let mut body = Vec::new();
        root.to_writer_binary(&mut body).unwrap();
        body
    }

    fn dict(entries: impl IntoIterator<Item = (String, Value)>) -> Value {
        let mut dictionary = plist::Dictionary::new();
        for (key, value) in entries {
            dictionary.insert(key, value);
        }
        Value::Dictionary(dictionary)
    }

    fn action_body(action_type: &str, params: Value) -> Vec<u8> {
        binary_plist(dict([
            ("type".to_owned(), Value::String(action_type.to_owned())),
            ("params".to_owned(), params),
        ]))
    }

    #[test]
    fn set_property_known_and_unknown_follow_source_response_mutations() {
        let known = set_property("/setProperty?actionAtItemEnd");
        assert_eq!(known.status, None);
        assert_eq!(
            known.headers,
            [("Content-Type".into(), PROPERTY_CONTENT_TYPE.into())]
        );
        let parsed = Value::from_reader_xml(Cursor::new(known.body)).unwrap();
        assert_eq!(
            parsed.as_dictionary().unwrap()["errorCode"].as_signed_integer(),
            Some(0)
        );

        let unknown = set_property("/setProperty?unknown");
        assert_eq!(unknown.status, None);
        assert_eq!(unknown.headers, [("Content-Length".into(), "0".into())]);
        assert!(unknown.body.is_empty());
    }

    #[test]
    fn set_property_uses_the_c_string_url_boundary() {
        assert_eq!(
            set_property_bytes(b"/setProperty?actionAtItemEnd\0ignored").headers[0].0,
            "Content-Type"
        );
        assert_eq!(
            set_property_bytes(b"/setProperty?unknown\0actionAtItemEnd").headers,
            [("Content-Length".into(), "0".into())]
        );
    }

    #[test]
    fn property_names_are_exact_and_get_property_does_not_mutate_response() {
        for property in [
            "actionAtItemEnd",
            "selectedMediaArray",
            "reverseEndTime",
            "forwardEndTime",
            "interstitialEvents",
            "textMarkupArray",
            "isInterestedInDateRange",
        ] {
            assert!(
                !set_property(&format!("/setProperty?{property}"))
                    .body
                    .is_empty()
            );
        }
        assert!(
            set_property("/setProperty?actionAtItemEnd=1")
                .body
                .is_empty()
        );
        assert_eq!(
            get_property("/getProperty?anything"),
            HandlerResponse::default()
        );
    }

    #[test]
    fn fairplay_setup2_header_is_cleared_by_response_reinitialization() {
        let response = fairplay_setup2();
        assert_eq!(response.status, Some((421, "Misdirected Request")));
        assert!(response.headers.is_empty());
        assert!(response.body.is_empty());
    }

    #[test]
    fn reverse_marks_event_even_when_multiple_connections_prevent_upgrade() {
        let accepted = reverse(1);
        assert_eq!(accepted.effect, ReverseEffect::MarkThisConnectionAsEvent);
        assert_eq!(accepted.response.status, Some((101, "Switching Protocols")));
        assert_eq!(
            accepted.response.headers[1],
            ("Upgrade".into(), "PTTH/1.0".into())
        );

        let rejected = reverse(2);
        assert_eq!(rejected.effect, ReverseEffect::MarkThisConnectionAsEvent);
        assert_eq!(rejected.response, HandlerResponse::default());
    }

    #[test]
    fn action_checks_session_and_binary_plist_then_emits_typed_effects() {
        let params = dict([(
            "item".to_owned(),
            dict([("uuid".to_owned(), Value::String("video-1".to_owned()))]),
        )]);
        let body = action_body("playlistRemove", params);
        assert_eq!(
            action(
                Some("session"),
                Some("session"),
                b"Content-Type: application/x-apple-binary-plist",
                &body
            ),
            ActionPlan::RemovePlaylist {
                uuid: "video-1".to_owned()
            }
        );
        assert_eq!(
            action(
                Some("session"),
                Some("changed"),
                b"Content-Type: application/x-apple-binary-plist",
                &body
            ),
            ActionPlan::BadRequest
        );
    }

    #[test]
    fn action_type_comparison_uses_the_c_string_prefix() {
        let params = dict([(
            "item".to_owned(),
            dict([("uuid".to_owned(), Value::String("video-1".to_owned()))]),
        )]);
        let body = action_body("playlistRemove\0ignored", params);

        assert_eq!(
            action(
                Some("session"),
                Some("session"),
                b"Content-Type: application/x-apple-binary-plist",
                &body,
            ),
            ActionPlan::RemovePlaylist {
                uuid: "video-1".to_owned(),
            }
        );
    }

    #[test]
    fn malformed_action_envelopes_and_required_parameters_are_bad_requests() {
        let header = b"Content-Type: application/x-apple-binary-plist";
        let mut malformed = vec![
            Vec::new(),
            binary_plist(Value::String("not a dictionary".to_owned())),
            binary_plist(dict([("params".to_owned(), dict([]))])),
            binary_plist(dict([
                ("type".to_owned(), Value::Integer(1.into())),
                ("params".to_owned(), dict([])),
            ])),
        ];

        let root_without_params = dict([("type".to_owned(), Value::String("futureAction".into()))]);
        malformed.push(binary_plist(root_without_params));
        malformed.push(action_body("playlistRemove", dict([])));
        malformed.push(action_body(
            "playlistRemove",
            dict([("item".to_owned(), Value::String("not a dictionary".into()))]),
        ));
        malformed.push(action_body("playlistInsert", dict([])));
        malformed.push(action_body(
            "unhandledURLResponse",
            dict([(
                "FCUP_Response_URL".to_owned(),
                Value::String("/media.m3u8".into()),
            )]),
        ));
        malformed.push(action_body(
            "unhandledURLResponse",
            dict([
                (
                    "FCUP_Response_URL".to_owned(),
                    Value::String("/media.m3u8".into()),
                ),
                (
                    "FCUP_Response_Data".to_owned(),
                    Value::Data(b"not an m3u8".to_vec()),
                ),
            ]),
        ));

        for body in malformed {
            assert_eq!(
                action(Some("session"), Some("session"), header, &body),
                ActionPlan::BadRequest
            );
        }
        assert_eq!(
            action(
                Some("session"),
                Some("session"),
                b"Content-Type: text/plain",
                b""
            ),
            ActionPlan::BadRequest
        );
        assert_eq!(
            action(Some("session"), None, header, b"bplist00"),
            ActionPlan::BadRequest
        );
    }

    #[test]
    fn action_validates_and_normalizes_fcup_playlist() {
        let params = dict([
            (
                "FCUP_Response_URL".to_owned(),
                Value::String("https://host/master.m3u8".to_owned()),
            ),
            (
                "FCUP_Response_Data".to_owned(),
                Value::Data(b"#EXTM3U\n#EXT-X-VERSION:3".to_vec()),
            ),
        ]);
        let body = action_body("unhandledURLResponse", params);
        assert_eq!(
            action(
                Some("s"),
                Some("s"),
                b"Content-Type: apple-binary-plist",
                &body
            ),
            ActionPlan::FcupPlaylist {
                url: "https://host/master.m3u8".to_owned(),
                is_master: true,
                playlist: b"#EXTM3U\n#EXT-X-VERSION:3\n".to_vec(),
            }
        );
    }

    #[test]
    fn action_insert_is_recognized_as_a_noop_and_unknown_types_are_ignored() {
        let insert = action_body("playlistInsert", dict([("item".to_owned(), dict([]))]));
        assert_eq!(
            action(
                Some("s"),
                Some("s"),
                b"Content-Type: apple-binary-plist",
                &insert
            ),
            ActionPlan::InsertPlaylist { uuid: None }
        );

        let unknown = action_body("futureAction", dict([]));
        assert_eq!(
            action(
                Some("s"),
                Some("s"),
                b"Content-Type: apple-binary-plist",
                &unknown
            ),
            ActionPlan::IgnoreUnknown {
                action_type: "futureAction".to_owned()
            }
        );
    }

    #[test]
    fn playlist_removal_effect_depends_on_current_uuid() {
        let action = ActionPlan::RemovePlaylist {
            uuid: "video-1".to_owned(),
        };
        assert_eq!(
            resolve_playlist_remove(&action, Some("video-1")),
            ActionEffect::RemoveCurrentPlaylist {
                uuid: "video-1".to_owned(),
                preserve_playlist_for_resume: true,
                persist_callback_resume_position: true,
            }
        );
        assert_eq!(
            resolve_playlist_remove(&action, Some("video-2")),
            ActionEffect::IgnoreNonCurrentRemoval {
                uuid: "video-1".to_owned(),
            }
        );
    }

    #[test]
    fn play_prefers_reusable_playlist_then_routes_direct_and_hls_locations() {
        let request = PlayRequest {
            session_id: "s".to_owned(),
            playback_uuid: "u".to_owned(),
            content_location: None,
            start_position_seconds: 4.0,
            client_proc_name: None,
        };
        assert_eq!(
            plan_play(
                &request,
                Some(&ReusablePlaylist {
                    id: 3,
                    playback_location: "http://local/resume".to_owned(),
                    resume_position_seconds: 10.0,
                    start_position_seconds: 2.0,
                }),
                "http://localhost:7000"
            ),
            PlayPlan::ReusePlaylist {
                id: 3,
                location: "http://local/resume".to_owned(),
                resume_position_seconds: 10.0,
                set_session_id: "s".to_owned(),
            }
        );

        let direct = PlayRequest {
            content_location: Some("https://example.test/movie.m3u8".to_owned()),
            ..request.clone()
        };
        assert!(matches!(
            plan_play(&direct, None, "unused"),
            PlayPlan::CreatePlaylist {
                source: PlaybackSource::DirectUrl { .. },
                ..
            }
        ));

        let hls = PlayRequest {
            content_location: Some("/video/master.m3u8?token=x".to_owned()),
            client_proc_name: Some("OtherApp".to_owned()),
            ..request
        };
        assert_eq!(
            plan_play(&hls, None, "http://127.0.0.1:7100"),
            PlayPlan::CreatePlaylist {
                playback_uuid: "u".to_owned(),
                set_session_id: "s".to_owned(),
                start_position_seconds: 4.0,
                source: PlaybackSource::Hls {
                    original_location: "/video/master.m3u8?token=x".to_owned(),
                    playback_location: "http://127.0.0.1:7100/master.m3u8?token=x".to_owned(),
                    uri_prefix: "/video".to_owned(),
                    client_proc_name: Some("OtherApp".to_owned()),
                    warn_unsupported_client: true,
                },
            }
        );

        let youtube = PlayRequest {
            client_proc_name: Some("YouTube".to_owned()),
            ..hls
        };
        assert!(matches!(
            plan_play(&youtube, None, "http://127.0.0.1:7100"),
            PlayPlan::CreatePlaylist {
                source: PlaybackSource::Hls {
                    warn_unsupported_client: false,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn play_binary_plist_parser_captures_envelope_fields() {
        let body = binary_plist(dict([
            ("uuid".to_owned(), Value::String("video-uuid".to_owned())),
            (
                "Content-Location".to_owned(),
                Value::String("https://host/video".to_owned()),
            ),
            ("Start-Position-Seconds".to_owned(), Value::Real(8.5)),
            (
                "clientProcName".to_owned(),
                Value::String("YouTube".to_owned()),
            ),
        ]));
        assert_eq!(
            parse_play(
                Some("session"),
                b"Content-Type: application/x-apple-binary-plist",
                &body
            ),
            Ok(PlayRequest {
                session_id: "session".to_owned(),
                playback_uuid: "video-uuid".to_owned(),
                content_location: Some("https://host/video".to_owned()),
                start_position_seconds: 8.5,
                client_proc_name: Some("YouTube".to_owned()),
            })
        );
    }

    #[test]
    fn hls_handler_orders_missing_video_upgrade_and_playlist_checks() {
        assert_eq!(
            plan_hls(None, false, Some("h2c"), "/master.m3u8"),
            HlsPlan::MissingCurrentVideo
        );
        assert_eq!(
            plan_hls(Some(0), false, Some("h2c"), "/master.m3u8"),
            HlsPlan::DeclineUpgrade
        );
        assert_eq!(
            plan_hls(Some(0), false, None, "/master.m3u8"),
            HlsPlan::MissingPlaylist
        );
        assert_eq!(
            plan_hls(Some(0), true, None, "/media.m3u8"),
            HlsPlan::FetchMedia {
                uri: "/media.m3u8".to_owned(),
                expand_condensed_uris: true,
            }
        );
    }

    #[test]
    fn hls_response_has_cors_date_and_only_adds_content_type_for_nonempty_body() {
        let plan = HlsPlan::FetchMaster;
        let ok = finish_hls(
            &plan,
            Some(b"#EXTM3U\n".to_vec()),
            "Sun, 22 Nov 2026 00:00:00 GMT",
        );
        assert_eq!(ok.status, None);
        assert!(
            ok.headers
                .contains(&("Content-Type".into(), HLS_CONTENT_TYPE.into()))
        );
        assert_eq!(ok.body, b"#EXTM3U\n");

        let missing = finish_hls(&plan, None, "date");
        assert_eq!(missing.status, Some((404, "Not Found")));
        assert_eq!(missing.headers.len(), 3);

        let missing_playlist = finish_hls(&HlsPlan::MissingPlaylist, None, "date");
        assert_eq!(missing_playlist.status, Some((404, "Not Found")));
        assert_eq!(missing_playlist.headers.len(), 3);
        assert_eq!(missing_playlist.headers[2], ("Date".into(), "date".into()));
    }

    #[test]
    fn master_playlist_prefixes_are_extracted_and_rewritten_as_bytes() {
        let master =
            b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nhttps://upstream/video/low.m3u8?x=1\n";
        assert_eq!(
            extract_media_playlist_uris(master, b"https://upstream/video"),
            [b"https://upstream/video/low.m3u8".to_vec()]
        );
        assert_eq!(
            rewrite_master_playlist(master, b"https://upstream/video", b"http://localhost:7000"),
            b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nhttp://localhost:7000/low.m3u8?x=1\n"
        );
    }

    #[test]
    fn malformed_trailing_master_uri_discards_the_entire_media_uri_table() {
        let master =
            b"#EXTM3U\nhttps://upstream/video/valid.m3u8\nhttps://upstream/video/incomplete\n";
        assert!(extract_media_playlist_uris(master, b"https://upstream/video").is_empty());
    }

    #[test]
    fn media_playlist_stats_sum_c_float_prefixes_and_detect_endlist() {
        assert_eq!(
            analyze_media_playlist(
                b"#EXTM3U\n#EXTINF:6.5,\na.ts\n#EXTINF:4e0,\nb.ts\n#EXT-X-ENDLIST\n"
            ),
            MediaPlaylistStats {
                chunks: 2,
                duration_seconds: 10.5,
                endlist: true,
            }
        );
        assert_eq!(
            analyze_media_playlist(b"#EXTM3U\n"),
            MediaPlaylistStats::default()
        );
    }

    #[test]
    fn youtube_condensed_media_uris_expand_in_source_order() {
        let playlist = b"#EXTM3U\n#YT-EXT-CONDENSED-URL:BASE-URI=\"https://cdn\";PARAMS=\"sig,token\";PREFIX=\"s/\"\n#EXTINF:6,\ns/a/b/first.ts\n#EXTINF:5,\ns/c/d/second.ts\n";
        assert_eq!(
            expand_condensed_media_playlist(playlist, 2).unwrap(),
            b"#EXTM3U\n#YT-EXT-CONDENSED-URL:BASE-URI=\"https://cdn\";PARAMS=\"sig,token\";PREFIX=\"s/\"\n#EXTINF:6,\nhttps://cdn/sig/a/token/b/first.ts\n#EXTINF:5,\nhttps://cdn/sig/c/token/d/second.ts\n"
        );
        assert_eq!(
            expand_condensed_media_playlist(b"#EXTM3U\n#EXTINF:1,\na.ts\n", 1),
            Some(b"#EXTM3U\n#EXTINF:1,\na.ts\n".to_vec())
        );
    }

    #[test]
    fn youtube_condensed_media_rejects_malformed_header_or_missing_segments() {
        assert_eq!(
            expand_condensed_media_playlist(
                b"#EXTM3U\n#YT-EXT-CONDENSED-URL:PREFIX=\"s/\"\n#EXTINF:1,\ns/a.ts\n",
                1
            ),
            None
        );
        assert_eq!(
            expand_condensed_media_playlist(
                b"#EXTM3U\n#YT-EXT-CONDENSED-URL:BASE-URI=\"https://cdn\";PARAMS=\"sig\";PREFIX=\"s/\"\n#EXTINF:1,\na.ts\n",
                1
            ),
            None
        );
    }

    #[test]
    fn master_playlist_language_selection_filters_renditions_and_sets_flags() {
        let master = b"#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",LANGUAGE=\"en-US\",DEFAULT=YES,AUTOSELECT=YES,URI=\"en.m3u8\"\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",LANGUAGE=\"fr-FR\",DEFAULT=NO,AUTOSELECT=NO,URI=\"fr.m3u8\"\n#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"s\",LANGUAGE=\"fr\",DEFAULT=YES,AUTOSELECT=YES,URI=\"sub.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nvideo.m3u8\n";
        assert_eq!(
            select_master_playlist_language(master, "fr", "", "en-US"),
            b"#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",LANGUAGE=\"fr-FR\",URI=\"fr.m3u8\",DEFAULT=YES,AUTOSELECT=YES\n#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"s\",LANGUAGE=\"fr\",URI=\"sub.m3u8\",DEFAULT=NO,AUTOSELECT=NO\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nvideo.m3u8\n"
        );
    }

    #[test]
    fn language_matching_ignores_case_and_normalizes_underscores() {
        assert!(language_prefix_match(b"pt_BR", b"PT-br"));
        assert!(language_prefix_match(b"pt", b"pt-BR"));
        assert!(!language_prefix_match(b"pt-BR", b"pt"));
        assert!(!language_prefix_match(b"pt", b"ptt"));
    }

    #[test]
    fn custom_profile_filters_resolution_fps_and_unknown_codecs() {
        let master = b"#EXTM3U\r\n#EXT-X-VERSION:3\r\n#EXT-X-STREAM-INF:BANDWIDTH=1,RESOLUTION=1920x1080,CODECS=\"avc1.640028,mp4a.40.2\",FRAME-RATE=30\r\nlow-30.m3u8\r\n#EXT-X-STREAM-INF:BANDWIDTH=2,RESOLUTION=1280x720,CODECS=\"avc1.64002A,mp4a.40.2\"\r\nmid-60.m3u8\r\n#EXT-X-STREAM-INF:BANDWIDTH=3,RESOLUTION=1920x1080,CODECS=\"avc1.64002A,mp4a.40.2\"\r\nhigh-60.m3u8\r\n#EXT-X-STREAM-INF:BANDWIDTH=4,RESOLUTION=1920x1080,CODECS=\"vp09.00.41.08\"\r\nvp9.m3u8\r\n#EXT-X-STREAM-INF:BANDWIDTH=5,RESOLUTION=1920x1080,CODECS=\"unknown\"\r\nunknown.m3u8\r\n";
        assert_eq!(
            filter_master_playlist(master, "AVC:1080,720"),
            b"#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-STREAM-INF:BANDWIDTH=1,RESOLUTION=1920x1080,CODECS=\"avc1.640028,mp4a.40.2\",FRAME-RATE=30\nlow-30.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2,RESOLUTION=1280x720,CODECS=\"avc1.64002A,mp4a.40.2\"\nmid-60.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=4,RESOLUTION=1920x1080,CODECS=\"vp09.00.41.08\"\nvp9.m3u8\n"
        );
    }

    #[test]
    fn custom_profile_without_codec_limits_keeps_supported_streams_through_2160p() {
        let master = b"#EXTM3U\n#EXT-X-STREAM-INF:RESOLUTION=3840x2160,CODECS=\"av01.0.08M.08\"\n4k.m3u8\n#EXT-X-STREAM-INF:RESOLUTION=4096x2161,CODECS=\"av01.0.08M.08\"\nabove.m3u8\n";
        assert_eq!(
            filter_master_playlist(master, "AVC:720"),
            b"#EXTM3U\n#EXT-X-STREAM-INF:RESOLUTION=3840x2160,CODECS=\"av01.0.08M.08\"\n4k.m3u8\n"
        );
    }
}
