//! Shared RFC 2617 password state for the initial RAOP SETUP request.

use std::io;
use std::sync::{Arc, Mutex};

const MAX_PASSWORD_PROMPTS: u32 = 3;
const NONCE_BYTES: usize = 16;

#[derive(Clone, Debug)]
pub struct PasswordAuth {
    mode: u8,
    password: Option<String>,
    state: Arc<Mutex<PasswordAuthState>>,
}

#[derive(Debug, Default)]
struct PasswordAuthState {
    nonce: Option<String>,
    random_password: Option<RandomPassword>,
    auth_fail_count: u32,
}

#[derive(Debug)]
struct RandomPassword {
    digits: String,
    device_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthorizationDecision {
    Authorized,
    Challenge {
        value: String,
        display_pin: Option<String>,
    },
}

impl Default for PasswordAuth {
    fn default() -> Self {
        Self::new(0, None)
    }
}

impl PasswordAuth {
    pub fn new(mode: u8, password: Option<String>) -> Self {
        Self {
            mode,
            password,
            state: Arc::new(Mutex::new(PasswordAuthState::default())),
        }
    }

    pub fn required(&self) -> bool {
        (self.mode == 2
            && self
                .password
                .as_ref()
                .is_some_and(|password| !password.is_empty()))
            || self.mode == 3
    }

    /// Apply the source's process-shared challenge state and connection-local
    /// authenticated flag to one initial SETUP request.
    pub fn authorize(
        &self,
        method: &[u8],
        authorization: Option<&[u8]>,
        device_id: &str,
        authenticated: &mut bool,
    ) -> io::Result<AuthorizationDecision> {
        if !self.required() || *authenticated {
            return Ok(AuthorizationDecision::Authorized);
        }

        let mut state = lock(&self.state);
        if self.mode == 3 {
            if authorization.is_none()
                && state
                    .random_password
                    .as_ref()
                    .is_some_and(|password| password.device_id != device_id)
            {
                state.random_password = None;
            }
            if state.random_password.is_some() && state.auth_fail_count >= MAX_PASSWORD_PROMPTS {
                eprintln!("Too many authentication failures: generate new random password");
                state.random_password = None;
            }
            if state.random_password.is_none() {
                let pin = uxplay_pairing::port::random_pin().unwrap_or(1234);
                state.random_password = Some(RandomPassword {
                    digits: format!("{pin:04}"),
                    device_id: device_id.to_owned(),
                });
                state.auth_fail_count = 0;
            }
        }

        let mut display_pin = None;
        if self.mode == 3
            && authorization.is_none()
            && let Some(password) = state.random_password.as_ref()
        {
            display_pin = Some(password.digits.clone());
            state.auth_fail_count = state.auth_fail_count.saturating_add(1);
        }

        if let Some(authorization) = authorization {
            let password = if self.mode == 3 {
                state
                    .random_password
                    .as_ref()
                    .map(|password| password.digits.as_bytes())
                    .unwrap_or_default()
            } else {
                self.password.as_deref().unwrap_or_default().as_bytes()
            };
            let digest_valid = uxplay_pairing::port::digest_verify(method, authorization, password)
                .map_err(|error| {
                    io::Error::other(format!("RAOP digest verification failed: {error:?}"))
                })?;
            if !digest_valid && self.mode != 3 {
                state.auth_fail_count = state.auth_fail_count.saturating_add(1);
            }

            let request_nonce = authorization_nonce(authorization);
            *authenticated = digest_valid
                && request_nonce.is_some_and(|nonce| {
                    state
                        .nonce
                        .as_deref()
                        .is_some_and(|expected| nonce == expected.as_bytes())
                });

            if *authenticated && self.mode == 3 {
                state.random_password = None;
            }
            state.nonce = None;
        }

        if *authenticated {
            return Ok(AuthorizationDecision::Authorized);
        }

        let nonce = random_nonce()?;
        state.nonce = Some(nonce.clone());
        Ok(AuthorizationDecision::Challenge {
            value: format!("Digest realm=\"raop\", nonce=\"{nonce}\""),
            display_pin,
        })
    }
}

/// Return the device ID for the source's first SETUP shape (encrypted key and
/// IV), or `None` for follow-up SETUP requests.
pub fn initial_setup_device_id(body: &[u8]) -> Option<String> {
    if !body.starts_with(b"bplist00") {
        return None;
    }
    let value = plist::Value::from_reader(std::io::Cursor::new(body)).ok()?;
    let dictionary = value.as_dictionary()?;
    if dictionary.get("ekey")?.as_data().is_none() || dictionary.get("eiv")?.as_data().is_none() {
        return None;
    }
    Some(
        dictionary
            .get("deviceID")
            .and_then(plist::Value::as_string)
            .unwrap_or_default()
            .to_owned(),
    )
}

fn authorization_nonce(authorization: &[u8]) -> Option<&[u8]> {
    let prefix = b"nonce=\"";
    let start = authorization
        .windows(prefix.len())
        .position(|window| window == prefix)?
        + prefix.len();
    let remainder = authorization.get(start..)?;
    let end = remainder.iter().position(|byte| *byte == b'"')?;
    (end == 32).then_some(&remainder[..end])
}

fn random_nonce() -> io::Result<String> {
    let mut bytes = [0; NONCE_BYTES];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uxplay_crypto::port::get_md5_hex;

    fn digest_header(method: &[u8], uri: &[u8], password: &[u8], nonce: &[u8]) -> Vec<u8> {
        let username = b"test-user";
        let realm = b"raop";
        let nc = b"00000001";
        let cnonce = b"c0ffee";
        let qop = b"auth";
        let mut raw = Vec::new();
        raw.extend_from_slice(username);
        raw.push(b':');
        raw.extend_from_slice(realm);
        raw.push(b':');
        raw.extend_from_slice(password);
        let h1 = get_md5_hex(&raw).unwrap();
        raw.clear();
        raw.extend_from_slice(method);
        raw.push(b':');
        raw.extend_from_slice(uri);
        let h2 = get_md5_hex(&raw).unwrap();
        raw.clear();
        raw.extend_from_slice(h1.as_bytes());
        raw.push(b':');
        raw.extend_from_slice(nonce);
        raw.push(b':');
        raw.extend_from_slice(nc);
        raw.push(b':');
        raw.extend_from_slice(cnonce);
        raw.push(b':');
        raw.extend_from_slice(qop);
        raw.push(b':');
        raw.extend_from_slice(h2.as_bytes());
        let response = get_md5_hex(&raw).unwrap();
        format!(
            "Digest username=\"test-user\", realm=\"raop\", nonce=\"{}\", uri=\"/\", qop=auth, nc=00000001, cnonce=\"c0ffee\", response=\"{response}\"",
            String::from_utf8_lossy(nonce)
        )
        .into_bytes()
    }

    #[test]
    fn fixed_password_challenge_accepts_the_digest_and_consumes_nonce() {
        let auth = PasswordAuth::new(2, Some("swordfish".into()));
        let mut authenticated = false;
        let AuthorizationDecision::Challenge { value, .. } = auth
            .authorize(b"SETUP", None, "device", &mut authenticated)
            .unwrap()
        else {
            panic!("expected digest challenge");
        };
        let nonce = authorization_nonce(value.as_bytes()).unwrap().to_vec();
        let header = digest_header(b"SETUP", b"/", b"swordfish", &nonce);
        assert_eq!(
            auth.authorize(b"SETUP", Some(&header), "device", &mut authenticated)
                .unwrap(),
            AuthorizationDecision::Authorized
        );
        assert!(authenticated);

        let mut another_connection = false;
        assert!(matches!(
            auth.authorize(b"SETUP", Some(&header), "device", &mut another_connection)
                .unwrap(),
            AuthorizationDecision::Challenge { .. }
        ));
    }

    #[test]
    fn invalid_fixed_password_digest_is_rechallenged() {
        let auth = PasswordAuth::new(2, Some("swordfish".into()));
        let mut authenticated = false;
        let AuthorizationDecision::Challenge { value, .. } = auth
            .authorize(b"SETUP", None, "device", &mut authenticated)
            .unwrap()
        else {
            panic!("expected initial challenge");
        };
        let nonce = authorization_nonce(value.as_bytes()).unwrap().to_vec();
        let header = digest_header(b"SETUP", b"/", b"wrong", &nonce);
        let AuthorizationDecision::Challenge {
            value: refreshed, ..
        } = auth
            .authorize(b"SETUP", Some(&header), "device", &mut authenticated)
            .unwrap()
        else {
            panic!("wrong password must not authorize");
        };
        assert_ne!(value, refreshed);
        assert!(!authenticated);
    }

    #[test]
    fn random_password_is_displayed_and_rotates_after_three_prompts() {
        let auth = PasswordAuth::new(3, None);
        let mut authenticated = false;
        let mut displayed = Vec::new();
        for _ in 0..4 {
            let AuthorizationDecision::Challenge { display_pin, .. } = auth
                .authorize(b"SETUP", None, "device", &mut authenticated)
                .unwrap()
            else {
                panic!("expected random-password challenge");
            };
            displayed.push(display_pin.unwrap());
        }
        assert_eq!(displayed[0], displayed[1]);
        assert_eq!(displayed[1], displayed[2]);
        let state = lock(&auth.state);
        assert_eq!(state.auth_fail_count, 1);
        assert!(
            state
                .random_password
                .as_ref()
                .is_some_and(|pin| pin.digits.len() == 4)
        );
    }

    #[test]
    fn initial_setup_detection_requires_encrypted_key_and_iv_data() {
        let mut dictionary = plist::Dictionary::new();
        dictionary.insert("deviceID".into(), plist::Value::String("abc".into()));
        dictionary.insert("ekey".into(), plist::Value::Data(vec![0; 72]));
        dictionary.insert("eiv".into(), plist::Value::Data(vec![0; 16]));
        let mut body = Vec::new();
        plist::Value::Dictionary(dictionary)
            .to_writer_binary(&mut body)
            .unwrap();
        assert_eq!(initial_setup_device_id(&body).as_deref(), Some("abc"));
        assert_eq!(initial_setup_device_id(b"not a plist"), None);
    }
}
