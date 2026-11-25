use std::ptr;

use fairplay_playfair::port::FairPlay;

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn main() {
    for mode in 0..4_u8 {
        let mut fairplay = FairPlay::new(ptr::null_mut());
        let mut setup_request = [0_u8; 16];
        for (index, byte) in setup_request.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(13).wrapping_add(mode);
        }
        setup_request[4] = 3;
        setup_request[14] = mode;
        let mut setup_response = [0_u8; 142];
        let setup_status = fairplay.setup(&setup_request, &mut setup_response);
        println!("setup-{mode} {setup_status} {}", hex(&setup_response));

        let mut handshake_request = [0_u8; 164];
        for (index, byte) in handshake_request.iter_mut().enumerate() {
            *byte = (index as u8)
                .wrapping_mul(17)
                .wrapping_add(mode.wrapping_mul(3));
        }
        handshake_request[4] = 3;
        handshake_request[12] = mode;
        let mut handshake_response = [0_u8; 32];
        let handshake_status = fairplay.handshake(&handshake_request, &mut handshake_response);
        println!(
            "handshake-{mode} {handshake_status} {}",
            hex(&handshake_response)
        );

        let mut encrypted_key = [0_u8; 72];
        for (index, byte) in encrypted_key.iter_mut().enumerate() {
            *byte = (index as u8)
                .wrapping_mul(29)
                .wrapping_add(mode.wrapping_mul(7));
        }
        let mut output_key = [0_u8; 16];
        let decrypt_status = fairplay.decrypt(&encrypted_key, &mut output_key);
        println!("decrypt-{mode} {decrypt_status} {}", hex(&output_key));
    }

    let mut fairplay = FairPlay::new(ptr::null_mut());
    let mut invalid_setup = [0_u8; 16];
    invalid_setup[4] = 0x7f;
    let mut setup_response = [0xa5_u8; 142];
    let setup_status = fairplay.setup(&invalid_setup, &mut setup_response);
    println!("invalid-setup {setup_status} {}", hex(&setup_response));
    let mut key_output = [0xa5_u8; 16];
    let decrypt_status = fairplay.decrypt(&[0; 72], &mut key_output);
    println!(
        "decrypt-after-invalid-setup {decrypt_status} {}",
        hex(&key_output)
    );

    let mut fairplay = FairPlay::new(ptr::null_mut());
    let mut invalid_v2_setup = [0_u8; 16];
    invalid_v2_setup[4] = 2;
    let mut setup_response = [0xa5_u8; 142];
    let setup_status = fairplay.setup(&invalid_v2_setup, &mut setup_response);
    println!("invalid-v2-setup {setup_status} {}", hex(&setup_response));
    let mut key_output = [0xa5_u8; 16];
    let decrypt_status = fairplay.decrypt(&[0; 72], &mut key_output);
    println!(
        "decrypt-after-invalid-v2-setup {decrypt_status} {}",
        hex(&key_output)
    );

    let mut valid_handshake = [0_u8; 164];
    valid_handshake[4] = 3;
    valid_handshake[12] = 0;
    let mut response = [0_u8; 32];
    let handshake_status = fairplay.handshake(&valid_handshake, &mut response);
    println!("state-handshake {handshake_status} {}", hex(&response));
    let mut invalid_handshake = [0_u8; 164];
    invalid_handshake[4] = 4;
    let invalid_status = fairplay.handshake(&invalid_handshake, &mut response);
    println!("invalid-handshake {invalid_status} {}", hex(&response));
    let decrypt_status = fairplay.decrypt(&[0; 72], &mut key_output);
    println!(
        "decrypt-after-invalid-handshake {decrypt_status} {}",
        hex(&key_output)
    );
}
