/*
 * Test-only driver for the unchanged UxPlay RAOP mirror receiver.
 * Copyright and license terms are those of the reference project.
 */
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "raop.h"
#include "raop_rtp_mirror.h"
#include "raop_ntp.h"
#include "crypto.h"
#include "logger.h"
#include "stream.h"
#include "compat.h"

#ifdef _WIN32
#include <windows.h>
#define SLEEP_MS(value) Sleep(value)
#define SOCKET_HANDLE SOCKET
#define INVALID_HANDLE INVALID_SOCKET
#define CLOSE_HANDLE closesocket
#else
#include <unistd.h>
#define SLEEP_MS(value) usleep((value) * 1000)
#define SOCKET_HANDLE int
#define INVALID_HANDLE (-1)
#define CLOSE_HANDLE close
#endif

static const unsigned char audio_key[] = "audio-key-fixtur";
static uint64_t stream_id = UINT64_C(0x0123456789abcdef);
#define AUDIO_KEY_LEN 16

static void log_callback(void *opaque, int level, const char *message)
{
    (void)opaque;
    (void)level;
    (void)message;
}

static int codec_callback(void *opaque, video_codec_t codec, unsigned short width_source,
                          unsigned short height_source, unsigned short x_offset,
                          unsigned short y_offset, unsigned short width, unsigned short height)
{
    (void)opaque;
    printf("codec,%d,%u,%u,%u,%u,%u,%u\n", codec, width_source, height_source,
           x_offset, y_offset, width, height);
    fflush(stdout);
    return 0;
}

static void frame_callback(void *opaque, raop_ntp_t *ntp, video_decode_struct *frame)
{
    (void)opaque;
    (void)ntp;
    printf("frame,%d,%d,%d,", frame->is_h265 ? 1 : 0, frame->nal_count, frame->data_len);
    for (int i = 0; i < frame->data_len; i++) printf("%02x", frame->data[i]);
    printf("\n");
    fflush(stdout);
}

static int send_all(SOCKET_HANDLE socket, const unsigned char *data, size_t length)
{
    size_t sent = 0;
    while (sent < length) {
#ifdef _WIN32
        int count = send(socket, (const char *)data + sent, (int)(length - sent), 0);
#else
        ssize_t count = send(socket, data + sent, length - sent, 0);
#endif
        if (count <= 0) return -1;
        sent += (size_t)count;
    }
    return 0;
}

static int send_packet(SOCKET_HANDLE socket, unsigned char kind, unsigned char option,
                       const unsigned char *payload, size_t payload_len)
{
    unsigned char header[128] = {0};
    uint32_t len = (uint32_t)payload_len;
    uint64_t timestamp = 42;
    memcpy(header, &len, sizeof(len));
    header[4] = kind;
    header[6] = option;
    memcpy(header + 8, &timestamp, sizeof(timestamp));
    if (kind == 1) {
        float width = 1920.0f, height = 1080.0f, offset = 0.0f;
        memcpy(header + 40, &width, sizeof(width));
        memcpy(header + 44, &height, sizeof(height));
        memcpy(header + 48, &offset, sizeof(offset));
        memcpy(header + 52, &offset, sizeof(offset));
        memcpy(header + 56, &width, sizeof(width));
        memcpy(header + 60, &height, sizeof(height));
    }
    if (send_all(socket, header, sizeof(header)) < 0) return -1;
    return send_all(socket, payload, payload_len);
}

static int encrypt_video_payload(const unsigned char *plain, int length, unsigned char *cipher)
{
    char key_label[64], iv_label[64];
    unsigned char key_hash[64] = {0}, iv_hash[64] = {0};
    unsigned int hash_len = 0;
    snprintf(key_label, sizeof(key_label), "AirPlayStreamKey%" PRIu64, stream_id);
    snprintf(iv_label, sizeof(iv_label), "AirPlayStreamIV%" PRIu64, stream_id);
    sha_ctx_t *hash = sha_init();
    if (!hash) return -1;
    sha_update(hash, (const unsigned char *)key_label, (int)strlen(key_label));
    sha_update(hash, audio_key, AUDIO_KEY_LEN);
    sha_final(hash, key_hash, &hash_len);
    sha_reset(hash);
    sha_update(hash, (const unsigned char *)iv_label, (int)strlen(iv_label));
    sha_update(hash, audio_key, AUDIO_KEY_LEN);
    sha_final(hash, iv_hash, &hash_len);
    sha_destroy(hash);
    aes_ctx_t *aes = aes_ctr_init(key_hash, iv_hash);
    if (!aes) return -1;
    aes_ctr_encrypt(aes, plain, cipher, length);
    aes_ctr_destroy(aes);
    return 0;
}

int main(void)
{
#ifdef _WIN32
    WSADATA winsock;
    if (WSAStartup(MAKEWORD(2, 2), &winsock) != 0) return 2;
#endif
    logger_t *logger = logger_init();
    logger_set_callback(logger, log_callback, NULL);
    logger_set_level(logger, LOGGER_WARNING);
    raop_callbacks_t callbacks = {0};
    callbacks.video_set_codec_and_size = codec_callback;
    callbacks.video_process = frame_callback;
    timing_protocol_t protocol = TP_NONE;
    raop_ntp_t *ntp = raop_ntp_init(logger, &callbacks, "127.0.0.1", 4, 0, &protocol);
    raop_rtp_mirror_t *mirror = raop_rtp_mirror_init(
        logger, &callbacks, ntp, "127.0.0.1", 4, audio_key);
    if (!ntp || !mirror) return 3;
    raop_rtp_mirror_init_aes(mirror, &stream_id);
    unsigned short port = 0;
    raop_rtp_mirror_start(mirror, &port, 0);
    if (port == 0) return 4;

    SOCKET_HANDLE client = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (client == INVALID_HANDLE) return 5;
    struct sockaddr_in address = {0};
    address.sin_family = AF_INET;
    address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    address.sin_port = htons(port);
    if (connect(client, (struct sockaddr *)&address, sizeof(address)) != 0) return 6;

    const unsigned char sps[] = {0x67, 0x42, 0x00, 0x1e};
    const unsigned char pps[] = {0x68, 0xce, 0x06, 0xe2};
    unsigned char codec_payload[19] = {0};
    codec_payload[6] = 0;
    codec_payload[7] = sizeof(sps);
    memcpy(codec_payload + 8, sps, sizeof(sps));
    codec_payload[12] = 1;
    codec_payload[13] = 0;
    codec_payload[14] = sizeof(pps);
    memcpy(codec_payload + 15, pps, sizeof(pps));
    if (send_packet(client, 1, 0x16, codec_payload, sizeof(codec_payload)) < 0) return 7;

    const unsigned char frame_plain[] = {0, 0, 0, 2, 0x65, 0xaa};
    unsigned char frame_cipher[sizeof(frame_plain)] = {0};
    if (encrypt_video_payload(frame_plain, sizeof(frame_plain), frame_cipher) < 0) return 8;
    if (send_packet(client, 0, 0, frame_cipher, sizeof(frame_cipher)) < 0) return 9;
    SLEEP_MS(250);
    CLOSE_HANDLE(client);
    raop_rtp_mirror_stop(mirror);
    raop_rtp_mirror_destroy(mirror);
    raop_ntp_destroy(ntp);
    logger_destroy(logger);
#ifdef _WIN32
    WSACleanup();
#endif
    return 0;
}
