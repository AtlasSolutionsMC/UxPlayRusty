#include <plist/plist.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const unsigned char hw_addr[] = {0x02, 0x00, 0x00, 0xa5, 0xc4, 0xb8};
static const unsigned char airplay_txt[] = {0x03, 'f', 'o', 'o', 0x03, 'b', 'a', 'r'};
static const unsigned char raop_txt[] = {0x04, 'r', 'a', 'o', 'p', 0x01, 'x'};
static const char public_key_hex[] =
    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
static const uint64_t features = UINT64_C(0x5a7ffee6);
static const char name[] = "UxPlay Test";
static const double initial_volume = -0.125;
static const uint16_t width = 1920;
static const uint16_t height = 1080;
static const uint8_t refresh_rate = 60;
static const uint8_t max_fps = 30;
static const uint8_t overscanned = 1;

static void set_string(plist_t dict, const char *key, const char *value) {
    plist_dict_set_item(dict, key, plist_new_string(value));
}

static void set_uint(plist_t dict, const char *key, uint64_t value) {
    plist_dict_set_item(dict, key, plist_new_uint(value));
}

static void set_bool(plist_t dict, const char *key, uint8_t value) {
    plist_dict_set_item(dict, key, plist_new_bool(value));
}

static void set_data(plist_t dict, const char *key,
                     const unsigned char *value, uint64_t length) {
    plist_dict_set_item(dict, key, plist_new_data((const char *)value, length));
}

static char *format_hw_addr(void) {
    static const char digits[] = "0123456789abcdef";
    char *formatted = calloc(1, 3 * sizeof(hw_addr));
    size_t offset = 0;
    for (size_t i = 0; i < sizeof(hw_addr); i++) {
        if (i) formatted[offset++] = ':';
        formatted[offset++] = digits[hw_addr[i] >> 4];
        formatted[offset++] = digits[hw_addr[i] & 0x0f];
    }
    return formatted;
}

static unsigned char *parse_public_key(size_t *length) {
    size_t hex_length = strlen(public_key_hex);
    if (hex_length % 2) return NULL;
    *length = hex_length / 2;
    unsigned char *key = malloc(*length);
    if (!key) return NULL;
    for (size_t i = 0; i < *length; i++) {
        unsigned int value = 0;
        if (sscanf(public_key_hex + (i * 2), "%2x", &value) != 1) {
            free(key);
            *length = 0;
            return NULL;
        }
        key[i] = (unsigned char)value;
    }
    return key;
}

static plist_t qualifier_body(const char *qualifier, char **bytes, uint32_t *length) {
    plist_t root = plist_new_dict();
    plist_t values = plist_new_array();
    plist_array_append_item(values, plist_new_string(qualifier));
    plist_dict_set_item(root, "qualifier", values);
    plist_to_bin(root, bytes, length);
    plist_free(root);
    return NULL;
}

static plist_t build_response(const char *url, const char *content_type,
                              const char *cseq, const char *body,
                              uint32_t body_length) {
    bool add_airplay = false;
    bool add_raop = false;
    const char *txt_airplay_name = "txtAirPlay";
    const char *txt_raop_name = "txtRAOP";
    plist_t response = plist_new_dict();

    if (content_type && strstr(content_type, "application/x-apple-binary-plist")) {
        plist_t request = NULL;
        plist_from_bin(body, body_length, &request);
        plist_t qualifier_array = plist_dict_get_item(request, "qualifier");
        if (PLIST_IS_ARRAY(qualifier_array)) {
            plist_t qualifier_node = plist_array_get_item(qualifier_array, 0);
            char *qualifier = NULL;
            plist_get_string_val(qualifier_node, &qualifier);
            if (qualifier) {
                if (!strcmp(qualifier, txt_airplay_name)) add_airplay = true;
                if (!strcmp(qualifier, txt_raop_name)) add_raop = true;
                plist_mem_free(qualifier);
            }
        }
        plist_free(request);
    }

    if (!cseq) {
        add_airplay = strstr(url, txt_airplay_name) != NULL;
        add_raop = strstr(url, txt_raop_name) != NULL;
    }

    if (add_airplay) set_data(response, txt_airplay_name, airplay_txt, sizeof(airplay_txt));
    if (add_raop) set_data(response, txt_raop_name, raop_txt, sizeof(raop_txt));
    if (content_type) return response;

    char *hw = format_hw_addr();
    set_string(response, "deviceID", hw);
    set_string(response, "macAddress", hw);
    free(hw);

    size_t pk_length = 0;
    unsigned char *pk = parse_public_key(&pk_length);
    set_data(response, "pk", pk, pk_length);
    free(pk);
    set_uint(response, "features", features);
    set_string(response, "name", name);
    set_string(response, "pi", "2e388006-13ba-4041-9a67-25dd4a43d536");
    set_uint(response, "vv", 2);
    set_uint(response, "statusFlags", 68);
    set_bool(response, "keepAliveLowPower", 1);
    set_string(response, "sourceVersion", "220.68");
    set_bool(response, "keepAliveSendStatsAsBody", 1);
    set_string(response, "model", "AppleTV3,2");
    if (!cseq) return response;

    plist_dict_set_item(response, "initialVolume", plist_new_real(initial_volume));

    plist_t audio_latencies = plist_new_array();
    plist_t latency0 = plist_new_dict();
    set_uint(latency0, "type", 100);
    set_uint(latency0, "inputLatencyMicros", 0);
    set_string(latency0, "audioType", "default");
    set_bool(latency0, "outputLatencyMicros", 0);
    plist_array_append_item(audio_latencies, latency0);
    plist_t latency1 = plist_new_dict();
    set_uint(latency1, "type", 101);
    set_string(latency1, "audioType", "default");
    set_uint(latency1, "inputLatencyMicros", 0);
    set_bool(latency1, "outputLatencyMicros", 0);
    plist_array_append_item(audio_latencies, latency1);
    plist_dict_set_item(response, "audioLatencies", audio_latencies);

    plist_t audio_formats = plist_new_array();
    plist_t format0 = plist_new_dict();
    set_uint(format0, "audioOutputFormats", 0x03fffffc);
    set_uint(format0, "type", 100);
    set_uint(format0, "audioInputFormats", 0x03fffffc);
    plist_array_append_item(audio_formats, format0);
    plist_t format1 = plist_new_dict();
    set_uint(format1, "audioOutputFormats", 0x03fffffc);
    set_uint(format1, "type", 101);
    set_uint(format1, "audioInputFormats", 0x03fffffc);
    plist_array_append_item(audio_formats, format1);
    plist_dict_set_item(response, "audioFormats", audio_formats);

    plist_t displays = plist_new_array();
    plist_t display = plist_new_dict();
    set_string(display, "uuid", "e0ff8a27-6738-3d56-8a16-cc53aacee925");
    set_uint(display, "widthPhysical", 0);
    set_uint(display, "heightPhysical", 0);
    set_uint(display, "width", width);
    set_uint(display, "height", height);
    set_uint(display, "widthPixels", width);
    set_uint(display, "heightPixels", height);
    set_bool(display, "rotation", 0);
    plist_dict_set_item(display, "refreshRate", plist_new_real(1.0 / refresh_rate));
    set_uint(display, "maxFPS", max_fps);
    set_bool(display, "overscanned", overscanned);
    set_uint(display, "features", 14);
    plist_array_append_item(displays, display);
    plist_dict_set_item(response, "displays", displays);
    return response;
}

static void emit(const char *label, plist_t response) {
    char *xml = NULL;
    uint32_t length = 0;
    plist_to_xml(response, &xml, &length);
    printf("--%s--\n", label);
    fwrite(xml, 1, length, stdout);
    plist_mem_free(xml);

    char *binary = NULL;
    if (plist_to_bin(response, &binary, &length) != PLIST_ERR_SUCCESS) {
        fputs("libplist failed to serialize oracle response\n", stderr);
        exit(EXIT_FAILURE);
    }
    printf("--%s-bin--\n", label);
    for (uint32_t i = 0; i < length; i++) printf("%02x", (unsigned char)binary[i]);
    putchar('\n');
    plist_mem_free(binary);
    plist_free(response);
}

int main(void) {
    char *airplay_body = NULL;
    uint32_t airplay_body_length = 0;
    qualifier_body("txtAirPlay", &airplay_body, &airplay_body_length);
    char *raop_body = NULL;
    uint32_t raop_body_length = 0;
    qualifier_body("txtRAOP", &raop_body, &raop_body_length);
    char *invalid_body = NULL;
    uint32_t invalid_body_length = 0;
    qualifier_body("unknown", &invalid_body, &invalid_body_length);

    emit("ble-airplay", build_response(
        "/info?txtAirPlay", NULL, NULL, NULL, 0));
    emit("ble-raop", build_response(
        "/info?txtRAOP", NULL, NULL, NULL, 0));
    emit("ble-both", build_response(
        "/info?txtAirPlay?txtRAOP", NULL, NULL, NULL, 0));
    emit("ble-plain", build_response(
        "/info", NULL, NULL, NULL, 0));
    emit("content-type-other", build_response(
        "/info?txtAirPlay", "application/json", NULL, NULL, 0));
    emit("qualifier-airplay", build_response(
        "/info", "application/x-apple-binary-plist", "7",
        airplay_body, airplay_body_length));
    emit("qualifier-raop", build_response(
        "/info", "application/x-apple-binary-plist", "8",
        raop_body, raop_body_length));
    emit("qualifier-invalid", build_response(
        "/info", "application/x-apple-binary-plist", "9",
        invalid_body, invalid_body_length));
    emit("cseq", build_response(
        "/info", NULL, "10", NULL, 0));

    plist_mem_free(airplay_body);
    plist_mem_free(raop_body);
    plist_mem_free(invalid_body);
    return 0;
}
