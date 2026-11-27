//! Combined legacy FairPlay v3 adapter and PlayFair algorithm.
//!
//! The source files have different upstream notices: the adapter is LGPL-2.1-or-later, while
//! `playfair.c` and the required portion of `omg_hax.c` are GPL-3.0. Their notices are retained
//! in `LICENSE-NOTICE.md` and `licenses/PLAYFAIR-GPL-3.0.md`.

use std::ffi::c_void;

use fairplay_v2::port::{FairPlayV2, NOMEM as V2_NOMEM, OK as V2_OK};
use zeroize::Zeroize;

#[path = "tables.rs"]
mod tables;

pub const INVALID: i32 = -1;
pub const OK: i32 = 0;

/// Stateful Rust equivalent of the opaque `fairplay_t`.
pub struct FairPlay {
    _logger: usize,
    keymsg: [u8; 164],
    keymsglen: u32,
    version: u32,
    v2: Option<FairPlayV2>,
}

impl FairPlay {
    pub fn new(logger: *mut c_void) -> Self {
        Self {
            _logger: logger as usize,
            keymsg: [0; 164],
            keymsglen: 0,
            version: 0,
            v2: None,
        }
    }

    pub fn reset(&mut self) {
        self.v2 = None;
        self.version = 0;
        self.keymsglen = 0;
        self.keymsg.zeroize();
    }

    pub fn setup(&mut self, request: &[u8; 16], response: &mut [u8; 142]) -> i32 {
        self.reset();
        if request[4] == 2 {
            let mut v2 = match FairPlayV2::new() {
                Ok(value) => value,
                Err(error) => {
                    return match error {
                        fairplay_v2::port::FairPlayError::NoMemory => V2_NOMEM,
                        fairplay_v2::port::FairPlayError::Invalid
                        | fairplay_v2::port::FairPlayError::Crypto(_) => fairplay_v2::port::CRYPTO,
                    };
                }
            };
            let status = v2.setup(request, response);
            if status != V2_OK {
                return status;
            }
            self.v2 = Some(v2);
            self.version = 2;
            return OK;
        }

        if request[4] != 3 || request[14] >= 4 {
            return INVALID;
        }

        let mode = usize::from(request[14]);
        let start = mode * 142;
        response.copy_from_slice(&tables::LEGACY_REPLY_MESSAGE[start..start + 142]);
        self.keymsglen = 0;
        self.version = 3;
        OK
    }

    pub fn handshake(&mut self, request: &[u8; 164], response: &mut [u8; 32]) -> i32 {
        if request[4] == 2
            && self.version == 2
            && let Some(v2) = self.v2.as_mut()
        {
            return v2.handshake(request, response);
        }
        if request[4] != 3 || request[12] >= 4 {
            self.reset();
            return INVALID;
        }

        // Legacy PlayFair accepts a v3 handshake without a preceding setup request.
        self.reset();
        self.version = 3;
        self.keymsg.copy_from_slice(request);
        self.keymsglen = 164;
        response[..12].copy_from_slice(&tables::FP_HEADER);
        response[12..].copy_from_slice(&request[144..164]);
        OK
    }

    pub fn decrypt(&self, input: &[u8; 72], output: &mut [u8; 16]) -> i32 {
        output.fill(0);
        if self.version == 2
            && let Some(v2) = self.v2.as_ref()
        {
            return v2.decrypt_key(input, output);
        }
        if self.version != 3 || self.keymsglen != 164 {
            return INVALID;
        }
        legacy::playfair_decrypt(&self.keymsg, input, output);
        OK
    }
}

impl Drop for FairPlay {
    fn drop(&mut self) {
        self.reset();
    }
}

mod legacy {
    use super::tables;
    use modified_md5_rs::port::modified_md5;
    use sap_hash_rs::port::sap_hash;

    const INDEX_MANGLE: [u8; 11] = [
        0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36, 0x6c,
    ];

    fn z_xor(input: &[u8; 16], output: &mut [u8; 16]) {
        for ((out, byte), key) in output.iter_mut().zip(input).zip(tables::Z_KEY) {
            *out = *byte ^ key;
        }
    }

    fn x_xor(input: &[u8; 16], output: &mut [u8; 16]) {
        for ((out, byte), key) in output.iter_mut().zip(input).zip(tables::X_KEY) {
            *out = *byte ^ key;
        }
    }

    fn table_s1(index: usize) -> &'static [u8] {
        let offset = ((31 * index) % 0x28) << 8;
        &tables::TABLE_S1[offset..offset + 256]
    }

    fn generate_key_schedule(key_material: &[u8; 16]) -> [[u32; 4]; 11] {
        let mut key_data = *key_material;
        for (byte, key) in key_data.iter_mut().zip(tables::T_KEY) {
            *byte ^= key;
        }
        let mut words = [
            u32::from_ne_bytes(key_data[0..4].try_into().unwrap_or([0; 4])),
            u32::from_ne_bytes(key_data[4..8].try_into().unwrap_or([0; 4])),
            u32::from_ne_bytes(key_data[8..12].try_into().unwrap_or([0; 4])),
            u32::from_ne_bytes(key_data[12..16].try_into().unwrap_or([0; 4])),
        ];
        let mut schedule = [[0_u32; 4]; 11];
        let mut table_index = 0;
        for round in 0..11 {
            schedule[round][0] = words[0];

            // C indexes key_data through an unsigned-char pointer spanning all four words.
            let mut bytes = [0_u8; 16];
            for (index, word) in words.iter().enumerate() {
                bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_ne_bytes());
            }
            let s0 = table_s1(table_index)[usize::from(bytes[13])];
            let s1 = table_s1(table_index + 1)[usize::from(bytes[14])];
            let s2 = table_s1(table_index + 2)[usize::from(bytes[15])];
            let s3 = table_s1(table_index + 3)[usize::from(bytes[12])];
            table_index += 4;
            let mut first: [u8; 4] = bytes[..4].try_into().unwrap_or([0; 4]);
            first[0] ^= s0 ^ INDEX_MANGLE[round];
            first[1] ^= s1;
            first[2] ^= s2;
            first[3] ^= s3;
            words[0] = u32::from_ne_bytes(first);

            schedule[round][1] = words[1];
            words[1] ^= words[0];
            schedule[round][2] = words[2];
            words[2] ^= words[1];
            schedule[round][3] = words[3];
            words[3] ^= words[2];
        }
        schedule
    }

    fn table_s3(position: usize, value: u8) -> u8 {
        tables::TABLE_S3[position * 256 + usize::from(value)]
    }

    fn permute_block_1(block: &mut [u8; 16]) {
        block[0] = table_s3(0, block[0]);
        block[4] = table_s3(4, block[4]);
        block[8] = table_s3(8, block[8]);
        block[12] = table_s3(12, block[12]);

        let tmp = block[13];
        block[13] = table_s3(1, block[9]);
        block[9] = table_s3(13, block[5]);
        block[5] = table_s3(9, block[1]);
        block[1] = table_s3(5, tmp);

        let tmp = block[2];
        block[2] = table_s3(10, block[10]);
        block[10] = table_s3(2, tmp);
        let tmp = block[6];
        block[6] = table_s3(14, block[14]);
        block[14] = table_s3(6, tmp);

        let tmp = block[3];
        block[3] = table_s3(15, block[7]);
        block[7] = table_s3(3, block[11]);
        block[11] = table_s3(7, block[15]);
        block[15] = table_s3(11, tmp);
    }

    fn table_s4(index: usize, value: u8) -> u8 {
        let offset = ((71 * index) % 144) << 8;
        tables::TABLE_S4[offset + usize::from(value)]
    }

    fn permute_block_2(block: &mut [u8; 16], round: usize) {
        block[0] = table_s4(round * 16, block[0]);
        block[4] = table_s4(round * 16 + 4, block[4]);
        block[8] = table_s4(round * 16 + 8, block[8]);
        block[12] = table_s4(round * 16 + 12, block[12]);

        let tmp = block[13];
        block[13] = table_s4(round * 16 + 13, block[9]);
        block[9] = table_s4(round * 16 + 9, block[5]);
        block[5] = table_s4(round * 16 + 5, block[1]);
        block[1] = table_s4(round * 16 + 1, tmp);

        let tmp = block[2];
        block[2] = table_s4(round * 16 + 2, block[10]);
        block[10] = table_s4(round * 16 + 10, tmp);
        let tmp = block[6];
        block[6] = table_s4(round * 16 + 6, block[14]);
        block[14] = table_s4(round * 16 + 14, tmp);

        let tmp = block[3];
        block[3] = table_s4(round * 16 + 3, block[7]);
        block[7] = table_s4(round * 16 + 7, block[11]);
        block[11] = table_s4(round * 16 + 11, block[15]);
        block[15] = table_s4(round * 16 + 15, tmp);
    }

    fn block_word(block: &[u8; 16], index: usize) -> u32 {
        let offset = index * 4;
        u32::from_ne_bytes([
            block[offset],
            block[offset + 1],
            block[offset + 2],
            block[offset + 3],
        ])
    }

    fn set_block_word(block: &mut [u8; 16], index: usize, word: u32) {
        let offset = index * 4;
        block[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
    }

    fn cycle(block: &mut [u8; 16], schedule: &[[u32; 4]; 11]) {
        for (word, key) in schedule[10].iter().enumerate() {
            set_block_word(block, word, block_word(block, word) ^ *key);
        }
        permute_block_1(block);

        for round in 0..9 {
            let key0 = schedule[9 - round][0].to_ne_bytes();
            let mut ab = tables::TABLE_S5[usize::from(block[3] ^ key0[3])]
                ^ tables::TABLE_S6[usize::from(block[2] ^ key0[2])]
                ^ tables::TABLE_S8[usize::from(block[0] ^ key0[0])]
                ^ tables::TABLE_S7[usize::from(block[1] ^ key0[1])];
            set_block_word(block, 0, ab);

            let key1 = schedule[9 - round][1].to_ne_bytes();
            ab = tables::TABLE_S6[usize::from(block[6] ^ key1[2])]
                ^ tables::TABLE_S5[usize::from(block[7] ^ key1[3])]
                ^ tables::TABLE_S7[usize::from(block[5] ^ key1[1])]
                ^ tables::TABLE_S8[usize::from(block[4] ^ key1[0])];
            set_block_word(block, 1, ab);

            let key2 = schedule[9 - round][2].to_ne_bytes();
            let key3 = schedule[9 - round][3].to_ne_bytes();
            let word2 = tables::TABLE_S5[usize::from(block[11] ^ key2[3])]
                ^ tables::TABLE_S6[usize::from(block[10] ^ key2[2])]
                ^ tables::TABLE_S7[usize::from(block[9] ^ key2[1])]
                ^ tables::TABLE_S8[usize::from(block[8] ^ key2[0])];
            set_block_word(block, 2, word2);
            let word3 = tables::TABLE_S5[usize::from(block[15] ^ key3[3])]
                ^ tables::TABLE_S6[usize::from(block[14] ^ key3[2])]
                ^ tables::TABLE_S7[usize::from(block[13] ^ key3[1])]
                ^ tables::TABLE_S8[usize::from(block[12] ^ key3[0])];
            set_block_word(block, 3, word3);
            permute_block_2(block, 8 - round);
        }

        for (word, key) in schedule[0].iter().enumerate() {
            set_block_word(block, word, block_word(block, word) ^ *key);
        }
    }

    fn table_s2(index: usize, value: u8) -> u8 {
        let offset = ((97 * index) % 144) << 8;
        tables::TABLE_S2[offset + usize::from(value)]
    }

    fn xor_block(left: &[u8], right: &[u8], output: &mut [u8]) {
        for ((out, a), b) in output.iter_mut().zip(left).zip(right) {
            *out = *a ^ *b;
        }
    }

    fn decrypt_message(message: &[u8; 164], decrypted: &mut [u8; 128]) {
        let mode = usize::from(message[12]);
        let mut block = [0_u8; 16];

        for i in 0..8 {
            let input_offset = if mode == 3 {
                0x80 - 0x10 * i
            } else {
                0x10 * (i + 1)
            };
            block.copy_from_slice(&message[input_offset..input_offset + 16]);

            for round in 0..9 {
                let base = 0x80 - 0x10 * round;
                let positions = [0, 4, 8, 12];
                for position in positions {
                    block[position] = table_s2(base + position, block[position])
                        ^ tables::MESSAGE_KEY[mode * 144 + base + position];
                }

                let tmp = block[13];
                block[13] =
                    table_s2(base + 13, block[9]) ^ tables::MESSAGE_KEY[mode * 144 + base + 13];
                block[9] =
                    table_s2(base + 9, block[5]) ^ tables::MESSAGE_KEY[mode * 144 + base + 9];
                block[5] =
                    table_s2(base + 5, block[1]) ^ tables::MESSAGE_KEY[mode * 144 + base + 5];
                block[1] = table_s2(base + 1, tmp) ^ tables::MESSAGE_KEY[mode * 144 + base + 1];

                let tmp = block[2];
                block[2] =
                    table_s2(base + 2, block[10]) ^ tables::MESSAGE_KEY[mode * 144 + base + 2];
                block[10] = table_s2(base + 10, tmp) ^ tables::MESSAGE_KEY[mode * 144 + base + 10];
                let tmp = block[6];
                block[6] =
                    table_s2(base + 6, block[14]) ^ tables::MESSAGE_KEY[mode * 144 + base + 6];
                block[14] = table_s2(base + 14, tmp) ^ tables::MESSAGE_KEY[mode * 144 + base + 14];

                let tmp = block[3];
                block[3] =
                    table_s2(base + 3, block[7]) ^ tables::MESSAGE_KEY[mode * 144 + base + 3];
                block[7] =
                    table_s2(base + 7, block[11]) ^ tables::MESSAGE_KEY[mode * 144 + base + 7];
                block[11] =
                    table_s2(base + 11, block[15]) ^ tables::MESSAGE_KEY[mode * 144 + base + 11];
                block[15] = table_s2(base + 15, tmp) ^ tables::MESSAGE_KEY[mode * 144 + base + 15];

                let words = std::array::from_fn::<_, 4, _>(|word| {
                    let offset = word * 4;
                    tables::TABLE_S9[usize::from(block[offset])]
                        ^ tables::TABLE_S9[0x100 + usize::from(block[offset + 1])]
                        ^ tables::TABLE_S9[0x200 + usize::from(block[offset + 2])]
                        ^ tables::TABLE_S9[0x300 + usize::from(block[offset + 3])]
                });
                for (word, value) in words.into_iter().enumerate() {
                    set_block_word(&mut block, word, value);
                }
            }

            // table_s10 is the final byte permutation. It mirrors the same in-place ordering.
            for position in [0, 4, 8, 12] {
                block[position] = tables::TABLE_S10[position * 256 + usize::from(block[position])];
            }
            let tmp = block[13];
            block[13] = tables::TABLE_S10[13 * 256 + usize::from(block[9])];
            block[9] = tables::TABLE_S10[9 * 256 + usize::from(block[5])];
            block[5] = tables::TABLE_S10[5 * 256 + usize::from(block[1])];
            block[1] = tables::TABLE_S10[256 + usize::from(tmp)];
            let tmp = block[2];
            block[2] = tables::TABLE_S10[2 * 256 + usize::from(block[10])];
            block[10] = tables::TABLE_S10[10 * 256 + usize::from(tmp)];
            let tmp = block[6];
            block[6] = tables::TABLE_S10[6 * 256 + usize::from(block[14])];
            block[14] = tables::TABLE_S10[14 * 256 + usize::from(tmp)];
            let tmp = block[3];
            block[3] = tables::TABLE_S10[3 * 256 + usize::from(block[7])];
            block[7] = tables::TABLE_S10[7 * 256 + usize::from(block[11])];
            block[11] = tables::TABLE_S10[11 * 256 + usize::from(block[15])];
            block[15] = tables::TABLE_S10[15 * 256 + usize::from(tmp)];

            if mode == 3 {
                let output_offset = 0x70 - 0x10 * i;
                let input_offset = if i < 7 { output_offset } else { 0 };
                xor_block(
                    &block,
                    if i < 7 {
                        &message[input_offset..input_offset + 16]
                    } else {
                        &tables::MESSAGE_IV[mode * 16..mode * 16 + 16]
                    },
                    &mut decrypted[output_offset..output_offset + 16],
                );
            } else {
                let output_offset = 0x10 * i;
                let input: &[u8] = if i > 0 {
                    &message[output_offset..output_offset + 16]
                } else {
                    &tables::MESSAGE_IV[mode * 16..mode * 16 + 16]
                };
                xor_block(
                    &block,
                    input,
                    &mut decrypted[output_offset..output_offset + 16],
                );
            }
        }
    }

    fn generate_session_key(old_sap: &[u8], message: &[u8; 164]) -> [u8; 16] {
        let mut decrypted = [0_u8; 128];
        decrypt_message(message, &mut decrypted);
        let mut new_sap = [0_u8; 320];
        new_sap[..17].copy_from_slice(&tables::STATIC_SOURCE_1);
        new_sap[17..17 + 128].copy_from_slice(&decrypted);
        new_sap[17 + 128..17 + 256].copy_from_slice(&old_sap[128..256]);
        new_sap[17 + 256..].copy_from_slice(&tables::STATIC_SOURCE_2);

        let mut session_key = tables::INITIAL_SESSION_KEY;
        for round in 0..5 {
            let base = &new_sap[round * 64..round * 64 + 64];
            let mut base_block = [0_u8; 64];
            base_block.copy_from_slice(base);
            let md5 = modified_md5(base, &session_key)
                .unwrap_or_else(|_| unreachable!("fixed-size session-key input"));
            session_key = sap_hash(&base_block);
            for word in 0..4 {
                let offset = word * 4;
                let current = u32::from_ne_bytes([
                    session_key[offset],
                    session_key[offset + 1],
                    session_key[offset + 2],
                    session_key[offset + 3],
                ]);
                let add = u32::from_ne_bytes([
                    md5[offset],
                    md5[offset + 1],
                    md5[offset + 2],
                    md5[offset + 3],
                ]);
                session_key[offset..offset + 4]
                    .copy_from_slice(&current.wrapping_add(add).to_ne_bytes());
            }
        }
        for word in session_key.as_chunks_mut::<4>().0 {
            word.reverse();
        }
        for byte in &mut session_key {
            *byte ^= 0x79;
        }
        session_key
    }

    pub fn playfair_decrypt(message: &[u8; 164], cipher_text: &[u8; 72], key_out: &mut [u8; 16]) {
        let session_key = generate_session_key(&tables::DEFAULT_SAP, message);
        let schedule = generate_key_schedule(&session_key);
        let mut block = [0_u8; 16];
        let mut decrypted = [0_u8; 16];
        z_xor(
            cipher_text[56..72].try_into().unwrap_or(&[0; 16]),
            &mut block,
        );
        cycle(&mut block, &schedule);
        for index in 0..16 {
            decrypted[index] = block[index] ^ cipher_text[16 + index];
        }
        x_xor(&decrypted, key_out);
        let unwrapped = *key_out;
        z_xor(&unwrapped, key_out);
    }
}
