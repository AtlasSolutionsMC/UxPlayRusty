"""Capture main's rejection after BleuIO falls back and probes a fake dongle."""

import importlib
import runpy
import sys
import types
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path


script_path = Path(sys.argv[1]).resolve()
backend_path = script_path.with_name("uxplay_beacon_module_BleuIO.py")
backend_globals = {"__name__": "uxplay_beacon_module_BleuIO", "__file__": str(backend_path)}
backend_module = types.ModuleType("uxplay_beacon_module_BleuIO")
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
serial_module.SerialException = type("SerialException", (Exception,), {})
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
original_import = importlib.import_module
original_argv = sys.argv
sys.modules.update(
    {
        "serial": serial_module,
        "serial.tools": tools_module,
        "serial.tools.list_ports": list_ports_module,
    }
)


def import_module(name, *args, **kwargs):
    if name == "uxplay_beacon_module_BleuIO":
        return backend_module
    return original_import(name, *args, **kwargs)


output = StringIO()
exit_code = None
try:
    with redirect_stdout(output):
        backend_text = backend_path.read_text(encoding="utf-8")
        exec(compile(backend_text, str(backend_path), "exec"), backend_globals)
        backend_globals["time"] = types.SimpleNamespace(sleep=lambda _seconds: None)
        backend_module.__dict__.update(backend_globals)
        importlib.import_module = import_module
        sys.argv = [
            str(script_path),
            "BleuIO",
            "--ipv4",
            "192.0.2.8",
            "--device",
            "COM_MISSING",
        ]
        runpy.run_path(str(script_path), run_name="__main__")
except SystemExit as error:
    exit_code = error.code
finally:
    importlib.import_module = original_import
    sys.argv = original_argv
    for name, module in original_modules.items():
        if module is None:
            sys.modules.pop(name, None)
        else:
            sys.modules[name] = module

print(output.getvalue(), end="")
print(f"exit={exit_code}")
print(f"events={serial_events!r}")
