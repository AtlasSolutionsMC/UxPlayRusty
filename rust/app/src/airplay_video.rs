//! Shared AirPlay HLS playlist storage and per-playlist FCUP sequence state.

use uxplay_airplay_http_handlers::{
    self as handlers, ActionEffect, ActionPlan, MediaPlaylistStats, PlayPlan, PlayRequest,
    ReusablePlaylist,
};

const MAX_AIRPLAY_VIDEO: usize = 10;
const MIN_STORED_VIDEO_SECONDS: f32 = 90.0;

#[derive(Clone, Debug, PartialEq)]
pub enum HlsAction {
    Fetch {
        url: String,
        session_id: String,
        request_id: u64,
    },
    StartPlayback {
        location: String,
        start_position_seconds: f32,
    },
    RemoveCurrent {
        uuid: String,
    },
    BadRequest,
    Ignore,
}

#[derive(Clone, Debug, Default)]
struct MediaPlaylist {
    uri: Vec<u8>,
    body: Option<Vec<u8>>,
    stats: MediaPlaylistStats,
    alias: Option<usize>,
}

#[derive(Clone, Debug, Default)]
struct AirPlayVideo {
    session_id: String,
    playback_uuid: String,
    playback_location: String,
    upstream_uri_prefix: String,
    start_position_seconds: f32,
    resume_position_seconds: f32,
    master_playlist: Option<Vec<u8>>,
    media_playlists: Vec<MediaPlaylist>,
    next_media_uri: usize,
    next_request_id: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlaylistCleanup {
    pub(crate) playback_uuid: String,
    pub(crate) duration_seconds: f32,
}

impl PlaylistCleanup {
    fn from_video(video: &AirPlayVideo) -> Self {
        Self {
            playback_uuid: video.playback_uuid.clone(),
            duration_seconds: video.duration_seconds(),
        }
    }
}

impl AirPlayVideo {
    fn reusable(&self) -> ReusablePlaylist {
        ReusablePlaylist {
            id: 0,
            playback_location: self.playback_location.clone(),
            resume_position_seconds: self.resume_position_seconds,
            start_position_seconds: self.start_position_seconds,
        }
    }

    fn duration_seconds(&self) -> f32 {
        self.media_playlists
            .first()
            .map(|playlist| playlist.stats.duration_seconds)
            .unwrap_or_default()
    }

    fn next_fcup_request_id(&mut self) -> u64 {
        self.next_request_id += 1;
        self.next_request_id
    }
}

/// Global playlist cache and current HLS transaction, matching `raop_t`'s
/// ten reusable `airplay_video_t` slots.
#[derive(Clone, Debug, Default)]
pub struct AirPlayVideoStore {
    videos: Vec<Option<AirPlayVideo>>,
    current_video: Option<usize>,
    requested_audio: String,
    requested_subtitles: String,
    system_languages: String,
    custom_profile: Option<String>,
}

impl AirPlayVideoStore {
    pub fn with_profile_and_languages(
        custom_profile: Option<String>,
        requested_audio: impl Into<String>,
        requested_subtitles: impl Into<String>,
        system_languages: impl Into<String>,
    ) -> Self {
        Self {
            requested_audio: requested_audio.into(),
            requested_subtitles: requested_subtitles.into(),
            system_languages: system_languages.into(),
            custom_profile,
            ..Self::default()
        }
    }

    pub fn current_video_index(&self) -> Option<usize> {
        self.current_video
    }

    pub fn current_session_id(&self) -> Option<&str> {
        self.current_video
            .and_then(|index| self.videos.get(index))
            .and_then(Option::as_ref)
            .map(|video| video.session_id.as_str())
    }

    pub fn current_playback_uuid(&self) -> Option<&str> {
        self.current_uuid()
    }

    pub fn playlist_index_for_uuid(&self, uuid: &str) -> Option<usize> {
        self.videos.iter().position(|entry| {
            entry
                .as_ref()
                .is_some_and(|video| video.playback_uuid == uuid)
        })
    }

    pub fn has_reusable_playlist(&self, uuid: &str) -> bool {
        self.playlist_index_for_uuid(uuid)
            .and_then(|index| self.videos[index].as_ref())
            .is_some_and(|video| !video.playback_location.is_empty())
    }

    pub fn current_playback_location(&self) -> Option<&str> {
        self.current_video
            .and_then(|index| self.videos.get(index))
            .and_then(Option::as_ref)
            .map(|video| video.playback_location.as_str())
    }

    pub fn current_has_playlist(&self) -> bool {
        self.current_video
            .and_then(|index| self.videos.get(index))
            .and_then(Option::as_ref)
            .is_some_and(|video| video.master_playlist.is_some())
    }

    /// Destroy every cached AirPlay video and clear the current video slot,
    /// matching `raop_destroy_airplay_video(raop, -1)`.
    pub fn clear_all(&mut self) {
        self.videos.clear();
        self.current_video = None;
    }

    /// Destroy the current video slot while retaining other cached playlists,
    /// matching `raop_destroy_airplay_video(raop, current_video)` on HLS EOS.
    pub fn finish_current(&mut self) -> bool {
        let Some(index) = self.current_video.take() else {
            return false;
        };
        let Some(slot) = self.videos.get_mut(index) else {
            return false;
        };
        slot.take().is_some()
    }

    #[cfg(test)]
    pub fn begin_play(&mut self, request: PlayRequest, local_prefix: &str) -> HlsAction {
        self.begin_play_with_diagnostics(request, local_prefix).0
    }

    pub(crate) fn begin_play_with_diagnostics(
        &mut self,
        request: PlayRequest,
        local_prefix: &str,
    ) -> (HlsAction, Vec<PlaylistCleanup>) {
        let reusable_index = self.videos.iter().position(|entry| {
            entry
                .as_ref()
                .is_some_and(|video| video.playback_uuid == request.playback_uuid)
        });
        let reusable_index = reusable_index.filter(|index| {
            self.videos[*index]
                .as_ref()
                .is_some_and(|video| !video.playback_location.is_empty())
        });
        let reusable = reusable_index.and_then(|index| {
            self.videos[index].as_ref().map(|video| {
                let mut reusable = video.reusable();
                reusable.id = index;
                reusable
            })
        });
        let plan = handlers::plan_play(&request, reusable.as_ref(), local_prefix);
        if let (Some(index), PlayPlan::ReusePlaylist { set_session_id, .. }) =
            (reusable_index, &plan)
        {
            let video = self.videos[index].as_mut().expect("reusable entry exists");
            video.session_id.clone_from(set_session_id);
            self.current_video = Some(index);
            return (
                HlsAction::StartPlayback {
                    location: video.playback_location.clone(),
                    start_position_seconds: video
                        .resume_position_seconds
                        .max(video.start_position_seconds),
                },
                Vec::new(),
            );
        }

        let PlayPlan::CreatePlaylist {
            playback_uuid,
            set_session_id,
            start_position_seconds,
            source,
        } = plan
        else {
            return (HlsAction::BadRequest, Vec::new());
        };
        let mut cleaned_playlists = self.remove_short_playlists();
        let (Some(index), evicted_playlist) = self.allocate_slot() else {
            return (HlsAction::BadRequest, cleaned_playlists);
        };
        if let Some(evicted_playlist) = evicted_playlist {
            cleaned_playlists.push(evicted_playlist);
        }
        let mut video = AirPlayVideo {
            session_id: set_session_id,
            playback_uuid,
            start_position_seconds,
            ..AirPlayVideo::default()
        };
        self.current_video = Some(index);
        let action = match source {
            handlers::PlaybackSource::DirectUrl { location } => {
                video.playback_location.clone_from(&location);
                let start_position_seconds = video.start_position_seconds;
                self.videos[index] = Some(video);
                HlsAction::StartPlayback {
                    location,
                    start_position_seconds,
                }
            }
            handlers::PlaybackSource::Hls {
                playback_location,
                uri_prefix,
                ..
            } => {
                video.playback_location = playback_location;
                video.upstream_uri_prefix = uri_prefix;
                let request_id = video.next_fcup_request_id();
                let action = HlsAction::Fetch {
                    url: video.playback_location.clone(),
                    session_id: video.session_id.clone(),
                    request_id,
                };
                self.videos[index] = Some(video);
                action
            }
        };
        (action, cleaned_playlists)
    }

    pub fn apply_action(&mut self, plan: ActionPlan, local_prefix: &str) -> HlsAction {
        match plan {
            ActionPlan::RemovePlaylist { uuid } => {
                let effect = handlers::resolve_playlist_remove(
                    &ActionPlan::RemovePlaylist { uuid: uuid.clone() },
                    self.current_uuid(),
                );
                match effect {
                    ActionEffect::RemoveCurrentPlaylist { .. } => {
                        self.current_video = None;
                        HlsAction::RemoveCurrent { uuid }
                    }
                    ActionEffect::IgnoreNonCurrentRemoval { .. } | ActionEffect::NoEffect => {
                        HlsAction::Ignore
                    }
                }
            }
            ActionPlan::InsertPlaylist { .. } | ActionPlan::IgnoreUnknown { .. } => {
                HlsAction::Ignore
            }
            ActionPlan::FcupPlaylist {
                url: _,
                is_master,
                playlist,
            } => {
                let Some(index) = self.current_video else {
                    return HlsAction::Ignore;
                };
                let Some(video) = self.videos.get_mut(index).and_then(Option::as_mut) else {
                    return HlsAction::Ignore;
                };
                if is_master {
                    let playlist = match self.custom_profile.as_deref() {
                        Some(profile) => handlers::filter_master_playlist(&playlist, profile),
                        None => playlist,
                    };
                    let playlist = handlers::select_master_playlist_language(
                        &playlist,
                        &self.requested_audio,
                        &self.requested_subtitles,
                        &self.system_languages,
                    );
                    let upstream_prefix = video.upstream_uri_prefix.as_bytes();
                    let media_uris =
                        handlers::extract_media_playlist_uris(&playlist, upstream_prefix);
                    if media_uris.is_empty() {
                        video.media_playlists.clear();
                    } else {
                        video.media_playlists = media_uris
                            .into_iter()
                            .map(|uri| MediaPlaylist {
                                uri,
                                ..MediaPlaylist::default()
                            })
                            .collect();
                    }
                    video.master_playlist = Some(handlers::rewrite_master_playlist(
                        &playlist,
                        upstream_prefix,
                        local_prefix.as_bytes(),
                    ));
                    video.next_media_uri = 0;
                } else {
                    let Some(media_index) = video.next_media_uri.checked_sub(1) else {
                        return HlsAction::Ignore;
                    };
                    let Some(uri) = video
                        .media_playlists
                        .get(media_index)
                        .map(|media| media.uri.clone())
                    else {
                        return HlsAction::Ignore;
                    };
                    let duplicate = video.media_playlists[..media_index]
                        .iter()
                        .position(|previous| previous.uri == uri);
                    let duplicate_matches = duplicate.is_some_and(|duplicate| {
                        video.media_playlists[duplicate].body.as_deref()
                            == Some(playlist.as_slice())
                    });
                    let Some(media) = video.media_playlists.get_mut(media_index) else {
                        return HlsAction::Ignore;
                    };
                    if media.body.is_none() {
                        if let Some(duplicate) = duplicate {
                            // The source asserts that duplicate URLs have equal bodies, then
                            // aliases to the first URL in release builds.
                            debug_assert!(duplicate_matches);
                            media.alias = Some(duplicate);
                        } else {
                            media.stats = handlers::analyze_media_playlist(&playlist);
                            media.body = Some(playlist);
                        }
                    }
                }
                self.next_fcup_action(index)
            }
            ActionPlan::BadRequest => HlsAction::BadRequest,
        }
    }

    pub fn playlist_for_request(&self, url: &str) -> Option<Vec<u8>> {
        let index = self.current_video?;
        let video = self.videos.get(index)?.as_ref()?;
        if url == "/master.m3u8" {
            return video.master_playlist.clone();
        }
        let uri = url.as_bytes();
        let media = video
            .media_playlists
            .iter()
            .find(|playlist| contains_bytes(&playlist.uri, uri))?;
        let original = media
            .alias
            .and_then(|alias| video.media_playlists.get(alias))
            .unwrap_or(media);
        handlers::expand_condensed_media_playlist(original.body.as_deref()?, original.stats.chunks)
    }

    pub fn media_playlist_stats_for_request(&self, url: &str) -> Option<MediaPlaylistStats> {
        let index = self.current_video?;
        let video = self.videos.get(index)?.as_ref()?;
        video
            .media_playlists
            .iter()
            .find(|playlist| contains_bytes(&playlist.uri, url.as_bytes()))
            .map(|playlist| playlist.stats)
    }

    pub fn next_media_playlist_is_duplicate(&self, playlist: &[u8]) -> bool {
        let Some(index) = self.current_video else {
            return false;
        };
        let Some(video) = self.videos.get(index).and_then(Option::as_ref) else {
            return false;
        };
        let Some(media_index) = video.next_media_uri.checked_sub(1) else {
            return false;
        };
        let Some(media) = video.media_playlists.get(media_index) else {
            return false;
        };
        video.media_playlists[..media_index]
            .iter()
            .any(|previous| previous.uri == media.uri && previous.body.as_deref() == Some(playlist))
    }

    pub fn set_resume_position(&mut self, uuid: &str, position_seconds: f32) {
        if let Some(video) = self
            .videos
            .iter_mut()
            .filter_map(Option::as_mut)
            .find(|video| video.playback_uuid == uuid)
        {
            video.resume_position_seconds = position_seconds;
        }
    }

    fn current_uuid(&self) -> Option<&str> {
        self.current_video
            .and_then(|index| self.videos.get(index))
            .and_then(Option::as_ref)
            .map(|video| video.playback_uuid.as_str())
    }

    fn remove_short_playlists(&mut self) -> Vec<PlaylistCleanup> {
        let mut cleaned_playlists = Vec::new();
        for video in &mut self.videos {
            if video
                .as_ref()
                .is_some_and(|video| video.duration_seconds() < MIN_STORED_VIDEO_SECONDS)
                && let Some(video) = video.take()
            {
                cleaned_playlists.push(PlaylistCleanup::from_video(&video));
            }
        }
        cleaned_playlists
    }

    fn allocate_slot(&mut self) -> (Option<usize>, Option<PlaylistCleanup>) {
        if self.videos.len() < MAX_AIRPLAY_VIDEO {
            self.videos.resize_with(MAX_AIRPLAY_VIDEO, || None);
        }
        let Some(index) = self.videos.iter().position(Option::is_none) else {
            return (None, None);
        };
        let stored_count = self.videos.iter().filter(|entry| entry.is_some()).count() + 1;
        let evicted_playlist = if stored_count == MAX_AIRPLAY_VIDEO {
            let evict = (index + 1) % MAX_AIRPLAY_VIDEO;
            self.videos[evict]
                .take()
                .map(|video| PlaylistCleanup::from_video(&video))
        } else {
            None
        };
        (Some(index), evicted_playlist)
    }

    fn next_fcup_action(&mut self, index: usize) -> HlsAction {
        let Some(video) = self.videos.get_mut(index).and_then(Option::as_mut) else {
            return HlsAction::Ignore;
        };
        if let Some(media) = video.media_playlists.get(video.next_media_uri) {
            let url = String::from_utf8_lossy(&media.uri).into_owned();
            video.next_media_uri += 1;
            let session_id = video.session_id.clone();
            let request_id = video.next_fcup_request_id();
            HlsAction::Fetch {
                url,
                session_id,
                request_id,
            }
        } else {
            HlsAction::StartPlayback {
                location: video.playback_location.clone(),
                start_position_seconds: video.start_position_seconds,
            }
        }
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty()
        || haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play_request(uuid: &str, content_location: &str) -> PlayRequest {
        PlayRequest {
            session_id: "session-1".to_owned(),
            playback_uuid: uuid.to_owned(),
            content_location: Some(content_location.to_owned()),
            start_position_seconds: 3.0,
            client_proc_name: None,
        }
    }

    #[test]
    fn hls_fcup_sequence_stores_master_and_media_before_starting_playback() {
        let mut store = AirPlayVideoStore::default();
        assert_eq!(
            store.begin_play(
                play_request("uuid-1", "/video/master.m3u8?token=x"),
                "http://localhost:7000"
            ),
            HlsAction::Fetch {
                url: "http://localhost:7000/master.m3u8?token=x".to_owned(),
                session_id: "session-1".to_owned(),
                request_id: 1,
            }
        );
        assert_eq!(
            store.apply_action(
                ActionPlan::FcupPlaylist {
                    url: "/video/master.m3u8".to_owned(),
                    is_master: true,
                    playlist: b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\n/video/low.m3u8\n"
                        .to_vec(),
                },
                "http://localhost:7000"
            ),
            HlsAction::Fetch {
                url: "/video/low.m3u8".to_owned(),
                session_id: "session-1".to_owned(),
                request_id: 2,
            }
        );
        assert_eq!(
            store.apply_action(
                ActionPlan::FcupPlaylist {
                    url: "/video/low.m3u8".to_owned(),
                    is_master: false,
                    playlist: b"#EXTM3U\n#EXTINF:4.5,\nsegment.ts\n#EXT-X-ENDLIST\n".to_vec(),
                },
                "http://localhost:7000"
            ),
            HlsAction::StartPlayback {
                location: "http://localhost:7000/master.m3u8?token=x".to_owned(),
                start_position_seconds: 3.0,
            }
        );
        assert_eq!(
            store.playlist_for_request("/master.m3u8").unwrap(),
            b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nhttp://localhost:7000/low.m3u8\n"
        );
        assert_eq!(
            store.playlist_for_request("/low.m3u8").unwrap(),
            b"#EXTM3U\n#EXTINF:4.5,\nsegment.ts\n#EXT-X-ENDLIST\n"
        );
    }

    #[test]
    fn fcup_request_ids_are_scoped_to_each_reusable_playlist() {
        let mut store = AirPlayVideoStore::default();
        assert!(matches!(
            store.begin_play(
                play_request("first", "/video/master.m3u8"),
                "http://localhost:7000"
            ),
            HlsAction::Fetch { request_id: 1, .. }
        ));
        assert!(matches!(
            store.apply_action(
                ActionPlan::FcupPlaylist {
                    url: "/video/master.m3u8".to_owned(),
                    is_master: true,
                    playlist: b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\n/video/low.m3u8\n/video/mid.m3u8\n/video/high.m3u8\n".to_vec(),
                },
                "http://localhost:7000"
            ),
            HlsAction::Fetch { request_id: 2, .. }
        ));
        assert!(matches!(
            store.apply_action(
                ActionPlan::FcupPlaylist {
                    url: "/video/low.m3u8".to_owned(),
                    is_master: false,
                    playlist: b"#EXTM3U\n#EXTINF:4.5,\nlow.ts\n".to_vec(),
                },
                "http://localhost:7000"
            ),
            HlsAction::Fetch { request_id: 3, .. }
        ));
        assert!(matches!(
            store.begin_play(
                play_request("first", "/video/master.m3u8"),
                "http://localhost:7000"
            ),
            HlsAction::StartPlayback { .. }
        ));
        assert!(matches!(
            store.apply_action(
                ActionPlan::FcupPlaylist {
                    url: "/video/mid.m3u8".to_owned(),
                    is_master: false,
                    playlist: b"#EXTM3U\n#EXTINF:4.5,\nmid.ts\n".to_vec(),
                },
                "http://localhost:7000"
            ),
            HlsAction::Fetch { request_id: 4, .. }
        ));
        assert!(matches!(
            store.begin_play(
                play_request("second", "/video/other/master.m3u8"),
                "http://localhost:7000"
            ),
            HlsAction::Fetch { request_id: 1, .. }
        ));
        store.clear_all();
        assert!(matches!(
            store.begin_play(
                play_request("third", "/video/master.m3u8"),
                "http://localhost:7000"
            ),
            HlsAction::Fetch { request_id: 1, .. }
        ));
    }

    #[test]
    fn malformed_master_uri_table_starts_playback_without_partial_fcup_fetches() {
        let mut store = AirPlayVideoStore::default();
        let local_prefix = "http://localhost:7000";
        assert!(matches!(
            store.begin_play(
                play_request("uuid-malformed-master", "/video/master.m3u8"),
                local_prefix
            ),
            HlsAction::Fetch { .. }
        ));

        assert_eq!(
            store.apply_action(
                ActionPlan::FcupPlaylist {
                    url: "/video/master.m3u8".to_owned(),
                    is_master: true,
                    playlist: b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\n/video/valid.m3u8\n/video/incomplete\n".to_vec(),
                },
                local_prefix
            ),
            HlsAction::StartPlayback {
                location: format!("{local_prefix}/master.m3u8"),
                start_position_seconds: 3.0,
            }
        );
    }

    #[test]
    fn reusable_playlist_uses_saved_position_and_updates_session_id() {
        let mut store = AirPlayVideoStore::default();
        store.begin_play(play_request("uuid-2", "https://origin/clip.mp4"), "unused");
        store.set_resume_position("uuid-2", 8.0);
        let mut request = play_request("uuid-2", "ignored");
        request.session_id = "session-2".to_owned();
        assert_eq!(
            store.begin_play(request, "unused"),
            HlsAction::StartPlayback {
                location: "https://origin/clip.mp4".to_owned(),
                start_position_seconds: 8.0,
            }
        );
        assert_eq!(store.current_session_id(), Some("session-2"));
    }

    #[test]
    fn playlist_remove_retains_hls_cache_for_resume_by_uuid() {
        let local_prefix = "http://localhost:7000";
        let mut store = AirPlayVideoStore::default();
        assert!(matches!(
            store.begin_play(
                play_request("uuid-resume", "/video/master.m3u8"),
                local_prefix
            ),
            HlsAction::Fetch { .. }
        ));
        assert!(matches!(
            store.apply_action(
                ActionPlan::FcupPlaylist {
                    url: "/video/master.m3u8".to_owned(),
                    is_master: true,
                    playlist: b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\n/video/media.m3u8\n"
                        .to_vec(),
                },
                local_prefix,
            ),
            HlsAction::Fetch { .. }
        ));
        let started = store.apply_action(
            ActionPlan::FcupPlaylist {
                url: "/video/media.m3u8".to_owned(),
                is_master: false,
                playlist: b"#EXTM3U\n#EXTINF:100,\nsegment.ts\n#EXT-X-ENDLIST\n".to_vec(),
            },
            local_prefix,
        );
        let HlsAction::StartPlayback { location, .. } = started else {
            panic!("complete media playlist starts HLS playback")
        };

        assert_eq!(
            store.apply_action(
                ActionPlan::RemovePlaylist {
                    uuid: "uuid-resume".to_owned(),
                },
                local_prefix,
            ),
            HlsAction::RemoveCurrent {
                uuid: "uuid-resume".to_owned(),
            }
        );
        assert_eq!(store.current_video_index(), None);
        store.set_resume_position("uuid-resume", 42.0);

        let mut resumed_request = play_request("uuid-resume", "ignored");
        resumed_request.session_id = "session-2".to_owned();
        resumed_request.content_location = None;
        resumed_request.start_position_seconds = 4.0;
        assert_eq!(
            store.begin_play(resumed_request, local_prefix),
            HlsAction::StartPlayback {
                location,
                start_position_seconds: 42.0,
            }
        );
        assert_eq!(store.current_session_id(), Some("session-2"));
    }

    #[test]
    fn master_playlist_fetch_uses_requested_audio_language_before_collecting_uris() {
        let mut store = AirPlayVideoStore::with_profile_and_languages(None, "fr", "", "en-US");
        assert!(matches!(
            store.begin_play(
                play_request("uuid-fr", "/video/master.m3u8"),
                "http://localhost:7000"
            ),
            HlsAction::Fetch { .. }
        ));
        let action = store.apply_action(
            ActionPlan::FcupPlaylist {
                url: "/master.m3u8".to_owned(),
                is_master: true,
                playlist: b"#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",LANGUAGE=\"en-US\",DEFAULT=YES,AUTOSELECT=YES,URI=\"/video/en.m3u8\"\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",LANGUAGE=\"fr-FR\",DEFAULT=NO,AUTOSELECT=NO,URI=\"/video/fr.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=1000\n/video/main.m3u8\n".to_vec(),
            },
            "http://localhost:7000",
        );
        assert_eq!(
            action,
            HlsAction::Fetch {
                url: "/video/fr.m3u8".to_owned(),
                session_id: "session-1".to_owned(),
                request_id: 2,
            }
        );
        let master = store.playlist_for_request("/master.m3u8").unwrap();
        assert!(
            !master
                .windows(b"/video/en.m3u8".len())
                .any(|window| window == b"/video/en.m3u8")
        );
        assert!(
            master
                .windows(b"http://localhost:7000/fr.m3u8".len())
                .any(|window| window == b"http://localhost:7000/fr.m3u8")
        );
    }

    #[test]
    fn custom_hls_profile_filters_variants_before_collecting_fcup_uris() {
        let mut store = AirPlayVideoStore::with_profile_and_languages(
            Some("AVC:720".to_owned()),
            "",
            "",
            "en-US",
        );
        store.begin_play(
            play_request("uuid-profile", "/video/master.m3u8"),
            "http://localhost:7000",
        );
        let action = store.apply_action(
            ActionPlan::FcupPlaylist {
                url: "/master.m3u8".to_owned(),
                is_master: true,
                playlist: b"#EXTM3U\n#EXT-X-STREAM-INF:RESOLUTION=1920x1080,CODECS=\"avc1.640028\"\n/video/high.m3u8\n#EXT-X-STREAM-INF:RESOLUTION=1280x720,CODECS=\"avc1.640028\"\n/video/low.m3u8\n".to_vec(),
            },
            "http://localhost:7000",
        );
        assert_eq!(
            action,
            HlsAction::Fetch {
                url: "/video/low.m3u8".to_owned(),
                session_id: "session-1".to_owned(),
                request_id: 2,
            }
        );
        let master = store.playlist_for_request("/master.m3u8").unwrap();
        assert!(
            !master
                .windows(b"/video/high.m3u8".len())
                .any(|window| window == b"/video/high.m3u8")
        );
        assert!(
            master
                .windows(b"http://localhost:7000/low.m3u8".len())
                .any(|window| window == b"http://localhost:7000/low.m3u8")
        );
    }

    #[test]
    fn clearing_all_videos_keeps_hls_profile_and_language_preferences() {
        let mut store = AirPlayVideoStore::with_profile_and_languages(
            Some("AVC:720".to_owned()),
            "fr",
            "en",
            "fr-FR",
        );
        store.begin_play(
            play_request("uuid-clear", "/video/master.m3u8"),
            "http://localhost:7000",
        );

        store.clear_all();

        assert!(store.videos.is_empty());
        assert_eq!(store.current_video_index(), None);
        assert_eq!(store.custom_profile.as_deref(), Some("AVC:720"));
        assert_eq!(store.requested_audio, "fr");
        assert_eq!(store.requested_subtitles, "en");
        assert_eq!(store.system_languages, "fr-FR");
    }

    #[test]
    fn finishing_current_video_keeps_other_reusable_playlists() {
        let local_prefix = "http://localhost:7000";
        let mut store = AirPlayVideoStore::default();
        store.begin_play(
            play_request("uuid-cached", "https://origin/cached.mp4"),
            local_prefix,
        );
        let cached_index = store.current_video_index().unwrap();
        store.videos[cached_index]
            .as_mut()
            .unwrap()
            .media_playlists
            .push(MediaPlaylist {
                stats: MediaPlaylistStats {
                    duration_seconds: 120.0,
                    ..MediaPlaylistStats::default()
                },
                ..MediaPlaylist::default()
            });

        store.begin_play(
            play_request("uuid-current", "https://origin/current.mp4"),
            local_prefix,
        );
        let current_index = store.current_video_index().unwrap();
        assert_ne!(current_index, cached_index);
        assert_eq!(
            store.current_playback_location(),
            Some("https://origin/current.mp4")
        );

        assert!(store.finish_current());
        assert_eq!(store.current_video_index(), None);
        assert!(store.videos[cached_index].is_some());
        assert!(store.videos[current_index].is_none());
        assert!(!store.finish_current());

        assert_eq!(
            store.begin_play(play_request("uuid-cached", "ignored"), local_prefix),
            HlsAction::StartPlayback {
                location: "https://origin/cached.mp4".to_owned(),
                start_position_seconds: 3.0,
            }
        );
    }

    #[test]
    fn duplicate_media_uri_is_aliased_to_the_first_playlist() {
        let mut store = AirPlayVideoStore::default();
        store.begin_play(
            play_request("uuid-duplicate", "/video/master.m3u8"),
            "http://localhost:7000",
        );
        assert_eq!(
            store.apply_action(
                ActionPlan::FcupPlaylist {
                    url: "/master.m3u8".to_owned(),
                    is_master: true,
                    playlist: b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\n/video/shared.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2\n/video/shared.m3u8\n".to_vec(),
                },
                "http://localhost:7000",
            ),
            HlsAction::Fetch {
                url: "/video/shared.m3u8".to_owned(),
                session_id: "session-1".to_owned(),
                request_id: 2,
            }
        );
        let media = b"#EXTM3U\n#EXTINF:90,\na.ts\n#EXT-X-ENDLIST\n";
        assert_eq!(
            store.apply_action(
                ActionPlan::FcupPlaylist {
                    url: "/video/shared.m3u8".to_owned(),
                    is_master: false,
                    playlist: media.to_vec(),
                },
                "http://localhost:7000",
            ),
            HlsAction::Fetch {
                url: "/video/shared.m3u8".to_owned(),
                session_id: "session-1".to_owned(),
                request_id: 3,
            }
        );
        assert_eq!(
            store.apply_action(
                ActionPlan::FcupPlaylist {
                    url: "/video/shared.m3u8".to_owned(),
                    is_master: false,
                    playlist: media.to_vec(),
                },
                "http://localhost:7000",
            ),
            HlsAction::StartPlayback {
                location: "http://localhost:7000/master.m3u8".to_owned(),
                start_position_seconds: 3.0,
            }
        );
        let video = store.videos[store.current_video.unwrap()].as_ref().unwrap();
        assert_eq!(video.media_playlists[1].alias, Some(0));
        assert!(video.media_playlists[1].body.is_none());
    }

    #[test]
    fn full_store_keeps_one_free_slot_and_evicts_the_next_slot() {
        let mut store = AirPlayVideoStore::default();
        for number in 0..11 {
            let uuid = format!("uuid-{number}");
            let prefix = format!("/video{number}");
            let content_location = format!("{prefix}/master.m3u8");
            store.begin_play(
                play_request(&uuid, &content_location),
                "http://localhost:7000",
            );
            store.apply_action(
                ActionPlan::FcupPlaylist {
                    url: "/master.m3u8".to_owned(),
                    is_master: true,
                    playlist: format!(
                        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\n{prefix}/media.m3u8\n"
                    )
                    .into_bytes(),
                },
                "http://localhost:7000",
            );
            store.apply_action(
                ActionPlan::FcupPlaylist {
                    url: "/media.m3u8".to_owned(),
                    is_master: false,
                    playlist: b"#EXTM3U\n#EXTINF:100,\nsegment.ts\n#EXT-X-ENDLIST\n".to_vec(),
                },
                "http://localhost:7000",
            );
        }

        assert_eq!(store.videos[0].as_ref().unwrap().playback_uuid, "uuid-10");
        assert!(store.videos[1].is_none());
        assert!(store.videos[2..].iter().all(Option::is_some));
    }

    #[test]
    fn begin_play_reports_short_cleanup_then_capacity_eviction() {
        let stored_video = |uuid: &str, duration_seconds: f32| {
            let mut video = AirPlayVideo {
                playback_uuid: uuid.to_owned(),
                ..AirPlayVideo::default()
            };
            video.media_playlists.push(MediaPlaylist {
                stats: MediaPlaylistStats {
                    duration_seconds,
                    ..MediaPlaylistStats::default()
                },
                ..MediaPlaylist::default()
            });
            video
        };
        let mut store = AirPlayVideoStore {
            videos: (0..MAX_AIRPLAY_VIDEO)
                .map(|index| {
                    Some(if index == 0 {
                        stored_video("short-ad", 12.345678)
                    } else {
                        stored_video(&format!("long-{index}"), 100.0 + index as f32)
                    })
                })
                .collect(),
            ..AirPlayVideoStore::default()
        };

        let (action, cleaned) = store.begin_play_with_diagnostics(
            play_request("new-video", "https://origin/new.mp4"),
            "unused",
        );

        assert_eq!(
            action,
            HlsAction::StartPlayback {
                location: "https://origin/new.mp4".to_owned(),
                start_position_seconds: 3.0,
            }
        );
        assert_eq!(
            cleaned,
            vec![
                PlaylistCleanup {
                    playback_uuid: "short-ad".to_owned(),
                    duration_seconds: 12.345678,
                },
                PlaylistCleanup {
                    playback_uuid: "long-1".to_owned(),
                    duration_seconds: 101.0,
                },
            ]
        );
        assert_eq!(store.videos[0].as_ref().unwrap().playback_uuid, "new-video");
        assert!(store.videos[1].is_none());

        let (reuse_action, no_cleanup) =
            store.begin_play_with_diagnostics(play_request("new-video", "ignored"), "unused");
        assert!(matches!(reuse_action, HlsAction::StartPlayback { .. }));
        assert!(no_cleanup.is_empty());
    }
}
