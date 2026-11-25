"""Capture BlueZ late-registration and Release callback behavior from UxPlay."""

import contextlib
import importlib.util
import io
import os
import sys
import types
from pathlib import Path


source_path = Path(sys.argv[1]).resolve()


class DBusException(Exception):
    pass


class ServiceObject:
    def __init__(self, _bus=None, path=None):
        self.path = path

    @staticmethod
    def remove_from_connection(_object):
        pass


def method(*_args, **_kwargs):
    return lambda callback: callback


dbus = types.ModuleType("dbus")
dbus.__path__ = []
exceptions = types.ModuleType("dbus.exceptions")
exceptions.DBusException = DBusException
service = types.ModuleType("dbus.service")
service.Object = ServiceObject
service.method = method
mainloop = types.ModuleType("dbus.mainloop")
mainloop.__path__ = []
glib = types.ModuleType("dbus.mainloop.glib")
glib.DBusGMainLoop = lambda **_kwargs: None
dbus.exceptions = exceptions
dbus.service = service
dbus.mainloop = mainloop
mainloop.glib = glib
sys.modules.update(
    {
        "dbus": dbus,
        "dbus.exceptions": exceptions,
        "dbus.service": service,
        "dbus.mainloop": mainloop,
        "dbus.mainloop.glib": glib,
    }
)

module_name = "uxplay_beacon_module_BlueZ"
spec = importlib.util.spec_from_file_location(module_name, source_path)
module = importlib.util.module_from_spec(spec)
with contextlib.redirect_stdout(io.StringIO()):
    spec.loader.exec_module(module)


class Manager:
    def UnregisterAdvertisement(self, _advertisement):
        pass


output = io.StringIO()
with contextlib.redirect_stdout(output):
    module.ad_manager = Manager()
    module.advertised_port = 7100
    module.advertised_address = "192.0.2.8"
    module.airplay_advertisement = object()
    module.beacon_off()
    module.register_ad_cb()
    print(
        "state after late success: "
        f"port={module.advertised_port}, address={module.advertised_address}"
    )

    module.ad_manager = Manager()
    module.advertised_port = 7200
    module.advertised_address = "192.0.2.9"
    module.airplay_advertisement = object()
    module.beacon_off()
    module.register_ad_error_cb(RuntimeError("late registration error"))
    print(
        "state after late error: "
        f"port={module.advertised_port}, address={module.advertised_address}"
    )

    advertisement = module.AirPlay_Service_Discovery_Advertisement.__new__(
        module.AirPlay_Service_Discovery_Advertisement
    )
    advertisement.path = (
        "/org/bluez/airplay_service_discovery_advertisement2"
    )
    original_exit = os._exit

    def capture_exit(code):
        raise SystemExit(code)

    os._exit = capture_exit
    try:
        try:
            advertisement.Release()
        except SystemExit as error:
            print(f"Release exit: {error.code}")
    finally:
        os._exit = original_exit

sys.stdout.write(output.getvalue())
