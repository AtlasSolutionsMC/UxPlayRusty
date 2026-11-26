/* Narrow oracle for the pure state behavior in the unchanged lib/dnssd.c. */
#include <stdio.h>
#include <string.h>
#include "dnssd.h"

void *dnssd_private_init(dnssd_t *dnssd_public, int *error)
{
    (void)error;
    return dnssd_public;
}

void dnssd_private_destroy(void *dnssd_private)
{
    (void)dnssd_private;
}

static void print_hex(const unsigned char *bytes, int length)
{
    for (int index = 0; index < length; ++index) {
        printf("%02x", bytes[index]);
    }
}

static void run_case(const char *label, const char *name, int name_len,
                     const unsigned char *hw_addr, int hw_addr_len,
                     unsigned char pin_pw)
{
    int error = -1;
    dnssd_t *state = dnssd_init(name, name_len, (const char *)hw_addr,
                                hw_addr_len, pin_pw, &error);
    printf("%s error=%d", label, error);
    if (state) {
        int stored_name_len = -1;
        int stored_hw_addr_len = -1;
        const char *stored_name = dnssd_get_name(state, &stored_name_len);
        const char *stored_hw_addr = dnssd_get_hw_addr(state, &stored_hw_addr_len);
        printf(" name=");
        print_hex((const unsigned char *)stored_name, stored_name_len);
        printf(" hw=");
        print_hex((const unsigned char *)stored_hw_addr, stored_hw_addr_len);
        printf(" pin=%u features=%016llx", state->pin_pw,
               (unsigned long long)dnssd_get_airplay_features(state));
        dnssd_destroy(state);
    }
    putchar('\n');
}

int main(void)
{
    static const unsigned char address[] = {0x00, 0x11, 0x22, 0xaa, 0xbb, 0xff};
    static const char trimmed[] = "Living Room.local.extra";
    static const char exact_local[] = ".local";
    static const char embedded_nul[] = {'a', '\0', 'b', '\0'};
    static const char empty[] = "";
    dnssd_t state = {0};

    run_case("trim", trimmed, (int)strlen(trimmed), address,
             (int)sizeof(address), 255);
    run_case("local", exact_local, (int)strlen(exact_local), address,
             (int)sizeof(address), 0);
    run_case("embedded", embedded_nul, 3, address,
             (int)sizeof(address), 0);
    run_case("empty", empty, 0, address, 0, 0);

    state.features1 = 0x5a7ffee6U;
    state.features2 = 0;
    dnssd_set_airplay_features(&state, 1, 0);
    dnssd_set_airplay_features(&state, 32, 1);
    dnssd_set_airplay_features(&state, 63, 1);
    dnssd_set_airplay_features(&state, -1, 1);
    dnssd_set_airplay_features(&state, 64, 1);
    dnssd_set_airplay_features(&state, 5, -1);
    dnssd_set_airplay_features(&state, 5, 2);
    printf("features value=%016llx\n",
           (unsigned long long)dnssd_get_airplay_features(&state));
    return 0;
}
