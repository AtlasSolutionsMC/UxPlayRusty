#include <stdio.h>
#include <stdlib.h>
#include <stdatomic.h>
#include <string.h>

#include <gst/gst.h>

#include "logger.h"
#include "mux_renderer.h"
#include "mux_renderer.c"

typedef struct trace_row_s {
    char stream;
    guint64 pts;
    guint64 dts;
    guint64 duration;
} trace_row_t;

static trace_row_t trace_rows[16];
static atomic_uint trace_count;

static void emit_log(void *context, int level, const char *message) {
    (void)context;
    printf("%d|%s\n", level, message);
}

static GstPadProbeReturn trace_buffer(GstPad *pad, GstPadProbeInfo *info, gpointer context) {
    (void)pad;
    GstBuffer *buffer = GST_PAD_PROBE_INFO_BUFFER(info);
    if (!buffer) {
        return GST_PAD_PROBE_OK;
    }
    unsigned int index = atomic_fetch_add(&trace_count, 1);
    if (index < G_N_ELEMENTS(trace_rows)) {
        trace_row_t *row = &trace_rows[index];
        row->stream = *(const char *)context;
        row->pts = GST_BUFFER_PTS_IS_VALID(buffer) ? GST_BUFFER_PTS(buffer) : G_MAXUINT64;
        row->dts = GST_BUFFER_DTS_IS_VALID(buffer) ? GST_BUFFER_DTS(buffer) : G_MAXUINT64;
        row->duration = GST_BUFFER_DURATION_IS_VALID(buffer) ? GST_BUFFER_DURATION(buffer) : G_MAXUINT64;
    }
    return GST_PAD_PROBE_OK;
}

static int compare_trace_rows(const void *left, const void *right) {
    const trace_row_t *a = left;
    const trace_row_t *b = right;
    if (a->stream != b->stream) {
        return (unsigned char)a->stream - (unsigned char)b->stream;
    }
    if (a->pts < b->pts) return -1;
    if (a->pts > b->pts) return 1;
    if (a->duration < b->duration) return -1;
    if (a->duration > b->duration) return 1;
    return 0;
}

static GstPad *observe_appsrc(GstElement *appsrc, const char *stream) {
    if (!appsrc) return NULL;
    GstPad *pad = gst_element_get_static_pad(appsrc, "src");
    if (pad) {
        gst_pad_add_probe(pad, GST_PAD_PROBE_TYPE_BUFFER, trace_buffer,
                          (gpointer)stream, NULL);
    }
    return pad;
}

int main(int argc, char **argv) {
    if (argc != 3) {
        return 2;
    }
    const char *scenario = argv[1];
    const char *filename = argv[2];
    gst_init(NULL, NULL);

    logger_t *logger = logger_init();
    logger_set_level(logger, LOGGER_DEBUG);
    logger_set_callback(logger, emit_log, NULL);

    if (strcmp(scenario, "h264-aac") == 0) {
        mux_renderer_init(logger, filename, true, true);
        mux_renderer_choose_video_codec(false);
    } else if (strcmp(scenario, "h265-video") == 0) {
        mux_renderer_init(logger, filename, false, true);
        mux_renderer_choose_video_codec(true);
    } else if (strcmp(scenario, "alac-video-config") == 0) {
        mux_renderer_init(logger, filename, true, true);
        mux_renderer_choose_audio_codec(2);
    } else if (strcmp(scenario, "codec-switch") == 0) {
        mux_renderer_init(logger, filename, true, true);
        mux_renderer_choose_video_codec(true);
        mux_renderer_choose_audio_codec(2);
    } else if (strcmp(scenario, "both-disabled") == 0) {
        mux_renderer_init(logger, filename, false, false);
        mux_renderer_choose_video_codec(false);
    } else if (strcmp(scenario, "timestamps") == 0) {
        mux_renderer_init(logger, filename, true, true);
        mux_renderer_choose_video_codec(false);
        GstPad *video_pad = observe_appsrc(GST_ELEMENT(renderer->video_appsrc), "V");
        GstPad *audio_pad = observe_appsrc(GST_ELEMENT(renderer->audio_appsrc), "A");
        const unsigned char video[] = {0, 0, 0, 1, 0x65, 0x88, 0x84};
        const unsigned char audio[] = {0x21, 0x10, 0x56, 0xe5};
        mux_renderer_push_video((unsigned char *)video, sizeof(video), 1000000000ULL);
        mux_renderer_push_audio((unsigned char *)audio, sizeof(audio), 1500000000ULL);
        mux_renderer_push_video((unsigned char *)video, sizeof(video), 2250000000ULL);
        if (video_pad) gst_object_unref(video_pad);
        if (audio_pad) gst_object_unref(audio_pad);
    } else {
        logger_destroy(logger);
        return 2;
    }

    mux_renderer_stop();
    mux_renderer_destroy();
    unsigned int count = atomic_load(&trace_count);
    if (count > G_N_ELEMENTS(trace_rows)) count = G_N_ELEMENTS(trace_rows);
    qsort(trace_rows, count, sizeof(trace_rows[0]), compare_trace_rows);
    for (unsigned int index = 0; index < count; index++) {
        trace_row_t *row = &trace_rows[index];
        g_print("TRACE|%c|", row->stream);
        if (row->pts == G_MAXUINT64) g_print("NONE"); else g_print("%" G_GUINT64_FORMAT, row->pts);
        g_print("|");
        if (row->dts == G_MAXUINT64) g_print("NONE"); else g_print("%" G_GUINT64_FORMAT, row->dts);
        g_print("|");
        if (row->duration == G_MAXUINT64) g_print("NONE"); else g_print("%" G_GUINT64_FORMAT, row->duration);
        g_print("\n");
    }
    logger_destroy(logger);
    return 0;
}
