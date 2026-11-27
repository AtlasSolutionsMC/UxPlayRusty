"""Capture BleuIO's caught serial-open exceptions from the unchanged module."""

import runpy
import sys
import types
from contextlib import redirect_stdout
from io import StringIO


module_path = sys.argv[1]
scenario = sys.argv[2]
events = []


class FakeSerialException(Exception):
    pass


class FakeSerial:
    def __init__(self, device, baudrate, timeout):
        events.append(("open", device, baudrate, timeout))
        raise FakeSerialException("mock permission denied")


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
module_names = ("serial", "serial.tools", "serial.tools.list_ports")
original_modules = {name: sys.modules.get(name) for name in module_names}
sys.modules.update(
    {
        "serial": serial_module,
        "serial.tools": tools_module,
        "serial.tools.list_ports": list_ports_module,
    }
)

output = StringIO()
result = None
exit_code = None
try:
    with redirect_stdout(output):
        source = runpy.run_path(module_path)
        module_globals = source["find_device"].__globals__
        if scenario == "probe":
            try:
                result = source["find_device"]("COM_TEST")
            except SystemExit as error:
                exit_code = error.code
        else:
            module_globals["serial_port"] = "COM_TEST"
            source["setup_beacon"]("192.0.2.8", 7100, 100, 100, None)
            if scenario == "start":
                result = source["beacon_on"]()
            elif scenario == "stop":
                result = source["beacon_off"]()
            else:
                raise ValueError(f"unknown scenario: {scenario}")
finally:
    for name, module in original_modules.items():
        if module is None:
            sys.modules.pop(name, None)
        else:
            sys.modules[name] = module

print(output.getvalue(), end="")
print(f"result={result!r} exit={exit_code!r}")
print(
    "state port={!r} address={!r}".format(
        module_globals["advertised_port"], module_globals["advertised_address"]
    )
)
print(f"events={events!r}")
