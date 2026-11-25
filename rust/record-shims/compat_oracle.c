/* C behavior oracle for UxPlay lib/compat.c; source remains read-only. */
#include <stdio.h>
#include <string.h>
#include "sockets.h"

int main(void) {
    const int errors[] = {0, 10004, 10035, 2147483647};
    for (size_t index = 0; index < sizeof(errors) / sizeof(errors[0]); index++) {
        int code = errors[index];
        const unsigned char *message = (const unsigned char *)wsa_strerror(code);
        printf("%d\t", code);
        for (size_t byte = 0; byte < strlen((const char *)message); byte++) {
            printf("%02x", message[byte]);
        }
        putchar('\n');
    }
    return 0;
}
