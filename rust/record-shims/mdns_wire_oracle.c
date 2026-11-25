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

static void print_packet(const char *label, const mdns_packet_t *packet)
{
    printf("%s|", label);
    print_hex(packet->bytes, packet->length);
    putchar('\n');
}

static void print_read(const char *label, const unsigned char *packet,
                       size_t packet_length, size_t start, size_t name_capacity)
{
    char name[256] = {0};
    size_t offset = start;
    int result;
    if (name_capacity > sizeof(name)) {
        name_capacity = sizeof(name);
    }
    result = mdns_read_name(packet, packet_length, &offset, name, name_capacity);
    printf("%s|%d|%zu|", label, result, offset);
    if (!result) {
        print_hex((const unsigned char *) name, strlen(name));
    }
    putchar('\n');
}

int main(void)
{
    mdns_packet_t packet = {0};
    mdns_put_u8(&packet, 0x123);
    mdns_put_u16(&packet, 0xabcd);
    mdns_put_u32(&packet, 0x12345678U);
    {
        static const unsigned char raw[] = {0, 0x7f, 0xff};
        mdns_put_bytes(&packet, raw, sizeof(raw));
    }
    print_packet("primitive", &packet);

    memset(&packet, 0, sizeof(packet));
    mdns_put_name(&packet, "AirPlay._tcp.local.");
    print_packet("name", &packet);

    memset(&packet, 0, sizeof(packet));
    mdns_put_name(&packet, "foo..bar");
    print_packet("empty-label", &packet);

    {
        char overlong_label[65];
        memset(overlong_label, 'a', sizeof(overlong_label));
        overlong_label[64] = '\0';
        memset(&packet, 0, sizeof(packet));
        int result = mdns_put_name(&packet, overlong_label);
        printf("long-label|%d|%zu\n", result, packet.length);
    }

    {
        unsigned char full[MDNS_MAX_PACKET] = {0};
        memset(&packet, 0, sizeof(packet));
        int fill_result = mdns_put_bytes(&packet, full, sizeof(full));
        int overflow_result = mdns_put_u8(&packet, 1);
        printf("packet-limit|%d|%d|%zu\n", fill_result, overflow_result, packet.length);
    }

    memset(&packet, 0, sizeof(packet));
    mdns_add_ptr(&packet, "_airplay._tcp.local", "UxPlay._airplay._tcp.local", 4500);
    print_packet("ptr", &packet);

    memset(&packet, 0, sizeof(packet));
    mdns_add_srv(&packet, "UxPlay._airplay._tcp.local", 7000, "uxplay.local", 4500);
    print_packet("srv", &packet);

    memset(&packet, 0, sizeof(packet));
    {
        static const unsigned char txt[] = {4, 'a', '=', '1', 4, 'b', '=', '2'};
        mdnsd_txt_t record = {{0}, (int) sizeof(txt)};
        memcpy(record.bytes, txt, sizeof(txt));
        mdns_add_txt(&packet, "UxPlay._airplay._tcp.local", &record, 120);
    }
    print_packet("txt", &packet);

    memset(&packet, 0, sizeof(packet));
    {
        static const unsigned char addr_bytes[4] = {1, 2, 3, 4};
        uint32_t addr;
        memcpy(&addr, addr_bytes, sizeof(addr));
        mdns_add_a(&packet, "uxplay.local", addr, 120);
    }
    print_packet("a", &packet);

    memset(&packet, 0, sizeof(packet));
    {
        static const unsigned char addr[16] = {
            0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 1
        };
        mdns_add_aaaa(&packet, "uxplay.local", addr, 120);
    }
    print_packet("aaaa", &packet);

    memset(&packet, 0, sizeof(packet));
    {
        static const unsigned char addr4[4] = {0};
        static const unsigned char addr6[16] = {0};
        uint32_t addr;
        memcpy(&addr, addr4, sizeof(addr));
        mdns_add_a(&packet, "uxplay.local", addr, 120);
        mdns_add_aaaa(&packet, "uxplay.local", addr6, 120);
    }
    print_packet("zero-addresses", &packet);

    {
        static const unsigned char uncompressed[] = {
            0,0,0,0,0,0,0,0,0,0,0,0,
            3,'w','w','w',7,'e','x','a','m','p','l','e',3,'c','o','m',0
        };
        print_read("read-name", uncompressed, sizeof(uncompressed), 12, 256);
        print_read("read-short-output", uncompressed, sizeof(uncompressed), 12, 4);
        printf("read-u16|%u|%u\n",
               mdns_read_u16((const unsigned char[]){0x12,0x34}),
               mdns_read_u16((const unsigned char[]){0xff,0xff}));
    }

    {
        static const unsigned char compressed[] = {
            0,0,0,0,0,0,0,0,0,0,0,0,
            3,'w','w','w',7,'e','x','a','m','p','l','e',3,'c','o','m',0,
            5,'_','a','i','r','p',0xc0,16
        };
        print_read("read-compressed", compressed, sizeof(compressed), 29, 256);
    }

    {
        static const unsigned char root[] = {0};
        static const unsigned char truncated[] = {3, 'a'};
        static const unsigned char bad_pointer[] = {0xc0, 0xff};
        static const unsigned char pointer_loop[] = {0xc0, 0};
        print_read("read-root", root, sizeof(root), 0, 256);
        print_read("read-truncated", truncated, sizeof(truncated), 0, 256);
        print_read("read-bad-pointer", bad_pointer, sizeof(bad_pointer), 0, 256);
        print_read("read-pointer-loop", pointer_loop, sizeof(pointer_loop), 0, 256);
    }

    printf("name-equals|%d|%d|%d\n",
           mdns_name_equals("_AirPlay._tcp.local", "_airplay._TCP.LOCAL"),
           mdns_name_equals("left.local", "right.local"),
           mdns_name_equals("prefix\0suffix", "PREFIX"));
    printf("query-type|%d|%d|%d\n",
           mdns_query_type_matches(1, 1),
           mdns_query_type_matches(255, 28),
           mdns_query_type_matches(12, 16));
    return 0;
}
