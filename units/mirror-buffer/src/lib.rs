//! Stateful AES-CTR byte handling from UxPlay's `mirror_buffer.c`.
//!
//! The source decrypts complete AES blocks as packets arrive and retains the
//! remaining keystream bytes for the next packet. Re-keying starts a new CTR
//! stream and drops any pending bytes from the previous stream.

use uxplay_crypto::port::{AesContext, AesMode, CryptoError, Direction, sha512};

pub const AES_KEY_BYTES: usize = 16;

pub struct MirrorBuffer {
    audio_key: [u8; AES_KEY_BYTES],
    aes: Option<AesContext>,
    pending_keystream: [u8; AES_KEY_BYTES],
    next_decrypt_count: usize,
}

impl MirrorBuffer {
    pub fn new(audio_key: &[u8]) -> Result<Self, CryptoError> {
        let audio_key: [u8; AES_KEY_BYTES] = audio_key
            .try_into()
            .map_err(|_| CryptoError::InvalidInput("audio AES key must be 16 bytes"))?;
        Ok(Self {
            audio_key,
            aes: None,
            pending_keystream: [0; AES_KEY_BYTES],
            next_decrypt_count: 0,
        })
    }

    /// Derive the per-stream video AES key and IV from the stream ID and audio key.
    pub fn init_aes(&mut self, stream_connection_id: u64) -> Result<(), CryptoError> {
        let key_label = format!("AirPlayStreamKey{stream_connection_id}");
        let iv_label = format!("AirPlayStreamIV{stream_connection_id}");
        let mut key_hash = sha512()?;
        key_hash.update(key_label.as_bytes())?;
        key_hash.update(&self.audio_key)?;
        let key_hash = key_hash.finish()?;

        let mut iv_hash = sha512()?;
        iv_hash.update(iv_label.as_bytes())?;
        iv_hash.update(&self.audio_key)?;
        let iv_hash = iv_hash.finish()?;

        self.aes = Some(AesContext::new(
            AesMode::Ctr,
            Direction::Encrypt,
            &key_hash[..AES_KEY_BYTES],
            &iv_hash[..AES_KEY_BYTES],
        )?);
        self.next_decrypt_count = 0;
        self.pending_keystream.fill(0);
        Ok(())
    }

    /// Decrypt one packet fragment and preserve the source's input-buffer mutation.
    ///
    /// Complete encrypted blocks are decrypted in `input`; output receives those
    /// bytes plus any partial-block bytes reconstructed from the prior packet.
    pub fn decrypt(&mut self, input: &mut [u8]) -> Result<Vec<u8>, CryptoError> {
        let mut output = vec![0; input.len()];
        let pending = self.next_decrypt_count;
        if pending > 0 {
            let present = pending.min(input.len());
            let keystream_start = AES_KEY_BYTES - pending;
            if present < pending {
                // C returns from inside its loop on the first iteration in this
                // case. Its remaining malloc-backed output bytes are undefined;
                // the source oracle zero-initializes them for deterministic
                // comparison. Preserve the defined first byte and pending-count
                // update without reading beyond this fragment.
                if present > 0 {
                    output[0] = input[0] ^ self.pending_keystream[keystream_start];
                }
                self.next_decrypt_count = pending - present;
                return Ok(output);
            }
            for index in 0..present {
                output[index] = input[index] ^ self.pending_keystream[keystream_start + index];
            }
        }

        let aes = self.aes.as_mut().ok_or(CryptoError::InvalidInput(
            "mirror AES stream is not initialized",
        ))?;
        aes.ctr_start_fresh_block()?;

        let encrypted_len = ((input.len() - pending) / AES_KEY_BYTES) * AES_KEY_BYTES;
        if encrypted_len > 0 {
            let start = pending;
            let end = start + encrypted_len;
            let plaintext = aes.ctr_decrypt(&input[start..end])?;
            input[start..end].copy_from_slice(&plaintext);
            output[start..end].copy_from_slice(&plaintext);
        }

        let rest_len = (input.len() - pending) % AES_KEY_BYTES;
        self.next_decrypt_count = 0;
        if rest_len > 0 {
            let rest_start = input.len() - rest_len;
            let mut block = [0; AES_KEY_BYTES];
            block[..rest_len].copy_from_slice(&input[rest_start..]);
            let keystream = aes.ctr_decrypt(&block)?;
            self.pending_keystream.copy_from_slice(&keystream);
            output[rest_start..].copy_from_slice(&keystream[..rest_len]);
            self.next_decrypt_count = AES_KEY_BYTES - rest_len;
        }
        Ok(output)
    }

    pub fn pending_bytes(&self) -> usize {
        self.next_decrypt_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUDIO_KEY: [u8; 16] = *b"audio-key-fixtur";

    fn encrypted_fragment(step: usize, len: usize) -> Vec<u8> {
        (0..len)
            .map(|index| (index.wrapping_mul(31).wrapping_add(step * 17)) as u8)
            .collect()
    }

    #[test]
    fn requires_a_sixteen_byte_audio_key_and_stream_initialization() {
        assert!(MirrorBuffer::new(b"short").is_err());
        let mut buffer = MirrorBuffer::new(&AUDIO_KEY).unwrap();
        assert!(buffer.decrypt(&mut [1, 2, 3]).is_err());
    }

    #[test]
    fn decrypts_packet_fragments_and_rekeys_without_stale_keystream() {
        let mut buffer = MirrorBuffer::new(&AUDIO_KEY).unwrap();
        let lengths = [0, 13, 7, 31, 1, 48, 15, 2, 16, 35];
        let mut rows = Vec::new();
        for phase in 0..2 {
            let stream_id = if phase == 0 {
                0x0123_4567_89ab_cdef
            } else {
                u64::MAX
            };
            buffer.init_aes(stream_id).unwrap();
            let phase_inputs = if phase == 0 {
                lengths.iter().copied().enumerate().collect::<Vec<_>>()
            } else {
                vec![(20, 21), (21, 5)]
            };
            for (step, (source_step, len)) in phase_inputs.into_iter().enumerate() {
                let mut input = encrypted_fragment(source_step, len);
                let output = buffer.decrypt(&mut input).unwrap();
                rows.push(format!("{phase},{step},{},{}", hex(&input), hex(&output)));
            }
        }
        assert_eq!(buffer.pending_bytes(), 6);
        let expected = include_str!("../reference/mirror_buffer.expected.txt")
            .lines()
            .collect::<Vec<_>>();
        let actual = rows.iter().map(String::as_str).collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn short_continuation_preserves_only_the_defined_source_output() {
        let mut buffer = MirrorBuffer::new(&AUDIO_KEY).unwrap();
        buffer.init_aes(7).unwrap();
        let mut first = [0x5a; 3];
        buffer.decrypt(&mut first).unwrap();
        let pending_before = buffer.pending_bytes();
        let mut short = [0xa5; 2];
        let out = buffer.decrypt(&mut short).unwrap();
        assert_eq!(out.len(), short.len());
        assert_eq!(out[1], 0);
        assert_eq!(buffer.pending_bytes(), pending_before - short.len());

        let pending_before_empty = buffer.pending_bytes();
        let empty = buffer.decrypt(&mut []).unwrap();
        assert!(empty.is_empty());
        assert_eq!(buffer.pending_bytes(), pending_before_empty);
    }

    fn hex(bytes: &[u8]) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(bytes.len() * 2);
        for &byte in bytes {
            output.push(DIGITS[(byte >> 4) as usize] as char);
            output.push(DIGITS[(byte & 0x0f) as usize] as char);
        }
        output
    }
}
