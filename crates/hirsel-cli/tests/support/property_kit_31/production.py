#!/usr/bin/env python3
"""Bounded production Ordered cells with a retained application readiness gate."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import selectors
import socket
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("--hirsel", required=True)
parser.add_argument("--root", required=True)
parser.add_argument("--capped", required=True)
parser.add_argument("--skip-build", action="store_true")
args = parser.parse_args()
root = Path(args.root).resolve()
root.mkdir(parents=True, exist_ok=True)
env = os.environ.copy()
env.update(HIRSEL_JOBS="4", HIRSEL_MEMORY_LIMIT="2G")
for option in ["HIRSEL_RT_WORKERS", "HIRSEL_RT_MIGRATE"]:
    env.pop(option, None)
selection = os.environ.get("PROPERTY_KIT_31_PRODUCTION_CELLS")
selected = set(selection.split(",")) if selection else None
receipts = []


def flush(receipt):
    receipts.append(receipt)
    with (root / "receipts.jsonl").open("a") as output:
        output.write(json.dumps(receipt) + "\n")
        output.flush()
        os.fsync(output.fileno())
    print("PROPERTY_KIT_31_CELL " + json.dumps(receipt), file=__import__("sys").stderr, flush=True)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def save(name, command, completed):
    (root / (name + ".argv.json")).write_text(json.dumps(command))
    (root / (name + ".stdout")).write_bytes(completed.stdout)
    (root / (name + ".stderr")).write_bytes(completed.stderr)
    (root / (name + ".exit")).write_text(str(completed.returncode) + "\n")


def run(name, command):
    completed = subprocess.run(command, env=env, capture_output=True)
    save(name, command, completed)
    return completed


if not args.skip_build:
    for backend in ["llvm", "cranelift"]:
        command = [args.hirsel, "build", str(root / "hirsel.json"), "--backend", backend, "-o", str(root / backend)]
        built = subprocess.run(command, env=os.environ, capture_output=True)
        save("build-" + backend, command, built)
        assert built.returncode == 0, built.stderr

records = root / "records"
records.mkdir(exist_ok=True)
output = root / "output"
output.mkdir(exist_ok=True)
workflows = [
    ("success", ["run", "reverse", "0", "2", "4", "64"], b"passed=2 rejected=0 exhausted=0\n", 0),
    ("error", ["run", "error", "0", "2", "4", "64"], b"property_error=90\n", 3),
    ("cancel", ["cancel"], b"canceled; joined\n", 5),
    ("replay", ["replay"], b"replayed category=7 minimum=7\n", 1),
]
count = 0
for backend in ["llvm", "cranelift"]:
    for width in [1, 2, 4]:
        for workflow, words, expected, code in workflows:
            cell = f"{backend}-w{width}-{workflow}"
            if selected is not None and cell not in selected:
                continue
            count += 1
            directory = root / cell
            directory.mkdir()
            control, tape, grants = (directory / name for name in ["control", "recording.tape", "grants.json"])
            for path in output.iterdir():
                path.unlink()
            (records / "failure.json").write_bytes((root / "failure.json").read_bytes())
            app_args = words + ["--ready"]
            grant = {
                "args": app_args,
                "io": {
                    "stdout": "stdout.bytes@1", "stdin": "stdin.bytes@1", "tasks": "tasks.cooperative@1",
                    "dirs": [
                        {"path": "records", "host": str(records), "access": "read"},
                        {"path": "output", "host": str(output), "access": "read_write"},
                    ],
                },
                "placement": {"cores": width, "commit": "shared", "reorder": "off", "control": str(control)},
            }
            grants.write_text(json.dumps(grant))
            command = [args.capped, "--timeout", "60", "prlimit", "--cpu=60:61", str(root / backend),
                       "--hirsel-record", str(tape), "--hirsel-grants", str(grants)] + app_args
            (directory / "argv.json").write_text(json.dumps(command))
            flush({"cell": cell, "status": "STARTED", "argv": command, "grants_sha256": digest(grants), "executable_sha256": digest(root / backend), "manifest_sha256": digest(root / "hirsel.json")})
            process = subprocess.Popen(command, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                with selectors.DefaultSelector() as selector:
                    selector.register(process.stdout, selectors.EVENT_READ)
                    assert selector.select(50), "no ready handshake"
                ready = process.stdout.readline()
                assert ready == b"ready\n", ready
                with socket.socket(socket.AF_UNIX) as connection:
                    connection.settimeout(5)
                    connection.connect(str(control))
                    connection.sendall(b'{"metrics":true}\n')
                    reply = connection.makefile("rb").readline()
                metrics = json.loads(reply)
                (directory / "ready.json").write_bytes(reply)
                if metrics["worker_phase"] != "running" or metrics["width"]["effective"] != width or metrics["width"]["pending"] != 0:
                    flush({"cell": cell, "status": "INCOMPLETE_CAPACITY", "metrics": metrics})
                    raise AssertionError(metrics)
                flush({"cell": cell, "status": "READY", "metrics": metrics})
                process.stdin.write(b"x")
                process.stdin.flush()
                # Keep stdin open and empty while cancel's actual read parks.
                process.wait(timeout=55)
                stdout, stderr = process.stdout.read(), process.stderr.read()
                (directory / "stdout").write_bytes(ready + stdout)
                (directory / "stderr").write_bytes(stderr)
                (directory / "exit").write_text(str(process.returncode))
                assert process.returncode == code and stdout == expected, (cell, process.returncode, stdout, stderr)
                if workflow == "cancel":
                    assert (output / "joined").read_bytes() == b"joined"
                original = digest(tape)
                for path in output.iterdir():
                    path.unlink()
                (records / "failure.json").unlink()
                for target in ["llvm", "cranelift"]:
                    replay = run(cell + "-replay-" + target, [args.capped, "--timeout", "60", "prlimit", "--cpu=60:61",
                                 str(root / target), "--hirsel-replay", str(tape)])
                    assert replay.returncode == code and replay.stdout == ready + expected, (cell, target, replay)
                    assert not (records / "failure.json").exists() and not list(output.iterdir()), "real replay effects"
                assert original == digest(tape)
                flush({"cell": cell, "status": "PASS", "width": metrics["width"], "worker_phase": metrics["worker_phase"],
                       "tape_sha256": original, "argv": command})
            except BaseException as error:
                flush({"cell": cell, "status": "FAILED", "error": str(error)})
                raise
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()
                for stream in [process.stdin, process.stdout, process.stderr]:
                    stream.close()

passed = [row for row in receipts if row["status"] == "PASS"]
assert count > 0 and (selected is None or count == len(selected)), "zero/unknown test selection"
(root / "receipts.json").write_text(json.dumps(receipts, indent=2) + "\n")
print(json.dumps({"passed": len(passed), "selected": count, "production": passed}))
