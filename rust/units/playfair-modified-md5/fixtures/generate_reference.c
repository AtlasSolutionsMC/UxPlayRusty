#include <stdint.h>
#include <math.h>
#include <stdio.h>
#include <string.h>

void modified_md5(unsigned char *block, unsigned char *key, unsigned char *out);

static void print_hex(const unsigned char *bytes, size_t length) {
    for (size_t i = 0; i < length; ++i) {
        printf("%02x", bytes[i]);
    }
}

static void emit(const char *name, unsigned char block[64], uint32_t key_words[4]) {
    unsigned char key[16];
    unsigned char output[16];
    memcpy(key, key_words, sizeof key);
    modified_md5(block, key, output);
    printf("%s ", name);
    print_hex(block, 64);
    printf(" ");
    print_hex(key, sizeof key);
    printf(" ");
    print_hex(output, sizeof output);
    printf("\n");
}

static void emit_round_constants(void) {
    printf("round-constants");
    for (int i = 0; i < 64; ++i) {
        uint32_t value = (uint32_t)((1LL << 32) * fabs(sin(i + 1)));
        printf(" %08x", value);
    }
    printf("\n");
}

int main(void) {
    unsigned char block[64];
    uint32_t key[4];

    emit_round_constants();

    memset(block, 0, sizeof block);
    key[0] = 0x67452301;
    key[1] = 0xefcdab89;
    key[2] = 0x98badcfe;
    key[3] = 0x10325476;
    emit("zero-block-md5-iv", block, key);

    for (unsigned i = 0; i < sizeof block; ++i) {
        block[i] = (unsigned char)i;
    }
    key[0] = 0x01234567;
    key[1] = 0x89abcdef;
    key[2] = 0xfedcba98;
    key[3] = 0x76543210;
    emit("ascending-block-distinct-key", block, key);

    for (unsigned i = 0; i < sizeof block; ++i) {
        block[i] = (unsigned char)(i * 37U + 11U);
    }
    key[0] = 0;
    key[1] = UINT32_MAX;
    key[2] = 0x80000000;
    key[3] = 1;
    emit("wrapped-pattern-boundary-key", block, key);

    for (unsigned i = 0; i < sizeof block; ++i) {
        block[i] = (unsigned char)(255U - i);
    }
    key[0] = 0xa5a5a5a5;
    key[1] = 0x5a5a5a5a;
    key[2] = 0xdeadbeef;
    key[3] = 0x12345678;
    emit("descending-block-mixed-key", block, key);

    uint32_t state = 0x6d2b79f5;
    for (unsigned i = 0; i < sizeof block; ++i) {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        block[i] = (unsigned char)(state >> 24);
    }
    for (unsigned i = 0; i < 4; ++i) {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        key[i] = state;
    }
    emit("xorshift32-block-key", block, key);
    return 0;
}
