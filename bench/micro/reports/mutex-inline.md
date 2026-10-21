# Inline Mutex payload measurements

Mutex cells now retain their payload as an `Option[T]` image, reusing the
channel image buffer and moving ownership without allocating on take or put.
The last handle closes the cell and drops a present payload through its
layout descriptor, including resource finalizers. Copied handles share an
exact atomic root count.

For the scalar increment loop, allocations per `mutex.with` fall from one
to zero. Executed instructions increase by 11.65%; this is an allocation and
lifetime correction with an instruction regression on this workload.

## Workload and method

[mutex-inline-loop.hir](mutex-inline-loop.hir) performs N increments through
`mutex.with`, then one read through `with`, and prints N. The baseline is
49b74942; the candidate is this implementation on that base. Both use LLVM
21.1.8, O3, whole-program partitioning, regions and reuse enabled, flat walks,
and the default runtime. Measurement binaries separately enable runtime
allocation counters.

`bench/micro/run` compares the default binaries in ten interleaved rounds,
seed 42, pinned to CPU 30. CPU 31's timing lock was occupied. Hardware counters
include total and user instructions and cycles. Every stdout checksum matches;
no samples are excluded. Session: `142b94ec-8047-4da8-bf00-218530de24ce`.

| Median count | Before | After | After / before |
|---|---:|---:|---:|
| Instructions | 342,672,347.5 | 382,585,443 | 1.116476 |
| User instructions | 340,437,050 | 380,331,535.5 | 1.117186 |
| Cycles | 97,127,638 | 120,433,475.5 | 1.239951 |

The within-session task-clock ratio is 1.255681, bootstrap 95% interval
[1.191471, 1.354074]. SMT sibling CPU 14 has median busy share zero for both
participants, with ranges 0–1 before and 0–0.5 after. The instruction counts
are the verdict; the time ratio describes this session only.

## Allocations

The measurement runtime reports the following total block allocations and
matching frees. Subtracting the N=0 count removes setup, output, and the final
read: `(allocations(10000) - allocations(0)) / 10000`.

| Loop iterations | Before allocations / frees | After allocations / frees |
|---|---:|---:|
| 0 | 4 / 4 | 3 / 3 |
| 10,000 | 10,004 / 10,004 | 3 / 3 |
| Allocations per loop call | 1 | 0 |

The runtime test
`mutex_images_move_without_allocations_and_finalize_at_last_handle` also
performs 1,000 take/put pairs without any host allocation, checking buffer
retention independently of these block counters.

## Reproduction

Copy the loop into a standalone package with the usual `std@1` dependency,
and build it at each revision using the release `hirsel-native-build` helper
with `regions-on-whole`. Set `HIRSEL_BENCH_MEASUREMENT=1` for the counter
binary. The helper links harness executables: run them as `program N` with
`HIRSEL_RT_ROOT` naming an empty directory; set `HIRSEL_RT_MEASUREMENT` to a
report outside that directory for counter runs.

Create a participant JSON list with before/after argv arrays ending in
`1000000`, each with the same `HIRSEL_RT_ROOT` environment, then run:

```sh
bench/micro/run participants.json --output measurements.json \
  --reference before --rounds 10 --seed 42 --cpu 30
```

Raw counters, allocation reports, build metadata, source, binaries, and their
participant manifest are archived under
`hirsel-briefs/mutex-inline-measurements-2026-10-21` on the measurement host.
Default binary SHA-256 values:

- Before: `65858930b04211d8ff87fa1dd22a79e9ec2e55db81983783004b3ce0bef19a13`
- After: `1aa214ab109e1dba0e2df0771f0d5013fae4b781f5ae256b27b2d4bb67a9f520`
