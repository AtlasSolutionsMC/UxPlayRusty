"""Capture HCI interval conversion at Rust's signed-64-bit boundaries from Python."""

import platform
import runpy
import sys
from contextlib import redirect_stdout
from io import StringIO


original_system = platform.system
platform.system = lambda: "Linux"
try:
    with redirect_stdout(StringIO()):
        source = runpy.run_path(sys.argv[1])
finally:
    platform.system = original_system

commands = []
source["setup_beacon"].__globals__["le_cmd"] = (
    lambda command, args: commands.append((command, args))
)

for interval in (2**63 - 1, -(2**63)):
    commands.clear()
    source["setup_beacon"]("127.0.0.1", 7100, interval, interval, None)
    print(f"{interval}: {' '.join(commands[0][1][:4])}")
