#include <stdarg.h>
#include <stdio.h>
#include <stdint.h>
#include <glib.h>
#include "audio_renderer.h"

static int logger_sentinel;
void logger_log(logger_t *logger, int level, const char *fmt, ...) {
    (void)logger;
    char buffer[4096];
    va_list args;
    va_start(args, fmt);
    vsnprintf(buffer, sizeof(buffer), fmt, args);
    va_end(args);
    printf("L%d:%s\n", level, buffer);
}

int main(void) {
    bool audio_sync = false;
    bool video_sync = false;
    logger_t *logger = (logger_t *)&logger_sentinel;
    printf("gstreamer_init=%d\n", gstreamer_init());
    audio_renderer_init(logger, "fakesink", &audio_sync, &video_sync, "");
    GMainLoop *loop = g_main_loop_new(NULL, FALSE);
    unsigned int watch0 = audio_renderer_listen(loop, 0);
    unsigned int watch1 = audio_renderer_listen(loop, 1);
    printf("watch_ids=%u,%u\n", watch0, watch1);
    unsigned char ct = 8;
    audio_renderer_start(&ct);
    unsigned char invalid[] = { 0xff, 0 };
    unsigned char valid[] = { 0x8c, 0 };
    int invalid_len = 2, valid_len = 2;
    unsigned short seqnum = 42;
    uint64_t ntp_time = 0;
    audio_renderer_render_buffer(invalid, &invalid_len, &seqnum, &ntp_time);
    audio_renderer_render_buffer(valid, &valid_len, &seqnum, &ntp_time);
    ct = 2;
    audio_renderer_start(&ct);
    unsigned char alac[] = { 0x20, 0 };
    audio_renderer_render_buffer(alac, &valid_len, &seqnum, &ntp_time);
    audio_renderer_set_volume(12.0);
    audio_renderer_flush();
    audio_renderer_stop();
    g_source_remove(watch0);
    g_source_remove(watch1);
    g_main_loop_unref(loop);
    audio_renderer_destroy();
    return 0;
}
