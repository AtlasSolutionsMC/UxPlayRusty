/*
 * Differential driver for the unmodified UxPlay video_renderer.c.
 * It invokes the reference initializer and emits the launch grammar recorded
 * by the reference logger. Build outputs stay inside this crate directory.
 */
#include <gst/gst.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "video_renderer.h"
#include "video_renderer.c"

static void capture_pipeline(void *cls, int level, const char *message) {
    (void) cls;
    (void) level;
    if (strncmp(message, "GStreamer video pipeline", 24) != 0) {
        return;
    }
    const char *launch = strchr(message, '"');
    const char *end = launch ? strrchr(launch + 1, '"') : NULL;
    if (!launch || !end) {
        return;
    }
    int id = 0;
    (void) sscanf(message, "GStreamer video pipeline %d:", &id);
    printf("C_ORACLE_PIPELINE_%d\t%.*s\n", id, (int) (end - launch - 1), launch + 1);
}

int main(int argc, char **argv) {
    if (argc != 8 && argc != 9) {
        fprintf(stderr, "usage: video_oracle H265 COVERART SYNC FLIP ROTATION RTP OPTIONS [URI]\n");
        return 2;
    }
    gst_init(NULL, NULL);
    logger_t *logger = logger_init();
    logger_set_level(logger, LOGGER_DEBUG);
    logger_set_callback(logger, capture_pipeline, NULL);
    videoflip_t flip[2] = {
        (videoflip_t) atoi(argv[4]),
        (videoflip_t) atoi(argv[5]),
    };
    const char *rtp_pipeline = strcmp(argv[6], "-") == 0 ? "" : argv[6];
    const char *sink_options = strcmp(argv[7], "-") == 0 ? "" : argv[7];
    const char *uri = argc == 9 && strcmp(argv[8], "-") != 0 ? argv[8] : NULL;
    video_renderer_init(
        logger,
        "UxPlay Oracle",
        flip,
        "h264parse",
        rtp_pipeline,
        "avdec_h264",
        "videoconvert",
        "fakesink",
        sink_options,
        false,
        atoi(argv[3]) != 0,
        atoi(argv[1]) != 0,
        atoi(argv[2]) != 0,
        3,
        uri);
    if (uri) {
        gint flags = 0;
        g_object_get(renderer_type[0]->pipeline, "flags", &flags, NULL);
        printf("C_ORACLE_FLAGS=%d\n", flags);
    }
    video_renderer_destroy();
    logger_destroy(logger);
    return 0;
}
