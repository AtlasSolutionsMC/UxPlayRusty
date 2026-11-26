#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "compat.h"

static const char *oracle_hostname = "Host One.example";
static int oracle_hostname_fails;
static int oracle_start_result;
static int oracle_gethostname(char *buffer, int length);

#define gethostname oracle_gethostname
#define mdnsd_init oracle_mdnsd_init
#define mdnsd_destroy oracle_mdnsd_destroy
#define mdnsd_set_services oracle_mdnsd_set_services
#define mdnsd_start oracle_mdnsd_start
#define mdnsd_stop oracle_mdnsd_stop
#define mdnsd_announce oracle_mdnsd_announce
#define mdnsd_goodbye oracle_mdnsd_goodbye
#include "dnssd_mdnsd.c"
#undef gethostname
#undef mdnsd_init
#undef mdnsd_destroy
#undef mdnsd_set_services
#undef mdnsd_start
#undef mdnsd_stop
#undef mdnsd_announce
#undef mdnsd_goodbye

#include "mdnsd.c"
#include "utils.c"
#include "compat.c"

static int oracle_gethostname(char *buffer, int length)
{
    if (oracle_hostname_fails) {
        return -1;
    }
    size_t copy_length = strlen(oracle_hostname);
    if (copy_length > (size_t) length) {
        copy_length = (size_t) length;
    }
    memcpy(buffer, oracle_hostname, copy_length);
    if (copy_length < (size_t) length) {
        buffer[copy_length] = '\0';
    }
    return 0;
}

static void print_hex(const unsigned char *bytes, size_t length)
{
    for (size_t index = 0; index < length; index++) {
        printf("%02x", bytes[index]);
    }
}

static void print_service(const char *event, const mdnsd_service_t *service)
{
    printf("%s|%s|%s|%u|%d|", event, service->type, service->name,
           (unsigned int) service->port, service->registered);
    print_hex(service->txt.bytes, (size_t) service->txt.length);
    putchar('\n');
}

mdnsd_t *oracle_mdnsd_init(const char *host_name)
{
    (void) host_name;
    return (mdnsd_t *) (uintptr_t) 1;
}

void oracle_mdnsd_destroy(mdnsd_t *mdnsd)
{
    (void) mdnsd;
}

void oracle_mdnsd_set_services(mdnsd_t *mdnsd,
                               const mdnsd_service_t *airplay,
                               const mdnsd_service_t *raop)
{
    (void) mdnsd;
    print_service("service-airplay", airplay);
    print_service("service-raop", raop);
}

int oracle_mdnsd_start(mdnsd_t *mdnsd)
{
    (void) mdnsd;
    printf("start|%d\n", oracle_start_result);
    return oracle_start_result;
}

void oracle_mdnsd_stop(mdnsd_t *mdnsd)
{
    (void) mdnsd;
    puts("stop");
}

void oracle_mdnsd_announce(mdnsd_t *mdnsd, uint32_t ttl)
{
    (void) mdnsd;
    printf("announce|%u\n", ttl);
}

void oracle_mdnsd_goodbye(mdnsd_t *mdnsd, const mdnsd_service_t *service)
{
    (void) mdnsd;
    print_service("goodbye", service);
}

static void print_names(const char *label, int result,
                        const dnssd_private_t *private_state)
{
    printf("%s|%d|%s|%s|%s\n", label, result, private_state->host_name,
           private_state->raop_name, private_state->airplay_name);
}

static void print_txt(const char *label, const mdnsd_txt_t *txt)
{
    printf("%s|%d|", label, txt->length);
    print_hex(txt->bytes, (size_t) txt->length);
    putchar('\n');
}

int main(void)
{
    static const unsigned char hardware_address[] = {
        0x00, 0x11, 0x22, 0xaa, 0xbb, 0xff
    };
    static const char instance_name[] = "Kitchen";
    static const char public_key[] = "0123456789abcdef";
    dnssd_private_t private_state = {0};
    dnssd_t public_state = {0};
    public_state.name = (char *) instance_name;
    public_state.name_len = (int) strlen(instance_name);
    public_state.hw_addr = (char *) hardware_address;
    public_state.hw_addr_len = (int) sizeof(hardware_address);
    public_state.pk = (char *) public_key;
    public_state.features1 = 0x5a7ffee6;
    public_state.features2 = 0;
    public_state.pin_pw = 1;
    public_state.dnssd_private = &private_state;

    int name_result = dnssd_prepare_names(&private_state, &public_state);
    print_names("names", name_result, &private_state);
    if (dnssd_build_raop_txt(&public_state) != 0 ||
        dnssd_build_airplay_txt(&public_state) != 0) {
        return 2;
    }
    print_txt("raop-txt", &private_state.raop_record);
    print_txt("airplay-txt", &private_state.airplay_record);

    private_state.mdnsd = (mdnsd_t *) (uintptr_t) 1;
    oracle_start_result = 0;
    printf("return-raop|%d\n", dnssd_register_raop(&public_state, 7000));
    oracle_start_result = 9;
    printf("return-airplay|%d\n", dnssd_register_airplay(&public_state, 7001));
    dnssd_unregister_raop(&public_state);
    dnssd_unregister_airplay(&public_state);
    dnssd_unregister_raop(&public_state);
    dnssd_unregister_airplay(&public_state);

    oracle_hostname = "";
    dnssd_private_t empty_host = {0};
    print_names("empty-host", dnssd_prepare_names(&empty_host, &public_state), &empty_host);
    oracle_hostname_fails = 1;
    dnssd_private_t failed_host = {0};
    print_names("failed-host", dnssd_prepare_names(&failed_host, &public_state), &failed_host);
    return 0;
}
