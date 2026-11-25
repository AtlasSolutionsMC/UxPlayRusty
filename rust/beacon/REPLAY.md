# Beacon source replay

The Windows CLI/config replay compares the original Python beacon and Rust binary on exit code, stdout, and stderr. It includes help, missing `--file`, invalid CLI IPv4, valid config (including the source's `finally: exit(err)` behavior), invalid IPv4, unknown config entry, empty config, comment-only config, and a UTF-8 Arabic-Indic config value that exercises locale decoding before digit validation.

Run after building with:

```powershell
$env:UXPLAY_REFERENCE_ROOT = 'D:\Save\Uxplay\UxPlay'
$env:UXPLAY_PYTHON = 'C:\path\to\python.exe'
pwsh -File rust/beacon/tests/replay-reference.ps1
```

Recorded on 2026-11-22 with Python 3.13.17 and Windows GNU Rust: all 10 cases matched exit code and both output streams byte-for-byte, including the no-device BleuIO diagnostic. Python reports `locale.getencoding() == "cp1252"`, matching the Windows ANSI code page read from the system registry. The `unicode_decimal` case verifies that UTF-8 digits are decoded through the active ANSI code page before `.isdigit()` validation. The Rust decoder uses strict code-page decoding and rejects the undefined CP-1252 byte slots. The reference tree was not modified. This replay does not start a BLE adapter; BlueZ, HCI, WinRT and serial-radio behavior require platform hardware validation.
