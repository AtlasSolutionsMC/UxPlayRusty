#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "../include/srp.h"

/* Compile the untouched original implementation into this comparison process. */
#define srp_random_seed ref_srp_random_seed
#define srp_create_salted_verification_key ref_srp_create_salted_verification_key
#define srp_create_server_ephemeral_key ref_srp_create_server_ephemeral_key
#define srp_verifier_new ref_srp_verifier_new
#define srp_verifier_delete ref_srp_verifier_delete
#define srp_verifier_is_authenticated ref_srp_verifier_is_authenticated
#define srp_verifier_get_username ref_srp_verifier_get_username
#define srp_verifier_get_session_key ref_srp_verifier_get_session_key
#define srp_verifier_get_session_key_length ref_srp_verifier_get_session_key_length
#define srp_verifier_verify_session ref_srp_verifier_verify_session
#include "srp.c"
#undef srp_random_seed
#undef srp_create_salted_verification_key
#undef srp_create_server_ephemeral_key
#undef srp_verifier_new
#undef srp_verifier_delete
#undef srp_verifier_is_authenticated
#undef srp_verifier_get_username
#undef srp_verifier_get_session_key
#undef srp_verifier_get_session_key_length
#undef srp_verifier_verify_session

#define CHECK(condition, message) do { \
    if (!(condition)) { fprintf(stderr, "FAIL: %s (line %d)\n", message, __LINE__); return 1; } \
} while (0)

static int same_bytes(const unsigned char *left, int left_len,
                      const unsigned char *right, int right_len) {
    return left_len == right_len && left_len >= 0 &&
           (left_len == 0 || memcmp(left, right, (size_t)left_len) == 0);
}

static int compare_hash_group(int alg, SRP_NGType ng_type, const char *n_hex, const char *g_hex,
                              int rfc5054_compat) {
    const unsigned char seed[] = {0, 1, 2, 3, 4, 5, 6, 7};
    const unsigned char client_A[] = {0x11};
    const unsigned char server_b[] = {0x17};
    const unsigned char bad_M[SHA512_DIGEST_LENGTH] = {0};
    const unsigned char *ref_s = NULL, *ref_v = NULL, *rust_s = NULL, *rust_v = NULL;
    int ref_s_len = 0, ref_v_len = 0, rust_s_len = 0, rust_v_len = 0;
    const unsigned char *ref_B = NULL, *rust_B = NULL;
    int ref_B_len = 0, rust_B_len = 0;
    struct SRPVerifier *ref_ver = NULL;
    struct SRPVerifier *rust_ver = NULL;
    const unsigned char *ref_key, *rust_key, *ref_hamk = NULL, *rust_hamk = NULL;
    int ref_key_len = 0, rust_key_len = 0;
    int expected_hash_len = alg == SRP_SHA1 ? 20 : alg == SRP_SHA224 ? 28 :
                            alg == SRP_SHA256 ? 32 : alg == SRP_SHA384 ? 48 : 64;

    ref_srp_random_seed(seed, (int)sizeof(seed));
    srp_random_seed(seed, (int)sizeof(seed));
    ref_srp_create_salted_verification_key((SRP_HashAlgorithm)alg, ng_type,
        "alice", (const unsigned char *)"password", 8, &ref_s, &ref_s_len,
        &ref_v, &ref_v_len, n_hex, g_hex);
    srp_create_salted_verification_key((SRP_HashAlgorithm)alg, ng_type,
        "alice", (const unsigned char *)"password", 8, &rust_s, &rust_s_len,
        &rust_v, &rust_v_len, n_hex, g_hex);
    CHECK(ref_s && ref_v && rust_s && rust_v, "salted verifier generation returned null");
    CHECK(ref_s_len > 0 && ref_s_len <= 16 && rust_s_len > 0 && rust_s_len <= 16,
          "Apple salt length is outside its 128-bit range");
    CHECK(ref_v_len > 0 && ref_v_len <= (ng_type == SRP_NG_CUSTOM ? 2 : 256) &&
          rust_v_len > 0 && rust_v_len <= (ng_type == SRP_NG_CUSTOM ? 2 : 256),
          "custom-group verifier has unexpected width");

    ref_srp_create_server_ephemeral_key((SRP_HashAlgorithm)alg, ng_type,
        ref_v, ref_v_len, server_b, (int)sizeof(server_b), &ref_B, &ref_B_len,
        n_hex, g_hex, rfc5054_compat);
    srp_create_server_ephemeral_key((SRP_HashAlgorithm)alg, ng_type,
        ref_v, ref_v_len, server_b, (int)sizeof(server_b), &rust_B, &rust_B_len,
        n_hex, g_hex, rfc5054_compat);
    CHECK(ref_B && rust_B && same_bytes(ref_B, ref_B_len, rust_B, rust_B_len),
          "deterministic server ephemeral B differs");
    free((void *)ref_B);
    free((void *)rust_B);
    ref_B = NULL; rust_B = NULL; ref_B_len = 0; rust_B_len = 0;

    ref_ver = ref_srp_verifier_new((SRP_HashAlgorithm)alg, ng_type, "alice",
        ref_s, ref_s_len, ref_v, ref_v_len, client_A, (int)sizeof(client_A),
        server_b, (int)sizeof(server_b), &ref_B, &ref_B_len, n_hex, g_hex,
        rfc5054_compat);
    rust_ver = srp_verifier_new((SRP_HashAlgorithm)alg, ng_type, "alice",
        ref_s, ref_s_len, ref_v, ref_v_len, client_A, (int)sizeof(client_A),
        server_b, (int)sizeof(server_b), &rust_B, &rust_B_len, n_hex, g_hex,
        rfc5054_compat);
    CHECK(ref_ver && rust_ver, "valid SRP verifier construction returned null");
    CHECK(same_bytes(ref_B, ref_B_len, rust_B, rust_B_len), "verifier B differs");
    CHECK(strcmp(ref_srp_verifier_get_username(ref_ver), "alice") == 0 &&
          strcmp(srp_verifier_get_username(rust_ver), "alice") == 0,
          "verifier username differs");
    CHECK(!ref_srp_verifier_is_authenticated(ref_ver) &&
          !srp_verifier_is_authenticated(rust_ver), "new verifier starts authenticated");

    ref_key = ref_srp_verifier_get_session_key(ref_ver, &ref_key_len);
    rust_key = srp_verifier_get_session_key(rust_ver, &rust_key_len);
    CHECK(ref_key_len == 2 * expected_hash_len && rust_key_len == ref_key_len,
          "Apple session-key length differs");
    CHECK(ref_srp_verifier_get_session_key_length(ref_ver) == ref_key_len &&
          srp_verifier_get_session_key_length(rust_ver) == rust_key_len,
          "session-key length getter differs");
    CHECK(same_bytes(ref_key, ref_key_len, rust_key, rust_key_len), "session key differs");

    ref_srp_verifier_verify_session(ref_ver, bad_M, &ref_hamk);
    srp_verifier_verify_session(rust_ver, bad_M, &rust_hamk);
    CHECK(ref_hamk == NULL && rust_hamk == NULL, "bad client proof was accepted");
    CHECK(!ref_srp_verifier_is_authenticated(ref_ver) &&
          !srp_verifier_is_authenticated(rust_ver), "bad proof changed auth state");

    /* The source struct is visible in this translation unit; its computed M is the oracle. */
    ref_srp_verifier_verify_session(ref_ver, ref_ver->M, &ref_hamk);
    srp_verifier_verify_session(rust_ver, ref_ver->M, &rust_hamk);
    CHECK(ref_hamk && rust_hamk &&
          same_bytes(ref_hamk, expected_hash_len, rust_hamk, expected_hash_len),
          "valid proof HAMK differs");
    CHECK(ref_srp_verifier_is_authenticated(ref_ver) &&
          srp_verifier_is_authenticated(rust_ver), "valid proof did not authenticate");

    ref_srp_verifier_delete(ref_ver);
    srp_verifier_delete(rust_ver);
    free((void *)ref_s); free((void *)ref_v); free((void *)rust_s); free((void *)rust_v);
    /* verifier_new owns its returned B pointer until verifier deletion. */
    return 0;
}

static int compare_invalid_client_public(void) {
    const unsigned char salt[] = {3}, verifier[] = {9}, server_b[] = {11};
    const unsigned char zero_A[] = {0}, n_A[] = {1, 1}; /* N = 0x0101 */
    const unsigned char *ref_B = (const unsigned char *)1, *rust_B = (const unsigned char *)1;
    int ref_B_len = -1, rust_B_len = -1;
    struct SRPVerifier *ref_ver, *rust_ver;
    ref_ver = ref_srp_verifier_new(SRP_SHA256, SRP_NG_CUSTOM, "alice", salt, 1,
        verifier, 1, zero_A, 1, server_b, 1, &ref_B, &ref_B_len, "0101", "03", 0);
    rust_ver = srp_verifier_new(SRP_SHA256, SRP_NG_CUSTOM, "alice", salt, 1,
        verifier, 1, zero_A, 1, server_b, 1, &rust_B, &rust_B_len, "0101", "03", 0);
    CHECK(!ref_ver && !rust_ver && !ref_B && !rust_B && ref_B_len == 0 && rust_B_len == 0,
          "zero A was not rejected consistently");
    ref_ver = ref_srp_verifier_new(SRP_SHA256, SRP_NG_CUSTOM, "alice", salt, 1,
        verifier, 1, n_A, 2, server_b, 1, &ref_B, &ref_B_len, "0101", "03", 0);
    rust_ver = srp_verifier_new(SRP_SHA256, SRP_NG_CUSTOM, "alice", salt, 1,
        verifier, 1, n_A, 2, server_b, 1, &rust_B, &rust_B_len, "0101", "03", 0);
    CHECK(!ref_ver && !rust_ver && !ref_B && !rust_B && ref_B_len == 0 && rust_B_len == 0,
          "A equal to N was not rejected consistently");
    return 0;
}

static int check_rust_rejects_malformed_pointer_ranges(void) {
    const unsigned char server_b[] = {11};
    const unsigned char *server_B = (const unsigned char *)1;
    const unsigned char *salt = (const unsigned char *)1;
    const unsigned char *verifier = (const unsigned char *)1;
    int server_B_len = -1, salt_len = -1, verifier_len = -1;
    srp_create_server_ephemeral_key(SRP_SHA256, SRP_NG_CUSTOM, NULL, 1,
        server_b, (int)sizeof(server_b), &server_B, &server_B_len, "0101", "03", 0);
    CHECK(!server_B && server_B_len == 0,
          "Rust wrapper did not reject null/nonempty verifier input");
    srp_create_salted_verification_key(SRP_SHA256, SRP_NG_CUSTOM, "alice", NULL, 1,
        &salt, &salt_len, &verifier, &verifier_len, "0101", "03");
    CHECK(!salt && !verifier && salt_len == 0 && verifier_len == 0,
          "Rust wrapper did not reject null/nonempty password input");
    return 0;
}

int main(void) {
    int alg, compat;
    for (alg = SRP_SHA1; alg <= SRP_SHA512; ++alg) {
        for (compat = 0; compat <= 1; ++compat) {
            if (compare_hash_group(alg, SRP_NG_CUSTOM, "0101", "03", compat)) return 1;
        }
    }
    if (compare_hash_group(SRP_SHA512, SRP_NG_2048, NULL, NULL, 1)) return 1;
    if (compare_invalid_client_public()) return 1;
    if (check_rust_rejects_malformed_pointer_ranges()) return 1;
    puts("SRP C differential passed: 5 hashes x 2 encodings, standard group SHA-512, valid/invalid A, bad proofs, and Rust pointer guards.");
    return 0;
}
