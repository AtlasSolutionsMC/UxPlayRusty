/*
 * Narrow harness preinclude for the unchanged raop_buffer.c.
 * The queue uses only RAOP_PACKET_LEN from raop_rtp.h and has no compatibility
 * or stream-layer calls. Skip those unrelated header surfaces in this host test.
 */
#ifndef RAOP_RTP_H
#define RAOP_RTP_H
#define RAOP_PACKET_LEN 32768
#endif

#ifndef COMPAT_H
#define COMPAT_H
#endif

#ifndef AIRPLAYSERVER_STREAM_H
#define AIRPLAYSERVER_STREAM_H
#endif
