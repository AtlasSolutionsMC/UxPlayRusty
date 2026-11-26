#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "mdnsd.c"

static void print_hex(const unsigned char *bytes, size_t length)
{
    for (size_t i = 0; i < length; ++i) {
        printf("%02x", bytes[i]);
    }
}

static void print_packet(const char *label, const mdns_packet_t *packet, int result)
{
    printf("%s|%d|", label, result);
    if (!result) {
        print_hex(packet->bytes, packet->length);
    }
    putchar('\n');
}

static void print_packet_length(const char *label, const mdns_packet_t *packet,
                                int result)
{
    printf("%s|%d|%zu\n", label, result, packet->length);
}

static void fill_service(mdnsd_service_t *service, const char *type,
                         const char *name, unsigned short port,
                         const char *key1, const char *value1,
                         const char *key2, const char *value2)
{
    memset(service, 0, sizeof(*service));
    snprintf(service->type, sizeof(service->type), "%s", type);
    snprintf(service->name, sizeof(service->name), "%s", name);
    service->port = port;
    service->registered = 1;
    mdnsd_txt_add(&service->txt, key1, value1);
    mdnsd_txt_add(&service->txt, key2, value2);
}

static void print_match(const mdnsd_t *mdnsd, const char *label,
                        const char *name, uint16_t type)
{
    int airplay = 0;
    int raop = 0;
    int host = 0;
    int matched = mdns_question_matches_service(mdnsd, name, type,
                                                &airplay, &raop, &host);
    printf("%s|%d|%d|%d|%d\n", label, matched, airplay, raop, host);
}

int main(void)
{
    mdnsd_t mdnsd = {0};
    mdns_packet_t packet = {0};
    static const unsigned char ipv4_bytes[] = {192, 168, 1, 77};
    static const unsigned char ipv6_bytes[] = {
        0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0x77
    };
    snprintf(mdnsd.host_name, sizeof(mdnsd.host_name), "UxPlay.local");
    memcpy(&mdnsd.ipv4_addr, ipv4_bytes, sizeof(ipv4_bytes));
    memcpy(mdnsd.ipv6_addr, ipv6_bytes, sizeof(ipv6_bytes));
    fill_service(&mdnsd.airplay, "_airplay._tcp.local",
                 "Living Room._airplay._tcp.local", 7000,
                 "deviceid", "010203040506", "features", "0x5A7FFFF7,0x1E");
    fill_service(&mdnsd.raop, "_raop._tcp.local",
                 "AABBCCDDEEFF@Living Room._raop._tcp.local", 5000,
                 "txtvers", "1", "ch", "2");

    print_packet("announcement4", &packet,
                 mdns_build_combined_response(&mdnsd, &packet, 1, 1, 1,
                                              MDNSD_TTL_SERVICE, MDNS_FAMILY_IPV4));
    memset(&packet, 0, sizeof(packet));
    print_packet("announcement6", &packet,
                 mdns_build_combined_response(&mdnsd, &packet, 1, 1, 1,
                                              MDNSD_TTL_SERVICE, MDNS_FAMILY_IPV6));
    memset(&packet, 0, sizeof(packet));
    print_packet("services-only4", &packet,
                 mdns_build_combined_response(&mdnsd, &packet, 1, 1, 0,
                                              MDNSD_TTL_SERVICE, MDNS_FAMILY_IPV4));
    memset(&packet, 0, sizeof(packet));
    print_packet("airplay-response4", &packet,
                 mdns_build_response(&mdnsd, &packet, &mdnsd.airplay, 1,
                                     MDNSD_TTL_SERVICE, MDNS_FAMILY_IPV4));
    memset(&packet, 0, sizeof(packet));
    print_packet("airplay-goodbye4", &packet,
                 mdns_build_response(&mdnsd, &packet, &mdnsd.airplay, 0,
                                     0, MDNS_FAMILY_IPV4));
    memset(&packet, 0, sizeof(packet));
    print_packet("airplay-goodbye6", &packet,
                 mdns_build_response(&mdnsd, &packet, &mdnsd.airplay, 0,
                                     0, MDNS_FAMILY_IPV6));
    memset(&packet, 0, sizeof(packet));
    print_packet("host-only4", &packet,
                 mdns_build_response(&mdnsd, &packet, NULL, 1,
                                     MDNSD_TTL_SERVICE, MDNS_FAMILY_IPV4));

    {
        mdnsd_t large = mdnsd;
        mdns_packet_t large_packet = {0};
        char value[241];
        memset(value, 'a', sizeof(value) - 1);
        value[sizeof(value) - 1] = '\0';
        memset(&large.airplay.txt, 0, sizeof(large.airplay.txt));
        memset(&large.raop.txt, 0, sizeof(large.raop.txt));
        mdnsd_txt_add(&large.airplay.txt, "x", value);
        mdnsd_txt_add(&large.airplay.txt, "y", value);
        mdnsd_txt_add(&large.raop.txt, "x", value);
        mdnsd_txt_add(&large.raop.txt, "y", value);
        print_packet_length("large-combined", &large_packet,
                            mdns_build_combined_response(&large, &large_packet,
                                                         1, 1, 1,
                                                         MDNSD_TTL_SERVICE,
                                                         MDNS_FAMILY_IPV4));
        memset(&large_packet, 0, sizeof(large_packet));
        print_packet_length("large-airplay", &large_packet,
                            mdns_build_response(&large, &large_packet,
                                               &large.airplay, 1,
                                               MDNSD_TTL_SERVICE,
                                               MDNS_FAMILY_IPV4));
        memset(&large_packet, 0, sizeof(large_packet));
        print_packet_length("large-raop", &large_packet,
                            mdns_build_response(&large, &large_packet,
                                               &large.raop, 1,
                                               MDNSD_TTL_SERVICE,
                                               MDNS_FAMILY_IPV4));
    }

    print_match(&mdnsd, "match-enumeration", "_services._dns-sd._udp.local",
                DNS_TYPE_PTR);
    print_match(&mdnsd, "match-airplay-type", "_airplay._tcp.local", DNS_TYPE_PTR);
    print_match(&mdnsd, "match-airplay-txt", "Living Room._airplay._tcp.local",
                DNS_TYPE_TXT);
    print_match(&mdnsd, "match-raop-srv", "AABBCCDDEEFF@Living Room._raop._tcp.local",
                DNS_TYPE_SRV);
    print_match(&mdnsd, "match-host-a", "UxPlay.local", DNS_TYPE_A);
    print_match(&mdnsd, "match-host-any", "uxplay.LOCAL", DNS_TYPE_ANY);
    print_match(&mdnsd, "match-unknown", "other.local", DNS_TYPE_PTR);
    mdnsd.airplay.registered = 0;
    print_match(&mdnsd, "match-unregistered", "_airplay._tcp.local", DNS_TYPE_PTR);
    return 0;
}
