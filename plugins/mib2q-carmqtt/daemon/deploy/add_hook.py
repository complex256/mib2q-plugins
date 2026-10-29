#!/usr/bin/env python3
"""Add or remove the carmqtt autostart line in the unit's startup.sh.

usage: add_hook.py add|remove IN OUT

The hook goes right after the tailscale hook if present, otherwise right
after the `inetd` line in start_late_drivers(). Both are best effort:
detached, no stdio, so a missing or failing carmqtt never affects boot.
"""
import sys

MARKER = "/mnt/app/root/carmqtt/carmqtt.sh"
HOOK = [
    "    ## carmqtt autostart. Best effort only: detached, no stdio.\n",
    "    ( sleep 45; [ -x /mnt/app/root/carmqtt/carmqtt.sh ] && /mnt/app/root/carmqtt/carmqtt.sh start ) </dev/null >/dev/null 2>&1 &\n",
]


def add(text: str) -> str:
    if MARKER in text:
        return text
    lines = text.splitlines(keepends=True)
    anchor = None
    for i, line in enumerate(lines):
        if "/mnt/app/root/tailscale/tailscale.sh start" in line:
            anchor = i
    if anchor is None:
        for i, line in enumerate(lines):
            if line.strip().startswith("inetd") and "/dev/null" in line:
                anchor = i
                break
    if anchor is None:
        raise ValueError("no tailscale hook or inetd line to anchor on")
    return "".join(lines[: anchor + 1] + HOOK + lines[anchor + 1 :])


def remove(text: str) -> str:
    return "".join(l for l in text.splitlines(keepends=True) if l not in HOOK)


def main() -> int:
    if len(sys.argv) != 4 or sys.argv[1] not in ("add", "remove"):
        print(__doc__, file=sys.stderr)
        return 2
    with open(sys.argv[2]) as f:
        text = f.read()
    out = add(text) if sys.argv[1] == "add" else remove(text)
    with open(sys.argv[3], "w") as f:
        f.write(out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
