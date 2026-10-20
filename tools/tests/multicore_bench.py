"""Run the B1 acceptance harness regressions as part of the gate's tool checks."""
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
suite = unittest.defaultTestLoader.discover(str(ROOT/'bench/multicore/tests'))
if __name__ == '__main__':
    sys.exit(not unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful())
