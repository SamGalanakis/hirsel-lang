# metricfold

Summarize a small job-run export as checked integer counters and latency
histograms. For example, a build agent can compare successful build jobs with
failed test jobs without retaining every measurement or every queue name.

From the repository root, with a built `hirsel` on PATH:

```sh
hirsel check examples/metricfold/hirsel.json
hirsel run examples/metricfold/hirsel.json --grants examples/metricfold/grants.json
hirsel build examples/metricfold/hirsel.json --backend llvm -o /tmp/metricfold
/tmp/metricfold --hirsel-grants examples/metricfold/grants.json jobs.ndjson
hirsel build examples/metricfold/hirsel.json --backend cranelift -o /tmp/metricfold-cl
/tmp/metricfold-cl --hirsel-grants examples/metricfold/grants.json jobs.ndjson
hirsel run examples/metricfold/hirsel.json --grants examples/metricfold/grants.json -- --help
hirsel run examples/metricfold/hirsel.json --grants examples/metricfold/grants.json -- invalid.ndjson
```

The grants template grants read access only to `examples/metricfold/fixtures`,
projected as `input`, plus stdout and stderr. Host paths are relative to the
grants template file; see the [shared grants-base recipe](../../docs/native-verification.md#resolve-directory-hosts-beside-the-grants-file)
for launch refusal context and a bad/correct template pair. To process another export, copy the template, set its
single `host` directory, and supply a relative filename. No process, socket,
clock, network, stdin or write grant is needed. `--help` needs only stdout.

Each physical line must be one UTF-8 JSON object with exactly these
four members (member order is immaterial):

```json
{"instrument":"jobs","unit":"job","labels":{"queue":"build","status":"ok"},"value":1}
{"instrument":"duration_ms","unit":"ms","labels":{"queue":"build","status":"ok"},"value":3}
```

The declared instruments are `jobs` (Counter, unit `job`) and `duration_ms`
(Histogram, unit `ms`, inclusive upper bounds `[1,3,6]`). Measurements must be
nonnegative integer JSON numbers fitting u64; fractions and exponents are
rejected. Negative values, including `-0`, are rejected through the metric type
check. This application explicitly chooses milliseconds; convert source
measurements before supplying them. No duration is inferred from event order.
A different unit for a declared name is a descriptor conflict.

`labels` contains zero to two text values named `queue` and `status`, each at
most 64 UTF-8 bytes. Unknown labels are rejected before the library's allowlist
filter, so an accidental high-cardinality field is visible. Duplicate JSON
keys, blank lines and malformed records are invalid. LF and CRLF work; a final
line without newline is processed. Line size includes a CR before LF, excludes
LF, and is checked before CR stripping.

Limits are fixed: 16,384 raw input bytes, 1,024 bytes per physical line, 64
records, two descriptors, two normal label series per instrument, two labels per
series, and four histogram buckets. Reads contain at most 256 bytes; the scanner
retains at most one bounded partial line. Input limits include separators and
are enforced before feeding the next segment. Reads may look ahead by at most
256 bytes; lookahead is bounded and is never accumulated as a whole file.
Records are validated before updating the registry. The current parsed JSON DOM
is bounded by the line cap and std's existing nesting policy. These are logical
collection/work bounds, not an RSS guarantee.

The first two distinct canonical label sets per instrument retain normal
series slots. Later unseen label sets contribute to `overflow`, which retains
counts, sums and buckets but no labels. Later observations of retained keys
still update their normal series. The report always spells out
`overflow_policy: "aggregate_unseen_without_labels"`. A histogram's final bucket
counts values greater than 6; it is distinct from the overflow label aggregate.
For values 1 through 10 the four counts are `[1,2,3,4]` and sum is 55.
All count/sum/bucket arithmetic uses the shipped checked integer reducer.

Success writes one compact JSON object plus LF to stdout, with empty stderr.
Instruments appear in declaration order, series in first-seen order, and label
keys in byte order. Output includes every instrument and its overflow aggregate,
including zero aggregates. Equal input gives byte-identical output. This is a
local snapshot, with no timestamp, exporter or telemetry protocol claim. It does
not use the offline evidence/checkpoint completeness protocol.

The first invalid record stops processing; no success snapshot is printed.
The internal typed `model.InvalidInput { line, reason }` becomes one JSON error
on stderr. `invalid.ndjson` produces:

```json
{"schema":1,"error":{"line":2,"kind":"invalid_value"}}
```

Input errors exit 2; line numbers start at 1, with 0 for usage, grant and open
errors. Success and help exit 0. Missing stdout/stderr or a failed diagnostic
write exits 3; an actual stdout write failure uses the ordinary typed
`AppError.Output` termination (exit 1). Returning an exit code runs normal file
cleanup. There are no child processes or socket paths to clean up.

The focused CLI harness embeds both sources, both checked-in NDJSON fixtures,
its verifier and the cap runner as explicit build inputs. It checks 44 workflows
through interpreter, LLVM and Cranelift, using an independent Python integer
accumulator and byte-exact stdout/stderr/status assertions. It checks fixture
hashes and directory membership, outside-grant access, and private temporary
cleanup. A separate pure model control runs with the existing interpreter and
checked-H adapters, including unchanged state after rejected measurements.
See [FRICTION.md](FRICTION.md) for retained authoring attempts and scope.

Reference: OpenTelemetry Rust SDK
[`histogram.rs:120-159,317-342`](https://github.com/open-telemetry/opentelemetry-rust/blob/fb6b53566815062f6be838d49cbea77019669d6f/opentelemetry-sdk/src/metrics/internal/histogram.rs#L317)
(Apache-2.0; [license](fixtures/LICENSE)). Only registry usage and the inclusive
bucket vector are adapted. The shipped `std@1/metrics` implements aggregation;
checked u64 integers, deterministic sequential execution, fixed declarations and
its existing overflow-series policy replace floating point and concurrent SDK
machinery. There is no SDK or protocol port.
