/* Deterministic differential oracle; includes the untouched reference source. */
static int fairplay_test_rand_bytes(unsigned char *buffer, int length);
#define RAND_bytes fairplay_test_rand_bytes
#include "../../../../../UxPlay/lib/fairplay_v2/fairplay_v2.c"
#undef RAND_bytes

#include <stdio.h>

static int
fairplay_test_rand_bytes(unsigned char *buffer, int length)
{
    int i;
    for (i = 0; i < length; i++) {
        buffer[i] = (uint8_t)((unsigned)i * 37u + 11u);
    }
    return 1;
}

static void
print_line(int result, const uint8_t *bytes, size_t length)
{
    size_t i;
    printf("%d ", result);
    for (i = 0; i < length; i++) {
        printf("%02x", bytes[i]);
    }
    putchar('\n');
}

static int
oracle_encrypt_wrapping_block(const uint8_t key[16], const uint8_t input[16], uint8_t output[16])
{
    EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
    int written = 0, final_written = 0;
    int ok;
    if (ctx == NULL) return 0;
    ok = EVP_EncryptInit_ex(ctx, EVP_aes_128_ecb(), NULL, key, NULL) == 1 &&
         EVP_CIPHER_CTX_set_padding(ctx, 0) == 1 &&
         EVP_EncryptUpdate(ctx, output, &written, input, 16) == 1 && written == 16 &&
         EVP_EncryptFinal_ex(ctx, output + written, &final_written) == 1 && final_written == 0;
    EVP_CIPHER_CTX_free(ctx);
    return ok;
}

int
main(void)
{
    uint8_t setup_request[16], setup_response[142];
    uint8_t handshake_request[164], handshake_response[32];
    uint8_t encrypted_key[72], bad_key[72], raw[16], mask[16];
    uint8_t wrapping_key[16], block[16], authenticated[52], mac_key[20], mac[EVP_MAX_MD_SIZE];
    unsigned mac_length = 0;
    unsigned i;
    uint8_t mode;
    fairplay_v2_t *fairplay;
    int result;

    for (mode = 0; mode < 4; mode++) {
        fairplay = fairplay_v2_init();
        if (fairplay == NULL) return 10;

        memset(setup_request, 0, sizeof(setup_request));
        record_header(setup_request, 1, 4);
        setup_request[12] = 2;
        setup_request[14] = mode;
        setup_request[15] = 0xbb;
        memset(setup_response, 0xa5, sizeof(setup_response));
        result = fairplay_v2_setup(fairplay, setup_request, setup_response);
        print_line(result, setup_response, sizeof(setup_response));

        memset(handshake_request, 0, sizeof(handshake_request));
        record_header(handshake_request, 3, 152);
        handshake_request[12] = mode;
        memcpy(handshake_request + 13, "\x8f\x1a\x9c", 3);
        for (i = 0; i < 128; i++) handshake_request[16 + i] = (uint8_t)(i * 13u + 7u);
        if (fairplay_v2_encrypt_sap(mode, handshake_request + 16, handshake_request + 16) != FAIRPLAY_V2_OK) return 11;
        if (fairplay_v2_decrypt_sap(mode, handshake_request + 16, fairplay->sender_sap) != FAIRPLAY_V2_OK) return 13;
        if (confirmation(fairplay->sender_sap, fairplay->receiver_sap, handshake_request + 144) != FAIRPLAY_V2_OK) return 14;
        memset(handshake_response, 0xa5, sizeof(handshake_response));
        result = fairplay_v2_handshake(fairplay, handshake_request, handshake_response);
        print_line(result, handshake_response, sizeof(handshake_response));

        memset(encrypted_key, 0, sizeof(encrypted_key));
        memcpy(encrypted_key, "FPLY\x01\x02\x01\x00", 8);
        store_be32(encrypted_key + 8, 60);
        store_be32(encrypted_key + 32, 16);
        for (i = 0; i < 16; i++) {
            raw[i] = (uint8_t)(0x10u + i);
            mask[i] = (uint8_t)(0x80u + i);
            block[i] = raw[i] ^ mask[i];
            encrypted_key[16 + i] = mask[i];
        }
        if (derive_wrapping_key(fairplay->sender_sap, fairplay->receiver_sap, wrapping_key) != FAIRPLAY_V2_OK) return 15;
        if (!oracle_encrypt_wrapping_block(wrapping_key, block, encrypted_key + 56)) return 16;
        memcpy(authenticated, encrypted_key, 36);
        memcpy(authenticated + 36, raw, sizeof(raw));
        if (derive_mac_key(fairplay->sender_sap, fairplay->receiver_sap, mac_key) != FAIRPLAY_V2_OK) return 17;
        if (HMAC(EVP_sha1(), mac_key, sizeof(mac_key), authenticated, sizeof(authenticated), mac, &mac_length) == NULL ||
            mac_length != 20) return 18;
        memcpy(encrypted_key + 36, mac, 20);
        memset(raw, 0xa5, sizeof(raw));
        result = fairplay_v2_decrypt_key(fairplay, encrypted_key, raw);
        print_line(result, raw, sizeof(raw));

        memcpy(bad_key, encrypted_key, sizeof(bad_key));
        bad_key[36] ^= 1;
        memset(raw, 0xa5, sizeof(raw));
        result = fairplay_v2_decrypt_key(fairplay, bad_key, raw);
        print_line(result, raw, sizeof(raw));

        handshake_request[144] ^= 1;
        memset(handshake_response, 0xa5, sizeof(handshake_response));
        result = fairplay_v2_handshake(fairplay, handshake_request, handshake_response);
        print_line(result, handshake_response, sizeof(handshake_response));
        memset(raw, 0xa5, sizeof(raw));
        result = fairplay_v2_decrypt_key(fairplay, encrypted_key, raw);
        print_line(result, raw, sizeof(raw));
        handshake_request[144] ^= 1;

        setup_request[15] ^= 1;
        memset(setup_response, 0xa5, sizeof(setup_response));
        result = fairplay_v2_setup(fairplay, setup_request, setup_response);
        print_line(result, setup_response, sizeof(setup_response));

        memset(handshake_response, 0xa5, sizeof(handshake_response));
        result = fairplay_v2_handshake(fairplay, handshake_request, handshake_response);
        print_line(result, handshake_response, sizeof(handshake_response));

        OPENSSL_cleanse(wrapping_key, sizeof(wrapping_key));
        OPENSSL_cleanse(mac_key, sizeof(mac_key));
        OPENSSL_cleanse(block, sizeof(block));
        OPENSSL_cleanse(authenticated, sizeof(authenticated));
        OPENSSL_cleanse(mac, sizeof(mac));
        fairplay_v2_destroy(fairplay);
    }
    return 0;
}
