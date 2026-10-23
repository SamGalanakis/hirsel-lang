# Deadline readiness control

Build `main.hir` with the production LLVM runtime. The same executable performs
1,024 atomic stores with either no scope deadline or a deadline 60 seconds in the
future. No file, network, child task or output is needed.

```sh
hirsel build --backend llvm bench/multicore/deadline-readiness/main.hir -o /tmp/deadline-after
bench/multicore/deadline-readiness/run --binary /tmp/deadline-after \
  --before /tmp/deadline-before --cpus 0,8,16,24 --output /tmp/deadline-session
```

The runner keeps three seeded interleaved rounds at requested widths 1/2/4,
whole-process cycles/instructions/context switches, argv, binary hashes and
stdout/stderr. It checks successful empty output and reports RED above 1.2×
deadline width-1 cycles. The deadline-free twin identifies unrelated width
overhead. Compare before/after only within one session. Instructions are the
verdict; shared-host cycles are diagnostic. Requested width alone does not
certify physical worker placement.

Use ordinary scoped CPUs for development. After the host owner provides a G7
timing window, select those CPUs and add `--timing-slice`; systemd-run then wraps
each measured process directly with a 2 GiB / 60 second cap. The runner reserves
no CPUs. Do not nest tools/capped inside that timing scope.
