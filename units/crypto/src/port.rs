//! Safe cryptographic operations used by the UxPlay C facade.

use foreign_types::ForeignTypeRef;
use openssl::{
    base64,
    error::ErrorStack,
    hash::{Hasher, MessageDigest},
    md_ctx::MdCtx,
    pkey::{Id, PKey, Private, Public},
    rand::rand_bytes,
    sign::{Signer, Verifier},
    symm::{Cipher, Crypter, Mode},
};
use std::{fs, path::Path, ptr};

pub const AES_BLOCK: usize = 16;
pub const KEY_BYTES: usize = 32;
pub const GCM_IV_BYTES: usize = 16;
pub const GCM_TAG_BYTES: usize = 16;

#[derive(Debug)]
pub enum CryptoError {
    OpenSsl(openssl::error::ErrorStack),
    InvalidInput(&'static str),
    Io(std::io::Error),
    KeyFormat,
}

impl From<openssl::error::ErrorStack> for CryptoError {
    fn from(value: openssl::error::ErrorStack) -> Self {
        Self::OpenSsl(value)
    }
}
impl From<std::io::Error> for CryptoError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub type Result<T> = std::result::Result<T, CryptoError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AesMode {
    Ctr,
    Cbc,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Decrypt,
    Encrypt,
}

pub struct AesContext {
    cipher: Cipher,
    key: [u8; AES_BLOCK],
    iv: [u8; AES_BLOCK],
    direction: Direction,
    crypter: Crypter,
    ctr_block_offset: usize,
}

impl AesContext {
    pub fn new(mode: AesMode, direction: Direction, key: &[u8], iv: &[u8]) -> Result<Self> {
        let key: [u8; AES_BLOCK] = key
            .try_into()
            .map_err(|_| CryptoError::InvalidInput("AES key must be 16 bytes"))?;
        let iv: [u8; AES_BLOCK] = iv
            .try_into()
            .map_err(|_| CryptoError::InvalidInput("AES IV must be 16 bytes"))?;
        let cipher = match mode {
            AesMode::Ctr => Cipher::aes_128_ctr(),
            AesMode::Cbc => Cipher::aes_128_cbc(),
        };
        let mut crypter = Crypter::new(cipher, to_mode(direction), &key, Some(&iv))?;
        crypter.pad(false);
        Ok(Self {
            cipher,
            key,
            iv,
            direction,
            crypter,
            ctr_block_offset: 0,
        })
    }

    pub fn reset(&mut self) -> Result<()> {
        self.crypter = Crypter::new(
            self.cipher,
            to_mode(self.direction),
            &self.key,
            Some(&self.iv),
        )?;
        self.crypter.pad(false);
        self.ctr_block_offset = 0;
        Ok(())
    }

    /// Mirrors EVP Update followed by Final for every call in the C facade.
    pub fn update_final(&mut self, input: &[u8]) -> Result<Vec<u8>> {
        let mut output = vec![0; input.len() + self.cipher.block_size()];
        let updated = self.crypter.update(input, &mut output)?;
        let finalized = self.crypter.finalize(&mut output[updated..])?;
        output.truncate(updated + finalized);
        Ok(output)
    }

    pub fn ctr_encrypt(&mut self, input: &[u8]) -> Result<Vec<u8>> {
        if self.cipher != Cipher::aes_128_ctr() || self.direction != Direction::Encrypt {
            return Err(CryptoError::InvalidInput("not an AES-CTR encrypt context"));
        }
        let output = self.update_final(input)?;
        self.ctr_block_offset = (self.ctr_block_offset + input.len()) % AES_BLOCK;
        Ok(output)
    }

    pub fn ctr_decrypt(&mut self, input: &[u8]) -> Result<Vec<u8>> {
        if self.cipher != Cipher::aes_128_ctr() {
            return Err(CryptoError::InvalidInput("not an AES-CTR context"));
        }
        self.update_final(input)
    }

    pub fn ctr_start_fresh_block(&mut self) -> Result<()> {
        let remaining = (AES_BLOCK - self.ctr_block_offset) % AES_BLOCK;
        if remaining != 0 {
            let _ = self.ctr_encrypt(&vec![0; remaining])?;
        }
        Ok(())
    }

    pub fn cbc_crypt(&mut self, input: &[u8], direction: Direction) -> Result<Vec<u8>> {
        if self.cipher != Cipher::aes_128_cbc() || self.direction != direction {
            return Err(CryptoError::InvalidInput("AES-CBC direction mismatch"));
        }
        self.update_final(input)
    }
}

fn to_mode(direction: Direction) -> Mode {
    match direction {
        Direction::Encrypt => Mode::Encrypt,
        Direction::Decrypt => Mode::Decrypt,
    }
}

pub fn gcm_encrypt(
    plaintext: &[u8],
    key: &[u8],
    iv: &[u8],
) -> Result<(Vec<u8>, [u8; GCM_TAG_BYTES])> {
    let key: &[u8; AES_BLOCK] = key
        .try_into()
        .map_err(|_| CryptoError::InvalidInput("AES-GCM key must be 16 bytes"))?;
    if iv.len() != GCM_IV_BYTES {
        return Err(CryptoError::InvalidInput("AES-GCM IV must be 16 bytes"));
    }
    let mut crypter = Crypter::new(Cipher::aes_128_gcm(), Mode::Encrypt, key, Some(iv))?;
    let mut ciphertext = vec![0; plaintext.len() + AES_BLOCK];
    let updated = if plaintext.is_empty() {
        0
    } else {
        crypter.update(plaintext, &mut ciphertext)?
    };
    let finalized = crypter.finalize(&mut ciphertext[updated..])?;
    ciphertext.truncate(updated + finalized);
    let mut tag = [0; GCM_TAG_BYTES];
    crypter.get_tag(&mut tag)?;
    Ok((ciphertext, tag))
}

pub struct GcmDecryption {
    pub plaintext: Vec<u8>,
    pub authenticated: bool,
}

pub fn gcm_decrypt(ciphertext: &[u8], key: &[u8], iv: &[u8], tag: &[u8]) -> Result<GcmDecryption> {
    let key: &[u8; AES_BLOCK] = key
        .try_into()
        .map_err(|_| CryptoError::InvalidInput("AES-GCM key must be 16 bytes"))?;
    if iv.len() != GCM_IV_BYTES {
        return Err(CryptoError::InvalidInput("AES-GCM IV must be 16 bytes"));
    }
    if tag.len() != GCM_TAG_BYTES {
        return Err(CryptoError::InvalidInput("AES-GCM tag must be 16 bytes"));
    }
    let mut crypter = Crypter::new(Cipher::aes_128_gcm(), Mode::Decrypt, key, Some(iv))?;
    crypter.set_tag(tag)?;
    let mut plaintext = vec![0; ciphertext.len() + AES_BLOCK];
    let updated = if ciphertext.is_empty() {
        0
    } else {
        crypter.update(ciphertext, &mut plaintext)?
    };
    match crypter.finalize(&mut plaintext[updated..]) {
        Ok(finalized) => {
            plaintext.truncate(updated + finalized);
            Ok(GcmDecryption {
                plaintext,
                authenticated: true,
            })
        }
        // EVP_DecryptUpdate has already written plaintext when tag verification fails.
        // The C facade leaves those bytes in its output buffer and returns -1.
        Err(_) => {
            plaintext.truncate(updated);
            Ok(GcmDecryption {
                plaintext,
                authenticated: false,
            })
        }
    }
}

pub enum X25519Key {
    Private(PKey<Private>),
    Public(PKey<Public>),
}
impl X25519Key {
    pub fn generate() -> Result<Self> {
        Ok(Self::Private(PKey::generate_x25519()?))
    }
    pub fn from_raw_public(raw: &[u8]) -> Result<Self> {
        if raw.len() != KEY_BYTES {
            return Err(CryptoError::InvalidInput(
                "X25519 public key must be 32 bytes",
            ));
        }
        Ok(Self::Public(PKey::public_key_from_raw_bytes(
            raw,
            Id::X25519,
        )?))
    }
    pub fn raw_public(&self) -> Result<[u8; KEY_BYTES]> {
        let raw = match self {
            Self::Private(k) => k.raw_public_key()?,
            Self::Public(k) => k.raw_public_key()?,
        };
        raw.try_into().map_err(|_| CryptoError::KeyFormat)
    }
    pub fn derive(&self, peer: &Self) -> Result<[u8; KEY_BYTES]> {
        let ours = match self {
            Self::Private(k) => k,
            Self::Public(_) => {
                return Err(CryptoError::InvalidInput(
                    "X25519 local key must be private",
                ));
            }
        };
        let mut deriver = openssl::derive::Deriver::new(ours)?;
        match peer {
            Self::Private(k) => deriver.set_peer(k)?,
            Self::Public(k) => deriver.set_peer(k)?,
        }
        let shared = deriver.derive_to_vec()?;
        shared.try_into().map_err(|_| CryptoError::KeyFormat)
    }
}

pub enum Ed25519Key {
    Private(PKey<Private>),
    Public(PKey<Public>),
}
impl Ed25519Key {
    pub fn generate_random() -> Result<Self> {
        Ok(Self::Private(PKey::generate_ed25519()?))
    }
    pub fn from_raw_public(raw: &[u8]) -> Result<Self> {
        if raw.len() != KEY_BYTES {
            return Err(CryptoError::InvalidInput(
                "Ed25519 public key must be 32 bytes",
            ));
        }
        Ok(Self::Public(PKey::public_key_from_raw_bytes(
            raw,
            Id::ED25519,
        )?))
    }
    pub fn from_raw_private(seed: &[u8]) -> Result<Self> {
        if seed.len() != KEY_BYTES {
            return Err(CryptoError::InvalidInput(
                "Ed25519 private seed must be 32 bytes",
            ));
        }
        Ok(Self::Private(PKey::private_key_from_raw_bytes(
            seed,
            Id::ED25519,
        )?))
    }
    pub fn raw_public(&self) -> Result<[u8; KEY_BYTES]> {
        let raw = match self {
            Self::Private(k) => k.raw_public_key()?,
            Self::Public(k) => k.raw_public_key()?,
        };
        raw.try_into().map_err(|_| CryptoError::KeyFormat)
    }
    pub fn copy(&self) -> Result<Self> {
        match self {
            Self::Private(k) => Ok(Self::Private(k.clone())),
            Self::Public(k) => Ok(Self::Public(k.clone())),
        }
    }
    pub fn sign(&self, data: &[u8]) -> Result<[u8; 64]> {
        let mut signature = [0; 64];
        let len = self.sign_into(data, &mut signature)?;
        if len != signature.len() {
            return Err(CryptoError::KeyFormat);
        }
        Ok(signature)
    }
    pub fn sign_into(&self, data: &[u8], signature: &mut [u8]) -> Result<usize> {
        let mut signer = match self {
            Self::Private(k) => Signer::new_without_digest(k)?,
            Self::Public(k) => {
                // The safe Signer API requires a private key at the type level. The C
                // source still calls EVP_DigestSignInit for a public-only key and then
                // exits through handle_error; mirror that operational error narrowly.
                let ctx = MdCtx::new()?;
                let mut pkey_ctx = ptr::null_mut();
                // SAFETY: ctx and k remain alive for this call; null digest/engine select
                // Ed25519's required one-shot mode, matching the source C call exactly.
                let initialized = unsafe {
                    openssl_sys::EVP_DigestSignInit(
                        ctx.as_ptr(),
                        &mut pkey_ctx,
                        ptr::null(),
                        ptr::null_mut(),
                        k.as_ptr(),
                    )
                };
                if initialized != 1 {
                    return Err(CryptoError::OpenSsl(ErrorStack::get()));
                }
                let mut signature_len = signature.len();
                // SAFETY: the context is initialized, and both slices supply the exact
                // readable/writable lengths passed to OpenSSL.
                let signed = unsafe {
                    openssl_sys::EVP_DigestSign(
                        ctx.as_ptr(),
                        signature.as_mut_ptr(),
                        &mut signature_len,
                        data.as_ptr(),
                        data.len(),
                    )
                };
                if signed != 1 {
                    return Err(CryptoError::OpenSsl(ErrorStack::get()));
                }
                return Ok(signature_len);
            }
        };
        Ok(signer.sign_oneshot(signature, data)?)
    }
    pub fn verify(&self, data: &[u8], signature: &[u8]) -> Result<bool> {
        let mut verifier = match self {
            Self::Private(k) => Verifier::new_without_digest(k)?,
            Self::Public(k) => Verifier::new_without_digest(k)?,
        };
        Ok(verifier.verify_oneshot(signature, data)?)
    }
    pub fn from_pem(pem: &[u8]) -> Result<Self> {
        let key = PKey::private_key_from_pem(pem)?;
        if key.id() != Id::ED25519 {
            return Err(CryptoError::KeyFormat);
        }
        Ok(Self::Private(key))
    }
    pub fn to_pem(&self) -> Result<Vec<u8>> {
        match self {
            Self::Private(k) => Ok(k.private_key_to_pem_pkcs8()?),
            Self::Public(_) => Err(CryptoError::InvalidInput("PEM export requires private key")),
        }
    }
    pub fn from_device_id(device_id: &[u8]) -> Result<Self> {
        const SALT: &[u8] = b"UxPlay-Persistent-Not-Secure-Public-Key";
        let mut hasher = Hasher::new(MessageDigest::sha512())?;
        hasher.update(SALT)?;
        hasher.update(device_id)?;
        let digest = hasher.finish()?;
        Self::from_raw_private(&digest[..KEY_BYTES])
    }
    /// Matches C behavior: load a valid Ed25519 PEM, otherwise create a random key and
    /// attempt to save it. `result` is true only when a file was created successfully.
    pub fn load_or_generate(device_id: &[u8], path: Option<&Path>) -> Result<(Self, bool)> {
        if let Some(path) = path
            && !path.as_os_str().is_empty()
        {
            if let Ok(bytes) = fs::read(path)
                && let Ok(key) = Self::from_pem(&bytes)
            {
                return Ok((key, false));
            }
            let key = Self::generate_random()?;
            let written = fs::write(path, key.to_pem()?).is_ok();
            return Ok((key, written));
        }
        Ok((Self::from_device_id(device_id)?, false))
    }
}

pub struct DigestContext {
    digest: MessageDigest,
    hasher: Option<Hasher>,
}
impl DigestContext {
    pub fn new(digest: MessageDigest) -> Result<Self> {
        Ok(Self {
            digest,
            hasher: Some(Hasher::new(digest)?),
        })
    }
    pub fn update(&mut self, bytes: &[u8]) -> Result<()> {
        self.hasher
            .as_mut()
            .ok_or(CryptoError::InvalidInput(
                "digest is finalized; reset first",
            ))?
            .update(bytes)?;
        Ok(())
    }
    pub fn finish(&mut self) -> Result<Vec<u8>> {
        let mut hasher = self.hasher.take().ok_or(CryptoError::InvalidInput(
            "digest is finalized; reset first",
        ))?;
        Ok(hasher.finish()?.to_vec())
    }
    pub fn reset(&mut self) -> Result<()> {
        self.hasher = Some(Hasher::new(self.digest)?);
        Ok(())
    }
}
pub fn sha512() -> Result<DigestContext> {
    DigestContext::new(MessageDigest::sha512())
}
pub fn md5() -> Result<DigestContext> {
    DigestContext::new(MessageDigest::md5())
}
pub fn get_md5_hex(input: &[u8]) -> Result<String> {
    let mut digest = md5()?;
    digest.update(input)?;
    Ok(digest
        .finish()?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
pub fn random_bytes(len: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![0; len];
    rand_bytes(&mut bytes)?;
    Ok(bytes)
}
pub fn pk_to_base64(input: &[u8]) -> String {
    base64::encode_block(input)
}
