//! Protocol tables and fixed data ported from UxPlay's PlayFair implementation.
//! Source tables: lib/playfair/omg_hax.h, lib/playfair/omg_hax.c, and lib/fairplay_playfair.c.
//! Copyright notices are in LICENSE-NOTICE.md; GPL-3.0 text is in licenses/PLAYFAIR-GPL-3.0.md.
const DATA: &[u8] = include_bytes!("../tables.bin");

const fn byte_array<const N: usize>(offset: usize) -> [u8; N] {
    let mut output = [0; N];
    let mut index = 0;
    while index < N {
        output[index] = DATA[offset + index];
        index += 1;
    }
    output
}

const fn word_array<const N: usize>(offset: usize) -> [u32; N] {
    let mut output = [0; N];
    let mut index = 0;
    while index < N {
        let byte = offset + index * 4;
        output[index] =
            u32::from_le_bytes([DATA[byte], DATA[byte + 1], DATA[byte + 2], DATA[byte + 3]]);
        index += 1;
    }
    output
}

pub static MESSAGE_KEY: [u8; 576] = byte_array::<576>(0);
pub static MESSAGE_IV: [u8; 64] = byte_array::<64>(576);
pub static Z_KEY: [u8; 16] = byte_array::<16>(640);
pub static X_KEY: [u8; 16] = byte_array::<16>(656);
pub static T_KEY: [u8; 16] = byte_array::<16>(672);
pub static TABLE_S1: [u8; 10240] = byte_array::<10240>(688);
pub static TABLE_S2: [u8; 36864] = byte_array::<36864>(10928);
pub static TABLE_S3: [u8; 4096] = byte_array::<4096>(47792);
pub static TABLE_S4: [u8; 36864] = byte_array::<36864>(51888);
pub static TABLE_S5: [u32; 256] = word_array::<256>(88752);
pub static TABLE_S6: [u32; 256] = word_array::<256>(89776);
pub static TABLE_S7: [u32; 256] = word_array::<256>(90800);
pub static TABLE_S8: [u32; 256] = word_array::<256>(91824);
pub static TABLE_S9: [u32; 1024] = word_array::<1024>(92848);
pub static TABLE_S10: [u8; 4096] = byte_array::<4096>(96944);
pub static INITIAL_SESSION_KEY: [u8; 16] = byte_array::<16>(101040);
pub static STATIC_SOURCE_1: [u8; 17] = byte_array::<17>(101056);
pub static STATIC_SOURCE_2: [u8; 47] = byte_array::<47>(101073);
pub static DEFAULT_SAP: [u8; 276] = byte_array::<276>(101120);
pub static FP_HEADER: [u8; 12] = byte_array::<12>(101396);
pub static LEGACY_REPLY_MESSAGE: [u8; 568] = byte_array::<568>(101408);
