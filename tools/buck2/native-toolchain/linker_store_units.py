#!/usr/bin/env python3
"""Run the capture archive controls without unrelated native CLI setup."""
from pathlib import Path
import os
import subprocess
import sys

llvm, harness = (Path(arg).resolve() for arg in sys.argv[1:])
env = {**os.environ, "LD_LIBRARY_PATH": str(llvm / "lib")}
sys.exit(subprocess.run([str(harness), "native::object_queries::capture::tests::",
    "--test-threads=1", "--nocapture"], env=env).returncode)
