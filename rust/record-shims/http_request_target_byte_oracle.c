#include <stddef.h>
#include <stdio.h>
#include <string.h>

#include "http_request.h"

static void print_case(const unsigned char *input, size_t length)
{
    for (size_t index = 0; index < length; ++index) {
        printf("%02x", input[index]);
    }
    printf("\t-\t");
    http_request_t *request = http_request_init();
    if (!request) {
        puts("\t<init-failed>");
        return;
    }
    (void)http_request_add_data(request, (const char *)input, length);
    const char *error = http_request_get_error_name(request);
    printf("%s\n", error ? error : "<null>");
    http_request_destroy(request);
}

static void print_split_case(const unsigned char *input, size_t length, size_t split)
{
    for (size_t index = 0; index < length; ++index) {
        printf("%02x", input[index]);
    }
    printf("\t%zu\t", split);
    http_request_t *request = http_request_init();
    if (!request) {
        puts("<init-failed>");
        return;
    }
    (void)http_request_add_data(request, (const char *)input, split);
    (void)http_request_add_data(request, (const char *)input + split, length - split);
    const char *error = http_request_get_error_name(request);
    printf("%s\n", error ? error : "<null>");
    http_request_destroy(request);
}

int main(void)
{
    static const unsigned char prefix[] = "GET /x";
    static const unsigned char suffix[] = "y HTTP/1.1\r\n\r\n";
    unsigned char input[sizeof(prefix) - 1 + 1 + sizeof(suffix) - 1];

    for (unsigned int value = 0; value <= 255; ++value) {
        memcpy(input, prefix, sizeof(prefix) - 1);
        input[sizeof(prefix) - 1] = (unsigned char)value;
        memcpy(input + sizeof(prefix), suffix, sizeof(suffix) - 1);

        print_case(input, sizeof(input));
    }

    static const unsigned char header_prefix[] = "GET / HTTP/1.1\r\nX-Test: ";
    static const unsigned char header_suffix[] = "\r\n\r\n";
    unsigned char header_input[sizeof(header_prefix) - 1 + 1 + sizeof(header_suffix) - 1];
    for (unsigned int value = 0; value <= 255; ++value) {
        memcpy(header_input, header_prefix, sizeof(header_prefix) - 1);
        header_input[sizeof(header_prefix) - 1] = (unsigned char)value;
        memcpy(header_input + sizeof(header_prefix), header_suffix, sizeof(header_suffix) - 1);

        print_case(header_input, sizeof(header_input));
    }

    static const unsigned char name_prefix[] = "GET / HTTP/1.1\r\nX";
    static const unsigned char name_suffix[] = ": value\r\n\r\n";
    unsigned char name_input[sizeof(name_prefix) - 1 + 1 + sizeof(name_suffix) - 1];
    for (unsigned int value = 0; value <= 255; ++value) {
        memcpy(name_input, name_prefix, sizeof(name_prefix) - 1);
        name_input[sizeof(name_prefix) - 1] = (unsigned char)value;
        memcpy(name_input + sizeof(name_prefix), name_suffix, sizeof(name_suffix) - 1);

        print_case(name_input, sizeof(name_input));
    }

    static const char *const line_ending_cases[] = {
        "GET /x\nX-Test: value\r\n\r\n",
        "GET /x\n\r\n",
        "GET /x\nBad Header: value\r\n\r\n",
        "GET /x\r y HTTP/1.1\r\n\r\n",
        "GET /x\r\nX-Test: value\r\n\r\n",
        "GET / HTTP/1.1\n",
        "GET / HTTP/1.1\nX-Test: value\r\n\r\n",
    };
    for (size_t index = 0; index < sizeof(line_ending_cases) / sizeof(line_ending_cases[0]); ++index) {
        print_case((const unsigned char *)line_ending_cases[index], strlen(line_ending_cases[index]));
    }
    static const unsigned char fragmented_line[] = "GET /fragmented\nX-Test: value\r\n\r\n";
    static const size_t split = sizeof("GET /fragmented\n") - 1;
    print_split_case(fragmented_line, sizeof(fragmented_line) - 1, split);
    return 0;
}
