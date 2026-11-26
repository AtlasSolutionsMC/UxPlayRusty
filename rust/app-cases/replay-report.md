# App replay

Build: Windows GNU release binary from the empty `rust/app/target/acceptance-release` target, launched with MSYS2 UCRT64 runtime/plugin paths.

Passed: 5/8

Fallback rate: 0.0% (0/8 cases answered by the original)

The three grouped replay reads below report connection resets before receiving a response: `hls-master-missing-current-video`, `play-invalid-body-with-session`, and `server-info`. Each case passes when replayed individually (1/1). The raw C/Rust HTTP response comparison passes all 8/8 cases after normalizing only the Date header.
