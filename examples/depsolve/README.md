# depsolve

Offline minimum-version selection over explicitly named local JSON catalogs.
Requirements are minimum versions: both D@1.0.0 and D@2.0.0 contribute their
requirements when reachable, and D@2.0.0 is selected. Cycles are legal. There is
no registry, SAT solver, package installation, or interaction with Hirsel's own
exact-major package selection.

From the repository root, with a matching `hirsel` toolchain:

```sh
hirsel run examples/depsolve/hirsel.json --grants examples/depsolve/grants.json -- resolve root.json 4 catalog.json
hirsel run examples/depsolve/hirsel.json --grants examples/depsolve/grants.json -- verify-lock lock.json root.json 4 catalog.json
```

The shipped fixture is the Go MVS cross dependency: selected B@1.0.0,
C@1.0.0, D@2.0.0, E@2.0.0. D@1.0.0's requirement on E@2.0.0 survives
D's upgrade. Output is one JSON document on stdout, followed by newline.
The supplied lock is bound to the exact fixture bytes. To save a new lock,
redirect resolve output to a file outside the input grant and then copy it
into your privately maintained read-only input directory.

Root: `{"dependencies":[{"name":"A","version":"1.0.0"}]}`.
Catalog: `{"packages":[{"name":"A","version":"1.0.0","dependencies":[]}]}`.
Names use ASCII letters, digits, `_`, `-`, `.` and at most 64 bytes. Input
filenames are portable leaf names (no `.` or `..`). Versions must be canonical
stable `major.minor.patch`, each component fitting u64. Ranges, prereleases,
metadata, partial versions and leading zeros are refused. This deliberately
does not implement Cargo or npm range resolution. Unknown JSON input members
follow the shipped ordinary derive decoder; lock verification is stricter and
compares every field, accepting only JSON object-key/whitespace differences.

Limits are 64 names, 128 version nodes, 256 total dependency edges (including
root edges), 32 catalog files, 64 KiB per catalog/root, 512 KiB total catalog
bytes and jobs 1–4. Duplicate name/version identities and duplicate catalog
filenames are refused before solving. Metadata sizes reserve catalog bytes
before any catalog task is admitted; reads are bounded by that reservation and
reject file growth. Supply unchanged files: this is an auditable content report,
not an atomic filesystem snapshot. Verification accepts a lock of at most 2 MiB.
Input byte caps bound the JSON DOM. A run-wide admission ledger checks names,
versions, duplicate identities and edges before typed catalog decoding.

Exit statuses: 0 success/help, 1 stale lock, 2 malformed input, domain error,
resource refusal or I/O error. These are application statuses; launcher,
language-failure and tooling outcomes retain their ordinary Hirsel classes.
Errors have `{error,detail,path}` with typed source or dependency provenance.
Missing versions include the shortest deterministic root-to-missing-node path.
The lock has schema 1, SHA-256 root/catalog digests, sorted selected versions and
visited identities, and a shortest provenance path for each selected version.
Catalog filenames and dependency adjacency are sorted before traversal, so input
file ordering and task completion order do not choose a different result.

The read-only `input` mount, stdout, Args and Tasks are the only authority used.
Edit `host` in a private grants file to your input directory; relative hosts
resolve beside that grants file. Placement is independent of logical jobs.

```sh
hirsel build examples/depsolve/hirsel.json --backend llvm -o /tmp/depsolve-llvm
HIRSEL_MEMORY_LIMIT=2G HIRSEL_JOBS=4 tools/capped --timeout 60 prlimit --cpu=60:61 /tmp/depsolve-llvm --hirsel-record /tmp/depsolve-llvm.tape --hirsel-grants /absolute/private/grants.json resolve root.json 4 catalog.json
hirsel run examples/depsolve/hirsel.json --replay /tmp/depsolve-llvm.tape
/tmp/depsolve-llvm --hirsel-replay /tmp/depsolve-llvm.tape
```

Native recording/observer options precede the grants operand; application argv
follows it. Keep tapes outside all mounted input directories. Use a separate
executable/tape for Cranelift (`--backend cranelift`). Requested placement cores
1/2/4 do not establish physical worker participation; qualification requires the
placement metrics/control evidence described in FRICTION.md.

Modules explicitly registered in hirsel.json: main (CLI/grants), catalog
(bounded concurrent reads), model (schema/admission), resolve (per-version MVS),
explain (generic recursive traces and typed problems), lock (report/verification).
Trace[String]/Problem[String] explain source failures; Trace[Requirement] and
Problem[Requirement] explain graph failures. Borrowed Map lookups retain catalog
ownership; enqueued trace descriptors own their copies. Failure in a batch
cancels and joins remaining siblings before returning the first source-order
error, and resources close through their ordinary drops.

The traversal ports Go cmd/go/internal/mvs/mvs.go:72–146, commit
`ebe2feefc400f813aefdc8a036e66d2619da6649`; vectors port mvs_test.go:16–79.
See LICENSE-Go. Deviations: bounded offline preload replaces network/goroutines;
serial deterministic BFS follows preload; stable semver replaces opaque version
ordering; the unversioned root is metadata, not a selected package. Upgrade and
downgrade operations are outside this program's scope. The independent test
oracle computes set closure then per-name maximum and checks literal upstream
blog/trim/cross build lists.

Validation receipts and outstanding obligations are recorded in FRICTION.md.

Focused validation (repository root):

```sh
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=depsolve_29
```

The harness partitions native workflows and boundaries per backend and serializes
native processes; one source/backend generation is built once per harness. To
retain host evidence, use `-c hirsel.tests=host` and executor arguments
`-- --env DEPSOLVE_29_EVIDENCE_DIR=/absolute/evidence --test-arg=depsolve_29`.
The two ignored full public-adapter audits document known unchanged-budget
INCOMPLETE cells. Passing runnable tests do not make those obligations, unsupported
adapters or physical-width qualification complete; see FRICTION.md.

Upstream opaque/integer test versions are normalized to stable `N.0.0` (bare
nonroot modules use `1.0.0`), and terminal modules are explicit empty catalog
entries. The upstream root itself is omitted from selected versions.
