"""Capture HCI subprocess-spawn exceptions through the unchanged monitor."""

import importlib
import os
import platform
import psutil
import runpy
import struct
import subprocess
import sys
import tempfile
import time
import traceback
import types
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path


script_path = Path(sys.argv[1]).resolve()
backend_path = script_path.with_name("uxplay_beacon_module_HCI.py")
scenario = sys.argv[2]
failure_call = {
    "reset": 1,
    "parameters": 2,
    "data": 3,
    "enable": 4,
    "stop": 5,
    "interrupt-stop": 5,
    "stop-nonzero": 5,
    "interrupt-stop-nonzero": 5,
}[scenario]
backend_globals = {"__name__": "uxplay_beacon_module_HCI", "__file__": str(backend_path)}
backend_module = types.ModuleType("uxplay_beacon_module_HCI")
original_system = platform.system
platform.system = lambda: "Linux"
try:
    backend_text = backend_path.read_text(encoding="utf-8")
    with redirect_stdout(StringIO()):
        exec(compile(backend_text, str(backend_path), "exec"), backend_globals)
finally:
    platform.system = original_system
backend_module.__dict__.update(backend_globals)

original_check_output = subprocess.check_output
original_run = subprocess.run
original_import = importlib.import_module
original_argv = sys.argv
original_sleep = time.sleep
original_home = os.environ.get("HOME")
run_calls = 0
check_outputs = iter(
    [
        "Devices:\n\thci0\t00:11:22:33:44:55\n",
        "LMP Version: 6.0 (0xE)\n",
    ]
)


def check_output(*_args, **_kwargs):
    return next(check_outputs)


def run(command, *_args, **_kwargs):
    global run_calls
    run_calls += 1
    if scenario in ("stop-nonzero", "interrupt-stop-nonzero") and run_calls == failure_call:
        raise subprocess.CalledProcessError(
            7, command, output="child stdout", stderr="child stderr"
        )
    if run_calls == failure_call:
        raise FileNotFoundError(2, "No such file or directory", command[0])
    return subprocess.CompletedProcess(command, 0, "", "")


def import_module(name, *args, **kwargs):
    if name == "uxplay_beacon_module_HCI":
        return backend_module
    return original_import(name, *args, **kwargs)


def sleep(_seconds):
    if scenario.startswith("interrupt-stop"):
        raise KeyboardInterrupt
    if scenario.startswith("stop") and handoff.exists():
        handoff.unlink()


def filtered_traceback(error, seen=None):
    seen = set() if seen is None else seen
    if id(error) in seen:
        return ""
    seen.add(id(error))
    frames = []
    for frame in traceback.extract_tb(error.__traceback__):
        try:
            filename = Path(frame.filename).resolve()
        except OSError:
            continue
        if filename in (script_path, backend_path):
            frames.append(frame)
    stack = traceback.StackSummary.from_list(frames).format()
    rendered = "Traceback (most recent call last):\n" + "".join(stack)
    if error.__context__ is not None:
        rendered = (
            filtered_traceback(error.__context__, seen)
            + "\nDuring handling of the above exception, another exception occurred:\n\n"
            + rendered
        )
    text = str(error)
    rendered += f"{type(error).__name__}: {text}\n" if text else f"{type(error).__name__}\n"
    return rendered


try:
    with tempfile.TemporaryDirectory(prefix="hci-spawn-traceback-") as directory:
        os.environ["HOME"] = directory
        handoff = Path(directory) / "handoff"
        pid = os.getpid()
        process_name = psutil.Process(pid).name()
        handoff.write_bytes(struct.pack("<HI", 7100, pid) + process_name.encode("utf-8"))
        output = []
        try:
            with open(os.devnull, "w", encoding="utf-8") as null:
                # Keep stdout out of the golden trace; it is independent of the exception stack.
                original_stdout = sys.stdout
                sys.stdout = null
                subprocess.check_output = check_output
                subprocess.run = run
                importlib.import_module = import_module
                time.sleep = sleep
                sys.argv = [
                    str(script_path),
                    "HCI",
                    "--path",
                    str(handoff),
                    "--ipv4",
                    "192.0.2.8",
                    "--advmin",
                    "100",
                    "--advmax",
                    "100",
                ]
                try:
                    runpy.run_path(str(script_path), run_name="__main__")
                except (FileNotFoundError, subprocess.CalledProcessError) as error:
                    output.append(filtered_traceback(error))
                finally:
                    sys.stdout = original_stdout
        finally:
            subprocess.check_output = original_check_output
            subprocess.run = original_run
            importlib.import_module = original_import
            time.sleep = original_sleep
            sys.argv = original_argv
finally:
    if original_home is None:
        os.environ.pop("HOME", None)
    else:
        os.environ["HOME"] = original_home

if not output:
    raise RuntimeError(f"scenario {scenario!r} did not reach the expected spawn failure")
print(output[0], end="")
