"""Capture the WinRT callback-then-stop failure through the Python monitor path."""

import importlib
import os
import platform
import psutil
import re
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
mode = sys.argv[2] if len(sys.argv) > 2 else "handoff-removal"
backend_path = script_path.with_name("uxplay_beacon_module_winrt.py")
source_output = StringIO()
backend_globals = {"__name__": "uxplay_beacon_module_winrt", "__file__": str(backend_path)}

for name in (
    "winrt",
    "winrt.windows",
    "winrt.windows.foundation",
    "winrt.windows.foundation.collections",
    "winrt.windows.devices",
    "winrt.windows.devices.bluetooth",
    "winrt.windows.devices.bluetooth.advertisement",
    "winrt.windows.storage",
    "winrt.windows.storage.streams",
):
    module = types.ModuleType(name)
    if name in {"winrt", "winrt.windows", "winrt.windows.foundation", "winrt.windows.devices",
                "winrt.windows.devices.bluetooth", "winrt.windows.storage"}:
        module.__path__ = []
    sys.modules[name] = module

configured = {"port": None, "sleeps": 0}
backend_module = types.ModuleType("uxplay_beacon_module_winrt")


class DummyPublisher:
    def stop(self):
        return None


def setup_beacon(_address, port, _advmin, _advmax, _index):
    configured["port"] = port
    backend_globals["publisher"] = DummyPublisher()


def beacon_on():
    return None if configured["port"] == 1 else configured["port"]


backend_module.setup_beacon = setup_beacon
backend_module.beacon_on = beacon_on
original_import = importlib.import_module
original_sleep = time.sleep
original_system = platform.system
original_argv = sys.argv
original_modules = {name: sys.modules.get(name) for name in (
    "winrt",
    "winrt.windows",
    "winrt.windows.foundation",
    "winrt.windows.foundation.collections",
    "winrt.windows.devices",
    "winrt.windows.devices.bluetooth",
    "winrt.windows.devices.bluetooth.advertisement",
    "winrt.windows.storage",
    "winrt.windows.storage.streams",
)}

temporary_directory = tempfile.TemporaryDirectory(prefix="uxplay-winrt-stop-")
handoff = Path(temporary_directory.name) / "handoff"
pid = os.getpid()
process_name = psutil.Process(pid).name()
handoff.write_bytes(struct.pack("<HI", 7100, pid) + process_name.encode("utf-8"))
sys.argv = [str(script_path), "--path", str(handoff), "--ipv4", "192.0.2.8"]


def import_module(name, *args, **kwargs):
    if name == "uxplay_beacon_module_winrt":
        return backend_module
    return original_import(name, *args, **kwargs)


def sleep_after_first_tick(_seconds):
    configured["sleeps"] += 1
    if configured["sleeps"] == 1:
        if mode == "handoff-removal":
            handoff.unlink()
        backend_globals["on_status_changed"](
            None, types.SimpleNamespace(status=types.SimpleNamespace(name="STOPPED"))
        )
        if mode == "interrupt":
            raise KeyboardInterrupt


importlib.import_module = import_module
time.sleep = sleep_after_first_tick
platform.system = lambda: "Windows"
failed = False
try:
    with redirect_stdout(source_output):
        backend_text = backend_path.read_text(encoding="utf-8")
        exec(compile(backend_text, str(backend_path), "exec"), backend_globals)
        backend_module.beacon_off = backend_globals["beacon_off"]
        source_text = script_path.read_text(encoding="utf-8")
        exec(compile(source_text, str(script_path), "exec"), {"__name__": "__main__", "__file__": str(script_path)})
except AttributeError as error:
    traceback_root = error.__traceback__
    while traceback_root and Path(traceback_root.tb_frame.f_code.co_filename).resolve() != script_path:
        traceback_root = traceback_root.tb_next
    if traceback_root is None:
        raise
    capture = StringIO()
    traceback.print_exception(type(error), error, traceback_root, file=capture)
    formatted = re.sub(
        r'  File ".*source-winrt-stopped-stop\.py", line \d+, in sleep_after_first_tick\n'
        r"    raise KeyboardInterrupt\n",
        "",
        capture.getvalue(),
    )
    print(formatted, end="")
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
    raise RuntimeError("source did not raise after STOPPED callback and handoff removal")
