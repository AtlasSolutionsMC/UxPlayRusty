#include <plist/plist.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

typedef struct time_range_s {
    double start;
    double duration;
} time_range_t;

static void emit(const char *label, plist_t root) {
    char *xml = NULL;
    uint32_t length = 0;
    plist_to_xml(root, &xml, &length);
    printf("--%s--\n", label);
    fwrite(xml, 1, length, stdout);
    plist_mem_free(xml);
    plist_free(root);
}

static plist_t new_range_array(const time_range_t *ranges, size_t count) {
    plist_t array = plist_new_array();
    for (size_t index = 0; index < count; index++) {
        plist_t range = plist_new_dict();
        plist_dict_set_item(range, "duration", plist_new_real(ranges[index].duration));
        plist_dict_set_item(range, "start", plist_new_real(ranges[index].start));
        plist_array_append_item(array, range);
    }
    return array;
}

int main(void) {
    const char *device_id = "02:00:00:a5:c4:b8";
    plist_t server = plist_new_dict();
    plist_dict_set_item(server, "features", plist_new_uint(0x27f));
    plist_dict_set_item(server, "macAddress", plist_new_string(device_id));
    plist_dict_set_item(server, "model", plist_new_string("AppleTV3,2"));
    plist_dict_set_item(server, "osBuildVersion", plist_new_string("12B435"));
    plist_dict_set_item(server, "protovers", plist_new_string("1.0"));
    plist_dict_set_item(server, "srcvers", plist_new_string("220.68"));
    plist_dict_set_item(server, "vv", plist_new_uint(2));
    plist_dict_set_item(server, "deviceid", plist_new_string(device_id));
    emit("server-info", server);

    const time_range_t loaded[] = {{12.5, 47.5}};
    const time_range_t seekable[] = {{0.0, 60.0}};
    plist_t playback = plist_new_dict();
    plist_dict_set_item(playback, "duration", plist_new_real(60.0));
    plist_dict_set_item(playback, "position", plist_new_real(12.5));
    plist_dict_set_item(playback, "rate", plist_new_real(1.0));
    plist_dict_set_item(playback, "readyToPlay", plist_new_uint(1));
    plist_dict_set_item(playback, "playbackBufferEmpty", plist_new_uint(0));
    plist_dict_set_item(playback, "playbackBufferFull", plist_new_uint(0));
    plist_dict_set_item(playback, "playbackLikelyToKeepUp", plist_new_uint(1));
    plist_dict_set_item(playback, "loadedTimeRanges", new_range_array(loaded, 1));
    plist_dict_set_item(playback, "seekableTimeRanges", new_range_array(seekable, 1));
    emit("playback-info", playback);
    return 0;
}
