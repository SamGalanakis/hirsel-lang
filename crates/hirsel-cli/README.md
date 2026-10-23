# hirsel-cli

Expose check, run, build, fmt and JSON through the compiler driver.

Every nonzero CLI exit includes a cause on stderr, including JSON checks (whose stdout JSON protocol is retained). Returned Error payloads use the PROG-3 host report, independently of stderr grants.

Allowed dependencies: hirsel-cli-args, hirsel-driver, hirsel-diag, hirsel-intern's timings and memory, and shared span/syntax types as needed, hirsel-guard, the memory guard every binary installs, and `mimalloc`, the allocator it guards; no phase internals.

The binary does not free its session at exit (`hirsel_engine::free_sessions`), except under `--mem-usage`, which reports what each table frees: the
process ends right after its command's output.

Invariants and boundary:

- Consumes arguments and source/config inputs; produces diagnostics, formatted source, stdout and exit code.
- Use Diagnostic rendering; JSON follows the documented versioned schema.
- Launch failures are tooling errors, not invented language rule diagnostics (PROG-3).
- Do not alter checker acceptance based on flags or topology (DET-3).

Commands (arguments use the shared lexopt parser):

- `hirsel --help` and `hirsel help` print usage on stdout and exit 0.
- `hirsel new <dir> [--lib]` creates a schema-1 package, source with one test and
  explicit stdout grants. A manifestless source selects the toolchain std@1;
  an exact sidecar continues to select its declared dependencies, including none.
- `hirsel check [--json] <file>` emits human diagnostics on stderr or one schema-v1
  diagnostic JSON object per stdout line. A clean complete check exits 0; any error
  diagnostic exits 1; an unavailable phase exits 2 with a tool message on stderr.
- `hirsel check --safe <file>` also prints, for an accepted program, one verdict
  line on stdout: `safe: <entry> ...` (exit 0), or `unsafe: <entry> reaches
  <escape>: <entry> -> ... -> <escape>` (exit 1), the shortest dependency path
  from the entry to an extern or `Unsafe` operation (CAP-6).
- `hirsel run <file>` requires complete checking/lowering and writes exact stdout
  bytes. Both engines use the shared [PROG-3 exit policy](../../docs/spec/programs.md#prog-3-entry).
  Without `--run`, `--grants`, `--simulation` or `--replay` it admits the empty-grant
  record in either engine: an entry taking a root capability is refused at launch
  (`hirsel: launch: {"kind":"not_granted","root":"Io"}`, exit 3), followed by a line
  naming `--grants` and `--simulation`, then an inline `--grants` template for
  exactly the entry's requested roots.
  A sole `Test` parameter selects test admission, including exported records and
  explicit grants; the empty record supplies only the deterministic Test context.
- `hirsel fmt [--check] <file>` writes canonical source to the file, or compares
  without writing with `--check`. A difference exits 1, agreement exits 0.
- `hirsel expand <source|manifest.json>` checks the program and prints each
  generated function body (`= comptime`, CT-5) of the file's declarations in
  source order, exactly as its generator wrote it, so diagnostic lines in
  generated text match. A rejected program prints its diagnostics and exits 1.
- `hirsel hash <source|manifest.json>` checks the program and prints one
  `<hash> <definition path>` line per source definition of its package, sorted by
  path: the LANG-6 definition hash, which renames keep and body edits move. A
  rejected program prints its diagnostics and exits 1.
- `hirsel doc <manifest.json> [-o out.md] [--json]` documents that package's
  public declarations without checking bodies. It retains full doc comments and
  signature contracts, stable definition-path anchors, and dependency links.
  std's manifest also publishes the core registry in `docs/api.md`.
- `hirsel precompute <package manifest.json> -o <directory>` checks the package
  and populates the ordinary query store. It requires V and refuses diagnostics.
  Toolchain packaging uses this same path for std (docs/building.md, "Ordinary std products").
- `hirsel tapes check <tape>...` joins a set of linked runner tapes (docs/spec/runners.md,
  RUN-11) without a program: it prints a `checked:` line per link whose ends are both in
  the set, an `unchecked:` line per link with one end missing, and a count (exit 0), or
  refuses the set on the first set of mismatches (`hirsel: tapes check: ...`, exit 1).
- `hirsel serve <program> --host <json>` is a runner host for `remote.tls@1` callers
  (docs/spec/runners.md, RUN-13): it listens at the host configuration's endpoint, prints
  `hirsel serve: listening on <endpoint>` on stderr once it accepts connections, and runs
  each admitted call to one of the program's callee entries as its own callee run, until
  the process ends.
- `--timings`, `--timings=json` and `--mem-usage`, anywhere before `--` on
  check, build, run and test, print the compile timings and heap report on
  stderr after the command (docs/building.md, "Compile timings"). Heap bytes
  are the live bytes the binary's allocator counts under `--mem-usage` only.
- `--no-verify`, anywhere before `--`, skips V for `run`, `test` and `build`
  (docs/building.md); any other command refuses it with exit 2.
- `--verbose`, anywhere before `--`, enables additional command diagnostics.
- `--verify-cache`, anywhere before `--` on check, build and run, recomputes
  result-store hits and compares their complete canonical records. It runs in
  this process, bypassing resident dispatch, and leaves V enabled by default.
- Invalid arguments exit 2. Non-executing source commands report IO failures as
  exit 1, without a RuleId; run and test setup failures use the shared exit policy. Invalid UTF-8
  is a SEM-3 language diagnostic at the first invalid byte boundary, before parsing.
  Paths beginning with `-` must be written with a directory prefix (`./-name.hir`).

Unavailable checking phases exit 2, including clean source in an incomplete
pipeline; unavailable execution is a tooling outcome (3). Tests exercise the binary's human/JSON lexer diagnostics and IO/usage
codes, plus injected tool operations for clean checking, run and formatting codes.

`hirsel run --simulation config.json program.hir` supplies a deterministic fake
filesystem and root arguments. The JSON object accepts `args`, `grants`, `projections` and
`directories` as string arrays, `files` as a string-valued map, `env` as an exact
name-to-string-or-null grant map, `stdin` as text or an array of bytes, and
`monotonic` as a nonnegative u64 and `wall` as signed i64 nanoseconds, and `process` as a
RunConfig `io.process` object whose `process.script@1` runs serve `proc.run`, and `net`
as a RunConfig `io.net` object whose `net.script@2` peers and listeners serve network calls. File strings are exact UTF-8 bytes; paths remain
confined to the simulated root. For example:

```json
{"args":["tree"],"files":{"tree/a.txt":"x\r\ny","tree/b.txt":"unchanged"}}
```

All source runs now use the declared registry and full compiler pipeline, then
`launch_with_runtime`. Invalid entry shape and compiler-invariant errors remain
tool errors. The spec opening example is retained in `tests/fixtures/opening.hir`;
its success test is explicitly ignored until the phase-owned validation findings
it reaches are resolved. An active test verifies that those
invariant failures produce a tool error without JSON language diagnostics.

Execution API errors retain prior stdout in ExecutionError. Launch carries the
original categorized cause plus output; ObservedExecution.outcome is an inner
Result, with tooling_output when no semantic observation exists. Pre-execution
launch errors stay outer errors. Ordinary run adapters write captured output
before returning the cause; unavailable execution exits 3.

Violated executable unchecked preconditions stop with UndefinedBehavior carrying
`undefined behaviour detected`, operation, source site and contract. Prior stdout
is retained, but DET-1, semantic parity and cleanup guarantees do not apply (#49).
No language failure is synthesized; observable returns None and CLI exits 3.
Malformed operation schemas remain API errors.

`hirsel run --native <file>` checks the program as `run` does (the same first
diagnostics), then builds it as an optimized x86-64 Linux executable, links it
against the runtime built from this checkout and runs it. Output, exit status
and the `runtime failure[RULE] at PACKAGE@VERSION/module/definition (FILE:LINE:COLUMN)` line match `run`. A program a native
stage cannot build yet prints `hirsel: native <stage>: ...` and exits 3; that is
a tool outcome and says nothing about the source. `--backend cranelift` builds
the same LIR with Cranelift, unoptimized, for a fast debug loop
(`crates/hirsel-cranelift`); `--backend llvm` selects LLVM O2; the default is Cranelift.

Check/run (including `--native` and `--simulation`) also accept an explicit
launch manifest JSON path. A source path loads its exact `.hir.json` sidecar
when present, otherwise standalone@0. `fmt` always formats a source file.

An explicit empty simulation `projections` array grants no Io projections. Its
allowed names are stdin, stderr, stdout, tasks, net, process, random and dir; an unknown name refuses
the simulation input. Root Io and Args grants remain separate. This fake-input
seam is independent of the production RunConfig admission contract.

Production source runs use `hirsel run <source|manifest> --run /absolute/run.json`,
with optional `--native`. The shared RunConfig parser binds the semantic program and
entry, validates every grant and snapshot, and completes host admission before entry.
Without a record, the existing simulated world supplies captured stdout, empty virtual
directories and no network; it grants no real host authority. `--simulation` remains an explicit fake-provider seam
and cannot combine with `--run` or `--native`.

`hirsel export --run <source|manifest> -o run.json` writes the current semantic binding
with empty grants. A launcher can explicitly fill its Io projections and Args/Env
snapshots before supplying that record; selecting the source grants no authority.

`hirsel build [--release | --debug] [--lto] [--backend llvm|cranelift] [--fuel] [--target x86_64-linux | aarch64-linux]
<source|manifest.json> -o <exe>` writes a standalone executable linked with the
production runtime. Default and release builds use LLVM O2; debug defaults to Cranelift
with source line tables. `--lto` selects whole-module LLVM O2. `--backend cranelift` lowers the same LIR with Cranelift: `--debug`
is its unoptimized level with DWARF 5 source line tables, and the default profile
uses its `speed` level. Release builds use LLVM; combining `--release` with
`--backend cranelift` exits 2 with that diagnostic. Its parity gate covers x86-64
and AArch64. Launch it with `--hirsel-run /absolute/run.json [args...]`; omitted
records deny external roots. `-- [args...]` explicitly supplies only Args. `--fuel` compiles fuel checks at function entries and loop heads, so the executable
can enforce a fuel budget (`budgets.limits@2`); without it a fuel budget is refused, and
`hirsel run --native` adds the checks only when its run has a fuel budget. A run that
spends a budget prints `budget exceeded: <kind>` and exits 5 in every launcher.

`build` and `run --native` accept `--proofs <sidecar>` with any number of
`--trust <tool@version>`: current `proved` records from a trusted checker
remove their checks from the native program (PS-12/PS-13, the sidecar format in
`docs/spec/proof-seam.md`). The proof report goes to stderr as
`hirsel: proof <id>: removed` or `kept (<reason>)` lines and a count line.
`--trust` needs `--proofs`, and `--proofs` needs a native engine. A malformed
sidecar is a tool error.

`hirsel run <source|manifest> --grants grants.json` binds a grants template to
that program and uses the same validation and admission as `--run`. These options
are exclusive, and neither combines with `--simulation`. Templates omit binding
fields and deny omitted roots. Standalone executables accept the corresponding
`--hirsel-grants grants.json [args...]` spelling.

`--record <tape>` beside `--run` or `--grants` streams the run's Io tape;
`hirsel run <source|manifest> --replay <tape>` replays one, in either engine, with no
grants and no host effect. Replay refuses a tape bound to another semantic revision,
entry or root set before entry (exit 3), and a divergence, missing or unconsumed event
after the output it confirmed (`hirsel: replay refused: ...`, exit 3), in either engine.
With `--native` the driver only binds the run; the executable admits it once and reports
an admission refusal, a Blocked run, a stop and a codec limit to the driver, so `hirsel run`
reports them as the interpreter's run does: a refusal `hirsel: launch: <refusal>` (exit 3),
Blocked with its parked waits (exit 4), and `hirsel: environmental failure: ...` (exit 6) for a run past the
codec's operational budget. `--replay` takes no `--run`,
`--grants` or `--record`, and neither option combines with `--simulation`. Executables
spell them `--hirsel-record <tape>` (before their launch input) and `--hirsel-replay <tape>`.
A recorded interpreter run whose record names runners also journals each callee run it
starts to `<tape>.call-<ordinal>` (unless the runner's `recording` is `none`); each such
tape is a call launch of the callee's entry and replays alone against the same program.

Simulation writes stdout and stderr to distinct tool sinks. Live Stdin/Stderr and
Env operations use the admitted runtime providers through HostLibrary; only exact
Env names in the run record are readable, with null distinct from a denied name.

Interpreted host runs (`--run` or `--grants`) have no default step budget.
Simulated runs and replays default to 1,000,000,000 execution steps. `--max-steps <n>`
selects an explicit nonnegative u64 budget, including zero; `--max-steps unbounded`
removes the step bound. `--native` rejects this interpreter option. Exhaustion deterministically exits 3 and reports
`evaluator tooling limit: Steps`. Driver/conformance adapter defaults and
compile-time evaluation budgets are independent.

`--revocations <file>` accompanies a live `--run` or `--grants` launch and supplies
explicit schema-1 `{"schema":1,"revoked":["grant-reference"]}` host evidence.
Executables accept `--hirsel-revocations <file>` before their launch input. The file
is read once at admission; replay uses the recorded decision and digest.

`hirsel run <source> --grants <template> -- args...` replaces the template's
Args snapshot with exactly the trailing UTF-8 arguments, including an explicit
empty snapshot. Without `--`, the template's Args are retained. `--run` keeps the
record's Args authoritative and rejects trailing arguments. Relative directory
hosts in a template resolve beside that template, then become canonical absolute
hosts before ordinary validation and admission.

`--run` and `--grants` accept inline JSON when the argument starts with `{`,
otherwise a path. Executables use `--hirsel-run` and `--hirsel-grants` to leave
the program's flag space available. Inline templates require absolute directory
hosts. `hirsel run <source|manifest> -- args...` grants only the exact Args
snapshot through host admission, with the default step budget of a run without a record.

The examples harness supplies the same default step budget as the CLI without a record
when invoking the interpreter in process; explicit test budgets remain selected
by their callers. Bench-lite exercises both 40-word and 200-word corpora.

`run --json` writes runtime failure reports to stderr with the complete definition,
file, line and column. Program output retains its original streams.

`hirsel test [--json] [--seed N] [--native [--backend llvm|cranelift]] manifest.json [filter...]` runs the package's explicitly marked tests in isolation, reports seeds and totals, and exits 1 on a failure. `--scheduler name@1:seed` runs each test under that scheduler; `--explore N [--scheduler random@1|pct@1]` runs each under N seeds of the policy (random@1 by default) and stops it at its first failure, printing the failing scheduler, the schedule count and a `replay:` command (docs/building.md, "Schedules"). Neither combines with `--bench`.
