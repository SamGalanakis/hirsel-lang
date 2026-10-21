# Idle RSS attribution

The reproducible trigger is `rss --binary /absolute/tasks-hirsel --output
/tmp/tasks-rss --limit 1.3`. It starts an empty service and a service over the
benchmark's 10,000-task snapshot, reads the selective readiness page, renders
the full list and creates a task. Raw mapping and allocator snapshots belong
to diagnostic runs; production performance comes from `bench`.

## Causes

The original production binary uses 6.24 MiB after cold readiness and 16.00 MiB
after seeded readiness, against Rust one-worker's 3.90 and 7.20 MiB. Full list
rendering leaves 31.64 MiB resident. These are resident snapshots, not peaks.

The separate measurement companion reports 50,026 live blocks and 4,417,904
bytes of live capacity after seeded readiness. Its region counters are all
zero. Changing the region pool's 8 MiB cap would not address this workload.

Two non-live components account for most of the excess:

- glibc arenas retain transient large buffers. A separate diagnostic fixing
  `MALLOC_MMAP_THRESHOLD_=131072` reduces seeded readiness from 16.32 to
  12.20 MiB and full-list RSS from 31.96 to 15.82 MiB. glibc increases its
  adaptive mmap threshold when freeing large mapped allocations, which lets
  later transient buffers enter arenas. Its free-byte counters describe
  reserved allocator storage, including nonresident pages; they are not RSS.
- The final request can free empty slabs after the last worker collection.
  At the next host wait the original heap has 2,031,616 idle slab bytes after
  readiness and 4,849,664 after listing, despite the 1 MiB cap. Their resident
  pages are 1.18 and 4.32 MiB in the companion. The cap needs enforcement at
  the scheduler's no-runnable-task boundary.

The allocator now owns Linux mappings for large blocks with a backing request
of at least 128 KiB and alignment at most 4 KiB. Matching release unmaps them
directly. Other alignments, other platforms and the system-allocator control
keep their existing paths. The immutable size/alignment prefix determines
the release path, so there is no ABI or allocation-header change. The idle
wait boundary also trims empty slabs to the existing cap when its idle count
exceeds that cap. The cheap count check avoids repeatedly scanning the live
board at unrelated host waits. Neither fix moves
or discards a live object, changes the board, or changes glibc's process-wide
policy.

This follows glibc's page-rounded large-allocation mmap/munmap structure
(`malloc/malloc.c:2254-2285,4395-4412`, revision `04e750e7`). Hirsel deliberately
uses a fixed threshold for its own blocks, preserving the host allocator's
policy for everything else. The residency regression test uses an isolated
process with glibc's mmap threshold raised to 32 MiB, frees a touched 512 KiB
buffer while another remains live, and checks the released pages with
`mincore`. It fails with every released page resident before the change.

## Attribution boundaries

`smaps` accounts separately for executable text, other executable mappings,
libraries, main stack, stacks identified by blocked threads' stack pointers,
guard-adjacent anonymous mappings and the private heap. Guard adjacency alone
does not identify a stack: glibc arenas can also border inaccessible reserves.
An 8 MiB
fiber stack is a lazy virtual reservation, not 8 MiB of resident memory.
`smaps_rollup` is retained beside each mapping snapshot.

The companion calls `hirsel_rt_v2_ctx_stats` at idle and reports backing spans.
The sampler counts present pagemap bits for each span and assigns shared
boundary pages once. Per-class rows retain live/free slots and capacities;
occupied slabs include headers, cursors, live blocks and free space. Capacity
bytes are not added to resident bytes. Runtime struct/descriptor-array sizes
are reported separately as metadata lower bounds. Other live host metadata,
allocator headers and arena free space remain in the residual private-heap
mapping, rather than being called live board storage.

Rust gets the same smaps/rollup snapshots and the same optional glibc arena
probe. It has no Hirsel slab or region counters. The preload and instrumented
companion are never used in the performance comparison.

## Final resident snapshots

Final production diagnostic, CPU30 (memory only, no timing claim), MiB:

| Idle boundary | Before Hirsel | Final Hirsel | Rust one worker | Final / Rust |
|---|---:|---:|---:|---:|
| Empty readiness | 6.24 | 6.51 | 3.86 | 1.69 |
| 10,000-task readiness | 16.00 | 13.12 | 7.18 | 1.83 |
| Full list, connection closed | 31.64 | 13.44 | 8.07 | 1.66 |
| Create after list, connection closed | 31.03 | 14.85 | 11.53 | 1.29 |

These before/after diagnostic runs establish memory retention, not speed.
The final production executable SHA256 is
`cdb6e9f0496bcb9b0fcdc4b6788ee3b19526268124f489d90dfeb1f8e859362f`;
Rust is `04ed8e623253b25ee1ccce4086868a771f777b420544d2ad75ce2800719d458b`.
Create meets 1.3x in this production run. Cold and list do not; their remaining
components follow. The instrumented companion/preload adds code and changes
allocator history, so its absolute totals must not replace production RSS.

The separate companion's additive resident breakdown, MiB (rounding applies):

| Component | Hirsel cold | Hirsel list | Hirsel create | Rust cold | Rust list | Rust create |
|---|---:|---:|---:|---:|---:|---:|
| Binary executable text | 4.094 | 4.121 | 4.121 | 1.105 | 1.105 | 1.105 |
| Other binary mappings | 0.625 | 0.598 | 0.598 | 0.312 | 0.309 | 0.309 |
| Libraries, including diagnostic preload | 1.613 | 1.613 | 1.613 | 2.262 | 2.340 | 2.340 |
| Main and identified thread stacks | 0.074 | 0.074 | 0.074 | 0.059 | 0.062 | 0.062 |
| Occupied slabs, including headers/free slots | 0.082 | 3.234 | 3.234 | n/a | n/a | n/a |
| Entirely idle slabs | 0.137 | 0.562 | 0.562 | n/a | n/a | n/a |
| Runtime large blocks | 0.035 | 0.953 | 0.953 | n/a | n/a | n/a |
| Runtime region spans | 0 | 0 | 0 | n/a | n/a | n/a |
| Other anonymous/heap, including unidentified stacks | 0.387 | 2.895 | 4.312 | 0.176 | 4.441 | 7.832 |
| Kernel special mappings | 0.004 | 0.004 | 0.004 | 0.004 | 0.004 | 0.004 |
| Total | 7.051 | 14.055 | 15.473 | 3.918 | 8.262 | 11.652 |

The seeded board uses 50,026 runtime blocks, 4,417,904 logical live bytes.
Slab payload capacity contains 2,565,728 live and 1,603,008 free bytes after
listing; free capacity shares pages with live objects and cannot simply be
unmapped. The dominant classes (capacity/alignment) are:

| Class | Live blocks | Live capacity bytes | Free capacity bytes | Resident bytes |
|---|---:|---:|---:|---:|
| 32/8 | 30,007 | 960,224 | 38,368 | 1,290,240 |
| 48/8 | 10,006 | 480,288 | 147,552 | 741,376 |
| 112/8 | 10,000 | 1,120,000 | 1,456 | 1,245,184 |
| 512/8 | 0 | 0 | 444,416 | 458,752 |

All four instrumented idle snapshots have exactly 1,048,576 bytes of idle slab
backing, within the existing cap. Heap, layout-pointer array, TaskCtx, Worker
and RunShared metadata sizes are respectively 3,592, 4,392, 824, 80 and 464
bytes: a 9,352-byte lower bound contained in the anonymous residual, not an
additional RSS row. Slab headers/slot overhead are included in slab residency.

After listing, glibc reports 37,756,928 arena bytes: 4,939,984 allocated and
32,816,944 free, plus 135,168 malloc-mapped bytes. After creating it reports
4,942,304 allocated and 32,814,624 free. These allocated bytes include
libc-backed runtime slabs; adding them to runtime capacities would double count.
The runtime's direct large mappings are outside mallinfo2. Rust's corresponding
arena counters are retained by the same diagnostic, with no runtime slab rows.
Rust list has 4,329,472 arena bytes (2,543,984 allocated, 1,785,488 free);
create has 8,982,528 (2,546,272 allocated, 6,436,256 free). Both also report
1,839,104 malloc-mapped bytes.

A separate `RSS_MALLOC_TRIM=1` companion run reduces Hirsel's list residual
from 2.895 to 0.719 MiB and create residual from 4.312 to 2.277 MiB. Rust's
post-create total also falls from 11.652 to 7.344 MiB. This identifies a large
libc-reclaimable component in both services; it is not a production change or
a precise live-object decomposition. The remaining anonymous residual includes
malloc page fragmentation/caches and host metadata outside runtime spans.

The smallest remaining triggers are the empty readiness request (resident text)
and one full-list request over the seeded board (partially occupied slabs and
host allocator retention). File-backed code accounts for most of the cold gap:
production text is 3.992 versus 1.105 MiB. A start-path symbol ordering change
is deferred: this change has no profile proving which reorder would reduce
resident windows without perturbing other workloads. No region-cap or stack-
reservation reduction is justified by these zero-region/small-stack readings.

## Performance tradeoff and verification

**The change does not meet the no-regression-above-2% goal.** The lower RSS
trades away list throughput and increases instructions for several operations.
This limitation is intentional and disclosed; there is no overall performance
parity claim.

Session `2026-10-21T01:58:58Z`, seed 20261020, ten fresh-process rounds for all
27 participant/cell combinations (270 samples, zero failed cells), server
CPU31/client CPU7, perf cycles and instructions, open-loop rate 16, three
seconds per load cell. Before is
`9d0b4bfa692b0efb09191c21303140c81b862e24e25531642934693a509d28a4`.
Run `bench --skip-build --samples-only --binary /absolute/final
--baseline-binary /absolute/before --kinds hirsel,rust-one-worker --rates 16
--seconds 3` to repeat the same comparison. All ratios below are final/before
within this session; intervals are paired bootstrap 95% CIs.

| Cell | Instructions ratio [95% CI] | Completed req/s ratio | Idle RSS ratio | Load RSS ratio |
|---|---:|---:|---:|---:|
| Cold | 0.999 [0.993, 1.008] | n/a | 1.001 | n/a |
| Restart | 1.010 [1.005, 1.010] | n/a | 0.824 | n/a |
| Create/16 | 1.127 [1.124, 1.129] | 1.000 | 0.822 | 0.797 |
| List/16 | 1.140 [1.128, 1.155] | 0.766 | 0.827 | 0.446 |
| Search/16 | 1.013 [1.012, 1.014] | 1.000 | 0.826 | 0.834 |
| Update/16 | 1.129 [1.127, 1.250] | 1.000 | 0.824 | 0.803 |
| SSE/1 connection | 1.050 [1.049, 1.056] | 0.953 | 0.825 | 0.770 |
| SSE/100 connections | 1.002 [1.002, 1.002] | 0.996 | 0.825 | 0.817 |
| SSE/1000 connections | 1.000 [1.000, 1.000] | 1.012 | 0.823 | 0.912 |

List completes 11.96 versus 15.62 requests/s (23.4% lower). SSE/1's reported
rate is 4.7% lower; its connection setup counters rise 5.0%. Create and update
still complete the offered rate, which does not establish equal capacity:
their instruction costs rise 12.7% and 12.9%. Direct mapping/release and
reclaiming the last request's empty slabs add work. An initial unconditional
idle trim had substantially higher search costs; the shipped idle-count guard
avoids that repeated scan. No result from that interrupted development session
is used in this final table.

Validation: runtime integration tests pass 304 with one ignored (305 total,
one more than the original 304 total); seven descriptor tests pass. The new
large-buffer test was run red on the original allocator, then green. All 102
MEM run cases pass the native smoke's four configurations, real Io replay,
Cranelift and runner/executable checks, split into eight disjoint selections.
`MEM-12-flat-list-builder-run` is excluded as required by the repository's host
rules. The service verifier passes 11 differential HTTP/HTML, four corrupt/save,
one SIGTERM and six provider checks (22). The companion's four idle-cap checks
pass. Rust formatting, Python compilation and diff whitespace checks pass.
