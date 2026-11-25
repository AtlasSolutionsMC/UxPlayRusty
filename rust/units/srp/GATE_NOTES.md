# SRP behavior-gate evidence

`tests/run_c_diff.ps1` builds this Rust C ABI library, compiles the unchanged source `lib/srp.c` into the C differential executable with reference symbol names, then compares both implementations in one process using the system OpenSSL.

Coverage includes all five hash algorithms, both original and RFC 5054 encodings on a custom group, the standard 2048-bit group, generated Apple-variant registration output shape, deterministic server B, deterministic verifier B, username, session key and its length accessors, wrong-proof rejection, successful proof/HAMK, and invalid A values 0 and N.

The generic rust-anything recorder cannot generate useful JSON scalar cases for this pointer-heavy C ABI with opaque verifier state. Its `gate` unsafe/manifest checks pass; fmt, clippy, cargo test, and build are reported as failed because that tool process selects the MSVC Rust toolchain, which has no `rustfmt`, `clippy`, or `link.exe` installed. The same checks pass with the installed `stable-x86_64-pc-windows-gnu` toolchain as shown in `CHECKPOINTS.md`. The generic behavior check remains incomplete because it has no cases; the custom differential harness is the unit behavior proof. A backend failure policy difference is recorded in `BEHAVIOR.md`. Do not mark app/session integration complete: no pairing handler was changed or wired to this crate.

For Windows GNU consumers, `build.rs` appends `-lbcrypt` after the OpenSSL archives because OpenSSL 4.1's Windows random provider calls `BCryptGenRandom`.
