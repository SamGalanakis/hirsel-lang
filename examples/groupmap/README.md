# groupmap

Reconcile explicitly named inventory aliases into connected components. For an
agent combining warehouse, supplier and legacy catalogs, transitive aliases
identify one item while isolated records remain visible. Alias pairs are
undirected equivalences; this program does not interpret dependencies.

From the repository root with a matching CLI and std image:

```sh
hirsel check --json examples/groupmap/hirsel.json
hirsel export --tir examples/groupmap/hirsel.json -o /tmp/groupmap.tir
hirsel run examples/groupmap/hirsel.json --grants examples/groupmap/grants.json -- --help
hirsel run examples/groupmap/hirsel.json --grants examples/groupmap/grants.json -- inventory.json
hirsel run examples/groupmap/hirsel.json --grants examples/groupmap/grants.json -- invalid.json
hirsel build examples/groupmap/hirsel.json --backend llvm -o /tmp/groupmap
/tmp/groupmap --hirsel-grants examples/groupmap/grants.json inventory.json
# Build the same package with --backend cranelift for the other native engine.
```

`FILE` is relative to the logical `input` directory grant. The checked-in template
permits only read access to `fixtures` (relative to the grants file), stdout and
stderr. Copy it and replace `host` with an absolute directory to use your own
inventory. Names are inert strings, never file paths or commands. Help needs only
stdout. No write, network, process, socket, task or clock authority is requested.

Input is one UTF-8 JSON object:

```json
{"records":[{"name":"warehouse:bolt"},{"name":"supplier:6mm"},{"name":"legacy:B6"},{"name":"seal"}],"aliases":[{"left":"supplier:6mm","right":"legacy:B6"},{"left":"warehouse:bolt","right":"supplier:6mm"}]}
```

Names are exact nonempty strings, without trimming or normalization. Duplicate
record declarations and unknown alias endpoints fail. Self and repeated pairs
are harmless but count toward the raw pair limit. Both arrays are required;
`{"records":[],"aliases":[]}` succeeds. Unknown fields are ignored under the
shipped record decoder policy, but count toward input bytes and must have valid
JSON syntax. Duplicate object keys, invalid UTF-8 and trailing values fail.

Success writes one compact JSON object plus LF to stdout and nothing to stderr.
For the input above:

```json
{"components":[{"label":"warehouse:bolt","members":["warehouse:bolt","supplier:6mm","legacy:B6"]},{"label":"seal","members":["seal"]}]}
```

Members follow declaration order. Components follow their first member's
position, which also supplies their label. Dense IDs follow declaration order,
but internal union-find representatives remain opaque. Reversing pair order or
swapping endpoints leaves the output bytes unchanged. The model takes an owned
root snapshot, computes each component's first member, and stably sorts IDs by
that member's index, preserving declaration order within equal keys. It then
constructs owned output names independent of the inventory and forest.

Invalid input writes a typed report to stderr and no stdout. `invalid.json`
catches a supplier alias whose record was omitted:

```json
{"error":"unknown_name","name":"vendor:hex-8"}
```

Reports always contain `error` and `name`; an unused name is empty. Codes are
`json`, `shape`, `name`, `duplicate_name`, `unknown_name`, `record_limit`,
`alias_limit`, `input_bytes_limit`, `usage`, `arguments`, `directory_grant`,
`file_open`, `file_read`, and defensive `forest` for unexpected std rejection.
Exit statuses use `std@1/process.ExitCode`: **0** for grouping/help, **2** for
invalid input, missing authority or I/O errors. Missing/failing output authority
can prevent the report; the program still returns 2. Launcher/compiler failures
have their own statuses.

Inclusive caps are **65,536 input bytes**, **64 records**, **64 raw alias
pairs**, and **128 UTF-8 bytes per name**, including references. The bounded
whole-file reader probes one byte beyond the limit and rejects before appending
that byte. The input byte cap bounds the JSON DOM, including ignored fields;
std additionally bounds nesting at 128. The decoder checks both array lengths
before deriving typed collections. The pure grouping entry checks their lengths
again before building its map, forest, root snapshot, sort list or output.
Names are checked before insertion into the map or lookup. Output has at most
64 components and exactly one member occurrence per record. These are logical
bounds, not a fixed RSS guarantee.

The focused harness compares exact stdout, stderr and status on the actual CLI
interpreter and both LLVM and Cranelift executables. Expected components come
from a separate adjacency reachability traversal. Checked-in rustc vectors have
explicit expected partitions and run with reversed pairs/endpoints too. The
matrix covers isolated and independent groups, transitivity, repeats/self pairs,
empty arrays, unknown and duplicate names, byte/record/pair/name limits, malformed
input, missing grants, help and repeated deterministic output. Every runtime
process is sequential and bounded by `tools/capped` at 60 seconds / 2 GiB.
Fixture hashes and an outside-grant sentinel must remain unchanged. Temporary
inputs, grants, sentinel and the private test directory are removed with
assertions. A second test uses the existing checked heap executor for the pure
model, forest value snapshots, cap and bounds errors, and zero live heap bytes.

```sh
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=groupmap_23
```

Prior art: rust-lang/rust at `15c10447a46529b68e38c7f138aeb95240e4d7d6`,
`compiler/rustc_data_structures/src/union_find.rs:28–95` and
`union_find/tests.rs:1–32`, MIT, Copyright The Rust Project Contributors;
[license](../../third-party/NOTICES.md#mit-rust). This ports usage and
singleton/transitive vectors, consuming the shipped implementation. Dense IDs,
explicit caps, named endpoints, typed rejection and stable public labels replace
rustc typed indices and direct representative assertions. The independent test
oracle uses traversal, not union-find. See [FRICTION.md](FRICTION.md).
