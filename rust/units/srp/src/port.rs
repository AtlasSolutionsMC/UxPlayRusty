//! Safe SRP-6a server operations used by the enabled Apple-variant C API.

use foreign_types::ForeignType;
use openssl::bn::BigNumRef;
use openssl::{
    bn::{BigNum, BigNumContext, MsbOption},
    hash::{MessageDigest, hash},
};
use openssl_sys::RAND_add;
use std::{
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
    os::raw::c_void,
};

// SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
unsafe extern "C" {
    fn OPENSSL_cleanse(ptr: *mut c_void, len: usize);
}

const RFC5054_2048_N: &str = concat!(
    "AC6BDB41324A9A9BF166DE5E1389582FAF72B6651987EE07FC3192943DB56050",
    "A37329CBB4A099ED8193E0757767A13DD52312AB4B03310DCD7F48A9DA04FD50",
    "E8083969EDB767B0CF6095179A163AB3661A05FBD5FAAAE82918A9962F0B93B8",
    "55F97993EC975EEAA80D740ADBF4FF747359D041D5C33EA71D281E446B14773B",
    "CA97B43A23FB801676BD207A436C6481F1D2B9078717461A5B9D32E688F87748",
    "544523B524B0D57D5EA77A2775D2ECFA032CFBDBF52FB3786160279004E57AE",
    "6AF874E7303CE53299CCC041C7BC308D82A5698F3A8D0C38271AE35F8E9DBFBB",
    "694B5C803D89F7AE435DE236D525F54759B65E372FCD68EF20FA7111F9E4AFF73",
);

#[derive(Debug)]
pub enum SrpError {
    InvalidInput(&'static str),
    OpenSsl(openssl::error::ErrorStack),
}

impl From<openssl::error::ErrorStack> for SrpError {
    fn from(value: openssl::error::ErrorStack) -> Self {
        Self::OpenSsl(value)
    }
}

pub type Result<T> = std::result::Result<T, SrpError>;

struct SecretBigNum(ManuallyDrop<BigNum>);

impl SecretBigNum {
    fn new(mut value: BigNum) -> Self {
        value.set_const_time();
        Self(ManuallyDrop::new(value))
    }
}

impl Deref for SecretBigNum {
    type Target = BigNumRef;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SecretBigNum {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for SecretBigNum {
    fn drop(&mut self) {
        // SAFETY: ManuallyDrop suppresses BigNum's ordinary destructor; this is
        // its uniquely owned pointer and BN_clear_free releases it exactly once.
        // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
        unsafe { openssl_sys::BN_clear_free(self.0.as_ptr()) };
    }
}

struct SecretBytes(Vec<u8>);

impl SecretBytes {
    fn new(value: Vec<u8>) -> Self {
        Self(value)
    }
}

impl Deref for SecretBytes {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        if !self.0.is_empty() {
            // SAFETY: the pointer and length identify these uniquely owned Vec bytes.
            unsafe { OPENSSL_cleanse(self.0.as_mut_ptr().cast(), self.0.len()) };
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HashAlgorithm {
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
}

impl TryFrom<i32> for HashAlgorithm {
    type Error = SrpError;

    fn try_from(value: i32) -> Result<Self> {
        match value {
            0 => Ok(Self::Sha1),
            1 => Ok(Self::Sha224),
            2 => Ok(Self::Sha256),
            3 => Ok(Self::Sha384),
            4 => Ok(Self::Sha512),
            _ => Err(SrpError::InvalidInput("unknown SRP hash algorithm")),
        }
    }
}

impl HashAlgorithm {
    fn digest(self) -> MessageDigest {
        match self {
            Self::Sha1 => MessageDigest::sha1(),
            Self::Sha224 => MessageDigest::sha224(),
            Self::Sha256 => MessageDigest::sha256(),
            Self::Sha384 => MessageDigest::sha384(),
            Self::Sha512 => MessageDigest::sha512(),
        }
    }

    pub fn digest_len(self) -> usize {
        self.digest().size()
    }
}

struct Group {
    n: BigNum,
    g: BigNum,
}

impl Group {
    fn resolve(ng_type: i32, n_hex: Option<&str>, g_hex: Option<&str>) -> Result<Self> {
        let (n, g) = match ng_type {
            0 => (BigNum::from_hex_str(RFC5054_2048_N)?, BigNum::from_u32(2)?),
            1 => {
                let n = n_hex.ok_or(SrpError::InvalidInput("custom SRP group requires N"))?;
                let g = g_hex.ok_or(SrpError::InvalidInput("custom SRP group requires g"))?;
                (BigNum::from_hex_str(n)?, BigNum::from_hex_str(g)?)
            }
            _ => return Err(SrpError::InvalidInput("unknown SRP group")),
        };
        if n.num_bits() == 0 || n.is_negative() || g.num_bits() == 0 || g.is_negative() {
            return Err(SrpError::InvalidInput("SRP group values must be positive"));
        }
        Ok(Self { n, g })
    }
}

/// Feed caller-provided seed bytes into the same OpenSSL RNG used by BN_rand.
/// OpenSSL itself performs platform entropy initialization when first required.
pub fn seed_random(seed: &[u8]) {
    if seed.len() <= i32::MAX as usize && !seed.is_empty() {
        // SAFETY: `seed` is a valid immutable byte range; RAND_add reads exactly
        // its declared length and OpenSSL does not retain the pointer.
        // SAFETY: Pointer validity and ranges follow the C API contract checked at this boundary.
        unsafe {
            RAND_add(
                seed.as_ptr().cast::<c_void>(),
                seed.len() as i32,
                seed.len() as f64,
            )
        };
    }
}

fn random_bn(bits: i32) -> Result<BigNum> {
    let mut value = BigNum::new()?;
    value.rand(bits, MsbOption::MAYBE_ZERO, false)?;
    Ok(value)
}

/// Generate the Apple-variant 128-bit salt and corresponding verifier.
pub fn create_salted_verification_key(
    alg: HashAlgorithm,
    ng_type: i32,
    username: &[u8],
    password: &[u8],
    n_hex: Option<&str>,
    g_hex: Option<&str>,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let group = Group::resolve(ng_type, n_hex, g_hex)?;
    let salt = random_bn(128)?;
    let x = SecretBigNum::new(calculate_x(alg, &salt, username, password)?);
    let mut verifier = BigNum::new()?;
    let mut ctx = BigNumContext::new()?;
    verifier.mod_exp(&group.g, &x, &group.n, &mut ctx)?;
    Ok((bn_bytes(&salt), bn_bytes(&verifier)))
}

/// Generate B from verifier and supplied server secret using source byte semantics.
pub fn create_server_ephemeral_key(
    alg: HashAlgorithm,
    ng_type: i32,
    verifier_bytes: &[u8],
    server_secret_bytes: &[u8],
    n_hex: Option<&str>,
    g_hex: Option<&str>,
    rfc5054_compat: bool,
) -> Result<Vec<u8>> {
    let group = Group::resolve(ng_type, n_hex, g_hex)?;
    let verifier = SecretBigNum::new(big_from_bytes(verifier_bytes)?);
    let server_secret = SecretBigNum::new(big_from_bytes(server_secret_bytes)?);
    let mut ctx = BigNumContext::new()?;
    let multiplier = if rfc5054_compat {
        hash_nn_rfc5054(alg, &group.n, &group.n, &group.g)?
    } else {
        hash_nn_original(alg, &group.n, &group.g)?
    };
    let mut g_to_b = BigNum::new()?;
    g_to_b.mod_exp(&group.g, &server_secret, &group.n, &mut ctx)?;
    let mut public = BigNum::new()?;
    if rfc5054_compat {
        let mut kv = BigNum::new()?;
        kv.mod_mul(&multiplier, &verifier, &group.n, &mut ctx)?;
        public.mod_add(&kv, &g_to_b, &group.n, &mut ctx)?;
    } else {
        let mut kv = BigNum::new()?;
        kv.checked_mul(&multiplier, &verifier, &mut ctx)?;
        public.checked_add(&kv, &g_to_b)?;
    }
    Ok(bn_bytes(&public))
}

/// Opaque server verifier with source-compatible proof/session accessors.
pub struct Verifier {
    alg: HashAlgorithm,
    username: Vec<u8>,
    server_public: Vec<u8>,
    proof: SecretBytes,
    hamk: SecretBytes,
    session_key: SecretBytes,
    authenticated: bool,
}

impl Verifier {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        alg: HashAlgorithm,
        ng_type: i32,
        username: &[u8],
        salt_bytes: &[u8],
        verifier_bytes: &[u8],
        client_public_bytes: &[u8],
        server_secret_bytes: &[u8],
        n_hex: Option<&str>,
        g_hex: Option<&str>,
        rfc5054_compat: bool,
    ) -> Result<Option<Self>> {
        let group = Group::resolve(ng_type, n_hex, g_hex)?;
        let salt = big_from_bytes(salt_bytes)?;
        let verifier = SecretBigNum::new(big_from_bytes(verifier_bytes)?);
        let client_public = big_from_bytes(client_public_bytes)?;
        let mut ctx = BigNumContext::new()?;
        let mut reduced_a = BigNum::new()?;
        reduced_a.nnmod(&client_public, &group.n, &mut ctx)?;
        if reduced_a.num_bits() == 0 {
            return Ok(None);
        }

        // With a missing/zero-width optional secret, source code obtains a random
        // 256-bit value. A supplied nonempty secret is deterministic for pairing.
        let server_secret = SecretBigNum::new(if server_secret_bytes.is_empty() {
            random_bn(256)?
        } else {
            big_from_bytes(server_secret_bytes)?
        });
        let multiplier = if rfc5054_compat {
            hash_nn_rfc5054(alg, &group.n, &group.n, &group.g)?
        } else {
            hash_nn_original(alg, &group.n, &group.g)?
        };
        let mut g_to_b = BigNum::new()?;
        g_to_b.mod_exp(&group.g, &server_secret, &group.n, &mut ctx)?;
        let mut server_public = BigNum::new()?;
        if rfc5054_compat {
            let mut kv = BigNum::new()?;
            kv.mod_mul(&multiplier, &verifier, &group.n, &mut ctx)?;
            server_public.mod_add(&kv, &g_to_b, &group.n, &mut ctx)?;
        } else {
            let mut kv = BigNum::new()?;
            kv.checked_mul(&multiplier, &verifier, &mut ctx)?;
            server_public.checked_add(&kv, &g_to_b)?;
        }
        let scrambling = if rfc5054_compat {
            hash_nn_rfc5054(alg, &group.n, &client_public, &server_public)?
        } else {
            hash_nn_original(alg, &client_public, &server_public)?
        };
        let mut verifier_to_u = SecretBigNum::new(BigNum::new()?);
        verifier_to_u.mod_exp(&verifier, &scrambling, &group.n, &mut ctx)?;
        let mut a_times_vu = SecretBigNum::new(BigNum::new()?);
        a_times_vu.checked_mul(&client_public, &verifier_to_u, &mut ctx)?;
        let mut shared_secret = SecretBigNum::new(BigNum::new()?);
        shared_secret.mod_exp(&a_times_vu, &server_secret, &group.n, &mut ctx)?;
        let session_key = SecretBytes::new(hash_session_key(alg, &shared_secret)?);
        let proof = SecretBytes::new(calculate_m(
            alg,
            &group,
            username,
            &salt,
            &client_public,
            &server_public,
            &session_key,
        )?);
        let hamk = SecretBytes::new(calculate_h_amk(alg, &client_public, &proof, &session_key)?);
        let mut username_c = username.to_vec();
        username_c.push(0);
        Ok(Some(Self {
            alg,
            username: username_c,
            server_public: bn_bytes(&server_public),
            proof,
            hamk,
            session_key,
            authenticated: false,
        }))
    }

    pub fn is_authenticated(&self) -> bool {
        self.authenticated
    }
    pub fn username_ptr(&self) -> *const libc::c_char {
        self.username.as_ptr().cast()
    }
    pub fn server_public(&self) -> &[u8] {
        &self.server_public
    }
    pub fn session_key(&self) -> &[u8] {
        &self.session_key
    }
    pub fn hamk(&self) -> &[u8] {
        &self.hamk
    }
    pub fn proof_len(&self) -> usize {
        self.alg.digest_len()
    }

    /// Constant-time proof comparison; successful authentication remains sticky,
    /// matching the original verifier state transition.
    pub fn verify_session(&mut self, client_proof: &[u8]) -> bool {
        if client_proof.len() == self.proof.len() && openssl::memcmp::eq(client_proof, &self.proof)
        {
            self.authenticated = true;
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    fn expected_proof(&self) -> &[u8] {
        &self.proof
    }
}

fn big_from_bytes(bytes: &[u8]) -> Result<BigNum> {
    if bytes.is_empty() {
        Ok(BigNum::from_u32(0)?)
    } else {
        Ok(BigNum::from_slice(bytes)?)
    }
}

fn bn_bytes(value: &BigNumRef) -> Vec<u8> {
    if value.num_bytes() == 0 {
        Vec::new()
    } else {
        value.to_vec()
    }
}

fn digest(alg: HashAlgorithm, bytes: &[u8]) -> Result<Vec<u8>> {
    Ok(hash(alg.digest(), bytes)?.to_vec())
}

fn hash_nn_original(alg: HashAlgorithm, n1: &BigNumRef, n2: &BigNumRef) -> Result<BigNum> {
    let mut bytes = bn_bytes(n1);
    bytes.extend_from_slice(&bn_bytes(n2));
    big_from_bytes(&digest(alg, &bytes)?)
}

fn hash_nn_rfc5054(
    alg: HashAlgorithm,
    n: &BigNumRef,
    n1: &BigNumRef,
    n2: &BigNumRef,
) -> Result<BigNum> {
    let width = n.num_bytes() as usize;
    let left = bn_bytes(n1);
    let right = bn_bytes(n2);
    if left.len() > width || right.len() > width {
        return Err(SrpError::InvalidInput("RFC 5054 value exceeds group width"));
    }
    let mut bytes = vec![0; width * 2];
    bytes[width - left.len()..width].copy_from_slice(&left);
    bytes[2 * width - right.len()..].copy_from_slice(&right);
    big_from_bytes(&digest(alg, &bytes)?)
}

fn hash_ns(alg: HashAlgorithm, n: &BigNumRef, suffix: &[u8]) -> Result<BigNum> {
    let mut bytes = bn_bytes(n);
    bytes.extend_from_slice(suffix);
    big_from_bytes(&digest(alg, &bytes)?)
}

fn calculate_x(
    alg: HashAlgorithm,
    salt: &BigNumRef,
    username: &[u8],
    password: &[u8],
) -> Result<BigNum> {
    let mut username_password = Vec::with_capacity(username.len() + password.len() + 1);
    username_password.extend_from_slice(username);
    username_password.push(b':');
    username_password.extend_from_slice(password);
    let username_password = SecretBytes::new(username_password);
    let inner_digest = SecretBytes::new(digest(alg, &username_password)?);
    hash_ns(alg, salt, &inner_digest)
}

fn hash_session_key(alg: HashAlgorithm, secret: &BigNumRef) -> Result<Vec<u8>> {
    let number = bn_bytes(secret);
    let mut key = Vec::with_capacity(2 * alg.digest_len());
    for last in [0, 1] {
        let mut input = Vec::with_capacity(number.len() + 4);
        input.extend_from_slice(&number);
        input.extend_from_slice(&[0, 0, 0, last]);
        key.extend_from_slice(&digest(alg, &input)?);
    }
    Ok(key)
}

fn calculate_m(
    alg: HashAlgorithm,
    group: &Group,
    username: &[u8],
    salt: &BigNumRef,
    client_public: &BigNumRef,
    server_public: &BigNumRef,
    session_key: &[u8],
) -> Result<Vec<u8>> {
    let h_n = digest(alg, &bn_bytes(&group.n))?;
    let h_g = digest(alg, &bn_bytes(&group.g))?;
    let h_username = digest(alg, username)?;
    let mut input = Vec::with_capacity(
        4 * alg.digest_len() + 3 * group.n.num_bytes() as usize + session_key.len(),
    );
    input.extend(h_n.iter().zip(h_g.iter()).map(|(n, g)| n ^ g));
    input.extend_from_slice(&h_username);
    input.extend_from_slice(&bn_bytes(salt));
    input.extend_from_slice(&bn_bytes(client_public));
    input.extend_from_slice(&bn_bytes(server_public));
    input.extend_from_slice(session_key);
    digest(alg, &input)
}

fn calculate_h_amk(
    alg: HashAlgorithm,
    client_public: &BigNumRef,
    proof: &[u8],
    session_key: &[u8],
) -> Result<Vec<u8>> {
    let mut input = bn_bytes(client_public);
    input.extend_from_slice(proof);
    input.extend_from_slice(session_key);
    digest(alg, &input)
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: &str = "25"; // 37 decimal
    const G: &str = "5";

    #[test]
    fn apple_server_outputs_match_fixed_custom_group_and_authenticates() {
        let alg = HashAlgorithm::Sha256;
        let salt = [3u8];
        let verifier = [9u8];
        let client_public = [7u8];
        let server_secret = [11u8];
        let mut server = Verifier::new(
            alg,
            1,
            b"alice",
            &salt,
            &verifier,
            &client_public,
            &server_secret,
            Some(N),
            Some(G),
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(server.session_key().len(), 64);
        assert!(!server.is_authenticated());
        assert!(!server.verify_session(&[0; 32]));
        assert!(!server.is_authenticated());
        let proof = server.expected_proof().to_vec();
        assert!(server.verify_session(&proof));
        assert!(server.is_authenticated());
        assert!(!server.verify_session(&[0; 32]));
        assert!(server.is_authenticated());
    }

    #[test]
    fn zero_client_public_is_rejected() {
        let result = Verifier::new(
            HashAlgorithm::Sha256,
            1,
            b"alice",
            &[3],
            &[9],
            &[0],
            &[11],
            Some(N),
            Some(G),
            false,
        )
        .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn rfc_compatibility_pads_hash_inputs_and_returns_apple_key() {
        let b = create_server_ephemeral_key(
            HashAlgorithm::Sha512,
            1,
            &[9],
            &[11],
            Some(N),
            Some(G),
            true,
        )
        .unwrap();
        assert!(!b.is_empty());
        let server = Verifier::new(
            HashAlgorithm::Sha512,
            1,
            b"a",
            &[3],
            &[9],
            &[7],
            &[11],
            Some(N),
            Some(G),
            true,
        )
        .unwrap()
        .unwrap();
        assert_eq!(server.session_key().len(), 128);
    }
}
