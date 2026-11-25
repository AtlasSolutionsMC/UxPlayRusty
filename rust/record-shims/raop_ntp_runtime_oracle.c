#include <inttypes.h>
#include <stdio.h>
#include <string.h>

#include "byteutils.h"

static void print_hex(const unsigned char *bytes, size_t length)
{
    for (size_t index = 0; index < length; index++) {
        printf("%02x", bytes[index]);
    }
}

int main(void)
{
    unsigned char request[32] = {
        0x80, 0xd2, 0x00, 0x07
    };
    byteutils_put_ntp_timestamp(request, 24, UINT64_C(1700000000123456789));
    printf("request|");
    print_hex(request, sizeof(request));
    printf("\n");

    byteutils_put_long_be(request, 8, UINT64_C(0xfedcba9876543210));
    byteutils_put_ntp_timestamp(request, 16, UINT64_C(1700000000987654321));
    byteutils_put_ntp_timestamp(request, 24, UINT64_C(1700000001123456789));
    printf("next-request|");
    print_hex(request, sizeof(request));
    printf("\n");

    unsigned char response[32] = {0};
    byteutils_put_long_be(response, 8, UINT64_C(0xe123456789abcdef));
    byteutils_put_long_be(response, 16, UINT64_C(0x0123456789abcdef));
    byteutils_put_long_be(response, 24, UINT64_C(0xfedcba9876543210));
    printf("response|%016" PRIx64 "|%016" PRIx64 "|%016" PRIx64 "\n",
           byteutils_get_long_be(response, 8),
           byteutils_get_long_be(response, 16),
           byteutils_get_long_be(response, 24));
    return 0;
}
