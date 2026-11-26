/* Oracle the libc scanf/printf operations used by raop_handlers.h. */
#include <inttypes.h>
#include <float.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct {
    const char *name;
    const char *input;
} input_case_t;

static const input_case_t volume_cases[] = {
    {"decimal", "0.5"},
    {"trailing", "-1.25junk"},
    {"space", "\t+0.25 tail"},
    {"invalid", "nope"},
    {"hex", "0x1.8p+1tail"},
    {"overflow", "1e9999"},
    {"underflow", "1e-9999"},
};

static const input_case_t progress_cases[] = {
    {"full", "1/2/3"},
    {"signed", "-1/+2/3"},
    {"partial", "9/x/3"},
    {"invalid", "x/y/z"},
    {"spaces", " 7/ 8/9"},
};

static void print_f32_bits(const char *kind, const char *name, float value)
{
    uint32_t bits = 0;
    memcpy(&bits, &value, sizeof(bits));
    printf("%s %s %08" PRIx32 "\n", kind, name, bits);
}

static void print_format(const char *name, float value)
{
    char body[25] = "volume: 0.0\r\n";
    (void) snprintf(body, sizeof(body), "volume: %9.6f\r\n", value);
    printf("F %s ", name);
    for (size_t i = 0; i < strlen(body); ++i) {
        printf("%02x", (unsigned char) body[i]);
    }
    putchar('\n');
}

int main(void)
{
    for (size_t i = 0; i < sizeof(volume_cases) / sizeof(volume_cases[0]); ++i) {
        float volume = 0.0f;
        (void) sscanf(volume_cases[i].input, "%f", &volume);
        print_f32_bits("V", volume_cases[i].name, volume);
    }

    for (size_t i = 0; i < sizeof(progress_cases) / sizeof(progress_cases[0]); ++i) {
        uint32_t start = 0, current = 0, end = 0;
        (void) sscanf(progress_cases[i].input,
                      "%" PRIu32 "/%" PRIu32 "/%" PRIu32,
                      &start, &current, &end);
        printf("P %s %" PRIu32 " %" PRIu32 " %" PRIu32 "\n",
               progress_cases[i].name, start, current, end);
    }

    print_format("half", 0.5f);
    print_format("negative", -1.25f);
    print_format("negative-zero", -0.0f);
    print_format("large", 12345.75f);
    print_format("maximum", FLT_MAX);
    return 0;
}
