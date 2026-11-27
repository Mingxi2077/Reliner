"""Capture one installed and then withdrawn IPv4 static route on an isolated FRR router.

Run inside the router namespace as a user with read access to Zebra and iproute2.
The operator installs and withdraws the route through FRR in another terminal.
For stack-static-lab.json, on r1 with the transit interface named eth1:

    python3 capture-frr-route.py stack-static-lab.json r1 203.0.113.0/24 eth1 \
        --origin lab-run-42 > r1-capture.json

Only successful reads are written. Prompts go to stderr. The output contains
the router's static IPv4 RIB; keep it with the authorized lab observations.
"""

import argparse
import ipaddress
import json
import os
import subprocess
import sys
import time


def read_command(argv, *, structured=True):
    result = subprocess.run(argv, capture_output=True, text=True, timeout=15)
    sys.stderr.write(result.stderr)
    result.check_returncode()
    return {"argv": argv, "exit_code": result.returncode, "stderr": result.stderr,
            "stdout": json.loads(result.stdout) if structured else result.stdout}


def sample(stage, prefix):
    print(f"Press Enter when {prefix} has been {stage} through FRR.", file=sys.stderr, flush=True)
    input()
    return {
        "monotonic_ns": time.monotonic_ns(),
        "rib": read_command(["vtysh", "-d", "zebra", "-c", "show ip route static json"]),
        "kernel": read_command(["ip", "-N", "-j", "-4", "route", "show", "table", "main",
                                "exact", prefix]),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("native")
    parser.add_argument("node_id")
    parser.add_argument("prefix", type=ipaddress.IPv4Network)
    parser.add_argument("ifname")
    parser.add_argument("--origin", required=True, help="identifier of the external lab run")
    args = parser.parse_args()
    with open(args.native, encoding="utf-8-sig") as source:
        native = json.load(source)
    prefix = str(args.prefix)
    capture = {
        "capture_version": 1,
        "native_id": native["id"],
        "native_revision": native["revision"],
        "native_format_version": native["format_version"],
        "origin": args.origin,
        "network_namespace": os.readlink("/proc/self/ns/net"),
        "node_id": args.node_id,
        "prefix": prefix,
        "ifname": args.ifname,
        "version": read_command(["vtysh", "-d", "zebra", "-c", "show version"], structured=False),
        "interface": read_command(["ip", "-j", "-4", "addr", "show", "dev", args.ifname]),
        "installed": sample("installed", prefix),
        "withdrawn": sample("withdrawn", prefix),
    }
    print(json.dumps(capture, indent=2))


if __name__ == "__main__":
    main()
