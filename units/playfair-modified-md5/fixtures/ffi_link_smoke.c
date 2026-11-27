#include "modified_md5_rs.h"

#include <stdio.h>
#include <string.h>

void reference_swap(uint32_t *a, uint32_t *b);
void reference_modified_md5(unsigned char *block, unsigned char *key, unsigned char *output);

static void initial_key(unsigned char key[16]) {
    const uint32_t words[4] = {0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476};
    memcpy(key, words, sizeof words);
}

int main(void) {
    uint32_t rust_pair[2] = {0x01234567, 0x89abcdef};
    uint32_t c_pair[2] = {0x01234567, 0x89abcdef};
    swap(&rust_pair[0], &rust_pair[1]);
    reference_swap(&c_pair[0], &c_pair[1]);
    if (memcmp(rust_pair, c_pair, sizeof rust_pair) != 0) {
        fputs("swap C ABI mismatch\n", stderr);
        return 1;
    }

    unsigned char block[64];
    for (unsigned i = 0; i < sizeof block; ++i) {
        block[i] = (unsigned char)(i * 37U + 11U);
    }
    unsigned char c_key[16];
    unsigned char rust_key[16];
    unsigned char alias_c_key[16];
    unsigned char alias_rust_key[16];
    unsigned char c_output[16];
    unsigned char rust_output[16];
    initial_key(c_key);
    memcpy(rust_key, c_key, sizeof c_key);

    reference_modified_md5(block, c_key, c_output);
    modified_md5(block, rust_key, rust_output);
    if (memcmp(c_output, rust_output, sizeof c_output) != 0) {
        fputs("modified_md5 C ABI mismatch\n", stderr);
        return 2;
    }

    initial_key(alias_c_key);
    memcpy(alias_rust_key, alias_c_key, sizeof alias_c_key);
    reference_modified_md5(block, alias_c_key, alias_c_key);
    rust_anything_modified_md5(block, alias_rust_key, alias_rust_key);
    if (memcmp(alias_c_key, alias_rust_key, sizeof alias_c_key) != 0) {
        fputs("modified_md5 aliasing mismatch\n", stderr);
        return 3;
    }

    puts("C ABI matches reference for swap, modified_md5, and aliased output");
    return 0;
}
