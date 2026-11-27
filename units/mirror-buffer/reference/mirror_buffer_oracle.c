/* Test-only driver for the unchanged UxPlay mirror_buffer and crypto sources. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#include "mirror_buffer.h"
#include "utils.h"

char *utils_hex_to_string(const unsigned char *hex, int hex_len)
{
    static const char digits[] = "0123456789abcdef";
    char *result = malloc((size_t)hex_len * 2 + 1);
    if (!result) return NULL;
    for (int i = 0; i < hex_len; i++) {
        result[i * 2] = digits[hex[i] >> 4];
        result[i * 2 + 1] = digits[hex[i] & 0x0f];
    }
    result[hex_len * 2] = '\0';
    return result;
}

static void print_hex(const unsigned char *bytes, int len)
{
    for (int i = 0; i < len; i++) printf("%02x", bytes[i]);
}

int main(void)
{
    static const unsigned char audio_key[] = "audio-key-fixtur";
    static const int lengths[] = {0, 13, 7, 31, 1, 48, 15, 2, 16, 35};
    mirror_buffer_t *buffer = mirror_buffer_init(NULL, audio_key);
    if (!buffer) return 2;

    for (int phase = 0; phase < 2; phase++) {
        uint64_t stream_id = phase == 0 ? UINT64_C(0x0123456789abcdef) : UINT64_MAX;
        mirror_buffer_init_aes(buffer, &stream_id);
        int count = phase == 0 ? (int)(sizeof(lengths) / sizeof(lengths[0])) : 2;
        for (int step = 0; step < count; step++) {
            int len = phase == 0 ? lengths[step] : (step == 0 ? 21 : 5);
            int source_step = phase == 0 ? step : 20 + step;
            unsigned char input[64] = {0};
            unsigned char output[64];
            for (int i = 0; i < len; i++)
                input[i] = (unsigned char)(i * 31 + source_step * 17);
            for (int i = 0; i < (int)sizeof(output); i++) output[i] = 0;
            mirror_buffer_decrypt(buffer, input, output, len);
            printf("%d,%d,", phase, step);
            print_hex(input, len);
            printf(",");
            print_hex(output, len);
            printf("\n");
        }
    }
    mirror_buffer_destroy(buffer);
    return 0;
}
