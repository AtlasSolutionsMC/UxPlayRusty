#include <assert.h>
#include <plist/plist.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include "http_response.h"

typedef struct {
    void *httpd;
    void *logger;
} raop_t;

typedef struct {
    raop_t *raop;
} raop_conn_t;

#define CONNECTION_TYPE_EVENT 1
#define LOGGER_DEBUG 7
#define LOGGER_ERR 3
#define SOCKET_GET_ERROR() 0
#define SOCKET_ERROR_STRING(error) "fixture error"
#define send fixture_send

static int httpd_get_connection_socket_by_type(void *httpd, int type, int newest) {
    (void)httpd;
    (void)type;
    (void)newest;
    return 1;
}

static void logger_log(void *logger, int level, const char *format, ...) {
    (void)logger;
    (void)level;
    (void)format;
}

static int logger_get_level(void *logger) {
    (void)logger;
    return 0;
}

static char *utils_data_to_text(const char *data, int length) {
    (void)data;
    (void)length;
    return NULL;
}

static int fixture_send(int socket_fd, const char *data, size_t length, int flags) {
    (void)socket_fd;
    (void)data;
    (void)length;
    (void)flags;
    return 0;
}

#include "fcup_request.h"

int main(int argc, char **argv) {
    if (argc != 3) {
        return 2;
    }

    int length = 0;
    char *xml = create_fcup_request(
        "http://localhost:7000/master.m3u8",
        7,
        "session-1",
        &length
    );
    if (xml == NULL || length < 0 || xml[length] != '\0') {
        free(xml);
        return 3;
    }

    FILE *xml_output = fopen(argv[1], "wb");
    if (xml_output == NULL) {
        free(xml);
        return 4;
    }

    int xml_written = (int)fwrite(xml, 1, (size_t)length, xml_output);
    int xml_close_result = fclose(xml_output);
    if (xml_written != length || xml_close_result != 0) {
        free(xml);
        return 5;
    }

    http_response_t *request = http_response_create();
    if (request == NULL) {
        free(xml);
        return 6;
    }
    http_response_reverse_request_init(request, "POST", "/event", "HTTP/1.1");
    http_response_add_header(request, "X-Apple-Session-ID", "session-1");
    http_response_add_header(request, "Content-Type", "text/x-apple-plist+xml");
    http_response_finish(request, xml, length);
    int request_length = 0;
    const char *request_data = http_response_get_data(request, &request_length);

    FILE *request_output = fopen(argv[2], "wb");
    if (request_output == NULL) {
        http_response_destroy(request);
        free(xml);
        return 7;
    }
    int request_written = (int)fwrite(request_data, 1, (size_t)request_length, request_output);
    int request_close_result = fclose(request_output);
    http_response_destroy(request);
    free(xml);
    return request_written == request_length && request_close_result == 0 ? 0 : 8;
}
