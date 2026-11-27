"""Capture pyserial readline's fresh timeout for every one-byte read."""

import runpy
import serial
import sys
import time
from types import SimpleNamespace


class DelayedSerial(serial.Serial):
    def __init__(self):
        self._timeout = 0.03
        self.is_open = True
        self.response = bytearray(b"OK\n")
        self.written = b""
        self.read_calls = 0

    @property
    def in_waiting(self):
        return len(self.response)

    def write(self, data):
        self.written += data
        return len(data)

    def read(self, size=1):
        assert self.timeout == 0.03
        time.sleep(0.02)
        self.read_calls += 1
        result = bytes(self.response[:size])
        del self.response[:size]
        return result

    def close(self):
        self.is_open = False


source = runpy.run_path(sys.argv[1])
# Keep the source helper's 100 ms settle delay out of this serial timing check.
source["time"] = SimpleNamespace(sleep=lambda _seconds: None)
source["os"].linesep = "\n"
serial_port = DelayedSerial()
started = time.perf_counter()
response = source["send_at_command"](serial_port, "AT")
elapsed = time.perf_counter() - started

print(f"response={response!r}")
print(f"written={serial_port.written!r}")
print(f"read_calls={serial_port.read_calls}")
print(f"readline_elapsed_gt_timeout={elapsed > serial_port.timeout}")
