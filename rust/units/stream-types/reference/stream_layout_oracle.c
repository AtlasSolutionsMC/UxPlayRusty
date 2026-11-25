#include <stddef.h>
#include <stdio.h>

#include "stream.h"

int main(void) {
    printf("video:%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu\n",
           sizeof(video_decode_struct), _Alignof(video_decode_struct),
           offsetof(video_decode_struct, is_h265),
           offsetof(video_decode_struct, nal_count),
           offsetof(video_decode_struct, data),
           offsetof(video_decode_struct, data_len),
           offsetof(video_decode_struct, ntp_time_local),
           offsetof(video_decode_struct, ntp_time_remote));
    printf("audio:%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu\n",
           sizeof(audio_decode_struct), _Alignof(audio_decode_struct),
           offsetof(audio_decode_struct, data),
           offsetof(audio_decode_struct, ct),
           offsetof(audio_decode_struct, data_len),
           offsetof(audio_decode_struct, sync_status),
           offsetof(audio_decode_struct, ntp_time_local),
           offsetof(audio_decode_struct, ntp_time_remote),
           offsetof(audio_decode_struct, rtp_time),
           offsetof(audio_decode_struct, seqnum));
    return 0;
}
