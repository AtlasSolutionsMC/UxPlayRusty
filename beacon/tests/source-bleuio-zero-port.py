"""Capture BleuIO's zero-port assertion through fake serial discovery and the real monitor."""

import importlib
import os
import platform
import psutil
import struct
import sys
import tempfile
import time
import traceback
import types
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path


script_path = Path(sys.argv[1]).resolve()
backend_path = script_path.with_name("uxplay_beacon_module_BleuIO.py")
backend_globals = {"__name__": "uxplay_beacon_module_BleuIO", "__file__": str(backend_path)}
source_output = StringIO()
serial_events = []


class FakeSerialException(Exception):
    pass


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
        serial_events.append(("write", command.decode("ascii")))
        self.response.extend(b"OK\r\n")
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
serial_module.SerialException = FakeSerialException
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
original_modules = {
    name: sys.modules.get(name)
    for name in ("serial", "serial.tools", "serial.tools.list_ports")
}
sys.modules.update(
    {
        "serial": serial_module,
        "serial.tools": tools_module,
        "serial.tools.list_ports": list_ports_module,
    }
)

backend_module = types.ModuleType("uxplay_beacon_module_BleuIO")
original_import = importlib.import_module
original_sleep = time.sleep
original_argv = sys.argv
original_system = platform.system
temporary_directory = tempfile.TemporaryDirectory(prefix="uxplay-bleuio-zero-")
handoff = Path(temporary_directory.name) / "handoff"
pid = os.getpid()
process_name = psutil.Process(pid).name()
handoff.write_bytes(struct.pack("<HI", 0, pid) + process_name.encode("utf-8"))
sys.argv = [str(script_path), "BleuIO", "--path", str(handoff), "--ipv4", "192.0.2.8"]


def import_module(name, *args, **kwargs):
    if name == "uxplay_beacon_module_BleuIO":
        return backend_module
    return original_import(name, *args, **kwargs)


importlib.import_module = import_module
time.sleep = lambda _seconds: None
platform.system = lambda: "Windows"
failed = False
try:
    with redirect_stdout(source_output):
        backend_text = backend_path.read_text(encoding="utf-8")
        exec(compile(backend_text, str(backend_path), "exec"), backend_globals)
        backend_module.__dict__.update(backend_globals)
        source_text = script_path.read_text(encoding="utf-8")
        exec(
            compile(source_text, str(script_path), "exec"),
            {"__name__": "__main__", "__file__": str(script_path)},
        )
except AssertionError as error:
    traceback_root = error.__traceback__
    while traceback_root and Path(traceback_root.tb_frame.f_code.co_filename).resolve() != script_path:
        traceback_root = traceback_root.tb_next
    if traceback_root is None:
        raise
    capture = StringIO()
    traceback.print_exception(type(error), error, traceback_root, file=capture)
    print(capture.getvalue(), end="")
    expected_serial_events = [
        ("open", "COM_TEST", 115200, 1),
        ("write", "AT\r\n"),
        ("close",),
        ("close",),
    ]
    if serial_events != expected_serial_events:
        raise RuntimeError(f"unexpected fake serial probe/cleanup sequence: {serial_events!r}")
    failed = True
finally:
    importlib.import_module = original_import
    time.sleep = original_sleep
    platform.system = original_system
    sys.argv = original_argv
    for name, previous in original_modules.items():
        if previous is None:
            sys.modules.pop(name, None)
        else:
            sys.modules[name] = previous
    temporary_directory.cleanup()

if not failed:
    raise RuntimeError("source did not raise its port assertion")
