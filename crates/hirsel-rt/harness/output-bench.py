#!/usr/bin/env python3
"""Paired ordinary output CPU-time samples; executable must link production rt."""
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import tempfile

executable = str(Path(sys.argv[1]).resolve())
rows = []
with tempfile.TemporaryDirectory(prefix="hirsel-output-") as work:
    root = Path(work) / "root"
    root.mkdir()
    with open(os.devnull, "wb") as output:
        for round_number in range(5):
            row = {}
            modes = ["c", "ordinary", "tape"]
            if round_number % 2:
                modes.reverse()
            for mode in modes:
                env = {"HIRSEL_RT_ROOT": str(root)}
                if mode == "tape":
                    env["HIRSEL_RT_TAPE"] = str(Path(work) / "tape")
                # time reports peak RSS too; nice is inherited from the command.
                run = subprocess.run(["/usr/bin/time", "-f", "rss_kib=%M", "nice", "-n", "10", executable, mode],
                                     env=env, stdout=output, stderr=subprocess.PIPE, check=True, text=True)
                cpu, rss = run.stderr.strip().splitlines()
                row[mode] = {"cpu_seconds": float(cpu), "rss_kib": int(rss.removeprefix("rss_kib="))}
            row["ordinary_over_c"] = row["ordinary"]["cpu_seconds"] / row["c"]["cpu_seconds"]
            row["tape_over_c"] = row["tape"]["cpu_seconds"] / row["c"]["cpu_seconds"]
            rows.append(row)
report = {"operations": 200000, "bytes_per_operation": 64, "rounds": rows,
          "ordinary_over_c_median": statistics.median(r["ordinary_over_c"] for r in rows),
          "tape_over_c_median": statistics.median(r["tape_over_c"] for r in rows)}
print(json.dumps(report, indent=2))
