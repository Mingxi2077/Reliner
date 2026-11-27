import runpy
import sys
import unittest
from pathlib import Path


class CaptureFrrRouteTest(unittest.TestCase):
    def test_preserves_stderr_from_successful_empty_route_dump(self):
        script = Path(__file__).resolve().parents[1] / "examples" / "capture-frr-route.py"
        read_command = runpy.run_path(str(script))["read_command"]
        warning = "Dump was interrupted and may be inconsistent."
        record = read_command([sys.executable, "-c",
                               f"import sys; print('[]'); print({warning!r}, file=sys.stderr)"])
        self.assertEqual(record["exit_code"], 0)
        self.assertEqual(record["stdout"], [])
        self.assertEqual(record["stderr"], warning + "\n")


if __name__ == "__main__":
    unittest.main()
