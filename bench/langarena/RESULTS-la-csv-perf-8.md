# CSV::Parse — borrowed record cursor

Current-main baseline: `58ddcd91`, LLVM native release, x86-64. Rust anchor:
LangArena `3c9f84ddaeec834c0631747cb789e8cfd602f993`, unmodified `csv_parse.rs`.
Full `run.js`: 100,000 rows, 70 measured iterations plus 14 warmup iterations;
checksum **1071523081** in every successful full-size capture.

Three seeded, interleaved instruction rounds on CPU 7, seed 20261022, through
`bench/langarena/compare.py`'s `Session.sample`. Every PMU event reports 100%
running coverage. These are instruction/RSS results; no wall-clock timing claim
or exclusive timing-lock wait. The raw logs retain sibling activity and times.

| Full-process metric | Before | After | Rust |
|---|---:|---:|---:|
| Median instructions | 193,197,021,144 | 56,242,465,236 | 15,288,000,701 |
| Median user instructions | 189,971,400,027 | 55,387,043,190 | 15,254,611,782 |
| Median peak RSS, KiB | 86,968 | 18,888 | 13,312 |
| glibc allocation calls, separate capture | 25,203,618 | 1,278 | 3,934 |
| glibc requested bytes, separate capture | 3,801,934,955 | 20,125,423 | 546,783,600 |

**H/R instructions: 12.637 → 3.679** (user-only: 12.453 → 3.631), a 70.9%
instruction reduction. RSS H/R: 6.533 → 1.419. glibc counts include malloc,
calloc and realloc throughout startup, input generation, warmup and measurement;
they measure system allocator traffic, not pooled Hirsel object allocations.

The initial current-main profile (`perf record -e instructions:u`, no lost
samples) attributes 17.04% to `csv.normalized`, 10.15% to `csv_step`, 7.41% to
`feed`, 11.27% to runtime allocation, 5.87% to sequence growth, and 8.31% to
`float_decimal.parse`. Parsing already calls Eisel-Lemire. No alternate decimal
algorithm or approximate conversion was introduced.

The cursor alone, before shared float changes, executes 64,591,263,397
instructions with 1,374 glibc allocation calls. Its profile identifies String
boundary conversion (`slice_text` + `get_field`, 14.28%) as the next removable
cost. The final candidate feeds `get_field_bytes` directly to `f64.parse_bytes`.
A separate control with the same coordinated numeric sources but the String
field consumer executes 59,637,705,723 instructions; the CSV byte-view change
accounts for a further 5.69% reduction from that control.
The final frozen numeric sources include the coordinated JSON lane's
`float_decimal.parse_bytes`, public f32/f64 byte wrappers, and the existing
integer leading-zero primitive in Lemire normalization. Their owner is the
JSON lane; this CSV change does not edit shared float sources.

Final profile: `slice_record` 29.91%, `float_decimal.parse_bytes` 19.73%,
`get_field_bytes` 8.89%, Lemire `compute` 5.29%. The remaining gap includes scalar
CSV scanning versus Rust's simd_csv reader and decimal/field dispatch costs.

## Reference and deviations

Ported csv-core 0.1.13, rust-csv commit
`05612e87e6e92910d40d7214b70952b8434fef9b`,
`csv-core/src/reader.rs:622-700` (record boundaries/output ends), `980-1068`
(field/quote transitions), `1254-1272` (ordinary-byte scan), and boundary/quoted
vectors around `1495-1612`. License and notice are retained.

1. Preserve std.csv's established strict Go dialect: reject bare quotes and
   text after closing quotes; normalize CRLF in quoted fields; retain bare CR
   except at EOF; preserve BOM, exact first-error positions, explicit width and
   normalized record bounds. csv-core's permissive grammar, BOM stripping,
   standalone-CR terminators, comments and configurable escape dialects are not
   substituted for that public contract.
2. Express the four reachable strict states with branches and an ordinary-byte
   run scan rather than csv-core's generated transition/equivalence tables.
   The unused permissive/configurable states are absent; strict errors and
   normalized byte accounting are explicit transitions.
3. Unchanged fields are spans of borrowed UTF-8 input. Unlike csv-core's caller
   output copying, only escaped/normalized fields share a decoded record buffer.
   `get_field` and `get_field_bytes` borrow their current-record storage.
4. Reuse growable field-span storage; allocate one owned escape buffer per
   affected record instead of csv-core's caller-provided reusable output slice.
   Existing Hirsel String/List conversion consumes owned storage; this avoids
   a new runtime primitive or ABI change. There is no allocation per field.
5. The additive cursor accepts a complete String with known EOF, so it has no
   `InputEmpty`/`OutputFull` results or chunk continuation. Existing incremental
   `Reader`, `feed`, `read_record`, and scoped streaming APIs remain available.
6. The benchmark previously cloned its source and built every row before
   constructing points. Upstream borrows input and consumes records individually.
   The port now follows that shape and retains the same points vector and sum
   order. Data generation and exact checksums are preserved.

## Verification and artifacts

Before implementation, the new API fixture fails with unresolved names. After:

- 18 CSV/importer corpus programs pass; no failing or unimplemented selections.
- Two focused driver tests pass: 156,248 short-input/record-limit comparisons
  against the strict parser, and native allocation invariants. Across 256 rows,
  one-field input allocates one runtime block; six-field input allocates two.
  Both have zero deep copies, COW detaches, copied bytes, and live bytes at exit.
- The exact registered fixture `conformance/SEM/SEM-3-csv-slice-run.hir` passes
  native smoke (11 runner executions, four configurations, real-I/O replay) and
  Cranelift parity (1/1 selected); the interpreter fixture also passes.
- Full-size task checksums pass on LLVM release and Cranelift.

Frozen binaries, compiler, runtime archives, source snapshots and hashes,
grants/config, perf records, raw per-round counters, allocation shim and logs
are retained under `/workspace/code/hirsel-briefs/la-csv-perf-8-artifacts/`.
`csv-before` is the baseline; `csv-candidate-v2` isolates the first CSV change;
`csv-byte-v2` is the final candidate with the four coordinated numeric source
files at JSON commit `63958dce`; `evidence/paired-final/samples.json` holds all
nine counted samples. Failed setup/profile/selection attempts are preserved
and excluded from successful results. Wall-clock ratios remain a follow-up
requiring the host's exclusive timing slot and ten interleaved rounds.
