# configcheck

Validate a rollout's TOML or YAML service settings into one application-owned
`ServiceConfig`, then print one normalized JSON line. Parsing and validation are
pure functions in [model.hir](model.hir); [main.hir](main.hir) handles arguments,
bounded reads, grants, output and exit status. No implicit coercion or schema DSL.

From the repository root, with a built `hirsel` on PATH:

```sh
hirsel check examples/configcheck/hirsel.json
hirsel run examples/configcheck/hirsel.json --grants examples/configcheck/grants.json
hirsel build examples/configcheck/hirsel.json --backend llvm -o /tmp/configcheck
/tmp/configcheck --hirsel-grants examples/configcheck/grants.json toml service.toml
```

The template's host directory `fixtures` is relative to the grants file. It grants
only read access to that directory and stdout; it needs no stderr, write, process,
socket or clock capability. Change its `args` to `["yaml", "service.yaml"]` for the
same configuration in YAML, `["yaml", "invalid.yaml"]` to reject a realistic
out-of-range deployment port, or `["--help"]` for help. Format selection is explicit;
the filename extension is ignored. For another directory, edit the template's
`io.dirs[0].host` and keep `access` set to `read`.

Both successful fixtures produce exactly:

```json
{"name":"checkout-api","port":8443,"retries":3,"upstreams":["catalog","payments"]}
```

`invalid.yaml` exits 2 and prints:

```json
{"error":{"kind":"port_range","path":"$[\"port\"]","line":null,"column":null}}
```

The model requires a nonempty text `name` (at most 128 UTF-8 bytes), an integer
`port` in 1..65535, optional integer `retries` in 0..10 (default 3), and optional
`upstreams` as a list of nonempty text names (default empty, each at most 128 bytes).
Unknown fields, duplicate keys and wrong kinds are rejected. Booleans, strings and
floats never become integers. Only one service record and one YAML document are
accepted; empty YAML is rejected. TOML datetimes and YAML null/custom-tagged values
are explicitly unsupported. YAML aliases follow std's expansion policy; merge-key
processing is explicitly disabled, so `<<` is an unknown application field. Core
YAML scalar resolution follows std's YAML 1.2 contract.

Reads inspect at most 8,193 bytes in chunks of at most 1,024, reject byte 8,193
before appending it, and accumulate at most 8 KiB. The pure parse API checks the
same byte cap before UTF-8 conversion. Application values have at most 64 entries
per collection, 256 total nodes, and depth 16 (root depth zero); these checks precede
application conversion or list growth. The parsers first materialize their own
bounded syntax/value trees: std caps collection nesting at 128 and YAML alias
expansion at 100,000 additional nodes. The application limit is checked after that
parse, not presented as a streaming parser limit. The checked-in fixtures are
small; boundary controls are generated privately by the integration harness.

Output key order is fixed, upstream order is retained, and text is JSON-escaped.
The first structural violation is reported in input traversal order, then the
first unknown root key, then required/name/port/retries/upstreams validation in
that order. Paths use JSON-quoted bracket keys and zero-based array indices, so
`$["a.b"]` differs from `$["a"]["b"]`. Parser errors retain std's one-based line
and Unicode scalar column at detection, with root path `$`; validation does not
invent coordinates. The typed `ConfigError` retains nested TOML/YAML errors or a
`FieldError` before the stable report is encoded.

Exit 0 means success or help; 2 means invalid input, usage, denied directory grant,
unreadable file or invalid UTF-8; 3 means stdout is unavailable. Reported input
failures leave stderr empty. A failed stdout write returns `AppError.Output` and
the launcher's ordinary error status 1. Input files are never changed.

The focused CLI tests check exact stdout, stderr and exit status for the same
fixtures on interpreter, LLVM and Cranelift. They also check source validation,
reachable TIR export, pure vectors with the existing checked-H runners and zero live heap bytes, read-only
file contents, deterministic output, caps, help, missing grants, and removal of
private files. Native compilation is setup; each runtime process is separately
bounded by `tools/capped` at 60 seconds and 2 GiB. Run:

```sh
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=configcheck_23
```

Reference structure and vectors:

- toml-rs/toml `ceac6e4412420d5a5c19e4f7293ad005cf1a31b2`,
  `crates/toml/src/de/mod.rs:1-77`: parse, then explicitly deserialize a typed
  application record. The Lisa owner example is checked by the pure vector entry.
- toml-lang/toml-test `ff49d109861c1ad25af53f687f2aef19ab650600`,
  `tests/valid/key/quoted-dots.toml:1-11` (planning excerpt 1-8): [quoted-dots.toml](fixtures/quoted-dots.toml)
  is retained verbatim, with application quoted/dotted key rejection twins.
- saphyr-rs/saphyr `b8205d5f60ea44f757f7d075fe4f3704bbe5dccc`,
  `saphyr/src/loader.rs:170-198` and `saphyr/tests/basic.rs:1-35`:
  the two-document values and [nested.yaml](fixtures/nested.yaml) are checked
  independently of service validation, including every nested record, coordinate and power value.

All three use the MIT license; copyrights and license text are retained in
[LICENSES.md](LICENSES.md). Deviations: Serde machinery is replaced by explicit
Hirsel conversion; the service policy rejects the upstream generic records and
multiple documents while the pure entry checks their parsed values. Python's
independent `tomllib` supplies valid TOML value expectations; YAML application
expectations are explicit policy literals. Shipped std parsers are reused intact.
See [FRICTION.md](FRICTION.md) for the attempted forms and repairs.
