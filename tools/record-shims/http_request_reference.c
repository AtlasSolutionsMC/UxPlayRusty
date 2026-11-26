#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "http_request.h"

static void print_bytes(const char *key, const char *value, size_t length)
{
    printf("%s=", key);
    if (!value) {
        puts("<null>");
        return;
    }
    for (size_t i = 0; i < length; i++) {
        printf("%02x", (unsigned char)value[i]);
    }
    putchar('\n');
}

static void print_c_string(const char *key, const char *value)
{
    print_bytes(key, value, value ? strlen(value) : 0);
}

int main(int argc, char **argv)
{
    size_t capacity = 4096;
    size_t length = 0;
    char *data = malloc(capacity);
    if (!data) return 2;

    FILE *input = argc > 1 ? fopen(argv[1], "rb") : stdin;
    if (!input) {
        free(data);
        return 2;
    }
    for (;;) {
        if (length == capacity) {
            capacity *= 2;
            char *grown = realloc(data, capacity);
            if (!grown) {
                free(data);
                return 2;
            }
            data = grown;
        }
        size_t count = fread(data + length, 1, capacity - length, input);
        length += count;
        if (count == 0) break;
    }
    if (input != stdin) fclose(input);

    http_request_t *request = http_request_init();
    if (!request) {
        free(data);
        return 2;
    }
    int result = http_request_add_data(request, data, length);
    printf("result=%d\ncomplete=%d\nerror=%d\n", result,
           http_request_is_complete(request), http_request_has_error(request));
    print_c_string("error_name", http_request_get_error_name(request));
    print_c_string("method", http_request_get_method(request));
    print_c_string("url", http_request_get_url(request));
    print_c_string("protocol", http_request_get_protocol(request));

    int body_length = 0;
    const char *body = http_request_get_data(request, &body_length);
    print_bytes("body", body, body ? (size_t)body_length : 0);

    char *headers = NULL;
    int headers_length = http_request_get_header_string(request, &headers);
    if (headers_length > 0 && headers) {
        print_bytes("headers", headers, (size_t)headers_length);
    } else {
        print_bytes("headers", headers, 0);
    }
    free(headers);
    http_request_destroy(request);
    free(data);
    return 0;
}
