"""Capture BleuIO's fallback when --device does not match an attached dongle."""

import runpy
import sys
import types
from contextlib import redirect_stdout
from io import StringIO


serial_events = []


class FakeSerial:
    def __init__(self, device, baudrate, timeout):
        serial_events.append(("open", device, baudrate, timeout))
        self.response = bytearray()

    def __enter__(self):
        return self

    def __exit__(self, _kind, _value, _traceback):
        self.close()

    @property
    def in_waiting(self):
        return len(self.response)

    def write(self, command):
        serial_events.append(("write", command))
        self.response.extend(b"OK\n")
        return len(command)

    def readline(self):
        line = self.response.index(b"\n") + 1
        result = bytes(self.response[:line])
        del self.response[:line]
        return result

    def close(self):
        serial_events.append(("close",))


serial_module = types.ModuleType("serial")
serial_module.__path__ = []
serial_module.Serial = FakeSerial
tools_module = types.ModuleType("serial.tools")
tools_module.__path__ = []
list_ports_module = types.ModuleType("serial.tools.list_ports")
list_ports_module.comports = lambda: [
    types.SimpleNamespace(
        device="COM_TEST",
        vid=0x2DCF,
        description="Fake BleuIO",
        hwid="USB VID:PID=2DCF:6001",
    )
]
tools_module.list_ports = list_ports_module
serial_module.tools = tools_module
names = ("serial", "serial.tools", "serial.tools.list_ports")
original_modules = {name: sys.modules.get(name) for name in names}
sys.modules.update(
    {
        "serial": serial_module,
        "serial.tools": tools_module,
        "serial.tools.list_ports": list_ports_module,
    }
)

output = StringIO()
try:
    with redirect_stdout(output):
        source = runpy.run_path(sys.argv[1])
        source["time"] = types.SimpleNamespace(sleep=lambda _seconds: None)
        selected = source["find_device"]("COM_MISSING")
finally:
    for name, module in original_modules.items():
        if module is None:
            sys.modules.pop(name, None)
        else:
            sys.modules[name] = module

print(output.getvalue(), end="")
print(f"selected={selected!r}")
print(f"events={serial_events!r}")
