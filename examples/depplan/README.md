# depplan

Plan a release, migration or build from named prerequisites. This is a dry-run
planner: node names are inert strings and are never executed or opened as paths.

From the repository root, with the matching `hirsel` CLI and std image installed:

```sh
hirsel check --json examples/depplan/hirsel.json
hirsel export --tir examples/depplan/hirsel.json -o /tmp/depplan.tir
hirsel run examples/depplan/hirsel.json --grants examples/depplan/grants.json -- --help
hirsel run examples/depplan/hirsel.json --grants examples/depplan/grants.json -- release.json
hirsel run examples/depplan/hirsel.json --grants examples/depplan/grants.json -- invalid.json
hirsel build examples/depplan/hirsel.json --backend llvm -o /tmp/depplan
/tmp/depplan --hirsel-grants examples/depplan/grants.json release.json
# The same build/run commands accept --backend cranelift.
```

The read-only grants template names only `fixtures` (relative to the grants file),
stdout and stderr. See the [shared grants-base recipe](../../docs/native-verification.md#resolve-directory-hosts-beside-the-grants-file)
for a preserved bad/correct pair launched from another working directory. For your own inputs, copy the template and replace `host` with
an absolute directory. `FILE` is relative to the logical `input` grant; it cannot
escape that directory. Help needs stdout only. No process, socket, clock, write,
network or task authority is requested.

Input is a UTF-8 JSON object with a `nodes` array. Every record requires `name`
and a `prerequisites` array of names, including `[]` when no prerequisites exist:

```json
{"nodes":[{"name":"deploy","prerequisites":["build"]},{"name":"build","prerequisites":[]}]}
```

Names are exact, nonempty UTF-8 strings, without trimming or normalization.
Duplicate node declarations and unknown prerequisite names fail. Repeated
prerequisites are deduplicated. Unknown JSON fields follow the shipped record
decoder policy: they are ignored, but their bytes and syntax still count.
Duplicate JSON object keys, invalid UTF-8, malformed JSON and trailing values fail.

Output lines use `f"{encoded}\n"`. String `+` is unsupported; f-strings are the
intended language form. For two String bindings, `f"{left}{right}"` or
`std@1/string.concat(left, right)` appends their text with `std@1` selected.
Other types need supported formatting or an explicit String conversion.

Success writes one compact JSON object plus LF to stdout, with no stderr.
`release.json` yields exactly:

```json
{"order":["build","migrate","smoke","deploy","announce"]}
```

Dense IDs follow declaration order. Initially ready nodes enter the FIFO in that
order; newly ready nodes join its tail in dependent declaration order. This
preserves declaration order for ready ties, including nodes released together.
An already queued node keeps its place ahead of a newly ready node; the result
is deterministic, not a globally lexicographically minimal topological order.
Every prerequisite precedes its dependent.

Output calls require their own typed capability: `out.print(output, text)` takes
`Stdout`, while `err.print(sink, text)` takes `Stderr`. Obtain and explicitly
forward the capability for the intended stream. A `uses {Stderr}` clause alone
does not supply a value, and passing `Stderr` to `out.print` remains CAP-1.
The diagnostic names the expected and supplied types and labels the supplied
argument; its suggestion to use `err.print` is advisory. Depplan's error path
must use `err.print` to keep reports off stdout.

Invalid input writes a typed report as compact JSON plus LF to stderr, with no
stdout. `invalid.json` catches an omitted migration before deployment:

```json
{"error":"unknown_name","node":"deploy","reference":"migrate","blocked":[]}
```

Reports always contain `error`, `node`, `reference` and `blocked`. Unused strings
are empty and unused lists are `[]`. Error codes are `json`, `shape`, `name`,
`duplicate_node`, `unknown_name`, `node_limit`, `edge_limit`, `prerequisite_limit`,
`input_bytes_limit`, `usage`, `arguments`, `directory_grant`, `file_open` and
`file_read`. The defensive `graph` code wraps an unexpected std graph rejection.
A `blocked` report lists **every** blocked name in declaration order, including
all downstream dependents; it does not claim to be a minimal cycle or a cycle
witness. For `a <- b <- a` with `downstream` depending on `b`, it reports all three.

Exit statuses use `std@1/process.ExitCode`: **0** for a plan or help, **1** for a
blocked graph, **2** for invalid input, missing authority or I/O failure. If the
necessary output grant is missing or fails, no report can be promised; the
program returns 2. Compiler/launcher failures have their own statuses.

Bounds are inclusive: **65,536 input bytes**, **64 node records**, **128 raw
prerequisite occurrences in total**, **64 occurrences per node**, and **128 UTF-8
bytes per name**, including references. Raw occurrences count before deduplication
so repeated names cannot bypass the work cap. Reads probe at most one byte past
the byte cap, checking before appending. The byte cap bounds the initial JSON DOM,
including unknown fields; std also bounds nesting at 128. The pure decoder checks
array lengths before building typed records. The planner checks counts again for
callers supplying records directly, before growing its bounded name map, per-node
set, edge list, ordered names or blocked report. The DOM is bounded by input bytes,
not by a promise of fixed RSS. Fixtures and boundary cases remain small.

The focused acceptance harness executes the actual interpreter CLI and builds
both LLVM and Cranelift executables. It checks exact stdout, stderr and status,
independent precedence, transitive-closure blocked sets, repeated output, byte /
record / reference / name boundaries, malformed input and missing grants. Runtime
processes run sequentially under `tools/capped` at 60 seconds / 2 GiB. Fixture
hashes and an outside-grant sentinel are unchanged after each run. Inputs, grants,
sentinel and the enclosing private test directory are removed with assertions.
A second test exercises pure decode/plan/error reporting on the existing checked
heap executor and asserts no live heap bytes.

```sh
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=depplan_23
```

Prior art: CPython `Lib/graphlib.py:56–85` and
`Lib/test/test_graphlib.py:40–57` at
`8b7d6c53eee2226b08bed761d5a1742b5adb8507` (PSF-2.0, Copyright (c) 2001 Python
Software Foundation; All Rights Reserved; license in
[third-party/NOTICES.md](../../third-party/NOTICES.md#python-software-foundation-license-version-2-cpython)).
The checked-in singleton, reverse-chain and disconnected-chain vectors adapt those
tests. All implicitly mentioned Python nodes become explicit string declarations.
Unlike Python's incremental API, duplicate declarations and unknown names are
rejected and repeated prerequisites are deduplicated. The shipped
`std@1/toposort.order` supplies FIFO Kahn; Python's `graphlib` supplies independent
expected success answers, with a separate precedence check. Transitive closure
supplies blocked expectations because CPython's cycle exception is a witness,
where Hirsel returns the complete blocked set. See [FRICTION.md](FRICTION.md) for
the attempted forms and repairs.

See the [tested JSON, text and time recipes](../../docs/std-api-recipes.md)
for public module names, contextual variants and ownership at each call.

Nested JSON conversion uses the explicit `node_from_json` helper. See the
[complete nested JSON helper recipe](../../docs/spec/std-library.md#nested-json-helpers)
for naming, module visibility and directional signatures. Missing or mistyped
helpers are labelled at the enclosing source field or payload.
