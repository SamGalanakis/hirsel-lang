# versionaudit

Check a pinned dependency list against explicit Cargo requirements, offline. This
is useful before proposing an update: prereleases need explicit opt-in, `^0.x`
updates have narrower bounds, and build metadata does not affect matching.
There is no registry access, dependency solver, package-manifest parsing or npm
range detection. Every `requirement` is passed to `semver.parse_cargo`; every
`version` is passed to strict `semver.parse`.

From a fresh checkout, use the CLI built following [Buck2 setup](../../tools/buck2/README.md).
Run from the repository root:

```sh
hirsel check examples/versionaudit/hirsel.json
hirsel run examples/versionaudit/hirsel.json --grants examples/versionaudit/grants.json
hirsel run examples/versionaudit/hirsel.json --grants examples/versionaudit/grants.json -- invalid.json
hirsel run examples/versionaudit/hirsel.json --grants examples/versionaudit/grants.json -- --help
hirsel build examples/versionaudit/hirsel.json --backend llvm -o /tmp/versionaudit
/tmp/versionaudit --hirsel-grants examples/versionaudit/grants.json success.json
# Use --backend cranelift for the other native backend.
```

`grants.json` grants stdout and read access only to `fixtures/`, resolved relative
to that grants file. Change its `host` to the directory holding your inputs.
The single argument is a basename within the logical `input` directory; empty,
dot/parent, slash, backslash, colon and NUL paths are refused. The program needs
no write, stderr, stdin, process, network, clock, task or environment grant.
`--help` needs only stdout and performs no file read.

The input is one JSON array. Each row must have exactly three string fields:

```json
[{"name":"serde","requirement":"^1.0.0","version":"1.0.228"}]
```

Names are nonempty and case sensitive. Duplicate decoded names and duplicate
JSON keys are errors; Unicode is preserved without normalization. Unknown fields,
missing fields, wrong kinds, trailing data and invalid UTF-8 are rejected.
Validation stops at the first failure in input order. No partial mismatch report
is published if any row is invalid.

Output is compact UTF-8 JSON plus LF. Mismatches retain the original name,
requirement and version and are sorted by UTF-8 name bytes. Success from the
supplied realistic fixture is exactly:

```json
{"mismatches":[],"summary":{"class":"ok","checked":3,"mismatched":0}}
```

For a valid list, the summary class is `ok` or `mismatch`; `checked` counts all
rows, and `mismatched` counts reported rows. The invalid fixture contains a
common copy/paste error, the non-SemVer `v1.0.228`, and reports:

```json
{"class":"invalid_input","error":"version","row":1,"detail":"Syntax","offset":0}
```

The model exposes a typed `InputError`. JSON and semver causes retain their typed
kind and byte offset in the JSON report. Rows are one based; row 0 means a file,
argument or whole JSON failure. JSON offsets are within the input bytes; semver
offsets are within the decoded requirement/version string. Other errors use an
empty detail and offset 0. Error codes are `input_bytes`, `row_limit`, `json`,
`fields`, `field_bytes`, `empty_name`, `duplicate_name`, `requirement`, `version`,
`usage`, `directory_grant`, `file_open`, `file_read`, and `path`.

Exit statuses are **0** for all matches or help, **1** for valid input containing
mismatches, and **2** for invalid input, grant failure or I/O failure. Stderr is
empty. Without stdout authority or when output fails, status 2 may have no report.
Compiler/launcher refusals happen before entry and have their own CLI status.

Bounds: 65,536 input bytes, 64 rows, 128 decoded UTF-8 bytes per name and 256 per
requirement/version. The counted file reader probes at most one byte beyond the
input cap and rejects before appending it. The pull reader rejects row 65 before
decoding it; only three fields per row are retained, plus at most one candidate
unknown key before rejection. Decoded strings are checked before storage in a row;
a single temporary JSON string can occupy up to the bounded input size. The name
map and mismatch list have at most 64 elements. This bounds work and data, not
exact RSS. An empty array is valid; an empty file is invalid. Pure validation and
rendering live in `model.hir`; file reads and process status selection are in
`main.hir`.

The focused CLI test compares exact stdout, stderr and exit status for the same
fixtures through H, LLVM and Cranelift, exports reachable TIR, checks input-file
hashes and asserts temporary input/grants cleanup. A separate pure-model test
uses the existing checked-heap runner. Runtime cells use tools/capped at 60 s and
2 GiB; native compilation is setup. All runtime effects use private temporary
paths. No replay format or shared runner changes are needed.

The range-policy vectors come from dtolnay/semver at
`280ebcb6edac3aa4cdc545dbff8a26c5ac4861fe`,
`src/eval.rs:1-65` and `tests/test_version_req.rs:39-75` (MIT; David Tolnay and
contributors). [LICENSE-semver](LICENSE-semver) preserves the upstream license.
The tests port the exact basic/exact lists, prerelease exclusions and build
metadata list. Deviations: use explicit `^1.0.0` where upstream writes the
implicit Cargo caret `1.0.0`; aggregate vectors into named JSON rows instead of
Rust assertions; add literal zero-major and application-policy controls. Parser
and evaluator implementations remain the shipped std versions. [FRICTION.md](FRICTION.md)
records the attempted forms and repairs.
