use super::port::{self, AesContext, AesMode, Direction, Ed25519Key, X25519Key};

fn unhex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn aes128_ctr_nist_sp800_38a_f5_1() {
    let key = unhex("2b7e151628aed2a6abf7158809cf4f3c");
    let iv = unhex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
    let plaintext = unhex("6bc1bee22e409f96e93d7e117393172a");
    let expected = unhex("874d6191b620e3261bef6864990db6ce");
    let mut ctx = AesContext::new(AesMode::Ctr, Direction::Encrypt, &key, &iv).unwrap();
    assert_eq!(ctx.ctr_encrypt(&plaintext).unwrap(), expected);
    ctx.reset().unwrap();
    assert_eq!(ctx.ctr_decrypt(&expected).unwrap(), plaintext);
}

#[test]
fn aes128_cbc_nist_sp800_38a_f2_1() {
    let key = unhex("2b7e151628aed2a6abf7158809cf4f3c");
    let iv = unhex("000102030405060708090a0b0c0d0e0f");
    let plaintext = unhex("6bc1bee22e409f96e93d7e117393172a");
    let expected = unhex("7649abac8119b246cee98e9b12e9197d");
    let mut enc = AesContext::new(AesMode::Cbc, Direction::Encrypt, &key, &iv).unwrap();
    assert_eq!(
        enc.cbc_crypt(&plaintext, Direction::Encrypt).unwrap(),
        expected
    );
    let mut dec = AesContext::new(AesMode::Cbc, Direction::Decrypt, &key, &iv).unwrap();
    assert_eq!(
        dec.cbc_crypt(&expected, Direction::Decrypt).unwrap(),
        plaintext
    );
}

#[test]
fn aes128_gcm_16_byte_iv_vector_and_auth_failure() {
    // Wycheproof AES-GCM tcId 72 exercises the 128-bit IV convention used by UxPlay.
    let key = unhex("b67b1a6efdd40d37080fbe8f8047aeb9");
    let iv = unhex("fa294b129972f7fc5bbd5b96bba837c9");
    let plaintext = [];
    let (ciphertext, tag) = port::gcm_encrypt(&plaintext, &key, &iv).unwrap();
    assert!(ciphertext.is_empty());
    assert_eq!(tag.as_slice(), unhex("a2cf26481517ec25085c5b17d0786183"));
    let decrypted = port::gcm_decrypt(&ciphertext, &key, &iv, &tag).unwrap();
    assert!(decrypted.authenticated);
    assert_eq!(decrypted.plaintext, plaintext);
    let mut bad_tag = tag;
    bad_tag[0] ^= 1;
    let rejected = port::gcm_decrypt(&ciphertext, &key, &iv, &bad_tag).unwrap();
    assert!(!rejected.authenticated);
    assert!(rejected.plaintext.is_empty());
}

#[test]
fn aes128_gcm_16_byte_iv_one_byte_known_answer() {
    // Wycheproof AES-GCM tcId 73: no AAD, one byte of plaintext, 128-bit IV.
    let key = unhex("209e6dbf2ad26a105445fc0207cd9e9a");
    let iv = unhex("9477849d6ccdfca112d92e53fae4a7ca");
    let plaintext = unhex("01");
    let expected_ciphertext = unhex("fd");
    let expected_tag = unhex("032df7bba5d8ea1a14f16f70bd0e14ec");
    let (ciphertext, tag) = port::gcm_encrypt(&plaintext, &key, &iv).unwrap();
    assert_eq!(ciphertext, expected_ciphertext);
    assert_eq!(tag.as_slice(), expected_tag);
    let decrypted = port::gcm_decrypt(&ciphertext, &key, &iv, &tag).unwrap();
    assert!(decrypted.authenticated);
    assert_eq!(decrypted.plaintext, plaintext);
    let mut bad_tag = tag;
    bad_tag[0] ^= 1;
    let rejected = port::gcm_decrypt(&ciphertext, &key, &iv, &bad_tag).unwrap();
    assert!(!rejected.authenticated);
    assert_eq!(rejected.plaintext, plaintext);
}

#[test]
fn ed25519_rfc8032_test_1() {
    let seed = unhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60");
    let expected_public = unhex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
    let expected_signature = unhex(concat!(
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155",
        "5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
    ));
    let key = Ed25519Key::from_raw_private(&seed).unwrap();
    assert_eq!(key.raw_public().unwrap().as_slice(), expected_public);
    let signature = key.sign(b"").unwrap();
    assert_eq!(signature.as_slice(), expected_signature);
    let public = Ed25519Key::from_raw_public(&expected_public).unwrap();
    assert!(public.verify(b"", &signature).unwrap());
    assert!(!public.verify(b"changed", &signature).unwrap());
}

#[test]
fn x25519_rfc7748_alice_bob_shared_secret() {
    let alice_private = unhex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
    let bob_private = unhex("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb");
    let alice = PKeyPrivateX25519::from_seed(&alice_private);
    let bob = PKeyPrivateX25519::from_seed(&bob_private);
    let expected_alice_public =
        unhex("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a");
    let expected_bob_public =
        unhex("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f");
    let expected_shared = unhex("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");
    assert_eq!(
        alice.raw_public().unwrap().as_slice(),
        expected_alice_public
    );
    assert_eq!(bob.raw_public().unwrap().as_slice(), expected_bob_public);
    assert_eq!(
        alice
            .derive(&X25519Key::from_raw_public(&expected_bob_public).unwrap())
            .unwrap()
            .as_slice(),
        expected_shared
    );
    assert_eq!(
        bob.derive(&X25519Key::from_raw_public(&expected_alice_public).unwrap())
            .unwrap()
            .as_slice(),
        expected_shared
    );
}

struct PKeyPrivateX25519;
impl PKeyPrivateX25519 {
    fn from_seed(seed: &[u8]) -> X25519Key {
        // Test-only helper derives a deterministic private key through OpenSSL's raw key API.
        X25519Key::Private(
            openssl::pkey::PKey::private_key_from_raw_bytes(seed, openssl::pkey::Id::X25519)
                .unwrap(),
        )
    }
}

#[test]
fn streaming_hashes_rfc1321_and_sha512_known_answers() {
    let mut md5 = port::md5().unwrap();
    md5.update(b"a").unwrap();
    md5.update(b"bc").unwrap();
    assert_eq!(
        md5.finish().unwrap(),
        unhex("900150983cd24fb0d6963f7d28e17f72")
    );
    md5.reset().unwrap();
    md5.update(b"abc").unwrap();
    assert_eq!(
        md5.finish().unwrap(),
        unhex("900150983cd24fb0d6963f7d28e17f72")
    );

    let mut sha = port::sha512().unwrap();
    sha.update(b"a").unwrap();
    sha.update(b"bc").unwrap();
    assert_eq!(
        sha.finish().unwrap(),
        unhex(concat!(
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a",
            "2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        ))
    );
}

#[test]
fn device_key_base64_random_and_pem_round_trip_invariants() {
    let (key_a, wrote_a) = Ed25519Key::load_or_generate(b"device-test", None).unwrap();
    let (key_b, wrote_b) = Ed25519Key::load_or_generate(b"device-test", None).unwrap();
    assert!(!wrote_a && !wrote_b);
    assert_eq!(key_a.raw_public().unwrap(), key_b.raw_public().unwrap());
    let pem = Ed25519Key::generate_random().unwrap().to_pem().unwrap();
    let loaded = Ed25519Key::from_pem(&pem).unwrap();
    let message = b"pem round-trip";
    assert!(
        loaded
            .verify(message, &loaded.sign(message).unwrap())
            .unwrap()
    );
    assert_eq!(port::pk_to_base64(b"abc"), "YWJj");
    let random = port::random_bytes(32).unwrap();
    assert_eq!(random.len(), 32);
}

#[test]
fn random_ed25519_key_file_round_trip() {
    let path = std::env::temp_dir().join(format!("uxplay-crypto-{}-key.pem", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let (created, wrote) =
        Ed25519Key::load_or_generate(b"ignored-for-key-file", Some(&path)).unwrap();
    assert!(wrote);
    let first_public = created.raw_public().unwrap();
    let (loaded, wrote_again) =
        Ed25519Key::load_or_generate(b"ignored-for-key-file", Some(&path)).unwrap();
    assert!(!wrote_again);
    assert_eq!(loaded.raw_public().unwrap(), first_public);
    let message = b"key-file round trip";
    assert!(
        loaded
            .verify(message, &created.sign(message).unwrap())
            .unwrap()
    );
    std::fs::remove_file(path).unwrap();
}
