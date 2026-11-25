#include "fairplay_v2.h"

#include <string.h>

int
main(void)
{
    fairplay_v2_t *fairplay = fairplay_v2_init();
    uint8_t request[16] = {'F', 'P', 'L', 'Y', 2, 1, 1, 0, 0, 0, 0, 4, 2, 0, 2, 0xbb};
    uint8_t setup_response[142];
    uint8_t handshake_request[164] = {'F', 'P', 'L', 'Y', 2, 1, 3, 0, 0, 0, 0, 152};
    uint8_t handshake_response[32];
    uint8_t encrypted_key[72] = {0};
    uint8_t raw_key[16];
    unsigned i;

    if (fairplay == NULL) return 1;
    memset(setup_response, 0xa5, sizeof(setup_response));
    if (fairplay_v2_setup(fairplay, request, setup_response) != FAIRPLAY_V2_OK) return 2;
    if (memcmp(setup_response, "FPLY\x02\x01\x02\0\0\0\0\x82\x02\x02", 14) != 0) return 3;

    request[15] ^= 1;
    memset(setup_response, 0xa5, sizeof(setup_response));
    if (fairplay_v2_setup(fairplay, request, setup_response) != FAIRPLAY_V2_INVALID) return 4;
    for (i = 0; i < sizeof(setup_response); i++) if (setup_response[i] != 0xa5) return 5;

    memset(handshake_response, 0xa5, sizeof(handshake_response));
    if (fairplay_v2_handshake(fairplay, handshake_request, handshake_response) != FAIRPLAY_V2_INVALID) return 6;
    for (i = 0; i < sizeof(handshake_response); i++) if (handshake_response[i] != 0xa5) return 7;

    memset(raw_key, 0xa5, sizeof(raw_key));
    if (fairplay_v2_decrypt_key(fairplay, encrypted_key, raw_key) != FAIRPLAY_V2_INVALID) return 8;
    for (i = 0; i < sizeof(raw_key); i++) if (raw_key[i] != 0) return 9;

    fairplay_v2_destroy(fairplay);
    fairplay_v2_destroy(NULL);
    return 0;
}
