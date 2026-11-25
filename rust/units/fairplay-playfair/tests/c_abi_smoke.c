#include "fairplay.h"

#include <string.h>

int main(void)
{
    fairplay_t *fairplay = fairplay_init(NULL);
    if (fairplay == NULL) return 1;

    unsigned char setup_request[16] = {'F', 'P', 'L', 'Y', 3, 1, 2, 0, 0, 0, 0, 130};
    unsigned char setup_response[142];
    memset(setup_response, 0xa5, sizeof(setup_response));
    if (fairplay_setup(fairplay, setup_request, setup_response) != 0) return 2;
    if (memcmp(setup_response, "FPLY\x03\x01\x02\0\0\0\0\x82\x02\0", 14) != 0) return 3;

    unsigned char handshake_request[164] = {'F', 'P', 'L', 'Y', 3, 1, 3, 0, 0, 0, 0, 152};
    unsigned char handshake_response[32];
    memset(handshake_response, 0xa5, sizeof(handshake_response));
    if (fairplay_handshake(fairplay, handshake_request, handshake_response) != 0) return 4;
    if (memcmp(handshake_response, "FPLY\x03\x01\x04\0\0\0\0\x14", 12) != 0) return 5;

    unsigned char encrypted_key[72] = {0};
    unsigned char raw_key[16];
    if (fairplay_decrypt(fairplay, encrypted_key, raw_key) != 0) return 6;

    fairplay_destroy(fairplay);
    fairplay_destroy(NULL);
    return 0;
}
