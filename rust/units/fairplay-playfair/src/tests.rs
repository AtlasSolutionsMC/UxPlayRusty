use std::ptr;

use super::port::{FairPlay, INVALID, OK};

#[test]
fn v3_setup_and_handshake_preserve_protocol_framing_for_each_mode() {
    for mode in 0..4_u8 {
        let mut fairplay = FairPlay::new(ptr::null_mut());
        let mut setup_request = [0; 16];
        setup_request[4] = 3;
        setup_request[14] = mode;
        let mut setup_response = [0xa5; 142];
        assert_eq!(fairplay.setup(&setup_request, &mut setup_response), OK);
        assert_eq!(&setup_response[..4], b"FPLY");
        assert_eq!(setup_response[13], mode);

        let mut handshake_request = [0; 164];
        handshake_request[4] = 3;
        handshake_request[12] = mode;
        for (index, byte) in handshake_request.iter_mut().enumerate() {
            if index != 4 && index != 12 {
                *byte = index as u8;
            }
        }
        let mut handshake_response = [0xa5; 32];
        assert_eq!(
            fairplay.handshake(&handshake_request, &mut handshake_response),
            OK
        );
        assert_eq!(&handshake_response[..12], b"FPLY\x03\x01\x04\0\0\0\0\x14");
        assert_eq!(&handshake_response[12..], &handshake_request[144..164]);
    }
}

#[test]
fn setup_rejects_invalid_version_without_changing_response_and_clears_state() {
    let mut fairplay = FairPlay::new(ptr::null_mut());
    let mut valid_request = [0; 16];
    valid_request[4] = 3;
    let mut response = [0; 142];
    assert_eq!(fairplay.setup(&valid_request, &mut response), OK);

    let mut invalid_request = [0; 16];
    invalid_request[4] = 4;
    response.fill(0xa5);
    assert_eq!(fairplay.setup(&invalid_request, &mut response), INVALID);
    assert_eq!(response, [0xa5; 142]);

    let mut key_out = [0xa5; 16];
    assert_eq!(fairplay.decrypt(&[0; 72], &mut key_out), INVALID);
    assert_eq!(key_out, [0; 16]);
}

#[test]
fn invalid_selector_two_setup_preserves_response_and_leaves_context_reset() {
    let mut fairplay = FairPlay::new(ptr::null_mut());
    let mut request = [0; 16];
    request[4] = 2;
    let mut response = [0xa5; 142];
    assert_eq!(fairplay.setup(&request, &mut response), INVALID);
    assert_eq!(response, [0xa5; 142]);

    let mut key_out = [0xa5; 16];
    assert_eq!(fairplay.decrypt(&[0; 72], &mut key_out), INVALID);
    assert_eq!(key_out, [0; 16]);
}

#[test]
fn handshake_can_start_without_setup_and_invalid_handshake_resets_context() {
    let mut fairplay = FairPlay::new(ptr::null_mut());
    let mut request = [0; 164];
    request[4] = 3;
    request[12] = 2;
    let mut response = [0; 32];
    assert_eq!(fairplay.handshake(&request, &mut response), OK);

    request[4] = 2;
    assert_eq!(fairplay.handshake(&request, &mut response), INVALID);
    let mut key_out = [0xa5; 16];
    assert_eq!(fairplay.decrypt(&[0; 72], &mut key_out), INVALID);
    assert_eq!(key_out, [0; 16]);
}

#[test]
fn legacy_decrypt_is_deterministic_after_the_unvalidated_v3_handshake() {
    for mode in 0..4_u8 {
        let mut fairplay = FairPlay::new(ptr::null_mut());
        let mut request = [0; 164];
        for (index, byte) in request.iter_mut().enumerate() {
            *byte = (index as u8)
                .wrapping_mul(17)
                .wrapping_add(mode.wrapping_mul(3));
        }
        request[4] = 3;
        request[12] = mode;
        let mut response = [0; 32];
        assert_eq!(fairplay.handshake(&request, &mut response), OK);

        let mut cipher_text = [0; 72];
        for (index, byte) in cipher_text.iter_mut().enumerate() {
            *byte = (index as u8)
                .wrapping_mul(29)
                .wrapping_add(mode.wrapping_mul(7));
        }
        let mut key_out = [0; 16];
        assert_eq!(fairplay.decrypt(&cipher_text, &mut key_out), OK);
        assert_ne!(key_out, [0; 16]);
    }
}
