/*
 * Focused C oracle for the scalar/selection expressions in the unchanged
 * lib/raop_handlers.h RAOP handlers. This intentionally avoids linking the
 * full renderer/network stack; handler-level integration remains a separate
 * gate documented in PORT_NOTES.md.
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define AUDIO_SAMPLE_RATE 44100
#define SECOND_IN_USECS 1000000

static unsigned int record_latency(int audio_delay_micros) {
    return (unsigned int)(((uint64_t)audio_delay_micros) * AUDIO_SAMPLE_RATE / SECOND_IN_USECS);
}

static int flush_sequence(const char *rtp_info) {
    int next_seq = -1;
    if (rtp_info && !strncmp(rtp_info, "seq=", 4)) {
        next_seq = (int)strtol(rtp_info + 4, NULL, 10);
    }
    return next_seq;
}

static const char *teardown_choice(int audio, int mirror) {
    if (audio) return "audio";
    if (mirror) return "mirror";
    return "all";
}

int main(void) {
    printf("record-latency|500000|%u\n", record_latency(500000));
    printf("record-latency|-1|%u\n", record_latency(-1));
    printf("flush|missing|%d\n", flush_sequence(NULL));
    printf("flush|seq=123tail|%d\n", flush_sequence("seq=123tail"));
    printf("flush|seq= +7tail|%d\n", flush_sequence("seq= +7tail"));
    printf("flush|seq=tail|%d\n", flush_sequence("seq=tail"));
    printf("flush|RTP-Info: seq=5|%d\n", flush_sequence("RTP-Info: seq=5"));
    printf("teardown|96,110|%s\n", teardown_choice(1, 1));
    printf("teardown|110|%s\n", teardown_choice(0, 1));
    printf("teardown|none|%s\n", teardown_choice(0, 0));
    return 0;
}
