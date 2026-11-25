#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Keep the untouched C reference and Rust exports in the same process. */
#define aes_ctr_init ref_aes_ctr_init
#define aes_ctr_reset ref_aes_ctr_reset
#define aes_ctr_encrypt ref_aes_ctr_encrypt
#define aes_ctr_decrypt ref_aes_ctr_decrypt
#define aes_ctr_start_fresh_block ref_aes_ctr_start_fresh_block
#define aes_ctr_destroy ref_aes_ctr_destroy
#define aes_cbc_init ref_aes_cbc_init
#define aes_cbc_reset ref_aes_cbc_reset
#define aes_cbc_encrypt ref_aes_cbc_encrypt
#define aes_cbc_decrypt ref_aes_cbc_decrypt
#define aes_cbc_destroy ref_aes_cbc_destroy
#define x25519_key_generate ref_x25519_key_generate
#define x25519_key_from_raw ref_x25519_key_from_raw
#define x25519_key_get_raw ref_x25519_key_get_raw
#define x25519_key_destroy ref_x25519_key_destroy
#define get_random_bytes ref_get_random_bytes
#define pk_to_base64 ref_pk_to_base64
#define x25519_derive_secret ref_x25519_derive_secret
#define gcm_encrypt ref_gcm_encrypt
#define gcm_decrypt ref_gcm_decrypt
#define ed25519_key_generate ref_ed25519_key_generate
#define ed25519_key_from_raw ref_ed25519_key_from_raw
#define ed25519_key_get_raw ref_ed25519_key_get_raw
#define ed25519_key_copy ref_ed25519_key_copy
#define ed25519_key_destroy ref_ed25519_key_destroy
#define ed25519_sign ref_ed25519_sign
#define ed25519_verify ref_ed25519_verify
#define sha_init ref_sha_init
#define sha_update ref_sha_update
#define sha_final ref_sha_final
#define sha_reset ref_sha_reset
#define sha_destroy ref_sha_destroy
#define md5_init ref_md5_init
#define md5_update ref_md5_update
#define md5_final ref_md5_final
#define md5_reset ref_md5_reset
#define md5_destroy ref_md5_destroy
#define get_md5 ref_get_md5
#include "../include/crypto.h"
#undef aes_ctr_init
#undef aes_ctr_reset
#undef aes_ctr_encrypt
#undef aes_ctr_decrypt
#undef aes_ctr_start_fresh_block
#undef aes_ctr_destroy
#undef aes_cbc_init
#undef aes_cbc_reset
#undef aes_cbc_encrypt
#undef aes_cbc_decrypt
#undef aes_cbc_destroy
#undef x25519_key_generate
#undef x25519_key_from_raw
#undef x25519_key_get_raw
#undef x25519_key_destroy
#undef get_random_bytes
#undef pk_to_base64
#undef x25519_derive_secret
#undef gcm_encrypt
#undef gcm_decrypt
#undef ed25519_key_generate
#undef ed25519_key_from_raw
#undef ed25519_key_get_raw
#undef ed25519_key_copy
#undef ed25519_key_destroy
#undef ed25519_sign
#undef ed25519_verify
#undef sha_init
#undef sha_update
#undef sha_final
#undef sha_reset
#undef sha_destroy
#undef md5_init
#undef md5_update
#undef md5_final
#undef md5_reset
#undef md5_destroy
#undef get_md5

char *utils_hex_to_string(const unsigned char *bytes, int length) {
    char *result = (char *)malloc((size_t)length * 2 + 1);
    if (result == NULL) return NULL;
    for (int i = 0; i < length; ++i) snprintf(result + i * 2, 3, "%02x", bytes[i]);
    return result;
}

#define CHECK(cond, message) do { \
    if (!(cond)) { fprintf(stderr, "FAIL: %s (line %d)\n", message, __LINE__); return 1; } \
} while (0)
#define LOAD(name, signature) signature name = (signature)GetProcAddress(rust, #name); \
    CHECK(name != NULL, "Rust export " #name)

typedef aes_ctx_t *(*aes_init_fn)(const uint8_t *, const uint8_t *);
typedef void (*aes_reset_fn)(aes_ctx_t *);
typedef void (*aes_crypt_fn)(aes_ctx_t *, const uint8_t *, uint8_t *, int);
typedef void (*aes_fresh_fn)(aes_ctx_t *);
typedef void (*aes_destroy_fn)(aes_ctx_t *);
typedef aes_ctx_t *(*cbc_init_fn)(const uint8_t *, const uint8_t *, aes_direction_t);
typedef x25519_key_t *(*x_gen_fn)(void);
typedef x25519_key_t *(*x_from_fn)(const uint8_t *);
typedef void (*x_raw_fn)(uint8_t *, const x25519_key_t *);
typedef void (*x_derive_fn)(uint8_t *, const x25519_key_t *, const x25519_key_t *);
typedef void (*x_destroy_fn)(x25519_key_t *);
typedef int (*random_fn)(uint8_t *, int);
typedef void (*base64_fn)(const uint8_t *, int, char *, int);
typedef int (*gcm_enc_fn)(const uint8_t *, int, uint8_t *, uint8_t *, uint8_t *, uint8_t *);
typedef int (*gcm_dec_fn)(uint8_t *, int, uint8_t *, uint8_t *, uint8_t *, uint8_t *);
typedef ed25519_key_t *(*ed_gen_fn)(const char *, const char *, int *);
typedef ed25519_key_t *(*ed_raw_fn)(const uint8_t *);
typedef void (*ed_get_fn)(uint8_t *, const ed25519_key_t *);
typedef ed25519_key_t *(*ed_copy_fn)(const ed25519_key_t *);
typedef void (*ed_destroy_fn)(ed25519_key_t *);
typedef void (*ed_sign_fn)(uint8_t *, size_t, const uint8_t *, size_t, const ed25519_key_t *);
typedef int (*ed_verify_fn)(const uint8_t *, size_t, const uint8_t *, size_t, const ed25519_key_t *);
typedef sha_ctx_t *(*sha_init_fn)(void);
typedef void (*sha_update_fn)(sha_ctx_t *, const uint8_t *, int);
typedef void (*sha_final_fn)(sha_ctx_t *, uint8_t *, unsigned int *);
typedef void (*sha_reset_fn)(sha_ctx_t *);
typedef void (*sha_destroy_fn)(sha_ctx_t *);
typedef md5_ctx_t *(*md5_init_fn)(void);
typedef void (*md5_update_fn)(md5_ctx_t *, const uint8_t *, int);
typedef void (*md5_final_fn)(md5_ctx_t *, uint8_t *, unsigned int *);
typedef void (*md5_reset_fn)(md5_ctx_t *);
typedef void (*md5_destroy_fn)(md5_ctx_t *);
typedef char *(*get_md5_fn)(char *);

int main(int argc, char **argv) {
    HMODULE rust = LoadLibraryA("uxplay_crypto.dll");
    CHECK(rust != NULL, "LoadLibrary uxplay_crypto.dll");
    if (argc == 2 && (strcmp(argv[1], "--source-error") == 0 || strcmp(argv[1], "--rust-error") == 0)) {
        const uint8_t public_bytes[32] = {0};
        const uint8_t message[] = "operational-error";
        uint8_t signature[64];
        if (strcmp(argv[1], "--source-error") == 0) {
            ed25519_key_t *key = ref_ed25519_key_from_raw(public_bytes);
            ref_ed25519_sign(signature, sizeof(signature), message, sizeof(message), key);
        } else {
            ed_raw_fn from_raw = (ed_raw_fn)GetProcAddress(rust, "ed25519_key_from_raw");
            ed_sign_fn sign = (ed_sign_fn)GetProcAddress(rust, "ed25519_sign");
            CHECK(from_raw && sign, "Rust Ed25519 error-path exports");
            ed25519_key_t *key = from_raw(public_bytes);
            sign(signature, sizeof(signature), message, sizeof(message), key);
        }
        return 99; /* Both implementations should exit from handle_error first. */
    }
    LOAD(aes_ctr_init, aes_init_fn);
    LOAD(aes_ctr_reset, aes_reset_fn);
    LOAD(aes_ctr_encrypt, aes_crypt_fn);
    LOAD(aes_ctr_decrypt, aes_crypt_fn);
    LOAD(aes_ctr_start_fresh_block, aes_fresh_fn);
    LOAD(aes_ctr_destroy, aes_destroy_fn);
    LOAD(aes_cbc_init, cbc_init_fn);
    LOAD(aes_cbc_reset, aes_reset_fn);
    LOAD(aes_cbc_encrypt, aes_crypt_fn);
    LOAD(aes_cbc_decrypt, aes_crypt_fn);
    LOAD(aes_cbc_destroy, aes_destroy_fn);
    LOAD(x25519_key_generate, x_gen_fn);
    LOAD(x25519_key_from_raw, x_from_fn);
    LOAD(x25519_key_get_raw, x_raw_fn);
    LOAD(x25519_derive_secret, x_derive_fn);
    LOAD(x25519_key_destroy, x_destroy_fn);
    LOAD(get_random_bytes, random_fn);
    LOAD(pk_to_base64, base64_fn);
    LOAD(gcm_encrypt, gcm_enc_fn);
    LOAD(gcm_decrypt, gcm_dec_fn);
    LOAD(ed25519_key_generate, ed_gen_fn);
    LOAD(ed25519_key_from_raw, ed_raw_fn);
    LOAD(ed25519_key_get_raw, ed_get_fn);
    LOAD(ed25519_key_copy, ed_copy_fn);
    LOAD(ed25519_key_destroy, ed_destroy_fn);
    LOAD(ed25519_sign, ed_sign_fn);
    LOAD(ed25519_verify, ed_verify_fn);
    LOAD(sha_init, sha_init_fn);
    LOAD(sha_update, sha_update_fn);
    LOAD(sha_final, sha_final_fn);
    LOAD(sha_reset, sha_reset_fn);
    LOAD(sha_destroy, sha_destroy_fn);
    LOAD(md5_init, md5_init_fn);
    LOAD(md5_update, md5_update_fn);
    LOAD(md5_final, md5_final_fn);
    LOAD(md5_reset, md5_reset_fn);
    LOAD(md5_destroy, md5_destroy_fn);
    LOAD(get_md5, get_md5_fn);

    const uint8_t aes_key[16] = {0x2b,0x7e,0x15,0x16,0x28,0xae,0xd2,0xa6,0xab,0xf7,0x15,0x88,0x09,0xcf,0x4f,0x3c};
    const uint8_t ctr_iv[16] = {0xf0,0xf1,0xf2,0xf3,0xf4,0xf5,0xf6,0xf7,0xf8,0xf9,0xfa,0xfb,0xfc,0xfd,0xfe,0xff};
    const uint8_t block[16] = {0x6b,0xc1,0xbe,0xe2,0x2e,0x40,0x9f,0x96,0xe9,0x3d,0x7e,0x11,0x73,0x93,0x17,0x2a};
    const uint8_t ctr_expected[16] = {0x87,0x4d,0x61,0x91,0xb6,0x20,0xe3,0x26,0x1b,0xef,0x68,0x64,0x99,0x0d,0xb6,0xce};
    uint8_t ref_out[64] = {0}, rust_out[64] = {0};
    aes_ctx_t *ref_ctr = ref_aes_ctr_init(aes_key, ctr_iv);
    aes_ctx_t *rust_ctr = aes_ctr_init(aes_key, ctr_iv);
    CHECK(ref_ctr && rust_ctr, "AES-CTR init");
    ref_aes_ctr_encrypt(ref_ctr, block, ref_out, 16);
    aes_ctr_encrypt(rust_ctr, block, rust_out, 16);
    CHECK(memcmp(ref_out, ctr_expected, 16) == 0, "reference CTR vector");
    CHECK(memcmp(rust_out, ctr_expected, 16) == 0, "Rust CTR vector");
    ref_aes_ctr_reset(ref_ctr); aes_ctr_reset(rust_ctr);
    memset(ref_out, 0, sizeof(ref_out)); memset(rust_out, 0, sizeof(rust_out));
    ref_aes_ctr_encrypt(ref_ctr, block, ref_out, 7); ref_aes_ctr_encrypt(ref_ctr, block + 7, ref_out + 7, 9);
    aes_ctr_encrypt(rust_ctr, block, rust_out, 7); aes_ctr_encrypt(rust_ctr, block + 7, rust_out + 7, 9);
    CHECK(memcmp(ref_out, rust_out, 16) == 0, "split CTR streaming equivalence");
    ref_aes_ctr_destroy(ref_ctr); aes_ctr_destroy(rust_ctr);

    ref_ctr = ref_aes_ctr_init(aes_key, ctr_iv); rust_ctr = aes_ctr_init(aes_key, ctr_iv);
    uint8_t ref_in_place[16], rust_in_place[16];
    memcpy(ref_in_place, block, 16); memcpy(rust_in_place, block, 16);
    ref_aes_ctr_encrypt(ref_ctr, ref_in_place, ref_in_place, 16);
    aes_ctr_encrypt(rust_ctr, rust_in_place, rust_in_place, 16);
    CHECK(memcmp(ref_in_place, rust_in_place, 16) == 0, "in-place CTR alias parity");
    ref_aes_ctr_destroy(ref_ctr); aes_ctr_destroy(rust_ctr);

    const uint8_t cbc_iv[16] = {0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15};
    const uint8_t cbc_expected[16] = {0x76,0x49,0xab,0xac,0x81,0x19,0xb2,0x46,0xce,0xe9,0x8e,0x9b,0x12,0xe9,0x19,0x7d};
    ref_ctr = ref_aes_cbc_init(aes_key, cbc_iv, AES_ENCRYPT);
    rust_ctr = aes_cbc_init(aes_key, cbc_iv, AES_ENCRYPT);
    CHECK(ref_ctr && rust_ctr, "AES-CBC init");
    ref_aes_cbc_encrypt(ref_ctr, block, ref_out, 16); aes_cbc_encrypt(rust_ctr, block, rust_out, 16);
    CHECK(memcmp(ref_out, cbc_expected, 16) == 0 && memcmp(ref_out, rust_out, 16) == 0, "CBC vector and parity");
    ref_aes_cbc_destroy(ref_ctr); aes_cbc_destroy(rust_ctr);
    ref_ctr = ref_aes_cbc_init(aes_key, cbc_iv, AES_DECRYPT);
    rust_ctr = aes_cbc_init(aes_key, cbc_iv, AES_DECRYPT);
    ref_aes_cbc_decrypt(ref_ctr, cbc_expected, ref_out, 16); aes_cbc_decrypt(rust_ctr, cbc_expected, rust_out, 16);
    CHECK(memcmp(ref_out, block, 16) == 0 && memcmp(ref_out, rust_out, 16) == 0, "CBC decrypt parity");
    ref_aes_cbc_reset(ref_ctr); aes_cbc_reset(rust_ctr);
    ref_aes_cbc_decrypt(ref_ctr, cbc_expected, ref_out, 16); aes_cbc_decrypt(rust_ctr, cbc_expected, rust_out, 16);
    CHECK(memcmp(ref_out, rust_out, 16) == 0, "CBC reset parity");
    ref_aes_cbc_destroy(ref_ctr); aes_cbc_destroy(rust_ctr);

    uint8_t gcm_key[16] = {0xb6,0x7b,0x1a,0x6e,0xfd,0xd4,0x0d,0x37,0x08,0x0f,0xbe,0x8f,0x80,0x47,0xae,0xb9};
    uint8_t gcm_iv[16] = {0xfa,0x29,0x4b,0x12,0x99,0x72,0xf7,0xfc,0x5b,0xbd,0x5b,0x96,0xbb,0xa8,0x37,0xc9};
    uint8_t expected_tag[16] = {0xa2,0xcf,0x26,0x48,0x15,0x17,0xec,0x25,0x08,0x5c,0x5b,0x17,0xd0,0x78,0x61,0x83};
    uint8_t empty = 0, ref_tag[16] = {0}, rust_tag[16] = {0};
    CHECK(ref_gcm_encrypt(&empty, 0, ref_out, gcm_key, gcm_iv, ref_tag) == 0, "reference GCM empty vector");
    CHECK(gcm_encrypt(&empty, 0, rust_out, gcm_key, gcm_iv, rust_tag) == 0, "Rust GCM empty vector");
    CHECK(memcmp(ref_tag, expected_tag, 16) == 0 && memcmp(ref_tag, rust_tag, 16) == 0, "GCM 16-byte IV tag parity");
    CHECK(ref_gcm_decrypt(ref_out, 0, ref_out, gcm_key, gcm_iv, ref_tag) == 0, "reference GCM decrypt");
    CHECK(gcm_decrypt(rust_out, 0, rust_out, gcm_key, gcm_iv, rust_tag) == 0, "Rust GCM decrypt");

    uint8_t gcm_data_key[16] = {0x20,0x9e,0x6d,0xbf,0x2a,0xd2,0x6a,0x10,0x54,0x45,0xfc,0x02,0x07,0xcd,0x9e,0x9a};
    uint8_t gcm_data_iv[16] = {0x94,0x77,0x84,0x9d,0x6c,0xcd,0xfc,0xa1,0x12,0xd9,0x2e,0x53,0xfa,0xe4,0xa7,0xca};
    uint8_t gcm_plaintext = 0x01, gcm_ref_ct = 0, gcm_rust_ct = 0;
    uint8_t gcm_ref_tag[16], gcm_rust_tag[16];
    const uint8_t gcm_expected_tag[16] = {0x03,0x2d,0xf7,0xbb,0xa5,0xd8,0xea,0x1a,0x14,0xf1,0x6f,0x70,0xbd,0x0e,0x14,0xec};
    CHECK(ref_gcm_encrypt(&gcm_plaintext, 1, &gcm_ref_ct, gcm_data_key, gcm_data_iv, gcm_ref_tag) == 1, "reference GCM data vector");
    CHECK(gcm_encrypt(&gcm_plaintext, 1, &gcm_rust_ct, gcm_data_key, gcm_data_iv, gcm_rust_tag) == 1, "Rust GCM data vector");
    CHECK(gcm_ref_ct == 0xfd && gcm_ref_ct == gcm_rust_ct && memcmp(gcm_ref_tag, gcm_expected_tag, 16) == 0 && memcmp(gcm_ref_tag, gcm_rust_tag, 16) == 0, "GCM data/tag parity");
    uint8_t gcm_ref_plain = 0, gcm_rust_plain = 0, bad_tag[16];
    memcpy(bad_tag, gcm_rust_tag, 16); bad_tag[0] ^= 1;
    CHECK(ref_gcm_decrypt(&gcm_ref_ct, 1, &gcm_ref_plain, gcm_data_key, gcm_data_iv, gcm_ref_tag) == 1, "reference GCM data decrypt");
    CHECK(gcm_decrypt(&gcm_rust_ct, 1, &gcm_rust_plain, gcm_data_key, gcm_data_iv, gcm_rust_tag) == 1 && gcm_rust_plain == gcm_plaintext, "Rust GCM data decrypt");
    gcm_ref_plain = 0xa5; gcm_rust_plain = 0xa5;
    CHECK(ref_gcm_decrypt(&gcm_ref_ct, 1, &gcm_ref_plain, gcm_data_key, gcm_data_iv, bad_tag) == -1, "reference GCM auth failure return");
    CHECK(gcm_decrypt(&gcm_rust_ct, 1, &gcm_rust_plain, gcm_data_key, gcm_data_iv, bad_tag) == -1, "Rust GCM auth failure return");
    CHECK(gcm_ref_plain == gcm_plaintext && gcm_rust_plain == gcm_ref_plain, "GCM authentication-failure output parity");

    uint8_t gcm_many_plain[37], gcm_many_ref_ct[37], gcm_many_rust_ct[37];
    uint8_t gcm_many_ref_tag[16], gcm_many_rust_tag[16], gcm_many_bad_tag[16];
    uint8_t gcm_many_ref_out[64], gcm_many_rust_out[64];
    for (size_t i = 0; i < sizeof(gcm_many_plain); ++i) gcm_many_plain[i] = (uint8_t)(i * 3 + 1);
    CHECK(ref_gcm_encrypt(gcm_many_plain, sizeof(gcm_many_plain), gcm_many_ref_ct, gcm_data_key, gcm_data_iv, gcm_many_ref_tag) == (int)sizeof(gcm_many_plain), "reference multi-block GCM encrypt");
    CHECK(gcm_encrypt(gcm_many_plain, sizeof(gcm_many_plain), gcm_many_rust_ct, gcm_data_key, gcm_data_iv, gcm_many_rust_tag) == (int)sizeof(gcm_many_plain), "Rust multi-block GCM encrypt");
    CHECK(memcmp(gcm_many_ref_ct, gcm_many_rust_ct, sizeof(gcm_many_ref_ct)) == 0 && memcmp(gcm_many_ref_tag, gcm_many_rust_tag, sizeof(gcm_many_ref_tag)) == 0, "multi-block GCM ciphertext/tag parity");
    memcpy(gcm_many_bad_tag, gcm_many_ref_tag, sizeof(gcm_many_bad_tag));
    gcm_many_bad_tag[7] ^= 0x80;
    memset(gcm_many_ref_out, 0xa5, sizeof(gcm_many_ref_out));
    memset(gcm_many_rust_out, 0xa5, sizeof(gcm_many_rust_out));
    CHECK(ref_gcm_decrypt(gcm_many_ref_ct, sizeof(gcm_many_ref_ct), gcm_many_ref_out, gcm_data_key, gcm_data_iv, gcm_many_bad_tag) == -1, "reference multi-block GCM auth failure");
    CHECK(gcm_decrypt(gcm_many_rust_ct, sizeof(gcm_many_rust_ct), gcm_many_rust_out, gcm_data_key, gcm_data_iv, gcm_many_bad_tag) == -1, "Rust multi-block GCM auth failure");
    CHECK(memcmp(gcm_many_ref_out, gcm_many_rust_out, sizeof(gcm_many_ref_out)) == 0, "full GCM auth-failure buffer parity");
    CHECK(memcmp(gcm_many_ref_out, gcm_many_plain, sizeof(gcm_many_plain)) == 0, "GCM auth failure retains decrypted plaintext");
    for (size_t i = sizeof(gcm_many_plain); i < sizeof(gcm_many_ref_out); ++i) {
        CHECK(gcm_many_ref_out[i] == 0xa5, "GCM auth failure leaves trailing output bytes untouched");
    }

    int ref_result = -1, rust_result = -1;
    ed25519_key_t *ref_ed = ref_ed25519_key_generate("device-test", "", &ref_result);
    ed25519_key_t *rust_ed = ed25519_key_generate("device-test", "", &rust_result);
    CHECK(ref_ed && rust_ed && ref_result == 0 && rust_result == 0, "device-ID Ed25519 key generation");
    uint8_t ref_pub[32], rust_pub[32], ref_sig[64], rust_sig[64];
    const uint8_t message[] = "C ABI parity";
    ref_ed25519_key_get_raw(ref_pub, ref_ed); ed25519_key_get_raw(rust_pub, rust_ed);
    CHECK(memcmp(ref_pub, rust_pub, 32) == 0, "device-ID public key parity");
    ref_ed25519_sign(ref_sig, 64, message, sizeof(message), ref_ed);
    ed25519_sign(rust_sig, 64, message, sizeof(message), rust_ed);
    CHECK(memcmp(ref_sig, rust_sig, 64) == 0, "Ed25519 signature parity");
    CHECK(ref_ed25519_verify(rust_sig, 64, message, sizeof(message), ref_ed) == 1, "reference Ed25519 verify");
    CHECK(ed25519_verify(ref_sig, 64, message, sizeof(message), rust_ed) == 1, "Rust Ed25519 verify");
    ed25519_key_t *ref_ed_public = ref_ed25519_key_from_raw(ref_pub);
    ed25519_key_t *rust_ed_public = ed25519_key_from_raw(rust_pub);
    CHECK(ref_ed25519_verify(rust_sig, 64, message, sizeof(message), ref_ed_public) == 1, "reference raw Ed25519 import");
    CHECK(ed25519_verify(ref_sig, 64, message, sizeof(message), rust_ed_public) == 1, "Rust raw Ed25519 import");
    ed25519_key_t *rust_copy = ed25519_key_copy(rust_ed);
    CHECK(rust_copy != NULL, "Ed25519 copy");
    ref_ed25519_key_destroy(ref_ed); ed25519_key_destroy(rust_ed); ed25519_key_destroy(rust_copy);
    ref_ed25519_key_destroy(ref_ed_public); ed25519_key_destroy(rust_ed_public);

    const char *ref_keyfile = ".\\uxplay_crypto_ref_smoke.pem";
    const char *rust_keyfile = ".\\uxplay_crypto_rust_smoke.pem";
    remove(ref_keyfile); remove(rust_keyfile);
    ref_result = -1; rust_result = -1;
    ref_ed = ref_ed25519_key_generate("ignored-device", ref_keyfile, &ref_result);
    rust_ed = ed25519_key_generate("ignored-device", rust_keyfile, &rust_result);
    CHECK(ref_ed && rust_ed && ref_result == 1 && rust_result == 1, "random Ed25519 key-file creation");
    uint8_t ref_file_pub[32], rust_file_pub[32];
    ref_ed25519_key_get_raw(ref_file_pub, ref_ed); ed25519_key_get_raw(rust_file_pub, rust_ed);
    ref_ed25519_key_destroy(ref_ed); ed25519_key_destroy(rust_ed);
    ref_result = -1; rust_result = -1;
    ref_ed = ref_ed25519_key_generate("ignored-device", ref_keyfile, &ref_result);
    rust_ed = ed25519_key_generate("ignored-device", rust_keyfile, &rust_result);
    CHECK(ref_ed && rust_ed && ref_result == 0 && rust_result == 0, "random Ed25519 key-file reload");
    uint8_t ref_file_loaded_pub[32], rust_file_loaded_pub[32];
    ref_ed25519_key_get_raw(ref_file_loaded_pub, ref_ed); ed25519_key_get_raw(rust_file_loaded_pub, rust_ed);
    CHECK(memcmp(ref_file_pub, ref_file_loaded_pub, 32) == 0 && memcmp(rust_file_pub, rust_file_loaded_pub, 32) == 0, "key-file public-key round trip");
    ref_ed25519_key_destroy(ref_ed); ed25519_key_destroy(rust_ed);
    remove(ref_keyfile); remove(rust_keyfile);

    sha_ctx_t *ref_sha = ref_sha_init(); sha_ctx_t *rust_sha = sha_init();
    uint8_t ref_hash[64], rust_hash[64]; unsigned int ref_hash_len = 0, rust_hash_len = 0;
    ref_sha_update(ref_sha, (const uint8_t *)"a", 1); ref_sha_update(ref_sha, (const uint8_t *)"bc", 2);
    sha_update(rust_sha, (const uint8_t *)"a", 1); sha_update(rust_sha, (const uint8_t *)"bc", 2);
    ref_sha_final(ref_sha, ref_hash, &ref_hash_len); sha_final(rust_sha, rust_hash, &rust_hash_len);
    CHECK(ref_hash_len == 64 && rust_hash_len == 64 && memcmp(ref_hash, rust_hash, 64) == 0, "streaming SHA-512 parity");
    ref_sha_reset(ref_sha); sha_reset(rust_sha);
    ref_sha_update(ref_sha, (const uint8_t *)"abc", 3); sha_update(rust_sha, (const uint8_t *)"abc", 3);
    ref_sha_final(ref_sha, ref_hash, NULL); sha_final(rust_sha, rust_hash, NULL);
    CHECK(memcmp(ref_hash, rust_hash, 64) == 0, "SHA-512 reset and optional length parity");
    ref_sha_destroy(ref_sha); sha_destroy(rust_sha);

    md5_ctx_t *ref_md5 = ref_md5_init(); md5_ctx_t *rust_md5 = md5_init();
    uint8_t ref_md5_out[16], rust_md5_out[16]; unsigned int ref_md5_len = 0, rust_md5_len = 0;
    ref_md5_update(ref_md5, (const uint8_t *)"abc", 3); md5_update(rust_md5, (const uint8_t *)"abc", 3);
    ref_md5_final(ref_md5, ref_md5_out, &ref_md5_len); md5_final(rust_md5, rust_md5_out, &rust_md5_len);
    CHECK(ref_md5_len == 16 && rust_md5_len == 16 && memcmp(ref_md5_out, rust_md5_out, 16) == 0, "streaming MD5 parity");
    ref_md5_reset(ref_md5); md5_reset(rust_md5);
    ref_md5_update(ref_md5, (const uint8_t *)"abc", 3); md5_update(rust_md5, (const uint8_t *)"abc", 3);
    ref_md5_final(ref_md5, ref_md5_out, NULL); md5_final(rust_md5, rust_md5_out, NULL);
    CHECK(memcmp(ref_md5_out, rust_md5_out, 16) == 0, "MD5 reset and optional length parity");
    ref_md5_destroy(ref_md5); md5_destroy(rust_md5);
    char *ref_md5_hex = ref_get_md5("abc"); char *rust_md5_hex = get_md5("abc");
    CHECK(ref_md5_hex && rust_md5_hex && strcmp(ref_md5_hex, rust_md5_hex) == 0, "get_md5 parity");
    free(ref_md5_hex); free(rust_md5_hex);

    x25519_key_t *ref_a = ref_x25519_key_generate(), *ref_b = ref_x25519_key_generate();
    x25519_key_t *rust_a = x25519_key_generate(), *rust_b = x25519_key_generate();
    CHECK(ref_a && ref_b && rust_a && rust_b, "X25519 random key generation");
    uint8_t ref_ab[32], ref_ba[32], rust_ab[32], rust_ba[32];
    ref_x25519_derive_secret(ref_ab, ref_a, ref_b); ref_x25519_derive_secret(ref_ba, ref_b, ref_a);
    x25519_derive_secret(rust_ab, rust_a, rust_b); x25519_derive_secret(rust_ba, rust_b, rust_a);
    CHECK(memcmp(ref_ab, ref_ba, 32) == 0 && memcmp(rust_ab, rust_ba, 32) == 0, "X25519 shared-secret symmetry");
    uint8_t ref_b_pub[32], rust_b_pub[32], ref_imported_ab[32], rust_imported_ab[32];
    ref_x25519_key_get_raw(ref_b_pub, ref_b); x25519_key_get_raw(rust_b_pub, rust_b);
    x25519_key_t *ref_b_imported = ref_x25519_key_from_raw(ref_b_pub);
    x25519_key_t *rust_b_imported = x25519_key_from_raw(rust_b_pub);
    CHECK(ref_b_imported && rust_b_imported, "X25519 raw public import");
    ref_x25519_derive_secret(ref_imported_ab, ref_a, ref_b_imported);
    x25519_derive_secret(rust_imported_ab, rust_a, rust_b_imported);
    CHECK(memcmp(ref_ab, ref_imported_ab, 32) == 0 && memcmp(rust_ab, rust_imported_ab, 32) == 0, "X25519 raw public round trip");
    ref_x25519_key_destroy(ref_b_imported); x25519_key_destroy(rust_b_imported);
    ref_x25519_key_destroy(ref_a); ref_x25519_key_destroy(ref_b); x25519_key_destroy(rust_a); x25519_key_destroy(rust_b);

    uint8_t ref_random[32], rust_random[32];
    CHECK(ref_get_random_bytes(ref_random, 32) == 1 && get_random_bytes(rust_random, 32) == 1, "random byte return and size");
    char ref_b64[5], rust_b64[5]; const uint8_t sample[] = {'a','b','c'};
    ref_pk_to_base64(sample, 3, ref_b64, sizeof(ref_b64)); pk_to_base64(sample, 3, rust_b64, sizeof(rust_b64));
    CHECK(strcmp(ref_b64, "YWJj") == 0 && strcmp(ref_b64, rust_b64) == 0, "base64 parity");

    FreeLibrary(rust);
    puts("C ABI source comparison passed");
    return 0;
}
