//! Pure protocol and route classification from `lib/raop.c`.
//!
//! This module chooses a source handler from the request protocol, method,
//! URL, and previously classified HLS connection state. It does not implement
//! the handler, mutate the connection, or build the common HTTP response.

const RTSP_1_0: &[u8] = b"RTSP/1.0";
const HTTP_1_1: &[u8] = b"HTTP/1.1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Handler {
    RaopFeedback,
    PairPinStart,
    PairSetupPin,
    PairSetup,
    PairVerify,
    FairPlaySetup,
    AudioMode,
    RaopInfo,
    RaopOptions,
    Setup,
    GetParameter,
    SetParameter,
    Record,
    Flush,
    Teardown,
    Reverse,
    Play,
    GetProperty,
    Scrub,
    Rate,
    Stop,
    Action,
    FairPlaySetup2,
    ServerInfo,
    PlaybackInfo,
    SetProperty,
    Hls,
}

/// The result of the source's handler-selection block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteDecision {
    /// Invoke this handler. The handler may change the default response.
    Handler(Handler),
    /// No handler matched; preserve the response initialized to `200 OK`.
    DefaultOk,
    /// No HTTP route matched in the reference server's registered URL table.
    NotFound,
    /// An unrecognized method under `RTSP/1.0` changes the response to 501.
    NotImplemented,
}

/// Select a handler using the branch and comparison rules in `raop.c:405-478`.
///
/// Inputs are byte strings because the C parser exposes NUL-terminated byte
/// strings. Matching is case-sensitive. Any bytes after the first NUL are
/// ignored, matching C `strcmp`, `strncmp`, and `strstr` on those fields.
///
/// The caller supplies `hls_request`, which is computed from connection state
/// elsewhere. When true, the source selects the HLS handler regardless of the
/// method, URL, or protocol.
pub fn classify(method: &[u8], url: &[u8], protocol: &[u8], hls_request: bool) -> RouteDecision {
    let method = c_string_prefix(method);
    let url = c_string_prefix(url);
    let protocol = c_string_prefix(protocol);

    if !hls_request && protocol == RTSP_1_0 {
        let handler = match method {
            b"POST" => match url {
                b"/feedback" => Some(Handler::RaopFeedback),
                b"/pair-pin-start" => Some(Handler::PairPinStart),
                b"/pair-setup-pin" => Some(Handler::PairSetupPin),
                b"/pair-setup" => Some(Handler::PairSetup),
                b"/pair-verify" => Some(Handler::PairVerify),
                b"/fp-setup" => Some(Handler::FairPlaySetup),
                b"/audioMode" => Some(Handler::AudioMode),
                _ => None,
            },
            b"GET" => contains(url, b"/info").then_some(Handler::RaopInfo),
            b"OPTIONS" => Some(Handler::RaopOptions),
            b"SETUP" => Some(Handler::Setup),
            b"GET_PARAMETER" => Some(Handler::GetParameter),
            b"SET_PARAMETER" => Some(Handler::SetParameter),
            b"RECORD" => Some(Handler::Record),
            b"FLUSH" => Some(Handler::Flush),
            b"TEARDOWN" => Some(Handler::Teardown),
            _ => return RouteDecision::NotImplemented,
        };
        return handler.map_or(RouteDecision::DefaultOk, RouteDecision::Handler);
    }

    if !hls_request && protocol == HTTP_1_1 {
        let handler = match method {
            b"POST" => match url {
                b"/reverse" => Some(Handler::Reverse),
                b"/play" => Some(Handler::Play),
                value if value.starts_with(b"/getProperty?") => Some(Handler::GetProperty),
                value if value.starts_with(b"/scrub?") => Some(Handler::Scrub),
                value if value.starts_with(b"/rate?") => Some(Handler::Rate),
                b"/stop" => Some(Handler::Stop),
                b"/action" => Some(Handler::Action),
                b"/fp-setup2" => Some(Handler::FairPlaySetup2),
                _ => None,
            },
            b"GET" => match url {
                b"/server-info" => Some(Handler::ServerInfo),
                b"/playback-info" => Some(Handler::PlaybackInfo),
                _ => None,
            },
            b"PUT" => url
                .starts_with(b"/setProperty?")
                .then_some(Handler::SetProperty),
            _ => None,
        };
        return handler.map_or(RouteDecision::NotFound, RouteDecision::Handler);
    }

    if hls_request {
        return RouteDecision::Handler(Handler::Hls);
    }

    RouteDecision::DefaultOk
}

fn c_string_prefix(value: &[u8]) -> &[u8] {
    let end = value
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(value.len());
    &value[..end]
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify_rtsp(method: &[u8], url: &[u8]) -> RouteDecision {
        classify(method, url, RTSP_1_0, false)
    }

    fn classify_http(method: &[u8], url: &[u8]) -> RouteDecision {
        classify(method, url, HTTP_1_1, false)
    }

    #[test]
    fn classifies_each_rtsp_post_handler_with_exact_paths() {
        let fixtures = [
            (b"/feedback".as_slice(), Handler::RaopFeedback),
            (b"/pair-pin-start", Handler::PairPinStart),
            (b"/pair-setup-pin", Handler::PairSetupPin),
            (b"/pair-setup", Handler::PairSetup),
            (b"/pair-verify", Handler::PairVerify),
            (b"/fp-setup", Handler::FairPlaySetup),
            (b"/audioMode", Handler::AudioMode),
        ];

        for (url, handler) in fixtures {
            assert_eq!(
                classify_rtsp(b"POST", url),
                RouteDecision::Handler(handler),
                "POST {}",
                String::from_utf8_lossy(url)
            );
        }
        assert_eq!(
            classify_rtsp(b"POST", b"/pair-setup/"),
            RouteDecision::DefaultOk
        );
    }

    #[test]
    fn rtsp_info_searches_for_substring_and_methods_ignore_url() {
        assert_eq!(
            classify_rtsp(b"GET", b"/path/info/txtAirPlay"),
            RouteDecision::Handler(Handler::RaopInfo)
        );
        assert_eq!(classify_rtsp(b"GET", b"/INFO"), RouteDecision::DefaultOk);
        assert_eq!(
            classify_rtsp(b"OPTIONS", b"/anything"),
            RouteDecision::Handler(Handler::RaopOptions)
        );
        assert_eq!(
            classify_rtsp(b"SETUP", b"/anything"),
            RouteDecision::Handler(Handler::Setup)
        );
    }

    #[test]
    fn rtsp_parameter_and_session_methods_match_the_source_table() {
        let fixtures = [
            (b"GET_PARAMETER".as_slice(), Handler::GetParameter),
            (b"SET_PARAMETER", Handler::SetParameter),
            (b"RECORD", Handler::Record),
            (b"FLUSH", Handler::Flush),
            (b"TEARDOWN", Handler::Teardown),
        ];

        for (method, handler) in fixtures {
            assert_eq!(
                classify_rtsp(method, b"/ignored"),
                RouteDecision::Handler(handler)
            );
        }
    }

    #[test]
    fn only_unknown_rtsp_methods_override_the_default_with_501() {
        assert_eq!(
            classify_rtsp(b"PATCH", b"/info"),
            RouteDecision::NotImplemented
        );
        assert_eq!(
            classify_rtsp(b"POST", b"/unknown"),
            RouteDecision::DefaultOk
        );
        assert_eq!(
            classify(b"PATCH", b"/unknown", b"RTSP/2.0", false),
            RouteDecision::DefaultOk
        );
    }

    #[test]
    fn classifies_http_post_handlers_using_exact_or_prefix_matching() {
        let fixtures = [
            (b"/reverse".as_slice(), Handler::Reverse),
            (b"/play", Handler::Play),
            (b"/getProperty?name=x", Handler::GetProperty),
            (b"/scrub?position=1", Handler::Scrub),
            (b"/rate?value=1", Handler::Rate),
            (b"/stop", Handler::Stop),
            (b"/action", Handler::Action),
            (b"/fp-setup2", Handler::FairPlaySetup2),
        ];

        for (url, handler) in fixtures {
            assert_eq!(
                classify_http(b"POST", url),
                RouteDecision::Handler(handler),
                "POST {}",
                String::from_utf8_lossy(url)
            );
        }
        assert_eq!(
            classify_http(b"POST", b"/getProperty"),
            RouteDecision::NotFound
        );
        assert_eq!(classify_http(b"POST", b"/play/"), RouteDecision::NotFound);
    }

    #[test]
    fn classifies_http_get_and_put_routes_exactly() {
        assert_eq!(
            classify_http(b"GET", b"/server-info"),
            RouteDecision::Handler(Handler::ServerInfo)
        );
        assert_eq!(
            classify_http(b"GET", b"/playback-info"),
            RouteDecision::Handler(Handler::PlaybackInfo)
        );
        assert_eq!(
            classify_http(b"GET", b"/server-info?x=1"),
            RouteDecision::NotFound
        );
        assert_eq!(
            classify_http(b"PUT", b"/setProperty?foo=bar"),
            RouteDecision::Handler(Handler::SetProperty)
        );
        assert_eq!(
            classify_http(b"PUT", b"/setProperty"),
            RouteDecision::NotFound
        );
    }

    #[test]
    fn hls_state_overrides_protocol_method_and_path() {
        assert_eq!(
            classify(b"PATCH", b"/unrecognized", b"RTSP/1.0", true),
            RouteDecision::Handler(Handler::Hls)
        );
        assert_eq!(
            classify(b"POST", b"/play", b"HTTP/1.1", true),
            RouteDecision::Handler(Handler::Hls)
        );
    }

    #[test]
    fn route_inputs_follow_c_string_nul_boundaries_and_case_sensitivity() {
        assert_eq!(
            classify(
                b"GET\0POST",
                b"/server-info\0/other",
                b"HTTP/1.1\0RTSP/1.0",
                false
            ),
            RouteDecision::Handler(Handler::ServerInfo)
        );
        assert_eq!(classify_http(b"post", b"/play"), RouteDecision::NotFound);
        assert_eq!(classify_http(b"POST", b"/Play"), RouteDecision::NotFound);
    }
}
