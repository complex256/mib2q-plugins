import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(__file__))
import add_hook  # noqa: E402

BASE = """start_late_drivers() {
    inetd > /dev/null 2>&1
    echo done
}
"""
WITH_TS = """start_late_drivers() {
    inetd > /dev/null 2>&1
    ## tailscaled autostart. Best effort only: detached, no stdio.
    ( sleep 30; [ -x /mnt/app/root/tailscale/tailscale.sh ] && /mnt/app/root/tailscale/tailscale.sh start ) </dev/null >/dev/null 2>&1 &
    echo done
}
"""


class AddHook(unittest.TestCase):
    def test_after_inetd(self):
        out = add_hook.add(BASE)
        lines = out.splitlines()
        self.assertIn("inetd", lines[1])
        self.assertIn("carmqtt autostart", lines[2])
        self.assertIn(add_hook.MARKER, lines[3])
        self.assertEqual(lines[4].strip(), "echo done")

    def test_after_tailscale(self):
        lines = add_hook.add(WITH_TS).splitlines()
        self.assertIn("tailscale.sh start", lines[3])
        self.assertIn("carmqtt autostart", lines[4])

    def test_idempotent_and_removable(self):
        once = add_hook.add(BASE)
        self.assertEqual(add_hook.add(once), once)
        self.assertEqual(add_hook.remove(once), BASE)

    def test_no_anchor(self):
        with self.assertRaises(ValueError):
            add_hook.add("echo nothing here\n")


if __name__ == "__main__":
    unittest.main()
