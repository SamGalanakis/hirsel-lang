# hirsel-cli

Expose check, run, build, fmt and JSON through the compiler driver.

Allowed dependencies: hirsel-cli-args, hirsel-driver, hirsel-diag and shared span/syntax types as needed; no phase internals.

Invariants and boundary:

- Consumes arguments and source/config inputs; produces diagnostics, formatted source, stdout and exit code.
- Use Diagnostic rendering; JSON follows the documented versioned schema.
- Launch failures are tooling errors, not invented language rule diagnostics (PROG-3).
- Do not alter checker acceptance based on flags or topology (DET-3).

Commands (arguments use the shared lexopt parser):

- `hirsel --help` and `hirsel help` print usage on stdout and exit 0.
- `hirsel check [--json] <file>` emits human diagnostics on stderr or one schema-v1
  diagnostic JSON object per stdout line. A clean complete check exits 0; any error
  diagnostic exits 1; an unavailable phase exits 2 with a tool message on stderr.
- `hirsel run <file>` requires complete checking/lowering and writes exact stdout
  bytes. Exit 0 is success; any runtime failure/nonzero program exit becomes 1.
- `hirsel fmt [--check] <file>` writes canonical source to the file, or compares
  without writing with `--check`. A difference exits 1, agreement exits 0.
- Invalid arguments exit 2. IO failures exit 1 and have no RuleId. Invalid UTF-8
  is a SEM-3 language diagnostic at the first invalid byte boundary, before parsing.
  Paths beginning with `-` must be written with a directory prefix (`./-name.hir`).

Unavailable phases always exit 2, including clean source in an incomplete
pipeline. Tests exercise the binary's human/JSON lexer diagnostics and IO/usage
codes, plus injected tool operations for clean checking, run and formatting codes.

`hirsel run --simulation config.json program.hir` supplies a deterministic fake
filesystem and root arguments. The JSON object accepts `args`, `grants`, `projections` and
`directories` as string arrays, `files` as a string-valued map, `env` as an exact
name-to-string-or-null grant map, `stdin` as text or an array of bytes, and
`monotonic` as a nonnegative u64 and `wall` as signed i64 nanoseconds. File strings are exact UTF-8 bytes; paths remain
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
before returning the cause; unavailable execution remains exit 2.

Violated executable unchecked preconditions stop with UndefinedBehavior carrying
`undefined behaviour detected`, operation, source site and contract. Prior stdout
is retained, but DET-1, semantic parity and cleanup guarantees do not apply (#49).
No language failure is synthesized; observable returns None and CLI exits 2.
Malformed operation schemas remain API errors.

`hirsel run --native <file>` checks the program as `run` does (the same first
diagnostics), then builds it as an optimized x86-64 Linux executable, links it
against the runtime built from this checkout and runs it. Output, exit status
and the `runtime failure[RULE] at line N` line match `run`. A program a native
stage cannot build yet prints `hirsel: native <stage>: ...` and exits 2; that is
a tool outcome and says nothing about the source.

Check/run (including `--native` and `--simulation`) also accept an explicit
launch manifest JSON path. A source path loads its exact `.hir.json` sidecar
when present, otherwise standalone@0. `fmt` always formats a source file.

An explicit empty simulation `projections` array grants no Io projections. Its
allowed names are stdin, stderr, stdout, tasks, net, random and dir; an unknown name refuses
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

`hirsel build [--release | --debug] [--target x86_64-linux | aarch64-linux]
<source|manifest.json> -o <exe>` writes a standalone executable linked with the
production runtime. Default builds use LLVM O2, release O3, debug O0 with source
line tables. Launch it with `--hirsel-run /absolute/run.json [args...]`; omitted
records deny external roots. `-- [args...]` explicitly supplies only Args.

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

Simulation writes stdout and stderr to distinct tool sinks. Live Stdin/Stderr and
Env operations use the admitted runtime providers through HostLibrary; only exact
Env names in the run record are readable, with null distinct from a denied name.

Interpreted `hirsel run` defaults to 1,000,000,000 execution steps. Use
`--max-steps <n>` to select an explicit nonnegative u64 budget, including zero.
The option works with simulated inputs and explicit host `--run` records;
`--native` rejects it. Exhaustion deterministically exits 2 and reports
`evaluator tooling limit: Steps`. Driver/conformance adapter defaults and
compile-time evaluation budgets are independent.
