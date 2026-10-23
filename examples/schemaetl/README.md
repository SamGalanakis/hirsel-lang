# schemaetl

Run a bounded schema-checked CSV/JSONL pipeline. `check` validates the explicit
manifest and pipeline; `explain` prints its file/field/credit contract; `run`
validates rows, transforms them, and publishes data plus a row-issue summary.

## Try it

From the repository root, build matching tools:

```sh
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 build //:hirsel-cli-bin-hirsel //:hirsel-std-image //tools/buck2/native-toolchain:llvm //tools/buck2/native-toolchain:kit //:hirsel-rt-archives --show-output
```

Set HIRSEL, HIRSEL_IMAGE, LLVM_SYS_231_PREFIX, HIRSEL_LINK_KIT and
HIRSEL_RUNTIME_ARCHIVES to the respective printed output paths. Put the LLVM
lib directory on LD_LIBRARY_PATH if your host requires it. Copy the fixtures
into an input directory; create an empty output directory. Copy grants.json
outside this repository and replace its two absolute directory placeholders.
Only explicitly granted directories are accessible. Tapes must stay outside both.

```sh
"$HIRSEL" run examples/schemaetl/hirsel.json --grants /absolute/grants.json -- check manifest.json pipeline.json
"$HIRSEL" run examples/schemaetl/hirsel.json --grants /absolute/grants.json -- explain manifest.json pipeline.json
HIRSEL_MEMORY_LIMIT=2G HIRSEL_JOBS=4 tools/capped --timeout 60 prlimit --cpu=60:61 \
  "$HIRSEL" run examples/schemaetl/hirsel.json --grants /absolute/grants.json \
  --record /absolute/etl.tape -- run manifest.json pipeline.json
"$HIRSEL" build examples/schemaetl/hirsel.json --backend llvm -o /absolute/etl-llvm
```

For standalone native launch, observer options precede grants; app arguments follow:

```sh
HIRSEL_MEMORY_LIMIT=2G HIRSEL_JOBS=4 tools/capped --timeout 60 prlimit --cpu=60:61 \
  /absolute/etl-llvm --hirsel-record /absolute/native.tape \
  --hirsel-grants /absolute/grants.json run manifest.json pipeline.json
/absolute/etl-llvm --hirsel-replay /absolute/native.tape
"$HIRSEL" run examples/schemaetl/hirsel.json --replay /absolute/native.tape
```

Use `--backend cranelift` for the other backend. Replay needs the same source
revision and immutable tape; it performs no application filesystem effects.
Production Ordered width comes from grants.placement.cores, independently of the
fixed HIRSEL_JOBS=4 wrapper/compiler allowance.

The shipped mixed fixture writes exactly these data.jsonl lines:

```jsonl
{"group":"b","amount":3,"active":true}
{"group":"a","amount":7,"active":true}
{"group":"b","amount":-1,"active":true}
```

summary.json counts five rows: three accepted, one filtered, one rejected. The
issue is file0, row3, field amount, code type, stage wire. Sorted group sums are
a=7 and b=2. Stdout is `complete` followed by LF. Files and JSON keys retain the
specified order; no clocks or hash-map iteration choose output order.

## Documents and failure contract

Manifest: `{"inputs":[{"path":"a.csv","format":"csv"},...]}`. Only csv and
jsonl formats are accepted. Paths are explicit relative file paths. Duplicate
paths, unknown config keys, duplicate schema/output names and invalid types fail.

Pipeline fields have name, type (string/integer/bool), required, and optional
`default`, `as` and `project`. `as` changes the output name; `project:false`
omits a field after validation/filtering/aggregation. Missing optional fields
without defaults become null. A present null is a wrong type. Defaults must
match the declared type; they also apply to missing required fields. CSV empty
cells are present empty strings, not missing values. Integers follow i64.parse;
booleans are exactly true/false. JSON numbers must be signed integers.

`filters:[{"field":"active","equals":true}]` uses typed equality; all filters
must accept a row. Optional `group:{"key":"team","sum":"amount"}` requires a
string key and integer summand, and checks i64 overflow before aggregate mutation.
Missing optional grouping values and overflow become transform row issues.
`policy` is fail-fast or report-and-skip. `jobs` is a logical credit count 1..4,
independent of physical executor width.

CSV requires a nonempty unique header; unknown or missing required headers are
source errors. Width mismatches are row issues. Malformed UTF8/CSV/JSONL aborts
rather than silently skipping a corrupt stream. JSONL blank lines are malformed.
Report mode counts every rejected row, retaining at most 32 issues in manifest/row
order. Fail-fast cancels and joins outstanding tasks before returning an error.
Expected application/config/provider/resource errors exit1; launcher language
failures/tooling refusals retain their separate 2/3 classes. check prints valid;
explain prints its bounded contract. Neither reads datasets or publishes files.

## Bounds and publication

8 input files; 65536 bytes/file; 1024 rows total (including filtered/rejected);
16 fields/row; 4096 record bytes (CSV uses the shipped normalized-CRLF metric,
JSONL counts raw bytes excluding LF); 64 groups; 32 retained issues; 4 credits;
262144 data output bytes. Configuration documents are also limited to 65536 bytes.
A bounded file reader probes one extra byte to distinguish exact cap from excess.
A shared admission counter rejects excess rows before retention. Std parser
scratch is bounded by the admitted file/record; app maps are checked before growth.

The sink releases a credit only after consuming that file in manifest order.
Thus active plus completed buffered batches never exceed jobs. Scoped CSV fields
are copied before advancement; tasks take owned plans and input descriptors and
return Shared batches. Flow owns each decoded-row transform and output-record
encoding; Boundary composes scalar, object and record schema dictionaries.

Run requires a fresh destination. Each output is separately published through an
AtomicFile: data.jsonl, summary.json, then completion.json. The final manifest is
the completion marker. These are three publications, not a multi-file transaction;
a later error can leave earlier data/summary files behind. Completion does not
promise crash durability. The stdout notification follows publication, so a stdout
failure can accompany an already complete dataset. Retry into a fresh directory.

## Verification and references

The focused harness is schema_etl_30 in hirsel-cli-test-cli-shard-1. Explicit
runner filters use `-- --env SCHEMA_ETL_30_CASE=...`; outer shell variables are
not forwarded by Buck. The Python csv/json oracle supplies full expected bytes.
Each reference fixture has a frozen 8M-step bound; Stops produce no completed
observation and are reported INCOMPLETE even when a tooling-disposition test passes.
See FRICTION.md for exact completed/incomplete engine and replay identities.

Prior art: Go ebe2feef encoding/csv/reader.go:197-248 and reader_test.go:19-145
(BSD-3-Clause), Tokio ce685ee sync/mpsc/bounded.rs:760-830 and
tests/sync_mpsc.rs:48-130 (MIT). REFERENCES.md retains licenses. Complete-record
pulling and reserve/send/release structure are adapted through shipped std
csv/json/schema/schema_checked/stream, with no copied codec. Manifest-window
credits replace channel permits, structured Hirsel tasks replace ambient tasks,
and malformed streams abort. CSV parser policy remains shipped std policy.

Application source: main65, plan299, decode208, validate244, stages165, sink248
lines; 1229 total. Sources are explicitly assigned to six manifest modules.

Verification disposition: 68 scripted reference/checked-H semantic passes and
70 host-native passes; 8 reference boundary cells remain Steps-incomplete.
Seventy workflow tapes give 264 four-engine replay passes and 16 incomplete
reference replay cells whose precise tooling outcome is not exposed by the replay
API (the original simulated runs separately report Steps). Twelve readiness-qualified Ordered success/error launches
at widths 1/2/4 have exact event agreement and another 48 four-engine replay
passes. Full native scripted/cancellation qualification is INCOMPLETE on
native-harness-gaps-30. These are provider-specific results, not an all-engine
completion claim. The two retained-tape inspection tests are explicitly ignored
by default; running them requires the private evidence root through the runner.
