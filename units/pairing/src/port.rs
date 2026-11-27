//! Safe implementation of the observable behavior in `lib/pairing.c`.

use openssl::hash::{MessageDigest, hash};
use openssl::pkey::PKey;
use std::{fs, path::Path};
use uxplay_crypto::port::{
    AesContext, AesMode, CryptoError, Direction, Ed25519Key, X25519Key, gcm_decrypt, gcm_encrypt,
    pk_to_base64, random_bytes,
};
use uxplay_srp::port::{
    HashAlgorithm, SrpError, Verifier, create_salted_verification_key, create_server_ephemeral_key,
};

unsafe extern "C" {
    fn OPENSSL_cleanse(ptr: *mut std::ffi::c_void, len: usize);
}

pub const PAIRING_SIG_SIZE: usize = 64;
pub const SRP_USERNAME_SIZE: usize = 24;
pub const SRP_SESSION_KEY_SIZE: usize = 40;
pub const SRP_VERIFIER_SIZE: usize = 256;
pub const SRP_SALT_SIZE: usize = 16;
pub const SRP_PROOF_SIZE: usize = 20;
pub const SRP_PRIVATE_KEY_SIZE: usize = 32;

const KEY_VERIFY: &[u8] = b"Pair-Verify-AES-Key";
const IV_VERIFY: &[u8] = b"Pair-Verify-AES-IV";
const KEY_SETUP: &[u8] = b"Pair-Setup-AES-Key";
const IV_SETUP: &[u8] = b"Pair-Setup-AES-IV";

/// Errors raised by the safe API for inputs that made the original C code fail.
#[derive(Debug)]
pub enum PairingError {
    Crypto(CryptoError),
    Srp(SrpError),
    InvalidInput(&'static str),
}

impl From<CryptoError> for PairingError {
    fn from(value: CryptoError) -> Self {
        Self::Crypto(value)
    }
}
impl From<SrpError> for PairingError {
    fn from(value: SrpError) -> Self {
        Self::Srp(value)
    }
}
impl From<openssl::error::ErrorStack> for PairingError {
    fn from(value: openssl::error::ErrorStack) -> Self {
        Self::Crypto(CryptoError::OpenSsl(value))
    }
}

pub type Result<T> = std::result::Result<T, PairingError>;

/// Persistent server identity (`pairing_t` in C).
pub struct Pairing {
    ed: Ed25519Key,
}

impl Pairing {
    /// Load a persisted key or create one; without a key path, derive it from the device id.
    pub fn init_generate(device_id: &[u8], keyfile: Option<&Path>) -> Result<(Self, bool)> {
        let device_id = c_bytes(device_id);
        let (ed, created) = if let Some(path) = keyfile.filter(|path| !path.as_os_str().is_empty())
        {
            if let Ok(bytes) = fs::read(path)
                && let Ok(key) = PKey::private_key_from_pem(&bytes)
            {
                // The C source accepts any PEM private-key type here; an incompatible key
                // then fails later when an Ed25519 operation is attempted.
                (Ed25519Key::Private(key), false)
            } else {
                let key = Ed25519Key::generate_random()?;
                let written = fs::write(path, key.to_pem()?).is_ok();
                (key, written)
            }
        } else {
            (Ed25519Key::from_device_id(device_id)?, false)
        };
        Ok((Self { ed }, created))
    }

    pub fn get_public_key(&self) -> Result<[u8; 32]> {
        Ok(self.ed.raw_public()?)
    }

    pub fn session(&self) -> Result<PairingSession> {
        Ok(PairingSession {
            status: PairingStatus::Initial,
            ed_ours: self.ed.copy()?,
            ed_theirs: None,
            ecdh_ours: None,
            ecdh_theirs: None,
            ecdh_secret: [0; 32],
            username: [0; SRP_USERNAME_SIZE + 1],
            client_pk: [0; 32],
            pair_setup: false,
            srp: None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingStatus {
    Initial,
    Setup,
    Handshake,
    Finished,
}

struct SrpState {
    salt: [u8; SRP_SALT_SIZE],
    verifier: [u8; SRP_VERIFIER_SIZE],
    session_key: Option<[u8; SRP_SESSION_KEY_SIZE]>,
    private_key: [u8; SRP_PRIVATE_KEY_SIZE],
}

/// Stateful pairing session (`pairing_session_t` in C).
pub struct PairingSession {
    status: PairingStatus,
    ed_ours: Ed25519Key,
    ed_theirs: Option<Ed25519Key>,
    ecdh_ours: Option<X25519Key>,
    ecdh_theirs: Option<X25519Key>,
    ecdh_secret: [u8; 32],
    username: [u8; SRP_USERNAME_SIZE + 1],
    client_pk: [u8; 32],
    pair_setup: bool,
    srp: Option<SrpState>,
}

impl PairingSession {
    pub fn status(&self) -> PairingStatus {
        self.status
    }

    pub fn set_setup_status(&mut self) {
        self.status = PairingStatus::Setup;
    }

    /// C return convention: 0 in setup/handshake states, -1 otherwise.
    pub fn check_handshake_status(&self) -> i32 {
        match self.status {
            PairingStatus::Setup | PairingStatus::Handshake => 0,
            PairingStatus::Initial | PairingStatus::Finished => -1,
        }
    }

    /// Establish an ephemeral X25519 exchange and load the peer's Ed25519 key.
    /// Returns the source C status (`0`, or `-1` after a finished session).
    pub fn handshake(&mut self, ecdh_key: &[u8; 32], ed_key: &[u8; 32]) -> Result<i32> {
        if self.status == PairingStatus::Finished {
            return Ok(-1);
        }
        let theirs_ecdh = X25519Key::from_raw_public(ecdh_key)?;
        let theirs_ed = Ed25519Key::from_raw_public(ed_key)?;
        let ours_ecdh = X25519Key::generate()?;
        let secret = ours_ecdh.derive(&theirs_ecdh)?;

        self.ecdh_theirs = Some(theirs_ecdh);
        self.ed_theirs = Some(theirs_ed);
        self.ecdh_secret = secret;
        self.ecdh_ours = Some(ours_ecdh);
        self.status = PairingStatus::Handshake;
        Ok(0)
    }

    pub fn get_public_key(&self) -> Result<Option<[u8; 32]>> {
        if self.status != PairingStatus::Handshake {
            return Ok(None);
        }
        Ok(Some(
            self.ecdh_ours
                .as_ref()
                .ok_or(PairingError::InvalidInput("missing local X25519 key"))?
                .raw_public()?,
        ))
    }

    /// C return convention: 0 before the exchange; 1 and copy the secret afterwards.
    pub fn get_ecdh_secret_key(&self, output: &mut [u8]) -> i32 {
        if self.status == PairingStatus::Initial {
            return 0;
        }
        if output.len() < self.ecdh_secret.len() {
            return -1;
        }
        output[..32].copy_from_slice(&self.ecdh_secret);
        1
    }

    /// Fill a source-compatible truncated nonce. Returns C statuses `0`, `-1`, or `-2`.
    pub fn make_nonce(
        &self,
        local_time: u64,
        client_data: &[u8],
        output: &mut [u8],
        len: i32,
    ) -> Result<i32> {
        if len > 64 {
            return Ok(-1);
        }
        if len <= 0 || output.len() < len as usize {
            return Ok(-2);
        }
        let client_len = client_data
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(client_data.len());
        let mut input = Vec::with_capacity(8 + client_len + 32);
        input.extend_from_slice(&local_time.to_ne_bytes());
        input.extend_from_slice(&client_data[..client_len]);
        input.extend_from_slice(&self.ed_ours.raw_public()?);
        let digest = hash(MessageDigest::sha512(), &input)?;
        output[..len as usize].copy_from_slice(&digest[..len as usize]);
        Ok(0)
    }

    /// Sign both X25519 public keys and encrypt the signature with the verify key/IV.
    /// Returns `-1` unless the session is in handshake state.
    pub fn get_signature(&self) -> Result<Option<[u8; PAIRING_SIG_SIZE]>> {
        if self.status != PairingStatus::Handshake {
            return Ok(None);
        }
        let ours = self
            .ecdh_ours
            .as_ref()
            .ok_or(PairingError::InvalidInput("missing local X25519 key"))?;
        let theirs = self
            .ecdh_theirs
            .as_ref()
            .ok_or(PairingError::InvalidInput("missing peer X25519 key"))?;
        let mut message = [0; PAIRING_SIG_SIZE];
        message[..32].copy_from_slice(&ours.raw_public()?);
        message[32..].copy_from_slice(&theirs.raw_public()?);
        let signature = self.ed_ours.sign(&message)?;
        let key = derive_key(KEY_VERIFY, &self.ecdh_secret, 16)?;
        let iv = derive_key(IV_VERIFY, &self.ecdh_secret, 16)?;
        let mut aes = AesContext::new(AesMode::Ctr, Direction::Encrypt, &key, &iv)?;
        let encrypted = aes.ctr_encrypt(&signature)?;
        Ok(Some(encrypted.try_into().map_err(|_| {
            PairingError::InvalidInput("unexpected signature size")
        })?))
    }

    /// Verify the peer's encrypted signature, returning C status `-1`, `-2`, or `0`.
    pub fn finish(&mut self, signature: &[u8; PAIRING_SIG_SIZE]) -> Result<i32> {
        if self.status != PairingStatus::Handshake {
            return Ok(-1);
        }
        let key = derive_key(KEY_VERIFY, &self.ecdh_secret, 16)?;
        let iv = derive_key(IV_VERIFY, &self.ecdh_secret, 16)?;
        let mut aes = AesContext::new(AesMode::Ctr, Direction::Encrypt, &key, &iv)?;
        let _fake_round = aes.ctr_encrypt(&[0; PAIRING_SIG_SIZE])?;
        let plaintext = aes.ctr_encrypt(signature)?;
        let ours = self
            .ecdh_ours
            .as_ref()
            .ok_or(PairingError::InvalidInput("missing local X25519 key"))?;
        let theirs = self
            .ecdh_theirs
            .as_ref()
            .ok_or(PairingError::InvalidInput("missing peer X25519 key"))?;
        let mut message = [0; PAIRING_SIG_SIZE];
        message[..32].copy_from_slice(&theirs.raw_public()?);
        message[32..].copy_from_slice(&ours.raw_public()?);
        let peer = self
            .ed_theirs
            .as_ref()
            .ok_or(PairingError::InvalidInput("missing peer Ed25519 key"))?;
        if !peer.verify(&message, &plaintext)? {
            return Ok(-2);
        }
        self.status = PairingStatus::Finished;
        Ok(0)
    }

    /// Create a server SRP offer. The returned salt intentionally preserves the C
    /// implementation's leading-zero slice quirk (`padded[..minimal_len]`).
    pub fn srp_new_user(&mut self, device_id: &[u8], pin: &[u8]) -> Result<Option<SrpOffer>> {
        let device_id = c_bytes(device_id);
        if device_id.len() > SRP_USERNAME_SIZE {
            return Ok(None);
        }
        self.username = [0; SRP_USERNAME_SIZE + 1];
        self.username[..device_id.len()].copy_from_slice(device_id);
        self.srp = None;

        let private_key: [u8; SRP_PRIVATE_KEY_SIZE] = random_bytes(SRP_PRIVATE_KEY_SIZE)?
            .try_into()
            .map_err(|_| PairingError::InvalidInput("unexpected random output size"))?;
        let (salt_min, verifier_min) = create_salted_verification_key(
            HashAlgorithm::Sha1,
            0,
            device_id,
            c_bytes(pin),
            None,
            None,
        )?;
        if salt_min.len() > SRP_SALT_SIZE || verifier_min.len() > SRP_VERIFIER_SIZE {
            return Err(PairingError::InvalidInput(
                "SRP value exceeds protocol width",
            ));
        }
        let mut salt = [0; SRP_SALT_SIZE];
        salt[SRP_SALT_SIZE - salt_min.len()..].copy_from_slice(&salt_min);
        let mut verifier = [0; SRP_VERIFIER_SIZE];
        verifier[SRP_VERIFIER_SIZE - verifier_min.len()..].copy_from_slice(&verifier_min);
        let server_public = create_server_ephemeral_key(
            HashAlgorithm::Sha1,
            0,
            &verifier_min,
            &private_key,
            None,
            None,
            true,
        )?;
        let salt_view = salt[..salt_min.len()].to_vec();
        self.srp = Some(SrpState {
            salt,
            verifier,
            session_key: None,
            private_key,
        });
        Ok(Some(SrpOffer {
            salt: salt_view,
            server_public,
        }))
    }

    /// Validate the client's 20-byte SRP proof, then replace it with HAMK in-place.
    /// Failed authentication clears pending SRP state, matching the C transition.
    pub fn srp_validate_proof(&mut self, client_public: &[u8], proof: &mut [u8]) -> Result<i32> {
        let Some(srp) = self.srp.as_ref() else {
            return Ok(-1);
        };
        if proof.len() < SRP_PROOF_SIZE {
            return Ok(-1);
        }
        let username_len = self
            .username
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.username.len());
        let verifier = Verifier::new(
            HashAlgorithm::Sha1,
            0,
            &self.username[..username_len],
            &srp.salt,
            &srp.verifier,
            client_public,
            &srp.private_key,
            None,
            None,
            true,
        )?;
        let Some(mut verifier) = verifier else {
            self.srp = None;
            return Ok(-1);
        };
        if !verifier.verify_session(&proof[..SRP_PROOF_SIZE]) {
            self.srp = None;
            return Ok(-1);
        }
        let key: [u8; SRP_SESSION_KEY_SIZE] = verifier
            .session_key()
            .try_into()
            .map_err(|_| PairingError::InvalidInput("unexpected SRP session key size"))?;
        let hamk = verifier.hamk();
        let copy_len = proof.len().min(hamk.len());
        self.srp.as_mut().expect("SRP state retained").session_key = Some(key);
        proof[..copy_len].copy_from_slice(&hamk[..copy_len]);
        Ok(0)
    }

    /// Decrypt/authenticate the client's Ed25519 key, then return its encrypted server-key reply.
    /// The SRP state is consumed before GCM authentication just as it is in the C implementation.
    pub fn confirm_pair_setup(
        &mut self,
        client_epk: &[u8; 32],
        auth_tag: &[u8; 16],
        pairing: &Pairing,
    ) -> Result<Option<([u8; 32], [u8; 16])>> {
        let Some(srp) = self.srp.take() else {
            return Ok(None);
        };
        let Some(session_key) = srp.session_key.as_ref().copied() else {
            return Ok(None);
        };
        let key = derive_key(KEY_SETUP, &session_key, 16)?;
        let mut iv: [u8; 16] = derive_key(IV_SETUP, &session_key, 16)?
            .try_into()
            .map_err(|_| PairingError::InvalidInput("unexpected setup IV size"))?;
        iv[15] = iv[15].wrapping_add(1);
        let decrypted = gcm_decrypt(client_epk, &key, &iv, auth_tag)?;
        if !decrypted.authenticated || decrypted.plaintext.len() != self.client_pk.len() {
            return Ok(None);
        }
        self.client_pk.copy_from_slice(&decrypted.plaintext);
        self.pair_setup = true;
        let public = pairing.get_public_key()?;
        iv[15] = iv[15].wrapping_add(1);
        let (ciphertext, tag) = gcm_encrypt(&public, &key, &iv)?;
        let ciphertext: [u8; 32] = ciphertext
            .try_into()
            .map_err(|_| PairingError::InvalidInput("unexpected GCM ciphertext size"))?;
        Ok(Some((ciphertext, tag)))
    }

    pub fn client_data(&self) -> (&[u8], Option<String>) {
        let len = self
            .username
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.username.len());
        let public = self.pair_setup.then(|| pk_to_base64(&self.client_pk));
        (&self.username[..len], public)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SrpOffer {
    pub salt: Vec<u8>,
    pub server_public: Vec<u8>,
}

/// RFC 2617 MD5 Digest authentication as implemented by the original permissive token scanner.
/// Missing tokens safely fail authentication; valid C requests retain case-sensitive comparison.
pub fn digest_verify(method: &[u8], authorization: &[u8], password: &[u8]) -> Result<bool> {
    let method = c_bytes(method);
    let authorization = c_bytes(authorization);
    let password = c_bytes(password);
    let mut cursor = 0;
    let Some(username) = token(authorization, &mut cursor, b"username", b'"', b'"') else {
        return Ok(false);
    };
    let Some(realm) = token(authorization, &mut cursor, b"realm", b'"', b'"') else {
        return Ok(false);
    };
    let Some(nonce) = token(authorization, &mut cursor, b"nonce", b'"', b'"') else {
        return Ok(false);
    };
    let Some(uri) = token(authorization, &mut cursor, b"uri", b'"', b'"') else {
        return Ok(false);
    };
    let qop = token(authorization, &mut cursor, b"qop", b'=', b',');
    let (nc, cnonce) = if qop.is_some() {
        let Some(nc) = token(authorization, &mut cursor, b"nc", b'=', b',') else {
            return Ok(false);
        };
        let Some(cnonce) = token(authorization, &mut cursor, b"cnonce", b'"', b'"') else {
            return Ok(false);
        };
        (Some(nc), Some(cnonce))
    } else {
        (None, None)
    };
    let Some(response) = token(authorization, &mut cursor, b"response", b'"', b'"') else {
        return Ok(false);
    };

    let mut raw = Vec::new();
    raw.extend_from_slice(username);
    raw.push(b':');
    raw.extend_from_slice(realm);
    raw.push(b':');
    raw.extend_from_slice(password);
    let h1 = md5_hex(&raw)?;
    raw.clear();
    raw.extend_from_slice(method);
    raw.push(b':');
    raw.extend_from_slice(uri);
    let h2 = md5_hex(&raw)?;
    raw.clear();
    raw.extend_from_slice(&h1);
    raw.push(b':');
    raw.extend_from_slice(nonce);
    raw.push(b':');
    if let (Some(nc), Some(cnonce), Some(qop)) = (nc, cnonce, qop) {
        raw.extend_from_slice(nc);
        raw.push(b':');
        raw.extend_from_slice(cnonce);
        raw.push(b':');
        raw.extend_from_slice(qop);
        raw.push(b':');
    }
    raw.extend_from_slice(&h2);
    let computed = md5_hex(&raw)?;
    Ok(computed.as_slice() == response)
}

pub fn ed25519_pk_to_base64(public_key: &[u8; 32]) -> String {
    pk_to_base64(public_key)
}

/// Generate the source-compatible nonzero 1..9999 PIN.
pub fn random_pin() -> Result<u16> {
    loop {
        let bytes: [u8; 2] = random_bytes(2)?
            .try_into()
            .map_err(|_| PairingError::InvalidInput("unexpected random output size"))?;
        let pin = u16::from_ne_bytes(bytes) % 10000;
        if pin != 0 {
            return Ok(pin);
        }
    }
}

fn derive_key(salt: &[u8], secret: &[u8], len: usize) -> Result<Vec<u8>> {
    if len > 64 {
        return Err(PairingError::InvalidInput(
            "SHA-512 derived key exceeds digest",
        ));
    }
    let mut input = Vec::with_capacity(salt.len() + secret.len());
    input.extend_from_slice(salt);
    input.extend_from_slice(secret);
    Ok(hash(MessageDigest::sha512(), &input)?[..len].to_vec())
}

fn md5_hex(input: &[u8]) -> Result<Vec<u8>> {
    let digest = hash(MessageDigest::md5(), input)?;
    let mut output = Vec::with_capacity(digest.len() * 2);
    for byte in digest.as_ref() {
        output.extend_from_slice(format!("{byte:02x}").as_bytes());
    }
    Ok(output)
}

fn token<'a>(
    input: &'a [u8],
    cursor: &mut usize,
    name: &[u8],
    start: u8,
    end: u8,
) -> Option<&'a [u8]> {
    let suffix = input.get(*cursor..)?;
    let relative_name = find_subslice(suffix, name)?;
    let after_name = *cursor + relative_name + name.len();
    let start_at = input[after_name..].iter().position(|&b| b == start)? + after_name;
    let value_start = start_at + 1;
    let value_end = input[value_start..].iter().position(|&b| b == end)? + value_start;
    *cursor = value_end.saturating_add(1);
    Some(&input[value_start..value_end])
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn c_bytes(bytes: &[u8]) -> &[u8] {
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    &bytes[..len]
}

impl Drop for PairingSession {
    fn drop(&mut self) {
        // Pairing secrets are kept in fixed-size arrays to mirror the C session layout.
        // SAFETY: both arrays are exclusively owned, initialized byte ranges.
        unsafe {
            OPENSSL_cleanse(self.ecdh_secret.as_mut_ptr().cast(), self.ecdh_secret.len());
        }
    }
}

impl Drop for SrpState {
    fn drop(&mut self) {
        // SAFETY: the arrays are exclusively owned and live for these exact lengths.
        unsafe {
            OPENSSL_cleanse(self.private_key.as_mut_ptr().cast(), self.private_key.len());
            OPENSSL_cleanse(self.verifier.as_mut_ptr().cast(), self.verifier.len());
            if let Some(key) = &mut self.session_key {
                OPENSSL_cleanse(key.as_mut_ptr().cast(), key.len());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairing() -> Pairing {
        Pairing {
            ed: Ed25519Key::from_device_id(b"test-device").unwrap(),
        }
    }

    fn to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn deterministic_identity_nonce_and_digest_match_the_c_oracle() {
        let rows = include_str!("../reference/pairing_oracle.expected.txt")
            .lines()
            .map(|line| line.split_once('\t').unwrap())
            .collect::<std::collections::HashMap<_, _>>();
        let expected = |key: &str| rows.get(key).copied().unwrap();
        let (pairing, created) = Pairing::init_generate(b"02:11:22:33:44:55", None).unwrap();
        assert_eq!(created, expected("init_result") == "1");
        assert_eq!(
            to_hex(&pairing.get_public_key().unwrap()),
            expected("public_key")
        );

        let mut session = pairing.session().unwrap();
        assert_eq!(
            session.check_handshake_status().to_string(),
            expected("initial_status")
        );
        let mut secret = [0xaa; 32];
        assert_eq!(
            session.get_ecdh_secret_key(&mut secret).to_string(),
            expected("initial_secret_status")
        );
        assert_eq!(to_hex(&secret), expected("initial_secret_buffer"));

        let mut nonce = [0; 32];
        assert_eq!(
            session
                .make_nonce(0x0102_0304_0506_0708, b"client-data", &mut nonce, 32,)
                .unwrap()
                .to_string(),
            expected("nonce_status")
        );
        assert_eq!(to_hex(&nonce), expected("nonce"));

        session.set_setup_status();
        assert_eq!(
            session.check_handshake_status().to_string(),
            expected("setup_status")
        );
        let authorization = br#"Digest username="Mufasa", realm="testrealm@host.com", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", uri="/dir/index.html", qop=auth, nc=00000001, cnonce="0a4f113b", response="6629fae49393a05397450978507c4ef1", opaque="5ccc069c403ebaf9f0171e9517f40e41""#;
        assert_eq!(
            digest_verify(b"GET", authorization, b"Circle Of Life").unwrap(),
            expected("digest_valid") == "1"
        );
        assert_eq!(
            digest_verify(b"GET", authorization, b"wrong").unwrap(),
            expected("digest_bad_password") == "1"
        );
    }

    #[test]
    fn digest_matches_rfc_2617_qop_auth_vector() {
        let auth = br#"Digest username="Mufasa",realm="testrealm@host.com",nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093",uri="/dir/index.html",qop=auth,nc=00000001,cnonce="0a4f113b",response="6629fae49393a05397450978507c4ef1",opaque="5ccc069c403ebaf9f0171e9517f40e41""#;
        assert!(digest_verify(b"GET", auth, b"Circle Of Life").unwrap());
        assert!(!digest_verify(b"GET", auth, b"wrong").unwrap());
        assert!(!digest_verify(b"GET", b"Digest username=missing", b"pw").unwrap());
    }

    #[test]
    fn status_nonce_and_client_data_follow_source_contract() {
        let mut session = pairing().session().unwrap();
        assert_eq!(session.check_handshake_status(), -1);
        let mut nonce = [0; 64];
        assert_eq!(
            session
                .make_nonce(0x0102_0304_0506_0708, b"client\0ignored", &mut nonce, 64)
                .unwrap(),
            0
        );
        assert_ne!(nonce, [0; 64]);
        assert_eq!(
            session.make_nonce(0, b"client", &mut nonce, 65).unwrap(),
            -1
        );
        assert_eq!(session.make_nonce(0, b"client", &mut nonce, 0).unwrap(), -2);
        assert_eq!(session.client_data(), (b"".as_slice(), None));
        session.set_setup_status();
        assert_eq!(session.check_handshake_status(), 0);
    }

    #[test]
    fn handshake_signature_and_invalid_peer_signature_status() {
        let server = pairing();
        let mut session = server.session().unwrap();
        let peer_ed = Ed25519Key::from_device_id(b"peer-device").unwrap();
        let peer_x = X25519Key::generate().unwrap();
        let peer_x_pub = peer_x.raw_public().unwrap();
        assert_eq!(
            session
                .handshake(&peer_x_pub, &peer_ed.raw_public().unwrap())
                .unwrap(),
            0
        );
        assert_eq!(session.check_handshake_status(), 0);
        assert!(session.get_public_key().unwrap().is_some());
        assert_eq!(session.finish(&[0; 64]).unwrap(), -2);
        assert_eq!(session.status(), PairingStatus::Handshake);
        assert_eq!(session.get_signature().unwrap().unwrap().len(), 64);
    }

    #[test]
    fn handshake_accepts_peer_signature_wrapped_after_source_fake_round() {
        let server = pairing();
        let mut session = server.session().unwrap();
        let peer_ed = Ed25519Key::from_device_id(b"peer-device").unwrap();
        let peer_x = X25519Key::generate().unwrap();
        let peer_x_pub = peer_x.raw_public().unwrap();
        session
            .handshake(&peer_x_pub, &peer_ed.raw_public().unwrap())
            .unwrap();
        let ours = session.get_public_key().unwrap().unwrap();
        let shared = peer_x.derive(session.ecdh_ours.as_ref().unwrap()).unwrap();
        let key = derive_key(KEY_VERIFY, &shared, 16).unwrap();
        let iv = derive_key(IV_VERIFY, &shared, 16).unwrap();
        let mut message = [0; PAIRING_SIG_SIZE];
        message[..32].copy_from_slice(&peer_x_pub);
        message[32..].copy_from_slice(&ours);
        let peer_signature = peer_ed.sign(&message).unwrap();
        let mut aes = AesContext::new(AesMode::Ctr, Direction::Encrypt, &key, &iv).unwrap();
        let _fake_round = aes.ctr_encrypt(&[0; PAIRING_SIG_SIZE]).unwrap();
        let encrypted = aes.ctr_encrypt(&peer_signature).unwrap();
        let encrypted: [u8; 64] = encrypted.try_into().unwrap();
        assert_eq!(session.finish(&encrypted).unwrap(), 0);
        assert_eq!(session.status(), PairingStatus::Finished);
        assert_eq!(
            session
                .handshake(&peer_x_pub, &peer_ed.raw_public().unwrap())
                .unwrap(),
            -1
        );
    }

    #[test]
    fn srp_offer_tracks_username_and_rejects_bad_proof() {
        let server = pairing();
        let mut session = server.session().unwrap();
        let offer = session
            .srp_new_user(b"AA:BB:CC:DD:EE:FF", b"1234")
            .unwrap()
            .unwrap();
        assert_eq!(offer.salt.len(), 16);
        assert!(!offer.server_public.is_empty());
        assert_eq!(session.client_data().0, b"AA:BB:CC:DD:EE:FF");
        assert_eq!(
            session.srp_validate_proof(&[0; 256], &mut [0; 20]).unwrap(),
            -1
        );
    }

    #[test]
    fn pair_setup_consumes_srp_before_tag_check_and_preserves_padded_base64() {
        let server = pairing();
        let mut session = server.session().unwrap();
        session.srp_new_user(b"device", b"1234").unwrap().unwrap();
        assert!(
            session
                .confirm_pair_setup(&[0; 32], &[0; 16], &server)
                .unwrap()
                .is_none()
        );
        assert!(
            session
                .confirm_pair_setup(&[0; 32], &[0; 16], &server)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            ed25519_pk_to_base64(&[0; 32]),
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
        );
    }

    #[test]
    fn pair_setup_authenticates_client_key_and_returns_server_key() {
        let server = pairing();
        let mut session = server.session().unwrap();
        let session_key = [0x31; SRP_SESSION_KEY_SIZE];
        session.srp = Some(SrpState {
            salt: [0; SRP_SALT_SIZE],
            verifier: [0; SRP_VERIFIER_SIZE],
            session_key: Some(session_key),
            private_key: [0; SRP_PRIVATE_KEY_SIZE],
        });
        let key = derive_key(KEY_SETUP, &session_key, 16).unwrap();
        let mut iv: [u8; 16] = derive_key(IV_SETUP, &session_key, 16)
            .unwrap()
            .try_into()
            .unwrap();
        iv[15] = iv[15].wrapping_add(1);
        let client_public = [0xA7; 32];
        let (client_epk, client_tag) = gcm_encrypt(&client_public, &key, &iv).unwrap();
        let client_epk: [u8; 32] = client_epk.try_into().unwrap();
        let response = session
            .confirm_pair_setup(&client_epk, &client_tag, &server)
            .unwrap()
            .unwrap();
        assert_eq!(session.client_data().0, b"");
        assert_eq!(
            session.client_data().1.unwrap(),
            pk_to_base64(&client_public)
        );

        iv[15] = iv[15].wrapping_add(1);
        let decrypted = gcm_decrypt(&response.0, &key, &iv, &response.1).unwrap();
        assert!(decrypted.authenticated);
        assert_eq!(decrypted.plaintext, server.get_public_key().unwrap());
    }

    #[test]
    fn persistent_ed25519_key_file_survives_reopen() {
        let path = std::env::temp_dir().join(format!("uxplay-pairing-{}.pem", std::process::id()));
        let (first, created) = Pairing::init_generate(b"device", Some(&path)).unwrap();
        assert!(created);
        let public = first.get_public_key().unwrap();
        let (second, created) = Pairing::init_generate(b"other-device", Some(&path)).unwrap();
        assert!(!created);
        assert_eq!(public, second.get_public_key().unwrap());
        let _ = fs::remove_file(path);
    }
}
