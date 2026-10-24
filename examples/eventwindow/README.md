# eventwindow

Checkpoint small, delayed job-event exports using event time. Arrival order is
used only for sequence admission; it never supplies a timestamp or duration.

```sh
mkdir -p /tmp/eventwindow-run/fixtures /tmp/eventwindow-run/output
cp examples/eventwindow/grants.template.json /tmp/eventwindow-run/grants.json
cp examples/eventwindow/fixtures/events.ndjson /tmp/eventwindow-run/fixtures/
hirsel run examples/eventwindow/hirsel.json --grants /tmp/eventwindow-run/grants.json -- init 5 2 state.json
cp /tmp/eventwindow-run/output/state.json /tmp/eventwindow-run/fixtures/
hirsel run examples/eventwindow/hirsel.json --grants /tmp/eventwindow-run/grants.json -- apply state.json events.ndjson result.json
```

Grant a read-only directory projected as `input`, a separate read/write directory
projected as `output`, and stderr. See `grants.template.json`; host directories
are relative to that file. Copy a committed result into the next invocation's
immutable input snapshot. Success is silent and exits 0; refusal reports
`eventwindow: <reason>` on stderr and exits 2 (3 if diagnostics are unavailable).
Paths resolve within their respective directory grants. No network, wall clock
or timer is used. An optional Signals grant enables an Interrupt race using the
existing Tasks capability; ordinary operation creates no children or workers.
Execution uses the existing Ordered default and makes no capacity/speedup claim.

Each LF/CRLF line is exactly one of:

```json
{"seq":0,"key":"jobs","at":-1,"value":3}
{"seq":1,"watermark":2}
```

Unknown/missing/duplicate fields, blank lines, invalid UTF-8, fractional or
exponential numbers, duplicate sequences and gaps refuse the entire batch.
`seq` equals the checkpoint's `next_seq`, initially 0. Every accepted record,
including a late event or equal watermark, advances it once. Values are signed
i64 integers; sequence and counts are u64. Ticks and offset are integers in
[-1,000,000,000, 1,000,000,000]; width is 1..3600. Offset is normalized modulo
width. Fixed windows are half open `[start,end)`, with
`start = floor((at-offset)/width)*width+offset`. For width 5/offset 2,
timestamp -1 belongs to [-3,2), and timestamp 2 to [2,7).

Initially there is no watermark and no event is late. Explicit watermarks may
stay equal but cannot decrease. A watermark closes every window with
`end <= watermark`. Later events with `at < watermark` increment `late` without
reopening a window; `at == watermark` is admitted. EOF never flushes a window.
Closed windows accumulate in `(end,start,key UTF-8 byte order)` order. The
combined checkpoint/report retains that history and unfinished windows, with
count/sum and min/max from two executed instances of the generic engine.

The version-1 JSON document is the only restart state. Its strict decoder checks
schema, normalized parameters, sequence accounting, watermark/count consistency,
sorted unique keys/windows, legal intervals, known keys, retained counts and
attainable count/sum/min/max arithmetic. Keys include late-only keys and are
limited over the checkpoint's entire lifetime. Checkpoint numbers are decimal
integer lexemes; JSON whitespace and object member order do not affect adoption.
Encoding has fixed member order, compact UTF-8 and one trailing LF. Complete
streams applied in one batch or contiguous batches produce identical final
checkpoint bytes. A checkpoint is a validated snapshot, not authenticated proof
of a historical stream.

All arithmetic that can affect aggregate state, sequence or endpoints is
checked. Loop/index arithmetic is bounded by admitted collection sizes. A batch
stages its whole output before creating an AtomicFile, then writes and commits
one document. A refusal leaves input and an existing output unchanged; only a
committed output may be used for restart. Ordinary destruction closes files and
abandons uncommitted atomic files on parse, arithmetic, cancellation or write
failure. Publication promises only the shipped AtomicFile+sync contract; there
is no distributed exactly-once or two-file atomicity claim.

Fixed bounds (none are tunable):

| Resource | Limit |
| --- | ---: |
| Raw batch including separators | 32768 bytes |
| Physical line, excluding LF, including CR | 1024 bytes |
| Records per batch | 128 |
| Key UTF-8 length (empty key permitted) | 32 bytes |
| Distinct keys retained over all batches | 16 |
| Live keyed windows | 32 |
| Emitted keyed windows retained | 128 |
| Checkpoint input and staged output | 65536 bytes each |

Admission precedes adding the excess element. The scanner holds one partial
line and a read has at most 256 bytes of lookahead. Checkpoint reads use the
shipped bounded reader plus a one-byte excess probe. JSON DOM bounds are
separate: one 1024-byte event line or one 65536-byte checkpoint source, with
std JSON's 128-container nesting bound. These are logical bounds, not RSS
guarantees. Escaped key emission has a temporary of at most 194 bytes. Output
appends check capacity before extending the staging buffer.

The CLI harness registers separately named semantic, invalid-checkpoint, cap,
fault, registry, Beam-bundle and scanner partitions. A Python oracle uses integer floor division and ordinary
dictionaries. It supplies upstream boundary vectors, delayed input, ties, split
checkpoints, every fixed cap, tampering and checked arithmetic controls. Native
tapes check exact file publication/cleanup and replay with original file
providers absent. Qualification receipts and incomplete cells are external;
see [FRICTION.md](FRICTION.md) for the actual limits and workarounds.

Adapted from Apache Beam `FixedWindows`, `window.py:392-426`, boundary tests
`window_test.py:98-116`, and the AfterWatermark sample/bundle groupings in `trigger_test.py:194-210`, commit
`fadfc789376137afabab0469bf1e43a0f10a6ad0`. Apache-2.0:
[LICENSE.beam](LICENSE.beam), [NOTICE.beam](NOTICE.beam). Integer ticks replace
seconds/duration objects, explicit input watermarks replace runner progress,
and a window has one final pane. Upstream bundle sizes 1,2,3,-3,-2,-1
include reversed arrivals; they are not watermark values. Their finite app
watermarks are explicit 10/20 ticks; upstream labels a/b/c become values 1/2/3.
Sequence admission and canonical atomic
snapshots are local policies. Beam triggers, runner and checkpoint formats are
not ported; no general Beam compatibility is claimed.
