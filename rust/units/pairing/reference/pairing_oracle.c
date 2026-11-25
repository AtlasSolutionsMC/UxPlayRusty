/* Differential oracle for the unchanged UxPlay pairing implementation. */
#include <stdio.h>
#include <string.h>
#include <stdint.h>
#include "pairing.h"

static void print_hex(const unsigned char *bytes, size_t length) {
    for (size_t index = 0; index < length; index++) {
        printf("%02x", bytes[index]);
    }
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    const char *device_id = "02:11:22:33:44:55";
    const char *authorization =
        "Digest username=\"Mufasa\", realm=\"testrealm@host.com\", "
        "nonce=\"dcd98b7102dd2f0e8b11d0f600bfb0c093\", "
        "uri=\"/dir/index.html\", qop=auth, nc=00000001, "
        "cnonce=\"0a4f113b\", response=\"6629fae49393a05397450978507c4ef1\", "
        "opaque=\"5ccc069c403ebaf9f0171e9517f40e41\"";
    int result = 0;
    pairing_t *pairing = pairing_init_generate(device_id, "", &result);
    if (!pairing) {
        return 2;
    }

    unsigned char public_key[ED25519_KEY_SIZE] = {0};
    pairing_get_public_key(pairing, public_key);
    printf("public_key\t");
    print_hex(public_key, sizeof(public_key));
    putchar('\n');
    printf("init_result\t%d\n", result);

    pairing_session_t *session = pairing_session_init(pairing);
    if (!session) {
        pairing_destroy(pairing);
        return 3;
    }
    printf("initial_status\t%d\n", pairing_session_check_handshake_status(session));

    unsigned char secret[ X25519_KEY_SIZE ];
    memset(secret, 0xaa, sizeof(secret));
    printf("initial_secret_status\t%d\n", pairing_get_ecdh_secret_key(session, secret));
    printf("initial_secret_buffer\t");
    print_hex(secret, sizeof(secret));
    putchar('\n');

    uint64_t local_time = UINT64_C(0x0102030405060708);
    const char client_data[] = "client-data";
    unsigned char nonce[32] = {0};
    printf("nonce_status\t%d\n", pairing_session_make_nonce(
        session, &local_time, client_data, nonce, (int)sizeof(nonce)));
    printf("nonce\t");
    print_hex(nonce, sizeof(nonce));
    putchar('\n');

    pairing_session_set_setup_status(session);
    printf("setup_status\t%d\n", pairing_session_check_handshake_status(session));
    printf("digest_valid\t%d\n", pairing_digest_verify("GET", authorization, "Circle Of Life"));
    printf("digest_bad_password\t%d\n", pairing_digest_verify("GET", authorization, "wrong"));

    pairing_session_destroy(session);
    pairing_destroy(pairing);
    return 0;
}
