#include <stdio.h>
#include <string.h>

#include "http_request.h"

static int check_request(const char *name, const char *input,
                         const char *expected_error,
                         const char *expected_method,
                         const char *expected_protocol,
                         const char *expected_body)
{
    http_request_t *request = http_request_init();
    if (!request) {
        fprintf(stderr, "%s: http_request_init failed\n", name);
        return 1;
    }

    (void) http_request_add_data(request, input, strlen(input));
    const char *error = http_request_get_error_name(request);
    const char *method = http_request_get_method(request);
    const char *protocol = http_request_get_protocol(request);
    int body_length = 0;
    const char *body = http_request_get_data(request, &body_length);
    size_t expected_body_length = strlen(expected_body);
    int failed = 0;

    if (strcmp(error, expected_error) != 0) failed = 1;
    if (expected_method && (!method || strcmp(method, expected_method) != 0)) failed = 1;
    if (expected_protocol && (!protocol || strcmp(protocol, expected_protocol) != 0)) failed = 1;
    if ((size_t) body_length != expected_body_length ||
        (expected_body_length && (!body || memcmp(body, expected_body, expected_body_length)))) {
        failed = 1;
    }

    if (failed) {
        fprintf(stderr, "%s: error=%s method=%s protocol=%s body_length=%d\n",
                name, error ? error : "<null>", method ? method : "<null>",
                protocol ? protocol : "<null>", body_length);
    } else {
        printf("PASS %s\n", name);
    }
    http_request_destroy(request);
    return failed;
}

int main(void)
{
    static const char *const methods[] = {
        "GET", "POST", "OPTIONS", "DESCRIBE", "ANNOUNCE", "SETUP", "PLAY",
        "PAUSE", "TEARDOWN", "GET_PARAMETER", "SET_PARAMETER", "REDIRECT",
        "RECORD", "FLUSH"
    };
    int failures = 0;
    char input[128];

    for (size_t i = 0; i < sizeof(methods) / sizeof(methods[0]); i++) {
        snprintf(input, sizeof(input), "%s * RTSP/1.0\r\nCSeq: 7\r\n\r\n", methods[i]);
        failures += check_request(methods[i], input, "HPE_OK", methods[i], "RTSP/1.0", "");
    }

    failures += check_request(
        "RTSP body",
        "ANNOUNCE /announce RTSP/1.1\r\nCSeq: 8\r\nContent-Length: 4\r\n\r\nbody",
        "HPE_OK", "ANNOUNCE", "RTSP/1.1", "body");
    failures += check_request(
        "RTSP version 2.0",
        "OPTIONS * RTSP/2.0\r\nCSeq: 9\r\n\r\n",
        "HPE_OK", "OPTIONS", "RTSP/2.0", "");
    failures += check_request(
        "unsupported RTSP version 1.2",
        "OPTIONS * RTSP/1.2\r\n\r\n",
        "HPE_INVALID_VERSION", NULL, NULL, "");
    failures += check_request(
        "unsupported RTSP version 2.1",
        "OPTIONS * RTSP/2.1\r\n\r\n",
        "HPE_INVALID_VERSION", NULL, NULL, "");
    failures += check_request(
        "malformed RTSP version token",
        "OPTIONS * RTSP/1.x\r\n\r\n",
        "HPE_INVALID_VERSION", NULL, NULL, "");
    failures += check_request(
        "malformed RTSP multi-digit version",
        "OPTIONS * RTSP/10.0\r\n\r\n",
        "HPE_INVALID_VERSION", NULL, NULL, "");
    failures += check_request(
        "unsupported RTSP method",
        "PATCH * RTSP/1.0\r\n\r\n",
        "HPE_INVALID_CONSTANT", NULL, NULL, "");
    failures += check_request(
        "unknown RTSP method",
        "CUSTOM * RTSP/1.0\r\n\r\n",
        "HPE_INVALID_METHOD", NULL, NULL, "");

    return failures ? 1 : 0;
}
