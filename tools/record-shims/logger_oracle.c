#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "logger.h"

typedef struct {
    const char *tag;
} capture_t;

static uint64_t fnv1a(const unsigned char *bytes, size_t length)
{
    uint64_t hash = UINT64_C(14695981039346656037);
    for (size_t index = 0; index < length; ++index) {
        hash ^= bytes[index];
        hash *= UINT64_C(1099511628211);
    }
    return hash;
}

static void capture(void *cls, int level, const char *message)
{
    const capture_t *capture_state = cls;
    size_t length = strlen(message);
    printf("%s %d %zu %016" PRIx64 "\n", capture_state->tag, level,
           length, fnv1a((const unsigned char *) message, length));
}

int main(void)
{
    logger_t *logger = logger_init();
    if (!logger) {
        return 2;
    }

    capture_t first = {"first"};
    capture_t replacement = {"replacement"};
    logger_set_callback(logger, capture, &first);

    logger_log(logger, LOGGER_INFO, "filtered");
    logger_log(logger, LOGGER_ERR, "visible:%d", 17);
    logger_log(logger, LOGGER_DEBUG_DATA, "filtered-debug-data");
    logger_set_level(logger, LOGGER_DEBUG_DATA);
    logger_log(logger, LOGGER_DEBUG_DATA, "packet");
    logger_log(logger, LOGGER_ERR, "%s", "before\0after");

    char nul_then_long[5007];
    memcpy(nul_then_long, "prefix", 6);
    nul_then_long[6] = '\0';
    memset(nul_then_long + 7, 'z', sizeof(nul_then_long) - 8);
    nul_then_long[sizeof(nul_then_long) - 1] = '\0';
    logger_log(logger, LOGGER_ERR, "%s", nul_then_long);

    char long_ascii[5001];
    memset(long_ascii, 'x', sizeof(long_ascii) - 1);
    long_ascii[sizeof(long_ascii) - 1] = '\0';
    logger_log(logger, LOGGER_ERR, "%s", long_ascii);

    char utf8[4103];
    size_t utf8_length = 0;
    for (size_t index = 0; index < 1364; ++index) {
        memcpy(utf8 + utf8_length, "\xe0\xb8\x81", 3);
        utf8_length += 3;
    }
    utf8[utf8_length++] = 'A';
    for (size_t index = 0; index < 3; ++index) {
        memcpy(utf8 + utf8_length, "\xe0\xb8\x81", 3);
        utf8_length += 3;
    }
    utf8[utf8_length] = '\0';
    logger_log(logger, LOGGER_ERR, "%s", utf8);

    logger_set_callback(logger, capture, &replacement);
    logger_log(logger, LOGGER_ERR, "callback-replaced");

    logger_destroy(logger);
    return 0;
}
