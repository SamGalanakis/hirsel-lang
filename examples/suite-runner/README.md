Run bounded suites of literal commands. `suite-runner` is a separate example;
`examples/testrun` retains its earlier streaming interface.

```sh
mkdir -p examples/suite-runner/reports
hirsel run examples/suite-runner/hirsel.json --grants examples/suite-runner/grants.json
hirsel run examples/suite-runner/hirsel.json --grants examples/suite-runner/grants.json -- list pass.json
hirsel run examples/suite-runner/hirsel.json --grants examples/suite-runner/grants.json -- plan pass.json fast 0 2
hirsel run examples/suite-runner/hirsel.json --grants examples/suite-runner/grants.json -- run pass.json report.json fast 0 2
```

The template uses the deterministic process provider and a shipped pass fixture.
For real commands, instantiate a private copy with `process.spawn@1`, exact
absolute executable rules, and absolute host directories. Remove `runs` and
`children` from the real process grant. A program receives literal argv, closed
stdin, no environment, working directory `/`, and `own_group:false`.

```json
{"jobs":2,"timeout_ms":10000,"failure_limit":0,"suites":[{"id":"unit","cases":[
 {"id":"true","argv":["/usr/bin/true"],"tags":["fast"],"expect":0,"retries":0,"timeout_ms":1000},
 {"id":"false","argv":["/usr/bin/false"],"tags":["fast"],"expect":1,"retries":1,"timeout_ms":1000}
]}]}
```

`list FILE` emits every case in manifest order. `plan FILE [TAG|- [INDEX COUNT]]`
emits selected and skipped cases with reasons. `run FILE REPORT` accepts the
same optional selector. Tags match exactly. `-` selects all tags. Shards are
zero-based, count 1..32; a base-31 polynomial over the ASCII case ID modulo
count gives stable membership independent of manifest ordering or concurrency.

Manifest fields are required and unknown fields are rejected. Bounds: 256 KiB
before JSON decoding; 4 suites, 32 cases total, unique suite and global case IDs
of 1..64 ASCII letters/digits/`_.-`; at most 8 distinct tags of that form;
1..64 argv words and 4096 total UTF-8 bytes including one NUL per word;
no embedded NUL and an absolute normalized executable path. Jobs are 1..4,
attempt deadlines 1..5000 ms, run deadline 1..30000 ms, retries 0..2, expected
exit an i32, failure_limit 0..96. Zero disables the failure threshold. Byte limits
are checked before copies; suite/case limits before item construction. JSON
parsing uses shipped std implementations on the bounded byte buffer.

Admissions scan manifest order for a ready case. A retry becomes ready only
after its previous attempt has been joined. The failure threshold counts failed
attempts and stops all new admissions, retries included, then joins active
children. Completed attempts retain their results. Selected cases never admitted
are `not_run`; canceled unfinished cases are `incomplete`; unselected cases are
`skipped`. A recovered retry is `flaky`, counted as failed, so it cannot hide an
earlier failure. Every admitted attempt is retained, including spawn refusal.
`child.wait` drains/discards both streams concurrently and consumes/reaps the
direct child. Output capture and descendant/process-tree termination are outside
the contract.

After joins, report.json is replaced atomically through the `reports` grant.
The version-1 JSON object contains manifest-ordered cases, all attempts, counts,
cutoff/expiry flags and `output:"discarded"`. Stdout prints the same object only
after commit. A publication failure preserves the previous report. An stdout
failure can occur after successful publication. Exit 0 means all selected cases
passed; 1 means failed/flaky/expired or a returned typed I/O error; 2 invalid
application input; 3 a missing suite runtime projection. Resource/tooling refusals
remain launcher outcomes, separate from application errors and language failure.

The new example also accepts `PROGRAM [MILLISECONDS]`, using the earlier
`testrun` stdout streaming recipe and verdict text, returning 0 after a verdict.
It limits this compatibility deadline to 5000 ms; the original testrun remains
available for its original deadline range. Suite mode handles both pipes.

Production standalone smoke (set HIRSEL to your matching CLI and EVIDENCE to
an external directory; PRIVATE_GRANTS is an instantiated absolute-path template
whose fixtures mount contains the real manifest above and whose reports mount
exists). Use distinct grant/tape paths per backend/width, with placement cores
1, 2, or 4 and `tasks.cooperative@1`, commit `shared`, reorder `auto`:

```sh
tools/capped --timeout 300 "$HIRSEL" build examples/suite-runner/hirsel.json --backend llvm -o "$EVIDENCE/app-llvm"
HIRSEL_MEMORY_LIMIT=2G HIRSEL_JOBS=4 tools/capped --timeout 60 prlimit --cpu=60:61 "$EVIDENCE/app-llvm" --hirsel-record "$EVIDENCE/llvm-1.tape" --hirsel-grants "$PRIVATE_GRANTS" run manifest.json report.json fast 0 1
HIRSEL_MEMORY_LIMIT=2G HIRSEL_JOBS=4 tools/capped --timeout 60 prlimit --cpu=60:61 "$EVIDENCE/app-llvm" --hirsel-replay "$EVIDENCE/llvm-1.tape"
```

Keep tapes outside granted mutable directories. Requested placement alone does
not establish effective width; the harness samples the production control
socket while a FIFO-held child is active. See FRICTION.md for actual engine,
replay, bounds and cleanup dispositions.

Modules: main CLI/typed errors and compatibility; manifest bounded decoding;
select pure selection/plan; run admission/scopes/retries; report aggregation and
atomic publication; model generic indexed results and error conversion.
`Outcome[Case,InvalidInput]` is the decode seam, and
`Outcome[i32,Fault]` is the execution/task-set seam.

Reference structure: CPython `Lib/unittest/runner.py:242–284` and
`Lib/test/test_unittest/test_runner.py:1264–1282`, commit
010bebbf9fd12bba1e3f40bdbddacd51a9c2bf20 (PSF License), provide
run/finalize/report lifecycle and empty/failfast controls. Go
`src/os/exec/exec.go:915–950` and `exec_test.go:470–525`, commit
ebe2feefc400f813aefdc8a036e66d2619da6649 (BSD-3-Clause), provide
wait-for-process-and-both-pipes and stdin-close vectors. License texts are in
LICENSES.md. Deviations: scopes replace exceptions/deferred cleanup; failure
threshold controls admissions while joining siblings; bounded explicit retries
retain failures; child.wait discards output; no shell, inherited environment,
groups, or generic runtime adapter is added. Existing batchrun task-set and
bounded-read recipes are reused.
