/*
 * Stubs for dependencies outside the queue semantics exercised by the oracle.
 * AES decrypt is intentionally observable and must not be called: all packets
 * use the source's four-byte cleartext empty-audio marker.
 */
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>

#include "crypto.h"
#include "logger.h"

struct aes_ctx_s {
    int unused;
};

static int decrypt_calls;

int raop_buffer_test_decrypt_calls(void)
{
    return decrypt_calls;
}

aes_ctx_t *aes_cbc_init(const uint8_t *key, const uint8_t *iv, aes_direction_t direction)
{
    (void)key;
    (void)iv;
    (void)direction;
    return calloc(1, sizeof(aes_ctx_t));
}

void aes_cbc_reset(aes_ctx_t *ctx)
{
    (void)ctx;
}

void aes_cbc_decrypt(aes_ctx_t *ctx, const uint8_t *input, uint8_t *output, int length)
{
    (void)ctx;
    (void)input;
    (void)output;
    (void)length;
    decrypt_calls++;
}

void aes_cbc_destroy(aes_ctx_t *ctx)
{
    free(ctx);
}

void logger_log(logger_t *logger, int level, const char *format, ...)
{
    (void)logger;
    (void)level;
    (void)format;
}
