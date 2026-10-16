#!/usr/bin/env python3
"""Fresh-process paired counters and RSS for the unchanged binarytrees algorithms."""
import json
import os
from pathlib import Path
import random
import shutil
import statistics
import subprocess
import sys
import time

build, rounds, *depths = sys.argv[1:]
build = Path(build)
rounds = int(rounds)
output = Path(os.environ.get("HIRSEL_BENCH_DATA", build / "measurements"))
output.mkdir(parents=True, exist_ok=True)
mimalloc = "/usr/lib/x86_64-linux-gnu/libmimalloc.so.2"
commands = {
    "glibc": [str(build / "bt-malloc")],
    "mimalloc": ["env", f"LD_PRELOAD={mimalloc}", str(build / "bt-malloc")],
    "jemalloc": ["env", "LD_PRELOAD=/usr/lib/x86_64-linux-gnu/libjemalloc.so.2", str(build / "bt-malloc")],
    "mimalloc-48B": ["env", f"LD_PRELOAD={mimalloc}", str(build / "bt-malloc-48")],
    "mimalloc-32B": ["env", f"LD_PRELOAD={mimalloc}", str(build / "bt-malloc-32")],
    "hirsel-walk": [str(build / "bt-hirsel-walk")],
    "hirsel": [str(build / "bt-hirsel")],
}
if os.environ.get("HIRSEL_BENCH_BASELINE"):
    commands["main32"] = [os.environ["HIRSEL_BENCH_BASELINE"]]
variants = os.environ.get("HIRSEL_BENCH_VARIANTS", " ".join(commands)).split()
assert rounds > 0 and all(v in commands for v in variants)
pin = ["taskset", "-c", os.environ["HIRSEL_BENCH_CPU"]] if "HIRSEL_BENCH_CPU" in os.environ else []
events = "cycles:u,instructions:u,L1-dcache-load-misses:u,cpu/event=0x64,umask=0x9/u"
perf = shutil.which("perf") is not None and subprocess.run(
    ["perf", "stat", "-e", events, "true"], capture_output=True
).returncode == 0
keys = ["cycles", "instructions", "l1_misses", "l2_demand_misses", "rss_kib"]
print("binarytrees paired counters; L2 = AMD demand instruction/data misses (event 0x64/0x9)", flush=True)
with (output / "paired.jsonl").open("w") as log:
    for depth in map(int, depths):
        expected = None
        for variant in variants:
            stdout = subprocess.check_output(pin + commands[variant] + [str(depth)])
            (output / f"d{depth}-{variant}.stdout").write_bytes(stdout)
            if expected is None:
                expected = stdout
            assert stdout == expected, (depth, variant, "output mismatch")
        rows = []
        for round in range(rounds):
            maximum_load = float(os.environ.get("HIRSEL_BENCH_MAX_LOAD", "inf"))
            while os.getloadavg()[0] >= maximum_load:
                print("waiting for a quiet measurement round:", subprocess.check_output(["uptime"], text=True).strip(), flush=True)
                time.sleep(30)
            round_load = os.getloadavg()[0]
            print(f"depth {depth} round {round + 1} starts at load {round_load:.2f}", flush=True)
            order = variants.copy()
            random.Random(1313 + depth * 5 + round).shuffle(order)
            for variant in order:
                prefix = output / f"d{depth}-r{round}-{variant}"
                timing = ["/usr/bin/time", "-f", "%M", "-o", str(prefix) + ".rss"]
                counting = ["perf", "stat", "-x,", "-e", events, "-o", str(prefix) + ".perf", "--"] if perf else []
                load_before = Path("/proc/loadavg").read_text().strip()
                start = time.monotonic()
                stdout = subprocess.check_output(pin + counting + timing + commands[variant] + [str(depth)])
                assert stdout == expected, (depth, round, variant, "output mismatch")
                row = {"depth": depth, "round": round, "variant": variant, "order": order,
                       "round_start_load": round_load,
                       "wall": time.monotonic() - start, "rss_kib": int(Path(str(prefix) + ".rss").read_text()),
                       "load_before": load_before, "load": Path("/proc/loadavg").read_text().strip()}
                if perf:
                    for line in Path(str(prefix) + ".perf").read_text().splitlines():
                        cols = line.split(",")
                        if len(cols) < 5 or not cols[0].strip().isdigit():
                            continue
                        if "cycles:u" in line:
                            key = "cycles"
                        elif "instructions:u" in line:
                            key = "instructions"
                        elif "L1-dcache-load-misses" in line:
                            key = "l1_misses"
                        elif "cpu/event=0x64,umask=0x9/" in line:
                            key = "l2_demand_misses"
                        else:
                            continue
                        row[key] = int(cols[0])
                rows.append(row)
                log.write(json.dumps(row) + "\n")
                log.flush()
                print(f"depth {depth} round {round + 1}/{rounds} {variant}: {row.get('cycles', 'unavailable')} cycles", flush=True)
        print(f"\ndepth {depth}; medians of {rounds} rounds\nvariant cycles-G instructions-G L1-M L2-M RSS-MiB", flush=True)
        for variant in variants:
            samples = [r for r in rows if r["variant"] == variant]
            values = [statistics.median(r.get(k, float("nan")) for r in samples) / scale
                      for k, scale in zip(keys, [1e9, 1e9, 1e6, 1e6, 1024])]
            print(variant, " ".join(f"{v:.3f}" for v in values), flush=True)
        if "hirsel" in variants and "main32" in variants and perf:
            pairs = []
            for round in range(rounds):
                pair = {r["variant"]: r for r in rows if r["round"] == round}
                pairs.append(pair["hirsel"]["cycles"] / pair["main32"]["cycles"])
            print("hirsel/main32 paired cycle ratios:", pairs, "median", statistics.median(pairs), flush=True)
