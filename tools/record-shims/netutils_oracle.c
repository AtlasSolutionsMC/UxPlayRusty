#include <stdio.h>
#include <string.h>

#include "compat.h"
#include "netutils.h"

static void print_bytes(const unsigned char *bytes, int length)
{
    for (int i = 0; i < length; ++i) {
        printf("%02x", bytes[i]);
    }
}

static void print_address(const char *label, const struct sockaddr *address)
{
    int length = -1;
    unsigned int zone_id = 99;
    unsigned short port = 0;
    unsigned char *bytes = netutils_get_address((void *)address, &length, &zone_id, &port);

    if (!bytes) {
        printf("%s null %d\n", label, length);
        return;
    }
    printf("%s %d ", label, length);
    print_bytes(bytes, length);
    printf(" %u %u\n", zone_id, port);
}

static void print_parsed(const char *label, int family, const char *source)
{
    struct sockaddr_storage address;
    memset(&address, 0, sizeof(address));
    int length = netutils_parse_address(family, source, &address, sizeof(address));
    if (length < 0) {
        printf("%s -1\n", label);
        return;
    }
    print_address(label, (const struct sockaddr *)&address);
}

int main(void)
{
    if (netutils_init() != 0) {
        fprintf(stderr, "netutils_init failed\n");
        return 1;
    }

    struct sockaddr_in ipv4;
    memset(&ipv4, 0, sizeof(ipv4));
    ipv4.sin_family = AF_INET;
    ipv4.sin_port = htons(7000);
    ipv4.sin_addr.s_addr = htonl(0xc0000209U);
    print_address("get-v4", (const struct sockaddr *)&ipv4);

    struct sockaddr_in6 mapped;
    memset(&mapped, 0, sizeof(mapped));
    mapped.sin6_family = AF_INET6;
    mapped.sin6_port = htons(5000);
    memset(mapped.sin6_addr.s6_addr + 10, 0xff, 2);
    mapped.sin6_addr.s6_addr[12] = 198;
    mapped.sin6_addr.s6_addr[13] = 51;
    mapped.sin6_addr.s6_addr[14] = 100;
    mapped.sin6_addr.s6_addr[15] = 4;
    mapped.sin6_scope_id = 21;
    print_address("get-v4-mapped", (const struct sockaddr *)&mapped);

    struct sockaddr_in6 scoped;
    memset(&scoped, 0, sizeof(scoped));
    scoped.sin6_family = AF_INET6;
    scoped.sin6_port = htons(9);
    scoped.sin6_addr.s6_addr[0] = 0xfe;
    scoped.sin6_addr.s6_addr[1] = 0x80;
    scoped.sin6_addr.s6_addr[15] = 1;
    scoped.sin6_scope_id = 7;
    print_address("get-v6-scope", (const struct sockaddr *)&scoped);

    print_parsed("parse-v4", AF_INET, "192.0.2.1");
    print_parsed("parse-v6-scope", AF_INET6, "fe80::1%5");
    print_parsed("parse-host", AF_INET, "localhost");
    print_parsed("parse-v6-v4-literal", AF_INET6, "192.0.2.1");

    netutils_cleanup();
    return 0;
}
