# `mirror_buffer.c` port notes

Source: `D:\Save\Uxplay\UxPlay\lib\mirror_buffer.c` and `mirror_buffer.h` (GPL-3.0-or-later headers). The UxPlay reference tree was compiled read-only for the oracle.

`src/lib.rs` owns the same state as the C object: the audio key, derived video AES-CTR stream, a saved partial keystream block, and the number of bytes to consume from the next packet. `init_aes` hashes the source's decimal stream key/IV labels and audio key with SHA-512, resets pending state, and installs the first 16 bytes as the AES-128 key and IV. `decrypt` mutates complete-block bytes in the input and returns a separate output buffer, matching the call shape used by `raop_rtp_mirror.c`.

`reference/run-mirror-buffer-oracle.ps1` compiles the unchanged C source and compares its checked-in 12-row fixture. Rust tests compare both mutated input and output across fragment boundaries and re-keying; Windows GNU `cargo test` (3 tests), Clippy with warnings denied, and formatting pass. Windows GNU linking uses the `-lbcrypt` post-archive argument for the existing OpenSSL dependency.

The C implementation returns from inside the pending-byte loop when a packet is shorter than the pending count. That initializes only the first output byte; the remainder is left uninitialized by its `malloc` caller. The oracle zero-initializes its output buffer to make this undefined region deterministic. Rust returns zero-filled bytes in that region while preserving the initialized first byte and pending-count transition. This unit is staged and is not a generic rust-anything-gated or live-session-integrated port yet.
