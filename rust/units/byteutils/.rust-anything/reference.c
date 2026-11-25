#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "byteutils.h"

static const uint64_t times_ns[] = {
    0ULL,
    1ULL,
    1700000000123456789ULL,
    UINT64_MAX,
    2208988800000000000ULL,
};
static const int offsets[] = {0, 1, 5, 12, 3};

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

static void print_bytes(const unsigned char *bytes, size_t len) {
    for (size_t i = 0; i < len; i++) printf("%02x", bytes[i]);
}

int main(void) {
    for (unsigned int pattern = 0; pattern < 5; pattern++) {
        unsigned char bytes[32];
        unsigned char long_output[32];
        unsigned char ntp_output[32];
        uint32_t float_bits;
        float value;
        fill(bytes, pattern);
        int offset = offsets[pattern];
        value = byteutils_get_float(bytes, offset);
        memcpy(&float_bits, &value, sizeof(float_bits));
        printf("%u\t", pattern);
        print_bytes(bytes, sizeof(bytes));
        printf("\t%d\t%04" PRIx16 "\t%08" PRIx32 "\t%016" PRIx64
               "\t%04" PRIx16 "\t%08" PRIx32 "\t%016" PRIx64
               "\t%08" PRIx32 "\t%" PRIu64 "\t",
               offset,
               byteutils_get_short(bytes, offset),
               byteutils_get_int(bytes, offset),
               byteutils_get_long(bytes, offset),
               byteutils_get_short_be(bytes, offset),
               byteutils_get_int_be(bytes, offset),
               byteutils_get_long_be(bytes, offset),
               float_bits,
               byteutils_get_ntp_timestamp(bytes, offset));
        memcpy(long_output, bytes, sizeof(bytes));
        memcpy(ntp_output, bytes, sizeof(bytes));
        byteutils_put_long_be(long_output, offset, 0x0123456789abcdefULL);
        byteutils_put_ntp_timestamp(ntp_output, offset, times_ns[pattern]);
        print_bytes(long_output, sizeof(long_output));
        printf("\t");
        print_bytes(ntp_output, sizeof(ntp_output));
        printf("\n");
    }
    return 0;
}
