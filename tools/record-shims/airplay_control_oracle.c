/* C-library oracle for the numeric conversions used by the AirPlay controls. */
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    const char *label;
    const char *value;
} test_case_t;

static const test_case_t cases[] = {
    {"decimal-prefix", "12.5tail"},
    {"space-sign", "\t -1.25junk"},
    {"invalid-exponent", "1e+tail"},
    {"leading-dot", ".75/"},
    {"no-number", "abc"},
    {"overflow", "1e9999"},
    {"underflow", "1e-9999"},
    {"hex-prefix", "0x1.8p+1tail"},
    {"hex-halfway", "0x1.000001p0"},
    {"negative-infinity", "-INFmore"},
    {"nan-payload", "NaN(payload)"},
};

int main(void)
{
    for (size_t index = 0; index < sizeof(cases) / sizeof(cases[0]); ++index) {
        const char *value = cases[index].value;
        char *double_end = NULL;
        char *float_end = NULL;
        double parsed_double = strtod(value, &double_end);
        float scrub_position = (float)parsed_double;
        float rate_value = strtof(value, &float_end);
        uint32_t scrub_bits = 0;
        uint32_t rate_bits = 0;
        memcpy(&scrub_bits, &scrub_position, sizeof(scrub_bits));
        memcpy(&rate_bits, &rate_value, sizeof(rate_bits));
        printf("%s %08" PRIx32 " %08" PRIx32 " %td %td\n",
               cases[index].label, scrub_bits, rate_bits,
               double_end - value, float_end - value);
    }
    return 0;
}
