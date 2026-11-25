/* Minimal link shim for the crypto.c recorder; source implementation is in UxPlay. */
#include <stdio.h>
#include <stdlib.h>
#include "utils.h"

char *utils_hex_to_string(const unsigned char *hex, int hex_len) {
    if (hex_len < 0) return NULL;
    char *result = (char *)malloc((size_t)hex_len * 2 + 1);
    if (result == NULL) return NULL;
    for (int i = 0; i < hex_len; ++i) {
        snprintf(result + i * 2, 3, "%02x", hex[i]);
    }
    return result;
}
