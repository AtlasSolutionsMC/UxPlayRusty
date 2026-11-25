#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "byteutils.h"

uint16_t reference_get_short(unsigned char *, int);
uint32_t reference_get_int(unsigned char *, int);
uint64_t reference_get_long(unsigned char *, int);
uint16_t reference_get_short_be(unsigned char *, int);
uint32_t reference_get_int_be(unsigned char *, int);
uint64_t reference_get_long_be(unsigned char *, int);
float reference_get_float(unsigned char *, int);
uint64_t reference_get_ntp_timestamp(unsigned char *, int);
void reference_put_long_be(unsigned char *, int, uint64_t);
void reference_put_ntp_timestamp(unsigned char *, int, uint64_t);

static void fill(unsigned char bytes[32], unsigned int pattern) {
    unsigned int state = 0x12345678U;
    for (unsigned int i = 0; i < 32; i++) {
        switch (pattern) {
        case 0: bytes[i] = 0; break;
        case 1: bytes[i] = 0xff; break;
        case 2: bytes[i] = (unsigned char)i; break;
        case 3: bytes[i] = (unsigned char)(i * 37U + 11U); break;
        default:
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            bytes[i] = (unsigned char)state;
            break;
        }
    }
}

int main(void) {
    static const int offsets[] = {0, 1, 5, 12, 3};
    static const uint64_t times[] = {0, 1, 1700000000123456789ULL, UINT64_MAX,
                                     2208988800000000000ULL};
    for (unsigned int pattern = 0; pattern < 5; pattern++) {
        unsigned char original[32], rust_long[32], c_long[32], rust_ntp[32], c_ntp[32];
        fill(original, pattern);
        int offset = offsets[pattern];
        float rust_float = byteutils_get_float(original, offset);
        float c_float = reference_get_float(original, offset);
        if (byteutils_get_short(original, offset) != reference_get_short(original, offset) ||
            byteutils_get_int(original, offset) != reference_get_int(original, offset) ||
            byteutils_get_long(original, offset) != reference_get_long(original, offset) ||
            byteutils_get_short_be(original, offset) != reference_get_short_be(original, offset) ||
            byteutils_get_int_be(original, offset) != reference_get_int_be(original, offset) ||
            byteutils_get_long_be(original, offset) != reference_get_long_be(original, offset) ||
            memcmp(&rust_float, &c_float, sizeof(float)) != 0 ||
            byteutils_get_ntp_timestamp(original, offset) !=
                reference_get_ntp_timestamp(original, offset)) {
            fprintf(stderr, "getter mismatch in pattern %u\n", pattern);
            return 1;
        }
        memcpy(rust_long, original, sizeof(original));
        memcpy(c_long, original, sizeof(original));
        memcpy(rust_ntp, original, sizeof(original));
        memcpy(c_ntp, original, sizeof(original));
        byteutils_put_long_be(rust_long, offset, 0x0123456789abcdefULL);
        reference_put_long_be(c_long, offset, 0x0123456789abcdefULL);
        byteutils_put_ntp_timestamp(rust_ntp, offset, times[pattern]);
        reference_put_ntp_timestamp(c_ntp, offset, times[pattern]);
        if (memcmp(rust_long, c_long, sizeof(rust_long)) != 0 ||
            memcmp(rust_ntp, c_ntp, sizeof(rust_ntp)) != 0) {
            fprintf(stderr, "setter mismatch in pattern %u\n", pattern);
            return 1;
        }
    }
    puts("C ABI matches all byteutils exports for five deterministic buffers");
    return 0;
}
