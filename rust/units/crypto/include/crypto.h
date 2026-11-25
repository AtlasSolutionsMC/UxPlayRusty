#ifndef UXPLAY_RUST_CRYPTO_H
#define UXPLAY_RUST_CRYPTO_H

#include <stddef.h>
#include <stdint.h>

/*
 * Pointer contract: fixed-size key/IV/tag/key buffers must point to at least the
 * declared size; length-delimited buffers must be valid for the supplied length.
 * AES/GCM input and output data may overlap. Context handles come from their
 * matching init/generate function and must be destroyed exactly once. The caller
 * owns and must free the string returned by get_md5().
 */

#ifdef __cplusplus
extern "C" {
#endif

#define AES_128_BLOCK_SIZE 16
#define X25519_KEY_SIZE 32
#define ED25519_KEY_SIZE 32

typedef enum aes_direction_e { AES_DECRYPT, AES_ENCRYPT } aes_direction_t;
typedef struct aes_ctx_s aes_ctx_t;
typedef struct x25519_key_s x25519_key_t;
typedef struct ed25519_key_s ed25519_key_t;
typedef struct sha_ctx_s sha_ctx_t;
typedef struct md5_ctx_s md5_ctx_t;

aes_ctx_t *aes_ctr_init(const uint8_t *key, const uint8_t *iv);
void aes_ctr_reset(aes_ctx_t *ctx);
void aes_ctr_encrypt(aes_ctx_t *ctx, const uint8_t *in, uint8_t *out, int len);
void aes_ctr_decrypt(aes_ctx_t *ctx, const uint8_t *in, uint8_t *out, int len);
void aes_ctr_start_fresh_block(aes_ctx_t *ctx);
void aes_ctr_destroy(aes_ctx_t *ctx);

aes_ctx_t *aes_cbc_init(const uint8_t *key, const uint8_t *iv, aes_direction_t direction);
void aes_cbc_reset(aes_ctx_t *ctx);
void aes_cbc_encrypt(aes_ctx_t *ctx, const uint8_t *in, uint8_t *out, int len);
void aes_cbc_decrypt(aes_ctx_t *ctx, const uint8_t *in, uint8_t *out, int len);
void aes_cbc_destroy(aes_ctx_t *ctx);

x25519_key_t *x25519_key_generate(void);
x25519_key_t *x25519_key_from_raw(const unsigned char data[X25519_KEY_SIZE]);
void x25519_key_get_raw(unsigned char data[X25519_KEY_SIZE], const x25519_key_t *key);
void x25519_key_destroy(x25519_key_t *key);
int get_random_bytes(unsigned char *buf, int num);
void pk_to_base64(const unsigned char *pk, int pk_len, char *pk_base64, int len);
void x25519_derive_secret(unsigned char secret[X25519_KEY_SIZE], const x25519_key_t *ours,
                          const x25519_key_t *theirs);

int gcm_encrypt(const unsigned char *plaintext, int plaintext_len, unsigned char *ciphertext,
                unsigned char *key, unsigned char *iv, unsigned char *tag);
int gcm_decrypt(unsigned char *ciphertext, int ciphertext_len, unsigned char *plaintext,
                unsigned char *key, unsigned char *iv, unsigned char *tag);

ed25519_key_t *ed25519_key_generate(const char *device_id, const char *keyfile, int *result);
ed25519_key_t *ed25519_key_from_raw(const unsigned char data[ED25519_KEY_SIZE]);
void ed25519_key_get_raw(unsigned char data[ED25519_KEY_SIZE], const ed25519_key_t *key);
ed25519_key_t *ed25519_key_copy(const ed25519_key_t *key);
void ed25519_key_destroy(ed25519_key_t *key);
void ed25519_sign(unsigned char *signature, size_t signature_len,
                  const unsigned char *data, size_t data_len, const ed25519_key_t *key);
int ed25519_verify(const unsigned char *signature, size_t signature_len,
                   const unsigned char *data, size_t data_len, const ed25519_key_t *key);

sha_ctx_t *sha_init(void);
void sha_update(sha_ctx_t *ctx, const uint8_t *in, int len);
void sha_final(sha_ctx_t *ctx, uint8_t *out, unsigned int *len);
void sha_reset(sha_ctx_t *ctx);
void sha_destroy(sha_ctx_t *ctx);

md5_ctx_t *md5_init(void);
void md5_update(md5_ctx_t *ctx, const uint8_t *in, int len);
void md5_final(md5_ctx_t *ctx, uint8_t *out, unsigned int *len);
void md5_reset(md5_ctx_t *ctx);
void md5_destroy(md5_ctx_t *ctx);
char *get_md5(char *string);

#ifdef __cplusplus
}
#endif
#endif
