/*!
 * Copyright (C) 2026 Omar Roth
 *
 * This library is free software; you can redistribute it and/or modify it
 * under the terms of the GNU Lesser General Public License as published by
 * the Free Software Foundation; either version 2.1 of the License, or
 * (at your option) any later version.
 */

//! Selector-2 FairPlay receiver logic ported from `fairplay_v2.c`.

use openssl::{
    hash::{Hasher, MessageDigest},
    memcmp,
    pkey::PKey,
    sign::Signer,
    symm::{Cipher, Crypter, Mode},
};
use zeroize::Zeroize;

pub const OK: i32 = 0;
pub const INVALID: i32 = -1;
pub const NOMEM: i32 = -2;
pub const CRYPTO: i32 = -3;

const SAP_LEN: usize = 128;

const INPUT_MASKS: [[u8; 16]; 4] = [
    [0; 16],
    [0; 16],
    [
        0x08, 0x0e, 0x31, 0x5d, 0xcc, 0xc1, 0x4e, 0x78, 0x90, 0x27, 0x54, 0xd8, 0x16, 0xda, 0x41,
        0x38,
    ],
    [0; 16],
];
const OUTPUT_MASKS: [[u8; 16]; 4] = [
    [
        0xd7, 0x22, 0xa1, 0xba, 0xaa, 0x10, 0x8a, 0xf5, 0xc0, 0xd8, 0x0b, 0x3c, 0xff, 0xed, 0x93,
        0xe1,
    ],
    [
        0x4b, 0x66, 0x8f, 0x57, 0xa9, 0x03, 0x86, 0x9d, 0xff, 0xdc, 0x78, 0x12, 0xb5, 0x70, 0x3e,
        0xc5,
    ],
    [
        0xbc, 0xb7, 0x4b, 0xb1, 0xce, 0xdd, 0xd3, 0x5e, 0x09, 0xc8, 0x2a, 0x73, 0xe6, 0x63, 0x76,
        0xee,
    ],
    [
        0x03, 0xe9, 0x4b, 0xd9, 0x12, 0xaf, 0x45, 0x8d, 0xc7, 0x7a, 0xf2, 0x51, 0x72, 0x6c, 0x16,
        0xa5,
    ],
];
const CHAIN_MASKS: [[u8; 16]; 4] = [
    [
        0x30, 0xe1, 0xe4, 0x4f, 0xaa, 0x03, 0x6e, 0x66, 0xde, 0xd2, 0x38, 0xde, 0xc6, 0x05, 0x20,
        0x3a,
    ],
    [
        0x75, 0x6e, 0xb0, 0x0f, 0xe6, 0xfa, 0x0e, 0xa3, 0xb6, 0xf0, 0x8b, 0x77, 0xfc, 0x72, 0x46,
        0x27,
    ],
    [
        0x1b, 0x5a, 0x63, 0x54, 0xa4, 0xf2, 0x59, 0x31, 0x77, 0x8e, 0xd0, 0x05, 0xbc, 0x0f, 0x72,
        0xe4,
    ],
    [
        0x6a, 0x63, 0x60, 0xe5, 0xe0, 0xc5, 0xed, 0xa4, 0x76, 0x0f, 0x48, 0xd5, 0x5e, 0x80, 0x92,
        0x84,
    ],
];

const ROUND_KEYS_HEX: [&str; 4] = [
    "523263286e2de137049c00832cd656070628a0846816a5207280964167be739daaa7fe01c2b15b21b031cd60d78fbefddd09aa0f1fb8f12eaf893c4e780682b3ba1ac7b3a5a2369d0a2b0ad3722d886072de17f3d77c216edd572bbdaf7aa3dd88d4d68a5fa8f7e482ffdc592d857f845f06895200ae7eb68251a2efafd4dd6b97c7f62b9769889d15382a72baecf71942af22dfd5c6aa42c0fe80307a1277296a7826bfc28ca7b268baa64b2d9d49bf",
    "d5a9ce40a0f13f2229f9fc95e08eec568e837ccb618bcbd7015ec427a8d2509339d0a009585b6bde5905aff9f1d7ff6a33c6a2a86b9dc9763298668fc34f99e5bf287b86d4b5b2f0e62dd47f25624d9a05cbc3b9d17e71493753a5361231e8ace2505270332e2339047d860f164c6ea38bcf5837b8e17b0ebc9cfd01aad093a27b13629bc3f219957f6ee494d5be7736cee667980d147e0d727a9a99a7c4edafafd5919340a4e654640182428969293a",
    "86989d3be7920dc2f5e37fd5f61124af7494da7735e813a22e6ac2d994cda344cb9ec155fe76d2f7d01c102e44d1b36af1f3c34e0f8511b9df9901979b48b2fdabc4975aa44186e37bd88774e0903589db5230bb7f13b65804cb312ce45b04a5c2a036d2bdb3808ab978b1a65d23b503a4754d9e19c6cd14a0be7cb2fd9dc9b17aa885ca636e48dec3d0346c3e4dfddd82fc4478e1920ca6224238ca1c0fc5177eedff55ed156b1c0842aafbfbe63371",
    "48cb53065e61a30ea823779991ed11e35c6b5d40f0605667e9369b7a54370eb8c4c0316034a06707dd96fc7d89a1f2c5f24997c7c6e9f0c01b7f0cbd92defe78e7f22b88211bdb483a64d7f5a8ba298d0357764a224cad0218287af7b092537a6cbaacad4ef601af56de7b58e64c2822058e3f234b783e8c1da645d4fbea6df602b27d2c49ca43a0546c0674af866b825dcd6e5514072df5406b2b81efed40033d2d5e53386c7df2add2e1aff7294558",
];

const AES_SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

const MAC_PREFIX: [u8; 16] = *b"asdiuAD8AKA9a>/?";
const MAC_SUFFIX: [u8; 16] = *b"!D4H&2*203kjgha9";
const WRAPPING_PREFIX: [u8; 16] = *b"#l*GY&cqnwo*&5sd";
const WRAPPING_SUFFIX: [u8; 16] = *b"57bv&%niulqpdg3z";

#[derive(Debug)]
pub enum FairPlayError {
    Crypto(openssl::error::ErrorStack),
    NoMemory,
    Invalid,
}

impl From<openssl::error::ErrorStack> for FairPlayError {
    fn from(value: openssl::error::ErrorStack) -> Self {
        Self::Crypto(value)
    }
}

/// Stateful receiver side of the fixed FairPlay selector-2 exchange.
pub struct FairPlayV2 {
    receiver_sap: [u8; SAP_LEN],
    sender_sap: [u8; SAP_LEN],
    mode: u8,
    has_setup: bool,
    has_handshake: bool,
}

impl FairPlayV2 {
    /// Initializes receiver SAP with the same fixed marker and secure random tail as C.
    pub fn new() -> Result<Self, FairPlayError> {
        let mut receiver_sap = [0; SAP_LEN];
        receiver_sap[1] = 1;
        let mut random =
            uxplay_crypto::port::random_bytes(126).map_err(|_| FairPlayError::Invalid)?;
        receiver_sap[2..].copy_from_slice(&random);
        random.zeroize();
        Ok(Self {
            receiver_sap,
            sender_sap: [0; SAP_LEN],
            mode: 0,
            has_setup: false,
            has_handshake: false,
        })
    }

    /// Constructs a deterministic receiver state for known-answer tests.
    #[cfg(any(test, feature = "oracle"))]
    pub(crate) fn with_receiver_sap(receiver_sap: [u8; SAP_LEN]) -> Self {
        Self {
            receiver_sap,
            sender_sap: [0; SAP_LEN],
            mode: 0,
            has_setup: false,
            has_handshake: false,
        }
    }

    pub(crate) fn reset_setup_state(&mut self) {
        self.has_setup = false;
        self.has_handshake = false;
        self.sender_sap.zeroize();
    }

    pub(crate) fn reset_handshake_state(&mut self) {
        self.has_handshake = false;
        self.sender_sap.zeroize();
    }

    pub fn setup(&mut self, request: &[u8; 16], response: &mut [u8; 142]) -> i32 {
        self.reset_setup_state();
        if !valid_record(request, 1, 4)
            || request[12] != 2
            || request[13] != 0
            || request[14] >= 4
            || request[15] != 0xbb
        {
            return INVALID;
        }
        response.fill(0);
        record_header(response, 2, 130);
        response[12] = 2;
        response[13] = request[14];
        if encrypt_sap(
            request[14],
            &self.receiver_sap,
            (&mut response[14..]).try_into().unwrap(),
        )
        .is_err()
        {
            return CRYPTO;
        }
        self.mode = request[14];
        self.has_setup = true;
        self.has_handshake = false;
        self.sender_sap.zeroize();
        OK
    }

    pub fn handshake(&mut self, request: &[u8; 164], response: &mut [u8; 32]) -> i32 {
        self.reset_handshake_state();
        if !self.has_setup
            || !valid_record(request, 3, 152)
            || request[12] != self.mode
            || request[13..16] != [0x8f, 0x1a, 0x9c]
        {
            return INVALID;
        }
        if decrypt_sap(
            self.mode,
            request[16..144].try_into().unwrap(),
            &mut self.sender_sap,
        )
        .is_err()
        {
            return CRYPTO;
        }
        let mut expected = match confirmation(&self.sender_sap, &self.receiver_sap) {
            Ok(value) => value,
            Err(error) => return error_code(error),
        };
        if !memcmp::eq(&expected, &request[144..164]) {
            expected.zeroize();
            self.sender_sap.zeroize();
            return INVALID;
        }

        record_header(response, 4, 20);
        response[12..32].copy_from_slice(&expected);
        expected.zeroize();
        self.has_handshake = true;
        OK
    }

    pub fn decrypt_key(&self, encrypted_key: &[u8; 72], raw_key: &mut [u8; 16]) -> i32 {
        raw_key.fill(0);
        if !self.has_handshake
            || &encrypted_key[..8] != b"FPLY\x01\x02\x01\x00"
            || load_be32(&encrypted_key[8..12]) != 60
            || load_be32(&encrypted_key[12..16]) != 0
            || load_be32(&encrypted_key[32..36]) != 16
        {
            return INVALID;
        }

        let mut wrapping_key = [0; 16];
        let mut mac_key = [0; 20];
        let mut recovered = [0; 16];
        let mut authenticated = [0; 52];
        let mut expected = [0; 20];
        let result = (|| {
            wrapping_key = derive_wrapping_key(&self.sender_sap, &self.receiver_sap)?;
            recovered =
                decrypt_wrapped_block(&wrapping_key, encrypted_key[56..72].try_into().unwrap())?;
            for (byte, mask) in recovered.iter_mut().zip(&encrypted_key[16..32]) {
                *byte ^= mask;
            }
            mac_key = derive_mac_key(&self.sender_sap, &self.receiver_sap)?;
            authenticated[..36].copy_from_slice(&encrypted_key[..36]);
            authenticated[36..].copy_from_slice(&recovered);
            expected = hmac_sha1(&mac_key, &authenticated)?;
            if !memcmp::eq(&expected, &encrypted_key[36..56]) {
                return Err(FairPlayError::Invalid);
            }
            raw_key.copy_from_slice(&recovered);
            Ok(())
        })();

        wrapping_key.zeroize();
        mac_key.zeroize();
        recovered.zeroize();
        authenticated.zeroize();
        expected.zeroize();
        match result {
            Ok(()) => OK,
            Err(error) => error_code(error),
        }
    }
}

impl Drop for FairPlayV2 {
    fn drop(&mut self) {
        self.receiver_sap.zeroize();
        self.sender_sap.zeroize();
        self.mode.zeroize();
        self.has_setup = false;
        self.has_handshake = false;
    }
}

fn error_code(error: FairPlayError) -> i32 {
    match error {
        FairPlayError::Invalid => INVALID,
        FairPlayError::NoMemory => NOMEM,
        FairPlayError::Crypto(_) => CRYPTO,
    }
}

fn context_error(error: openssl::error::ErrorStack) -> FairPlayError {
    if error.errors().is_empty() {
        FairPlayError::NoMemory
    } else {
        FairPlayError::Crypto(error)
    }
}

fn valid_record(record: &[u8], kind: u8, payload_length: u32) -> bool {
    &record[..4] == b"FPLY"
        && record[4] == 2
        && record[5] == 1
        && record[6] == kind
        && record[7] == 0
        && load_be32(&record[8..12]) == payload_length
}

fn record_header(record: &mut [u8], kind: u8, payload_length: u32) {
    record[..4].copy_from_slice(b"FPLY");
    record[4..8].copy_from_slice(&[2, 1, kind, 0]);
    record[8..12].copy_from_slice(&payload_length.to_be_bytes());
}

fn load_be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes[..4].try_into().unwrap())
}

fn aes_block(mode: u8, input: &[u8; 16], direction: Mode) -> Result<[u8; 16], FairPlayError> {
    let mut keys = decode_round_keys(mode)?;
    let mut state = *input;
    match direction {
        Mode::Encrypt => encrypt_block(&mut state, &keys),
        Mode::Decrypt => decrypt_block(&mut state, &keys),
    }
    keys.zeroize();
    Ok(state)
}

fn decode_round_keys(mode: u8) -> Result<[[u8; 16]; 11], FairPlayError> {
    let encoded = ROUND_KEYS_HEX
        .get(mode as usize)
        .ok_or(FairPlayError::Invalid)?
        .as_bytes();
    if encoded.len() != 352 {
        return Err(FairPlayError::Invalid);
    }
    let mut keys = [[0_u8; 16]; 11];
    for index in 0..176 {
        let high = hex_nibble(encoded[index * 2]).ok_or(FairPlayError::Invalid)?;
        let low = hex_nibble(encoded[index * 2 + 1]).ok_or(FairPlayError::Invalid)?;
        keys[index / 16][index % 16] = (high << 4) | low;
    }
    Ok(keys)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn xor_key(state: &mut [u8; 16], key: &[u8; 16]) {
    for (byte, key_byte) in state.iter_mut().zip(key) {
        *byte ^= key_byte;
    }
}

fn encrypt_block(state: &mut [u8; 16], keys: &[[u8; 16]; 11]) {
    xor_key(state, &keys[0]);
    for byte in state.iter_mut() {
        *byte = AES_SBOX[*byte as usize];
    }
    shift_rows(state);
    for key in keys.iter().take(10).skip(1) {
        mix_columns(state);
        xor_key(state, key);
        for byte in state.iter_mut() {
            *byte = AES_SBOX[*byte as usize];
        }
        shift_rows(state);
    }
    xor_key(state, &keys[10]);
}

fn decrypt_block(state: &mut [u8; 16], keys: &[[u8; 16]; 11]) {
    let mut inverse_sbox = [0_u8; 256];
    for (index, value) in AES_SBOX.iter().enumerate() {
        inverse_sbox[*value as usize] = index as u8;
    }
    xor_key(state, &keys[10]);
    for round in (1..10).rev() {
        inverse_shift_rows(state);
        for byte in state.iter_mut() {
            *byte = inverse_sbox[*byte as usize];
        }
        xor_key(state, &keys[round]);
        inverse_mix_columns(state);
    }
    inverse_shift_rows(state);
    for byte in state.iter_mut() {
        *byte = inverse_sbox[*byte as usize];
    }
    xor_key(state, &keys[0]);
    inverse_sbox.zeroize();
}

fn shift_rows(state: &mut [u8; 16]) {
    let mut previous = *state;
    for row in 0..4 {
        for column in 0..4 {
            state[4 * column + row] = previous[4 * ((column + row) & 3) + row];
        }
    }
    previous.zeroize();
}

fn inverse_shift_rows(state: &mut [u8; 16]) {
    let mut previous = *state;
    for row in 0..4 {
        for column in 0..4 {
            state[4 * column + row] = previous[4 * ((column + 4 - row) & 3) + row];
        }
    }
    previous.zeroize();
}

fn gf_multiply(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0;
    while b != 0 {
        if b & 1 != 0 {
            product ^= a;
        }
        a = (a << 1) ^ if a & 0x80 != 0 { 0x1b } else { 0 };
        b >>= 1;
    }
    product
}

fn mix_columns(state: &mut [u8; 16]) {
    for offset in (0..16).step_by(4) {
        let a = state[offset];
        let b = state[offset + 1];
        let c = state[offset + 2];
        let d = state[offset + 3];
        state[offset] = gf_multiply(a, 2) ^ gf_multiply(b, 3) ^ c ^ d;
        state[offset + 1] = a ^ gf_multiply(b, 2) ^ gf_multiply(c, 3) ^ d;
        state[offset + 2] = a ^ b ^ gf_multiply(c, 2) ^ gf_multiply(d, 3);
        state[offset + 3] = gf_multiply(a, 3) ^ b ^ c ^ gf_multiply(d, 2);
    }
}

fn inverse_mix_columns(state: &mut [u8; 16]) {
    for offset in (0..16).step_by(4) {
        let a = state[offset];
        let b = state[offset + 1];
        let c = state[offset + 2];
        let d = state[offset + 3];
        state[offset] =
            gf_multiply(a, 14) ^ gf_multiply(b, 11) ^ gf_multiply(c, 13) ^ gf_multiply(d, 9);
        state[offset + 1] =
            gf_multiply(a, 9) ^ gf_multiply(b, 14) ^ gf_multiply(c, 11) ^ gf_multiply(d, 13);
        state[offset + 2] =
            gf_multiply(a, 13) ^ gf_multiply(b, 9) ^ gf_multiply(c, 14) ^ gf_multiply(d, 11);
        state[offset + 3] =
            gf_multiply(a, 11) ^ gf_multiply(b, 13) ^ gf_multiply(c, 9) ^ gf_multiply(d, 14);
    }
}

fn encrypt_sap(
    mode: u8,
    plaintext: &[u8; SAP_LEN],
    encrypted: &mut [u8; SAP_LEN],
) -> Result<(), FairPlayError> {
    if mode >= 4 {
        return Err(FairPlayError::Invalid);
    }
    let mut previous = [0_u8; 16];
    for block in 0..8 {
        let mut state = [0_u8; 16];
        for i in 0..16 {
            state[i] = plaintext[block * 16 + i] ^ INPUT_MASKS[mode as usize][i];
            if block > 0 {
                state[i] ^= previous[i] ^ CHAIN_MASKS[mode as usize][i];
            }
        }
        let encrypted_block = aes_block(mode, &state, Mode::Encrypt)?;
        previous = encrypted_block;
        for i in 0..16 {
            encrypted[block * 16 + i] = encrypted_block[i] ^ OUTPUT_MASKS[mode as usize][i];
        }
        state.zeroize();
    }
    previous.zeroize();
    Ok(())
}

fn decrypt_sap(
    mode: u8,
    encrypted: &[u8; SAP_LEN],
    plaintext: &mut [u8; SAP_LEN],
) -> Result<(), FairPlayError> {
    if mode >= 4 {
        return Err(FairPlayError::Invalid);
    }
    let mut previous = [0_u8; 16];
    for block in 0..8 {
        let mut current = [0_u8; 16];
        let mut state = [0_u8; 16];
        for i in 0..16 {
            state[i] = encrypted[block * 16 + i] ^ OUTPUT_MASKS[mode as usize][i];
        }
        current.copy_from_slice(&state);
        let mut plaintext_block = aes_block(mode, &state, Mode::Decrypt)?;
        for i in 0..16 {
            plaintext[block * 16 + i] = plaintext_block[i] ^ INPUT_MASKS[mode as usize][i];
            if block > 0 {
                plaintext[block * 16 + i] ^= previous[i] ^ CHAIN_MASKS[mode as usize][i];
            }
        }
        previous = current;
        current.zeroize();
        state.zeroize();
        plaintext_block.zeroize();
    }
    previous.zeroize();
    Ok(())
}

fn digest(md: MessageDigest, pieces: &[&[u8]]) -> Result<Vec<u8>, FairPlayError> {
    let mut hasher = Hasher::new(md).map_err(context_error)?;
    for piece in pieces {
        hasher.update(piece)?;
    }
    Ok(hasher.finish()?.to_vec())
}

fn derive_mac_key(
    sender_sap: &[u8; SAP_LEN],
    receiver_sap: &[u8; SAP_LEN],
) -> Result<[u8; 20], FairPlayError> {
    digest(
        MessageDigest::sha1(),
        &[&MAC_PREFIX, sender_sap, receiver_sap, &MAC_SUFFIX],
    )?
    .try_into()
    .map_err(|_| FairPlayError::Invalid)
}

fn derive_wrapping_key(
    sender_sap: &[u8; SAP_LEN],
    receiver_sap: &[u8; SAP_LEN],
) -> Result<[u8; 16], FairPlayError> {
    digest(
        MessageDigest::md5(),
        &[&WRAPPING_PREFIX, sender_sap, receiver_sap, &WRAPPING_SUFFIX],
    )?
    .try_into()
    .map_err(|_| FairPlayError::Invalid)
}

fn hmac_sha1(key: &[u8], input: &[u8]) -> Result<[u8; 20], FairPlayError> {
    let pkey = PKey::hmac(key).map_err(FairPlayError::Crypto)?;
    let mut signer = Signer::new(MessageDigest::sha1(), &pkey).map_err(FairPlayError::Crypto)?;
    signer.update(input)?;
    signer
        .sign_to_vec()?
        .try_into()
        .map_err(|_| FairPlayError::Invalid)
}

fn confirmation(
    sender_sap: &[u8; SAP_LEN],
    receiver_sap: &[u8; SAP_LEN],
) -> Result<[u8; 20], FairPlayError> {
    let mut key = derive_mac_key(sender_sap, receiver_sap)?;
    let output = hmac_sha1(&key, &[2, 2]);
    key.zeroize();
    output
}

fn decrypt_wrapped_block(key: &[u8; 16], encrypted: &[u8; 16]) -> Result<[u8; 16], FairPlayError> {
    decrypt_ecb_with_key(key, encrypted)
}

fn decrypt_ecb_with_key(key: &[u8; 16], encrypted: &[u8; 16]) -> Result<[u8; 16], FairPlayError> {
    let mut crypter =
        Crypter::new(Cipher::aes_128_ecb(), Mode::Decrypt, key, None).map_err(context_error)?;
    crypter.pad(false);
    let mut output = [0_u8; 32];
    let mut written = crypter.update(encrypted, &mut output)?;
    written += crypter.finalize(&mut output[written..])?;
    if written != 16 {
        output.zeroize();
        return Err(FairPlayError::Invalid);
    }
    let plaintext = output[..16].try_into().unwrap();
    output.zeroize();
    Ok(plaintext)
}

/// Emits a deterministic protocol transcript for the C differential harness.
#[cfg(feature = "oracle")]
#[doc(hidden)]
pub fn oracle_transcript() -> String {
    let mut transcript = String::new();
    for mode in 0..4_u8 {
        let mut receiver = [0_u8; SAP_LEN];
        receiver[1] = 1;
        for (index, byte) in receiver[2..].iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(37).wrapping_add(11);
        }
        let mut fairplay = FairPlayV2::with_receiver_sap(receiver);

        let mut setup_request = [0_u8; 16];
        record_header(&mut setup_request, 1, 4);
        setup_request[12..16].copy_from_slice(&[2, 0, mode, 0xbb]);
        let mut setup_response = [0xa5; 142];
        let setup_result = fairplay.setup(&setup_request, &mut setup_response);
        push_line(&mut transcript, setup_result, &setup_response);

        let mut sender = [0_u8; SAP_LEN];
        for (index, byte) in sender.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(13).wrapping_add(7);
        }
        let mut handshake_request = [0_u8; 164];
        record_header(&mut handshake_request, 3, 152);
        handshake_request[12..16].copy_from_slice(&[mode, 0x8f, 0x1a, 0x9c]);
        encrypt_sap(
            mode,
            &sender,
            (&mut handshake_request[16..144]).try_into().unwrap(),
        )
        .unwrap();
        handshake_request[144..164].copy_from_slice(&confirmation(&sender, &receiver).unwrap());
        let mut handshake_response = [0xa5; 32];
        let handshake_result = fairplay.handshake(&handshake_request, &mut handshake_response);
        push_line(&mut transcript, handshake_result, &handshake_response);

        let raw = core::array::from_fn(|index| 0x10_u8.wrapping_add(index as u8));
        let mask = core::array::from_fn(|index| 0x80_u8.wrapping_add(index as u8));
        let mut encrypted_key = test_key_record(&sender, &receiver, &raw, &mask);
        let mut raw_key = [0xa5; 16];
        let decrypt_result = fairplay.decrypt_key(&encrypted_key, &mut raw_key);
        push_line(&mut transcript, decrypt_result, &raw_key);

        encrypted_key[36] ^= 1;
        raw_key.fill(0xa5);
        let bad_mac_result = fairplay.decrypt_key(&encrypted_key, &mut raw_key);
        push_line(&mut transcript, bad_mac_result, &raw_key);

        let mut bad_handshake = handshake_request;
        bad_handshake[144] ^= 1;
        handshake_response.fill(0xa5);
        let bad_handshake_result = fairplay.handshake(&bad_handshake, &mut handshake_response);
        push_line(&mut transcript, bad_handshake_result, &handshake_response);
        raw_key.fill(0xa5);
        let after_bad_handshake = fairplay.decrypt_key(&encrypted_key, &mut raw_key);
        push_line(&mut transcript, after_bad_handshake, &raw_key);

        setup_request[15] ^= 1;
        setup_response.fill(0xa5);
        let invalid_setup_result = fairplay.setup(&setup_request, &mut setup_response);
        push_line(&mut transcript, invalid_setup_result, &setup_response);

        handshake_response.fill(0xa5);
        let after_reset_result = fairplay.handshake(&handshake_request, &mut handshake_response);
        push_line(&mut transcript, after_reset_result, &handshake_response);
    }
    transcript
}

#[cfg(feature = "oracle")]
fn push_line(output: &mut String, result: i32, bytes: &[u8]) {
    use core::fmt::Write as _;
    let _ = write!(output, "{result} ");
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output.push('\n');
}

#[cfg(test)]
pub(crate) fn test_encrypt_sap(mode: u8, plaintext: &[u8; SAP_LEN]) -> [u8; SAP_LEN] {
    let mut encrypted = [0; SAP_LEN];
    encrypt_sap(mode, plaintext, &mut encrypted).unwrap();
    encrypted
}

#[cfg(test)]
pub(crate) fn test_confirmation(sender: &[u8; SAP_LEN], receiver: &[u8; SAP_LEN]) -> [u8; 20] {
    confirmation(sender, receiver).unwrap()
}

#[cfg(any(test, feature = "oracle"))]
pub(crate) fn test_key_record(
    sender: &[u8; SAP_LEN],
    receiver: &[u8; SAP_LEN],
    raw: &[u8; 16],
    mask: &[u8; 16],
) -> [u8; 72] {
    let mut record = [0_u8; 72];
    record[..8].copy_from_slice(b"FPLY\x01\x02\x01\x00");
    record[8..12].copy_from_slice(&60_u32.to_be_bytes());
    record[16..32].copy_from_slice(mask);
    record[32..36].copy_from_slice(&16_u32.to_be_bytes());
    let block = core::array::from_fn(|index| raw[index] ^ mask[index]);
    let mut wrapping_key = derive_wrapping_key(sender, receiver).unwrap();
    record[56..72].copy_from_slice(&encrypt_ecb_with_key(&wrapping_key, &block).unwrap());
    let mut authenticated = [0_u8; 52];
    authenticated[..36].copy_from_slice(&record[..36]);
    authenticated[36..].copy_from_slice(raw);
    let mut mac_key = derive_mac_key(sender, receiver).unwrap();
    record[36..56].copy_from_slice(&hmac_sha1(&mac_key, &authenticated).unwrap());
    wrapping_key.zeroize();
    mac_key.zeroize();
    authenticated.zeroize();
    record
}

#[cfg(any(test, feature = "oracle"))]
fn encrypt_ecb_with_key(key: &[u8; 16], plaintext: &[u8; 16]) -> Result<[u8; 16], FairPlayError> {
    let mut crypter = Crypter::new(Cipher::aes_128_ecb(), Mode::Encrypt, key, None)?;
    crypter.pad(false);
    let mut output = [0_u8; 32];
    let mut written = crypter.update(plaintext, &mut output)?;
    written += crypter.finalize(&mut output[written..])?;
    if written != 16 {
        output.zeroize();
        return Err(FairPlayError::Invalid);
    }
    let encrypted = output[..16].try_into().unwrap();
    output.zeroize();
    Ok(encrypted)
}
