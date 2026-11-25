use super::port::{FairPlayV2, INVALID, OK, test_confirmation, test_encrypt_sap, test_key_record};

fn record_header(record: &mut [u8], kind: u8, payload_length: u32) {
    record[..4].copy_from_slice(b"FPLY");
    record[4..8].copy_from_slice(&[2, 1, kind, 0]);
    record[8..12].copy_from_slice(&payload_length.to_be_bytes());
}

#[test]
fn setup_handshake_and_key_decryption_cover_each_selector_mode() {
    for mode in 0..4_u8 {
        let mut receiver = [0_u8; 128];
        receiver[1] = 1;
        for (index, byte) in receiver[2..].iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(37).wrapping_add(11);
        }
        let mut fairplay = FairPlayV2::with_receiver_sap(receiver);

        let mut setup_request = [0_u8; 16];
        record_header(&mut setup_request, 1, 4);
        setup_request[12..16].copy_from_slice(&[2, 0, mode, 0xbb]);
        let mut setup_response = [0xa5; 142];
        assert_eq!(fairplay.setup(&setup_request, &mut setup_response), OK);
        assert_eq!(
            &setup_response[..14],
            &[b'F', b'P', b'L', b'Y', 2, 1, 2, 0, 0, 0, 0, 130, 2, mode]
        );

        let mut sender = [0_u8; 128];
        for (index, byte) in sender.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(13).wrapping_add(7);
        }
        let mut handshake_request = [0_u8; 164];
        record_header(&mut handshake_request, 3, 152);
        handshake_request[12..16].copy_from_slice(&[mode, 0x8f, 0x1a, 0x9c]);
        handshake_request[16..144].copy_from_slice(&test_encrypt_sap(mode, &sender));
        handshake_request[144..164].copy_from_slice(&test_confirmation(&sender, &receiver));
        let mut handshake_response = [0xa5; 32];
        assert_eq!(
            fairplay.handshake(&handshake_request, &mut handshake_response),
            OK
        );
        assert_eq!(
            &handshake_response[..12],
            b"FPLY\x02\x01\x04\x00\x00\x00\x00\x14"
        );

        let key: [u8; 16] = core::array::from_fn(|index| 0x10_u8.wrapping_add(index as u8));
        let mask: [u8; 16] = core::array::from_fn(|index| 0x80_u8.wrapping_add(index as u8));
        let encrypted_key = test_key_record(&sender, &receiver, &key, &mask);
        let mut raw_key = [0xa5; 16];
        assert_eq!(fairplay.decrypt_key(&encrypted_key, &mut raw_key), OK);
        assert_eq!(raw_key, key);

        let mut bad_key = encrypted_key;
        bad_key[36] ^= 1;
        raw_key.fill(0xa5);
        assert_eq!(fairplay.decrypt_key(&bad_key, &mut raw_key), INVALID);
        assert_eq!(raw_key, [0; 16]);

        setup_request[15] ^= 1;
        setup_response.fill(0xa5);
        assert_eq!(fairplay.setup(&setup_request, &mut setup_response), INVALID);
        assert_eq!(setup_response, [0xa5; 142]);
        handshake_response.fill(0xa5);
        assert_eq!(
            fairplay.handshake(&handshake_request, &mut handshake_response),
            INVALID
        );
        assert_eq!(handshake_response, [0xa5; 32]);
    }
}

#[test]
fn invalid_handshake_clears_previous_sender_state_and_key_output() {
    let receiver = [0x31_u8; 128];
    let mut fairplay = FairPlayV2::with_receiver_sap(receiver);
    let mut setup_request = [0_u8; 16];
    record_header(&mut setup_request, 1, 4);
    setup_request[12..16].copy_from_slice(&[2, 0, 0, 0xbb]);
    let mut setup_response = [0; 142];
    assert_eq!(fairplay.setup(&setup_request, &mut setup_response), OK);

    let sender = [0x72_u8; 128];
    let mut handshake_request = [0_u8; 164];
    record_header(&mut handshake_request, 3, 152);
    handshake_request[12..16].copy_from_slice(&[0, 0x8f, 0x1a, 0x9c]);
    handshake_request[16..144].copy_from_slice(&test_encrypt_sap(0, &sender));
    handshake_request[144..164].copy_from_slice(&test_confirmation(&sender, &receiver));
    let mut handshake_response = [0; 32];
    assert_eq!(
        fairplay.handshake(&handshake_request, &mut handshake_response),
        OK
    );

    handshake_request[144] ^= 1;
    assert_eq!(
        fairplay.handshake(&handshake_request, &mut handshake_response),
        INVALID
    );
    let mut raw_key = [0xa5; 16];
    assert_eq!(fairplay.decrypt_key(&[0; 72], &mut raw_key), INVALID);
    assert_eq!(raw_key, [0; 16]);
}
