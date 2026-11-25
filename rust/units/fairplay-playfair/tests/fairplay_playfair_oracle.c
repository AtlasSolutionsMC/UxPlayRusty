#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "fairplay.h"

static void print_hex(const unsigned char *bytes, size_t length)
{
    for (size_t i = 0; i < length; ++i) {
        printf("%02x", bytes[i]);
    }
}

int main(void)
{
    for (unsigned char mode = 0; mode < 4; ++mode) {
        fairplay_t *fairplay = fairplay_init(NULL);
        if (!fairplay) return 2;

        unsigned char setup_request[16] = {0};
        for (size_t i = 0; i < sizeof(setup_request); ++i) {
            setup_request[i] = (unsigned char)(i * 13 + mode);
        }
        setup_request[4] = 3;
        setup_request[14] = mode;
        unsigned char setup_response[142] = {0};
        int setup_status = fairplay_setup(fairplay, setup_request, setup_response);
        printf("setup-%u %d ", mode, setup_status);
        print_hex(setup_response, sizeof(setup_response));
        putchar('\n');

        unsigned char handshake_request[164] = {0};
        for (size_t i = 0; i < sizeof(handshake_request); ++i) {
            handshake_request[i] = (unsigned char)(i * 17 + mode * 3);
        }
        handshake_request[4] = 3;
        handshake_request[12] = mode;
        unsigned char handshake_response[32] = {0};
        int handshake_status = fairplay_handshake(fairplay, handshake_request, handshake_response);
        printf("handshake-%u %d ", mode, handshake_status);
        print_hex(handshake_response, sizeof(handshake_response));
        putchar('\n');

        unsigned char encrypted_key[72] = {0};
        for (size_t i = 0; i < sizeof(encrypted_key); ++i) {
            encrypted_key[i] = (unsigned char)(i * 29 + mode * 7);
        }
        unsigned char output_key[16] = {0};
        int decrypt_status = fairplay_decrypt(fairplay, encrypted_key, output_key);
        printf("decrypt-%u %d ", mode, decrypt_status);
        print_hex(output_key, sizeof(output_key));
        putchar('\n');
        fairplay_destroy(fairplay);
    }

    fairplay_t *fairplay = fairplay_init(NULL);
    if (!fairplay) return 2;
    unsigned char invalid_setup[16] = {0};
    invalid_setup[4] = 0x7f;
    unsigned char setup_response[142];
    memset(setup_response, 0xa5, sizeof(setup_response));
    int setup_status = fairplay_setup(fairplay, invalid_setup, setup_response);
    printf("invalid-setup %d ", setup_status);
    print_hex(setup_response, sizeof(setup_response));
    putchar('\n');
    unsigned char key_output[16];
    memset(key_output, 0xa5, sizeof(key_output));
    unsigned char encrypted_key[72] = {0};
    int decrypt_status = fairplay_decrypt(fairplay, encrypted_key, key_output);
    printf("decrypt-after-invalid-setup %d ", decrypt_status);
    print_hex(key_output, sizeof(key_output));
    putchar('\n');

    fairplay_t *v2_fairplay = fairplay_init(NULL);
    if (!v2_fairplay) return 2;
    unsigned char invalid_v2_setup[16] = {0};
    invalid_v2_setup[4] = 2;
    memset(setup_response, 0xa5, sizeof(setup_response));
    setup_status = fairplay_setup(v2_fairplay, invalid_v2_setup, setup_response);
    printf("invalid-v2-setup %d ", setup_status);
    print_hex(setup_response, sizeof(setup_response));
    putchar('\n');
    memset(key_output, 0xa5, sizeof(key_output));
    decrypt_status = fairplay_decrypt(v2_fairplay, encrypted_key, key_output);
    printf("decrypt-after-invalid-v2-setup %d ", decrypt_status);
    print_hex(key_output, sizeof(key_output));
    putchar('\n');
    fairplay_destroy(v2_fairplay);

    unsigned char valid_handshake[164] = {0};
    valid_handshake[4] = 3;
    unsigned char handshake_response[32] = {0};
    int handshake_status = fairplay_handshake(fairplay, valid_handshake, handshake_response);
    printf("state-handshake %d ", handshake_status);
    print_hex(handshake_response, sizeof(handshake_response));
    putchar('\n');
    unsigned char invalid_handshake[164] = {0};
    invalid_handshake[4] = 4;
    int invalid_status = fairplay_handshake(fairplay, invalid_handshake, handshake_response);
    printf("invalid-handshake %d ", invalid_status);
    print_hex(handshake_response, sizeof(handshake_response));
    putchar('\n');
    memset(key_output, 0xa5, sizeof(key_output));
    decrypt_status = fairplay_decrypt(fairplay, encrypted_key, key_output);
    printf("decrypt-after-invalid-handshake %d ", decrypt_status);
    print_hex(key_output, sizeof(key_output));
    putchar('\n');
    fairplay_destroy(fairplay);
    return 0;
}
