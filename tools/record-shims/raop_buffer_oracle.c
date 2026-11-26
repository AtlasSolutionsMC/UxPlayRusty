/* Behavioral trace for the unchanged `lib/raop_buffer.c` queue. */
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "raop_buffer.h"

extern int raop_buffer_test_decrypt_calls(void);

static const unsigned char marker[4] = {0x00, 0x68, 0x34, 0x00};
static unsigned char packet[16];

static void make_packet(unsigned short sequence, uint32_t timestamp)
{
    memset(packet, 0, sizeof(packet));
    packet[0] = 0x80;
    packet[1] = 0x60;
    packet[2] = (unsigned char)(sequence >> 8);
    packet[3] = (unsigned char)sequence;
    packet[4] = (unsigned char)(timestamp >> 24);
    packet[5] = (unsigned char)(timestamp >> 16);
    packet[6] = (unsigned char)(timestamp >> 8);
    packet[7] = (unsigned char)timestamp;
    memcpy(packet + 12, marker, sizeof(marker));
}

static void enqueue(raop_buffer_t *buffer, const char *label, unsigned short sequence,
                    uint32_t timestamp, int use_sequence)
{
    make_packet(sequence, timestamp);
    printf("ENQ %s rc=%d\n", label,
           raop_buffer_enqueue(buffer, packet, (unsigned short)sizeof(packet), use_sequence));
}

static void enqueue_length(raop_buffer_t *buffer, const char *label, unsigned short length)
{
    unsigned char invalid[16] = {0};
    printf("ENQ %s rc=%d\n", label,
           raop_buffer_enqueue(buffer, invalid, length, 1));
}

static void dequeue(raop_buffer_t *buffer, const char *label, int no_resend)
{
    unsigned int length = 0;
    uint32_t timestamp = 0;
    unsigned short sequence = 0;
    unsigned char *payload = raop_buffer_dequeue(buffer, &length, &timestamp,
                                                  &sequence, no_resend);
    if (!payload) {
        printf("DEQ %s empty\n", label);
        return;
    }

    printf("DEQ %s seq=%u ts=%" PRIu32 " len=%u data=", label,
           sequence, timestamp, length);
    for (unsigned int index = 0; index < length; ++index) {
        printf("%02x", payload[index]);
    }
    putchar('\n');
    free(payload);
}

static int resend(void *opaque, unsigned short sequence, unsigned short count)
{
    const char *label = opaque;
    printf("RESEND %s seq=%u count=%u\n", label, sequence, count);
    return 0;
}

static void range(raop_buffer_t *buffer)
{
    for (unsigned int sequence = 102; sequence <= 356; ++sequence) {
        make_packet((unsigned short)sequence, sequence);
        (void)raop_buffer_enqueue(buffer, packet, (unsigned short)sizeof(packet), 1);
    }
    puts("ENQ full-range count=255");
}

int main(void)
{
    const unsigned char key[16] = {0};
    const unsigned char iv[16] = {0};
    raop_buffer_t *buffer = raop_buffer_init(NULL, key, iv);
    if (!buffer) {
        fputs("raop_buffer_init failed\n", stderr);
        return 1;
    }

    dequeue(buffer, "initial", 0);
    enqueue_length(buffer, "short", 11);
    enqueue_length(buffer, "header-only", 12);

    enqueue(buffer, "gap-first", 10, 0xabcdef01, 1);
    enqueue(buffer, "duplicate", 10, 0x11111111, 1);
    enqueue(buffer, "gap-third", 12, 0xabcdef03, 1);
    dequeue(buffer, "gap-first", 0);
    raop_buffer_handle_resends(buffer, resend, "gap");
    dequeue(buffer, "gap-wait", 0);
    dequeue(buffer, "gap-skip", 1);
    dequeue(buffer, "gap-last", 0);
    enqueue(buffer, "late-after-drain", 11, 0xabcdef02, 1);

    raop_buffer_flush(buffer, -1);
    enqueue(buffer, "wrap-last", 65535, 0xffff0001, 1);
    enqueue(buffer, "wrap-zero", 0, 0x00000002, 1);
    dequeue(buffer, "wrap-last", 0);
    dequeue(buffer, "wrap-zero", 0);

    raop_buffer_flush(buffer, -1);
    enqueue(buffer, "window-old", 1, 1, 1);
    enqueue(buffer, "window-boundary", 257, 257, 1);
    dequeue(buffer, "window-boundary", 0);

    raop_buffer_flush(buffer, -1);
    enqueue(buffer, "sequence-fallback", 42, 42, 0);
    dequeue(buffer, "sequence-fallback", 0);

    raop_buffer_flush(buffer, -1);
    enqueue(buffer, "full-prime", 100, 100, 1);
    dequeue(buffer, "full-prime", 0);
    range(buffer);
    raop_buffer_handle_resends(buffer, resend, "full");
    dequeue(buffer, "full-window-head", 0);
    dequeue(buffer, "full-window-next", 0);

    printf("AES decrypt calls=%d\n", raop_buffer_test_decrypt_calls());
    raop_buffer_destroy(buffer);
    return 0;
}
