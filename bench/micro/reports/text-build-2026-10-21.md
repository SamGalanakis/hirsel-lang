# Text and HTML builders, 2026-10-21

The baseline is `49b74942`. Instruction counts decide the result. The final
comparison runs the same 10k-task list cell and 1M-line print program, before,
after, and Rust, in one seeded, shuffled ten-round CPU31 session. Servers stay
on CPU31 and the open-loop client on CPU7. Raw results are retained beside this
report. Timing ratios apply only within that session.

## Implementation

F-strings reserve the UTF-8 literal lower bound in their existing parser
desugaring. The checker selects a private `TextFinish` operation only when all
holes have primitive formatting. It freezes the existing list with the same
LIR instructions as `bytes.from_list`, retaining ownership without validating
UTF-8 again. Custom `@format` functions still finish with checked UTF-8, since
they can append arbitrary bytes. No TIR/LIR construct or runtime ABI changes.

The word-sized decimal path keeps the existing digit-pair/four-digit algorithm
but no longer widens u64/i64/u8 output into the u128 dispatcher. i64 minimum's
magnitude is computed with unsigned wrapping arithmetic. Wider integer paths
retain their existing behavior.

The tasks renderer appends markup, integer ids and escaped fields into the
caller buffer. Joined tags and nested JSON/HTML attributes stream directly;
the latter retains the exact old JSON-then-HTML spellings. The task list
reserves fixed-markup capacity for visible rows. The HTTP page path consumes
`bytes.from_list(take out)`, removing the final page-to-unique-Bytes graph copy.
The public String-returning page wrapper remains available to its callers.

## Prior art and deviations

- Rust `19833af7224424e8b53e288097d56b6af43d5bcf`,
  `library/alloc/src/fmt.rs:650-662`, and `library/core/src/fmt/mod.rs:754-806`:
  one reserved String and `fmt::Write`. Hirsel reserves a literal lower bound
  instead of Rust's doubled estimate, and keeps its ordinary source formatter
  calls rather than introducing a formatting IR.
- The same Rust revision, `library/alloc/src/string.rs:1038-1040`:
  `String::from_utf8_unchecked`. Hirsel exposes no unchecked public conversion;
  the compiler-private operation is available only to its proven f-strings.
  The interpreter still validates when constructing its host Rust String.
- itoa `1577ed901354d0d7448ac162328f9dbf5183124c`,
  `src/lib.rs:264-320` and `245-255`: four digits from two pairs, then a pair
  and leading digit; ASCII establishes UTF-8 validity. Hirsel uses initialized
  Inline scratch storage and checked indexing, and preserves its existing
  direct append for values below 10000 to avoid scratch initialization.
- Go `c0be4674865b06f0be851e27cef93136708be506`,
  `src/html/escape.go:166-181`, `src/strings/replace.go:550-579`,
  `src/html/template/js.go:254-263,336-354`: replacement tables and unchanged
  spans into one writer. Hirsel retains its five existing HTML spellings and
  the tasks app's existing JSON-then-HTML bytes; Go's broader JS escaping policy
  would change response bytes and is deliberately not adopted.

## Baseline instruction profiles

`perf record -e instructions:u -c 100000 -g --call-graph dwarf,8192` captured
both workloads. List has 21k samples and zero lost samples. Print has 7k
samples and 72 lost samples, so its attribution is approximate. The retained
[symbol table](text-build-2026-10-21/before-symbols.json) contains all resolved
flat self symbols; raw call stacks were inspected for the paths below.

| Symbol / caller | Source call site | Cause |
|---|---|---|
| `alloc::grow`, `seq_grow`, `reserve_byte_list` | `text.append` at stdlib/std@1/text.hir:4; renderer literal/hole appends | Repeated temporary and destination growth |
| `bytes_append`, `memmove` | stdlib/std@1/html.hir:51, text append, rendered fragment copies | Span copying and growing buffers |
| `string_from_bytes`, UTF-8 validator | stdlib/std@1/text.hir:9, compiler-generated `Finish` | Finished temporary f-string validation |
| `copy_graph`, `unique_copy` | examples/tasks/app.hir's former `html_reply` body conversion; view tag JSON conversion; model selection | Explicit unique conversions, including the whole final HTML |
| `internal.decimal.append` | stdlib/std@1/u64.hir:16 and generated primitive f-string holes | Digit formatting through the wide dispatcher |
| `alloc`, `clone_block`, `detach` | First `text.append` to an empty f-string list | Materializing the initially static empty builder |
| release/drop helpers | F-string block exit, rendered temporary fields, final response cleanup | Temporary and final allocation ownership |
| stdout sink/flush, `native_print`, `write_output` | bench/micro/programs/print.hir:18 | Per-line output effects |

Flat self buckets are not complete additive call-site costs. Unknown PCs and
inlined ownership instructions remain charged to containing functions. A
missing sampled symbol is not proof of zero executions.

## Verification

The selected corpus passes 65/65 cases: the 64 unchanged format/f-string cases and the new
invalid custom formatter regression. The agree runner passes its 36 originals
and 76 selected variants (50 reach the existing runtime budget). Lowering passes
18/18 tests; the new parser reservation test passes. The stdlib suite runs every
shard: 277 passed, 135 ignored, one pre-existing HTTP/2 replay assertion failed.
That failure is already recorded as
`hirsel-driver-test-it-stdlib::stdlib_http::http2_adapter_scripts_record_and_replay`
in `tools/known-failures.tsv`: its connect-event assertion is inappropriate for
the server accept fixture.

The final renderer passes all 22 differential HTTP/HTML/storage checks.
The HTML escaper corpus case passes independently. All eight append/fusion
tests pass, including primitive UTF-8 trust versus custom checked formatting
and the scalar/Inline-only digit-emitter check. All five tasks integration tests
pass: 22 package tests in both interpreted and LLVM modes, interpreted/native
end-to-end checks, the strict compiler-memory bound and event-stream retention.
The combined `tasks_` command exceeded its 600-second budget; each exact case
was subsequently run as its own command and passed without raising limits.

## Final ten-round session

The [raw session](text-build-2026-10-21/session.json) contains all 60 samples,
per-request server counters, sibling-load snapshots, executable hashes, stdout
checksums and paired bootstrap confidence intervals. Session seed is 20261021;
measurements began at the recorded UTC time and use one lock across both
workloads. Reproduce with `python3 text-build-2026-10-21/measure.py BEFORE AFTER
OUTPUT`: each directory holds the frozen `print`, `tasks`, `rust` and grants
artifacts built by the benchmark scripts; the Rust server stays in its normal
app build directory.

| Workload | Before / Rust user instructions | After / Rust user instructions | After / Rust total instructions | After / Rust cycles |
|---|---:|---:|---:|---:|
| List, 10k tasks, 16 req/s | 2.030 | 0.742 | 0.756 | 0.837 |
| Print, 1M lines | 1.827 | 1.212 | 1.050 | 1.037 |

Both requested user-instruction thresholds are met. The same-session print
stdout checksums agree for every participant and round. List response bytes
are separately checked by the differential HTTP verifier.

The additional [HTML row pair](text-build-2026-10-21/html-row.json) uses ten
interleaved rounds on CPU30 and identical FNV output checksums. It is 1.924×
Rust user instructions (1.821× total), with 1.278× cycles in that separate
session. It is a smaller kernel than the app renderer: the profile assigns
33.15% self samples to its source loop/checksum, 10.68% to `html.escape_into`
and 9.26% to `bytes_append` (7.02 points through `text.append`, 2.24 through
HTML escaping). Hirsel calls source escaping/append/format functions while
Rust inlines these operations; ordinary bounds/ownership and append-room checks
remain. There is no per-field temporary String or final byte copy in that pair.


## Remaining costs after the change

The final [flat symbols](text-build-2026-10-21/after-symbols.json) retain every
resolved symbol. List has about 5k instruction samples and print about 6k,
with zero lost samples in either final profile.

List's largest self symbols are `memmove` (15.08%), `bytes_append` (10.88%),
`reserve_byte_list` (9.72%), `text.append` (8.98%), `decimal.append_u64` (6.84%)
and `html.escape_into` (6.65%). These now belong to writing the actual page,
append-room checks, integer conversion and escaping into its one buffer.
The lower-bound markup reservation can still grow when variable field text
exceeds it: 6.36 sample points of `memmove` resolve through `text.append` and
`seq_grow`, and 6.20 resolve to its ordinary byte append. Exact output sizing
would require another pass over the rendered fields.

The remaining `copy_graph` (4.28% self) and `unique_copy_value` stacks resolve
to `model.select`'s `unique item` at `examples/tasks/model.hir:371`, not the
response body. `model.all_tags` (4.98% self) retains its existing tag collection
and cleanup at `examples/tasks/model.hir:384-398`. Those domain operations are
outside this rendering change. `js_string_into` contributes 1.79% self with no
intermediate quoted String.

Print's resolved user symbols include `decimal.append_u64` (2.12% self),
`native_print` (1.74%), last-owner destruction (1.26%) and `seq_grow` (1.23%).
They resolve to the numeric f-string hole, `out.print` at
`bench/micro/programs/print.hir:18`, f-string block cleanup and reserved builder
growth. The `instructions:u` record also reports unresolved kernel-looking PCs
around writes; these are retained as unknown, not attributed to text formatting.
Sampling and unwind artifacts cannot replace the ten-round hardware counts.

No requested performance threshold remains missed. The separate HTML row
kernel's remaining scalar loop/escape/append overhead is reported above. The
only failing required test is the pre-existing HTTP/2 replay assertion already
tracked in `tools/known-failures.tsv`; the minimal trigger is
`stdlib_http::http2_adapter_scripts_record_and_replay`, whose server fixture
accepts instead of producing the asserted `net.connect` event.

Landing also reconciles main's inline mutex callback proof by calling the
`view.page_bytes` factory with its declared `unique Bytes` result. That factory
consumes `bytes.from_list(take out)` and adds no explicit unique conversion.
The frozen measurements precede this function-boundary refactor; the recorded
binary hashes identify the measured artifacts. A focused checker/native build
and differential HTTP control validate the merged application.
