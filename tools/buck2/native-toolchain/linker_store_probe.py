#!/usr/bin/env python3
"""Exercise stored capture/replay with the actual packaged linker on the pool."""
from pathlib import Path
import os
import subprocess
import sys

linker, llvm, harness = (Path(arg).resolve() for arg in sys.argv[1:])
env = {**os.environ, "HIRSEL_CAPTURE_LINKER": str(linker),
       "HIRSEL_CAPTURE_LLVM": str(llvm), "LD_LIBRARY_PATH": str(llvm / "lib")}
sys.exit(subprocess.run([str(harness), "--ignored", "--exact",
    "native::object_queries::capture::tests::packaged_capture_replays_changed_inputs_without_originals",
    "--test-threads=1", "--nocapture"], env=env).returncode)
