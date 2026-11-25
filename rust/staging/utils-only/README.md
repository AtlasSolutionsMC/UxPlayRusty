# Isolated C recorder source

`utils.c` is copied byte-for-byte from `D:\Save\Uxplay\UxPlay\lib\utils.c` solely so rust-anything's per-unit recorder compiles this file without walking the entire CMake project. The rust-anything bridge manifest points to this staging folder; do not include this C copy in the Rust application. The final Rust unit is based on the original source and recorded outputs.
