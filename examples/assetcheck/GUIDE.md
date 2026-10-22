# assetcheck

Verify local release assets against a JSON manifest, using explicitly granted,
read-only directories. SHA256 checks agreement with the supplied manifest. They
do not establish provenance, authenticity, or a signature. This tool never
fetches, extracts, signs, or writes assets.

From the repository root, with the matching Hirsel toolchain:

```sh
hirsel test examples/assetcheck/hirsel.json
hirsel test examples/assetcheck/hirsel.json --native --backend llvm
hirsel test examples/assetcheck/hirsel.json --native --backend cranelift
```

Create `examples/assetcheck/assets/empty` as an empty file, and put this in
`examples/assetcheck/manifests/release.json` (create both directories first):

```json
{"assets":[{"path":"empty","bytes":0,"sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}]}
```

The checked-in grants resolve their host paths relative to the grants file.
They grant stdout and read access to those two directories only. For other
fixture roots, copy the grants and set each `host` to its absolute path. Keep
recordings outside both granted roots.

```sh
hirsel run examples/assetcheck/hirsel.json --grants examples/assetcheck/grants.json -- verify release.json
hirsel run examples/assetcheck/hirsel.json --grants examples/assetcheck/grants.json --record /tmp/assetcheck-success.tape -- verify release.json
hirsel run examples/assetcheck/hirsel.json --replay /tmp/assetcheck-success.tape
hirsel run examples/assetcheck/hirsel.json --replay /tmp/assetcheck-success.tape --native --backend llvm
hirsel run examples/assetcheck/hirsel.json --replay /tmp/assetcheck-success.tape --native --backend cranelift
```

Successful stdout is exactly one JSON line:

```json
{"ok":true,"findings":[{"path":"empty","status":"ok","bytes":0,"sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}]}
```

`verify <manifest>` accepts a manifest path relative to the `manifests` grant.
Asset paths are relative to the separate `assets` grant, independent of the
manifest's directory. UTF-8 names are supported. All paths must use canonical
relative spelling: empty names, absolute paths, empty/dot/parent components,
trailing slashes, NUL, backslash and colon are refused. Duplicate paths are
refused before any asset is opened. Directory capabilities also enforce host
confinement, including symlink escape refusal. The tool does no recursive search.

The manifest contains an `assets` array. Each entry requires `path` (string),
`bytes` (unsigned integer), and `sha256` (64 hexadecimal characters, either case).
Unknown fields follow std's typed decoder policy and are skipped; duplicate JSON
keys, missing required fields and malformed JSON are rejected. An empty array
produces `{"ok":true,"findings":[]}`. Extra fields are not signed or authenticated.

Application policy limits are 16,384 manifest bytes, 64 entries, and 65,536 bytes
per asset; these are not language limits. Expected lengths above the file cap
are rejected before asset I/O. Reads enforce both byte caps from observed bytes,
including a one-byte EOF/overflow probe, without trusting file metadata. Hashing
uses 4,096-byte buffered chunks and the public incremental SHA256 API.

Findings appear in manifest order, including entries after a mismatch. Status is
`ok`, `length`, `digest`, `length_digest`, `missing`, `read`, or `limit`. Completed
reads include their actual length and lowercase SHA256. Incomplete reads use
`bytes:0,sha256:""`; these fields are unavailable, not a partial digest or measured
zero length. Only after every finding is known does the app emit its report.

A matching report exits 0 with empty stderr. A report with any failing finding
exits 1 and stderr is `error: Mismatch` plus a newline. Invalid input or a missing
grant exits 1 with empty stdout and a launcher error on stderr. Tooling and
replay divergence use Hirsel's separate launch status policy.

For native executables, pass application arguments explicitly; they replace
the grants template's Args snapshot:

```sh
hirsel build examples/assetcheck/hirsel.json --debug --backend cranelift -o /tmp/assetcheck
/tmp/assetcheck --hirsel-grants "$PWD/examples/assetcheck/grants.json" verify release.json
```

The independent Python verifier creates private fixture roots with `hashlib`
digests, checks exact reports and failures, records on H/LLVM/Cranelift and replays
each tape on all three engines with both original host roots hidden. Native
executables are built once per backend and launched directly for these checks. It also
performs fresh native runs with original grants and checks host sentinels.
Its output directory must not already exist.

```sh
python3 examples/verify-dogfood7.py --hirsel /absolute/path/to/hirsel --output /tmp/assetcheck-evidence
```

Prior art: Go `crypto/sha256/example_test.go:15–41` and
`sha256_test.go:25–35`, commit `ebe2feefc400f813aefdc8a036e66d2619da6649`
(BSD-3-Clause; see the repository's Go notice). The open/incremental-write/final
sum structure and empty/short vectors are retained. Adaptations are explicit
Dir authority, buffered borrowed chunks, bounded reads, typed manifest validation
and deterministic aggregate findings. All hashing, hex conversion and JSON
parsing/encoding are supplied by std; no private codec or hash is ported here.
