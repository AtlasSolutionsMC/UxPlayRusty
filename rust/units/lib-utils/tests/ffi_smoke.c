#include <windows.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define utils_strsep c_utils_strsep
#define utils_read_file c_utils_read_file
#define utils_hwaddr_raop c_utils_hwaddr_raop
#define utils_hwaddr_airplay c_utils_hwaddr_airplay
#define utils_parse_hex c_utils_parse_hex
#define utils_hex_to_string c_utils_hex_to_string
#define utils_data_to_string c_utils_data_to_string
#define utils_data_to_text c_utils_data_to_text
#define ntp_timestamp_to_time c_ntp_timestamp_to_time
#define ntp_timestamp_to_seconds c_ntp_timestamp_to_seconds
#define gmt_time_string c_gmt_time_string
#define utils_ipaddress_to_string c_utils_ipaddress_to_string
#define utils_strip_data_from_plist_xml c_utils_strip_data_from_plist_xml
#define parse_int c_parse_int
#include "../../../staging/utils-only/utils.c"

typedef char *(__cdecl *fn_utils_strsep)(char **, const char *);
typedef int (__cdecl *fn_utils_read_file)(char **, const char *);
typedef int (__cdecl *fn_utils_hwaddr_raop)(char *, int, const char *, int);
typedef int (__cdecl *fn_utils_hwaddr_airplay)(char *, int, const char *, int);
typedef char *(__cdecl *fn_utils_parse_hex)(const char *, int, int *);
typedef char *(__cdecl *fn_utils_hex_to_string)(const unsigned char *, int);
typedef char *(__cdecl *fn_utils_data_to_string)(const unsigned char *, int, int);
typedef char *(__cdecl *fn_utils_data_to_text)(const char *, int);
typedef void (__cdecl *fn_ntp_timestamp_to_time)(uint64_t, char *, size_t);
typedef void (__cdecl *fn_ntp_timestamp_to_seconds)(uint64_t, char *, size_t);
typedef const char *(__cdecl *fn_gmt_time_string)(void);
typedef int (__cdecl *fn_utils_ipaddress_to_string)(int, const unsigned char *, unsigned int, char *, int);
typedef char *(__cdecl *fn_utils_strip_data_from_plist_xml)(char *);
typedef int (__cdecl *fn_parse_int)(const char *);
typedef void (__cdecl *fn_free)(void *);

static int checks = 0;
static int fail(const char *label) {
    fprintf(stderr, "FAIL %s\n", label);
    return 1;
}
#define CHECK(label, expression) do { checks++; if (!(expression)) return fail(label); } while (0)

static FARPROC required_symbol(HMODULE module, const char *name) {
    FARPROC symbol = GetProcAddress(module, name);
    if (!symbol) fprintf(stderr, "missing export: %s\n", name);
    return symbol;
}
#define LOAD(name) fn_##name rust_##name = (fn_##name)required_symbol(module, "rust_anything_" #name); if (!rust_##name) return 2

int main(int argc, char **argv) {
    if (argc != 2) return fail("usage: ffi-smoke.exe <utils_rs.dll>");
    HMODULE module = LoadLibraryA(argv[1]);
    if (!module) { fprintf(stderr, "LoadLibrary failed: %lu\n", GetLastError()); return 2; }
    HMODULE crt = LoadLibraryA("msvcrt.dll");
    fn_free rust_free = crt ? (fn_free)GetProcAddress(crt, "free") : NULL;
    if (!rust_free) return fail("load matching msvcrt free");

    LOAD(utils_strsep);
    LOAD(utils_read_file);
    LOAD(utils_hwaddr_raop);
    LOAD(utils_hwaddr_airplay);
    LOAD(utils_parse_hex);
    LOAD(utils_hex_to_string);
    LOAD(utils_data_to_string);
    LOAD(utils_data_to_text);
    LOAD(ntp_timestamp_to_time);
    LOAD(ntp_timestamp_to_seconds);
    LOAD(gmt_time_string);
    LOAD(utils_ipaddress_to_string);
    LOAD(utils_strip_data_from_plist_xml);
    LOAD(parse_int);

    char c_text[] = "left::right::";
    char r_text[] = "left::right::";
    char *c_cursor = c_text, *r_cursor = r_text;
    CHECK("strsep first", strcmp(c_utils_strsep(&c_cursor, "::"), rust_utils_strsep(&r_cursor, "::")) == 0);
    CHECK("strsep first cursor", strcmp(c_cursor, r_cursor) == 0);
    CHECK("strsep second", strcmp(c_utils_strsep(&c_cursor, "::"), rust_utils_strsep(&r_cursor, "::")) == 0);
    CHECK("strsep trailing empty", strcmp(c_utils_strsep(&c_cursor, "::"), rust_utils_strsep(&r_cursor, "::")) == 0);
    CHECK("strsep exhausted", c_utils_strsep(&c_cursor, "::") == rust_utils_strsep(&r_cursor, "::") && c_cursor == NULL && r_cursor == NULL);

    unsigned char address[] = {0, 15, 160, 255};
    char c_mac[16] = {0}, r_mac[16] = {0};
    int c_count = c_utils_hwaddr_raop(c_mac, sizeof(c_mac), (const char *)address, sizeof(address));
    int r_count = rust_utils_hwaddr_raop(r_mac, sizeof(r_mac), (const char *)address, sizeof(address));
    CHECK("hwaddr raop", c_count == r_count && strcmp(c_mac, r_mac) == 0);
    memset(c_mac, 0, sizeof(c_mac)); memset(r_mac, 0, sizeof(r_mac));
    c_count = c_utils_hwaddr_airplay(c_mac, sizeof(c_mac), (const char *)address, sizeof(address));
    r_count = rust_utils_hwaddr_airplay(r_mac, sizeof(r_mac), (const char *)address, sizeof(address));
    CHECK("hwaddr airplay", c_count == r_count && strcmp(c_mac, r_mac) == 0);

    int c_len = -1, r_len = -1;
    char *c_hex = c_utils_parse_hex("00aFff", 6, &c_len);
    char *r_hex = rust_utils_parse_hex("00aFff", 6, &r_len);
    CHECK("parse hex", c_hex && r_hex && c_len == r_len && !memcmp(c_hex, r_hex, c_len));
    free(c_hex); rust_free(r_hex);
    unsigned char raw[] = {0, 175, 255};
    char *c_string = c_utils_hex_to_string(raw, sizeof(raw));
    char *r_string = rust_utils_hex_to_string(raw, sizeof(raw));
    CHECK("hex to string", c_string && r_string && strcmp(c_string, r_string) == 0);
    free(c_string); rust_free(r_string);
    c_string = c_utils_data_to_string(raw, sizeof(raw), 2);
    r_string = rust_utils_data_to_string(raw, sizeof(raw), 2);
    CHECK("data to string", c_string && r_string && strcmp(c_string, r_string) == 0);
    free(c_string); rust_free(r_string);
    char text[] = {'a', '\r', 'b', 0, '\r'};
    c_string = c_utils_data_to_text(text, sizeof(text));
    r_string = rust_utils_data_to_text(text, sizeof(text));
    CHECK("data to text", c_string && r_string && memcmp(c_string, r_string, sizeof(text) + 1) == 0);
    free(c_string); rust_free(r_string);

    const uint64_t timestamp = 1700000000123456789ULL;
    char c_time[64] = {0}, r_time[64] = {0};
    c_ntp_timestamp_to_time(timestamp, c_time, sizeof(c_time));
    rust_ntp_timestamp_to_time(timestamp, r_time, sizeof(r_time));
    CHECK("ntp time", strcmp(c_time, r_time) == 0);
    memset(c_time, 0, sizeof(c_time)); memset(r_time, 0, sizeof(r_time));
    c_ntp_timestamp_to_seconds(timestamp, c_time, sizeof(c_time));
    rust_ntp_timestamp_to_seconds(timestamp, r_time, sizeof(r_time));
    CHECK("ntp seconds", strcmp(c_time, r_time) == 0);

    unsigned char ip4[] = {192, 168, 1, 7};
    char c_ip[64] = {0}, r_ip[64] = {0};
    int c_ip_len = c_utils_ipaddress_to_string(sizeof(ip4), ip4, 0, c_ip, sizeof(c_ip));
    int r_ip_len = rust_utils_ipaddress_to_string(sizeof(ip4), ip4, 0, r_ip, sizeof(r_ip));
    CHECK("ipv4", c_ip_len == r_ip_len && strcmp(c_ip, r_ip) == 0);
    unsigned char ip6[] = {0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7};
    memset(c_ip, 0, sizeof(c_ip)); memset(r_ip, 0, sizeof(r_ip));
    c_ip_len = c_utils_ipaddress_to_string(sizeof(ip6), ip6, 3, c_ip, sizeof(c_ip));
    r_ip_len = rust_utils_ipaddress_to_string(sizeof(ip6), ip6, 3, r_ip, sizeof(r_ip));
    CHECK("ipv6 zone", c_ip_len == r_ip_len && strcmp(c_ip, r_ip) == 0);

    char c_xml[] = "<plist>\n<data>\n  AAAA\n  BBBB\n</data>\n</plist>\n";
    char r_xml[] = "<plist>\n<data>\n  AAAA\n  BBBB\n</data>\n</plist>\n";
    c_string = c_utils_strip_data_from_plist_xml(c_xml);
    r_string = rust_utils_strip_data_from_plist_xml(r_xml);
    CHECK("plist xml", c_string && r_string && strcmp(c_string, r_string) == 0);
    free(c_string); rust_free(r_string);

    FILE *file = fopen("ffi-smoke.bin", "wb");
    if (!file) return fail("create file");
    const unsigned char file_data[] = {0, 1, 13, 255};
    fwrite(file_data, 1, sizeof(file_data), file); fclose(file);
    char *c_file = NULL, *r_file = NULL;
    int c_file_len = c_utils_read_file(&c_file, "ffi-smoke.bin");
    int r_file_len = rust_utils_read_file(&r_file, "ffi-smoke.bin");
    CHECK("read file", c_file_len == r_file_len && c_file_len == sizeof(file_data) && !memcmp(c_file, r_file, sizeof(file_data)));
    free(c_file); rust_free(r_file); remove("ffi-smoke.bin");

    CHECK("parse int", c_parse_int("+42") == rust_parse_int("+42") && rust_parse_int("+42") == 42);
    const char *c_gmt = c_gmt_time_string();
    const char *r_gmt = rust_gmt_time_string();
    CHECK("gmt string", c_gmt && r_gmt && strlen(c_gmt) >= 20 && strlen(r_gmt) >= 20 && strstr(c_gmt, " GMT") && strstr(r_gmt, " GMT"));

    printf("PASS linked C reference differential: %d checks across 14 exports\n", checks);
    FreeLibrary(crt);
    FreeLibrary(module);
    return 0;
}
