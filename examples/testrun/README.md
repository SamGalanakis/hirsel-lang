Run a test program with a deadline, streaming its output:

```sh
cat > /tmp/testrun-grants.json <<'JSON'
{"io": {"stdout": "stdout.bytes@1", "tasks": "tasks.cooperative@1",
        "monotonic_clock": "clock.monotonic.ns@1", "dirs": [],
        "process": {"provider": "process.spawn@1", "rules": [{"program": "/usr/bin/make"}]}},
 "args": ["/usr/bin/make", "60000"]}
JSON
hirsel run examples/testrun/hirsel.json --grants /tmp/testrun-grants.json
hirsel run --native examples/testrun/hirsel.json --grants /tmp/testrun-grants.json
```

The test is spawned with `proc.spawn` (granted by its exact path, with no environment and
`/` as its working directory) and its standard output is read with `child.read_stdout`,
which parks the reading task until bytes arrive, so each line prints as the test writes
it. The main task is the watchdog: it reads the monotonic clock every 10 ms and at the
deadline cancels the reading task, whose read returns `Canceled`; returning drops the
`Child`, which kills the test. A test that ends first is reaped with `child.wait` and its
exit status decides `PASS` or `FAIL`.
