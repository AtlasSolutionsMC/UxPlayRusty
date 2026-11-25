"""Capture the source script's local/global advertised_port behavior through runpy."""

import runpy
import sys

source_script = sys.argv[1]
namespace = runpy.run_path(source_script)
source_globals = namespace["start_beacon"].__globals__

source_globals.update(
    port=7100,
    ipv4_str="192.0.2.8",
    advmin=100,
    advmax=100,
    index=0,
    setup_beacon=lambda *args: True,
    beacon_on=lambda: 7100,
    beacon_off=lambda: None,
    advertised_port=None,
    beacon_is_running=False,
)
source_globals["start_beacon"]()

print(f"beacon_is_running={source_globals['beacon_is_running']}")
print(f"module_global_advertised_port={source_globals['advertised_port']!r}")
print(f"changed_port_matches={source_globals['check_port'](7101)}")
