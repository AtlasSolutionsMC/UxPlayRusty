# RAOP buffer C/Rust differential

Run from PowerShell with:

```powershell
& .\rust\record-shims\raop_buffer_oracle.ps1
```

The harness compiles the unchanged `lib/raop_buffer.c` and `lib/byteutils.c` from the UxPlay
reference, then compares the C trace, the Rust queue trace, and
`raop_buffer_oracle.expected.txt`.

Coverage includes invalid and header-only datagrams, duplicate and late packets, resend-range
selection, waiting and no-resend dequeue behavior, sequence wrap at 65535, flushing at the 256
packet window boundary, the `use_seqnum == 0` fallback, and the full-window missing-head case.
All accepted packets carry the exact four-byte cleartext marker, so AES decryption must not run;
the harness asserts zero decrypt calls.

The stubs are limited to the unrelated AES-CBC functions and no-op logger callback. The AES stub
counts any decrypt call. The preinclude skips `raop_rtp.h`, `compat.h`, and `stream.h` declaration
surfaces unused by this leaf and supplies the source packet-length constant; `global.h`,
`raop_buffer.h`, `byteutils.c`, and the actual queue source remain the reference definitions.
`byteutils.c` links against `ws2_32` for its byte-order functions.

This is a queue-state differential, not a complete `raop_buffer.c` gate: encrypted-payload AES
behavior, logging, packet arrival times, and live RTP/session integration are outside this trace.
