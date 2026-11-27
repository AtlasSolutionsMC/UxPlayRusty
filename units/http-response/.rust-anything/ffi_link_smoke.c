#define http_response_create ref_http_response_create
#define http_response_init ref_http_response_init
#define http_response_reverse_request_init ref_http_response_reverse_request_init
#define http_response_add_header ref_http_response_add_header
#define http_response_finish ref_http_response_finish
#define http_response_set_disconnect ref_http_response_set_disconnect
#define http_response_get_disconnect ref_http_response_get_disconnect
#define http_response_get_data ref_http_response_get_data
#define http_response_destroy ref_http_response_destroy
#include "http_response.c"
#undef http_response_create
#undef http_response_init
#undef http_response_reverse_request_init
#undef http_response_add_header
#undef http_response_finish
#undef http_response_set_disconnect
#undef http_response_get_disconnect
#undef http_response_get_data
#undef http_response_destroy

http_response_t *http_response_create(void);
void http_response_init(http_response_t *response, const char *protocol, int code, const char *message);
void http_response_reverse_request_init(http_response_t *request, const char *method, const char *url,
                                        const char *protocol);
void http_response_add_header(http_response_t *response, const char *name, const char *value);
void http_response_finish(http_response_t *response, const char *data, int datalen);
void http_response_set_disconnect(http_response_t *response, int disconnect);
int http_response_get_disconnect(http_response_t *response);
const char *http_response_get_data(http_response_t *response, int *datalen);
void http_response_destroy(http_response_t *response);

#include <stdio.h>
#include <string.h>

static int compare_data(http_response_t *reference, http_response_t *rust, const char *label)
{
    int reference_len = 0;
    int rust_len = 0;
    const char *reference_data = ref_http_response_get_data(reference, &reference_len);
    const char *rust_data = http_response_get_data(rust, &rust_len);
    if (reference_len != rust_len || memcmp(reference_data, rust_data, (size_t)reference_len) != 0) {
        fprintf(stderr, "%s mismatch (reference=%d, rust=%d)\n", label, reference_len, rust_len);
        return 0;
    }
    return 1;
}

static int empty_body_case(int with_content_type)
{
    http_response_t *reference = ref_http_response_create();
    http_response_t *rust = http_response_create();
    if (!reference || !rust) return 0;
    ref_http_response_init(reference, "HTTP/1.1", 204, "No Content");
    http_response_init(rust, "HTTP/1.1", 204, "No Content");
    if (with_content_type) {
        ref_http_response_add_header(reference, "Content-Type", "text/plain");
        http_response_add_header(rust, "Content-Type", "text/plain");
    } else {
        ref_http_response_add_header(reference, "X-Test", "yes");
        http_response_add_header(rust, "X-Test", "yes");
    }
    ref_http_response_finish(reference, NULL, 0);
    http_response_finish(rust, NULL, 0);
    int result = compare_data(reference, rust, with_content_type ? "empty typed" : "empty untyped");
    ref_http_response_destroy(reference);
    http_response_destroy(rust);
    return result;
}

static int response_body_and_reinit_case(void)
{
    http_response_t *reference = ref_http_response_create();
    http_response_t *rust = http_response_create();
    if (!reference || !rust) return 0;
    const char body[] = {'A', '\0', 'B'};
    ref_http_response_init(reference, "HTTP/1.1", 200, "OK");
    http_response_init(rust, "HTTP/1.1", 200, "OK");
    ref_http_response_add_header(reference, "Content-Type", "application/octet-stream");
    http_response_add_header(rust, "Content-Type", "application/octet-stream");
    ref_http_response_set_disconnect(reference, -9);
    http_response_set_disconnect(rust, -9);
    ref_http_response_finish(reference, body, sizeof(body));
    http_response_finish(rust, body, sizeof(body));
    int result = ref_http_response_get_disconnect(reference) == http_response_get_disconnect(rust)
        && compare_data(reference, rust, "binary response body");

    ref_http_response_reverse_request_init(reference, "POST", "/pair-setup", "PTTH/1.0");
    http_response_reverse_request_init(rust, "POST", "/pair-setup", "PTTH/1.0");
    result = result && compare_data(reference, rust, "reinitialized reverse request");
    ref_http_response_destroy(reference);
    http_response_destroy(rust);
    return result;
}

static int growth_case(void)
{
    http_response_t *reference = ref_http_response_create();
    http_response_t *rust = http_response_create();
    if (!reference || !rust) return 0;
    char header[1801];
    char body[3000];
    memset(header, 'v', sizeof(header) - 1);
    header[sizeof(header) - 1] = '\0';
    memset(body, 'z', sizeof(body));
    ref_http_response_init(reference, "HTTP/1.1", 200, "OK");
    http_response_init(rust, "HTTP/1.1", 200, "OK");
    ref_http_response_add_header(reference, "X-Long", header);
    http_response_add_header(rust, "X-Long", header);
    ref_http_response_finish(reference, body, sizeof(body));
    http_response_finish(rust, body, sizeof(body));
    int result = compare_data(reference, rust, "buffer growth");
    ref_http_response_destroy(reference);
    http_response_destroy(rust);
    return result;
}

int main(void)
{
    if (!response_body_and_reinit_case()) return 1;
    if (!empty_body_case(1) || !empty_body_case(0)) return 2;
    if (!growth_case()) return 3;
    ref_http_response_destroy(NULL);
    http_response_destroy(NULL);
    puts("4 C ABI scenarios match the original HTTP response builder");
    return 0;
}
