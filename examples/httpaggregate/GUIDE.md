# Run httpaggregate

`httpaggregate URL...` fetches 1–16 endpoints in argument order, then prints one
JSON array sorted by exact UTF-8 name bytes. Each endpoint must return status
200 and strict JSON `{"items":[{"name":"wool","value":3}]}`. Names are nonempty,
at most 256 UTF-8 bytes; values are u32 integer tokens. Each body is at most
65536 bytes and contains at most 1024 items. Duplicate, unknown and missing
keys fail. Totals use checked u64 addition. An empty collection prints `[]` plus
LF. Every failure leaves stdout empty. `--help` and `-h` print help and exit 0.

## Public APIs and policy

The entry uses `arg.len/text`, `cli.parse`, `io.net/stdout`, and `out.write`.
`http.request(n, config, Get(), url, [])`, `send_bodiless`, `receive_head`,
and `read_all(mut request, 65536)` provide a scoped request/body lifecycle.
The app sets `max_redirects=0`, `max_body_bytes=65536`, `decompress=false`,
and `status_as_error=false`, then requires status exactly 200 itself.
No retry or redirect is attempted. Compressed responses are rejected rather
than decoded. Connect/read/write timeouts are each 5 seconds, using durations
without a clock grant. Request resource drop closes the connection on every
return path. Unlike `fetch`, `read_all` takes the explicit aggregate body bound.

The pure model uses `json.parse` (including duplicate decoded-key rejection),
explicit object/array conversion, `u32.parse`, `map.get/insert`,
`list.sort(mut names, string.compare)`, `json.write_text_into`, and
`u64.format_into`. JSON punctuation is fixed; names use std's escaping.
Sorting keys makes output independent of map iteration. Rendering completes
before the single stdout write. A stdout device failure can still fail that write.
Only Net and stdout are granted, with no filesystem output, environment, clock,
stdin or task grants. Args carry the URLs. Network grants name the endpoint's
address and service (port), not a URL path.

## Build and verify

From the checkout root, configure the matching compiler, std image, LLVM, link
kit and runtime archives as in [native verification](../../docs/native-verification.md).
Use an external scratch directory and keep failed attempts:

```sh
export HIRSEL_JOBS=1 HIRSEL_DAEMON=off
aggregate_dir=$(mktemp -d)
manifest="$PWD/examples/httpaggregate/hirsel.json"
hirsel test "$manifest"
hirsel test --native --backend llvm "$manifest"
hirsel test --native --backend cranelift "$manifest"
hirsel build "$manifest" --backend llvm -o "$aggregate_dir/httpaggregate-llvm"
hirsel build "$manifest" --backend cranelift -o "$aggregate_dir/httpaggregate-cranelift"
```

These are the only two application builds. Reuse the exact executables below.
Pure package tests do not use a live network.

## Loopback, record and offline replay

The fixture helper binds an ephemeral loopback port, writes narrow grants and
URLs under your scratch directory, and serves the independent JSON fixtures.
Keep this process running until the recording commands finish:

```sh
python3 crates/hirsel-cli/tests/support/httpaggregate_9/verify.py \
  --serve examples/httpaggregate/fixtures --output "$aggregate_dir"
```

In a second shell with the same `aggregate_dir` and `manifest`:

```sh
base=$(cat "$aggregate_dir/base.txt")
hirsel run "$manifest" --grants "$aggregate_dir/grants.json" -- --help
hirsel run "$manifest" --grants "$aggregate_dir/grants.json" \
  --record "$aggregate_dir/interpreted.tape" -- "$base/first" "$base/second"
for backend in llvm cranelift; do
  "$aggregate_dir/httpaggregate-$backend" --hirsel-record "$aggregate_dir/$backend.tape" \
    --hirsel-grants "$aggregate_dir/grants.json" "$base/first" "$base/second"
done
# All three print exactly fixtures/combined.out, exit 0, empty stderr.
hirsel run "$manifest" --grants "$aggregate_dir/grants.json" \
  --record "$aggregate_dir/failure-interpreted.tape" -- "$base/first" "$base/bad-json"
for backend in llvm cranelift; do
  "$aggregate_dir/httpaggregate-$backend" --hirsel-record "$aggregate_dir/failure-$backend.tape" \
    --hirsel-grants "$aggregate_dir/grants.json" "$base/first" "$base/bad-json"
done
# All three failures: exit 1, empty stdout, error: Input(InvalidJson) plus LF on stderr.
```

Stop the fixture server with Ctrl-C. A live request now fails. Replay all six
recordings through every engine, with no grants or trailing operands:

```sh
for producer in interpreted llvm cranelift; do
  for prefix in '' failure-; do
    tape="$aggregate_dir/$prefix$producer.tape"
    hirsel run "$manifest" --replay "$tape"
    for backend in llvm cranelift; do
      "$aggregate_dir/httpaggregate-$backend" --hirsel-replay "$tape"
    done
  done
done
```

Each replay must reproduce its recorded stdout, stderr and exit status. Tapes
contain the admitted run, endpoint addresses and response bytes; keep them private.

The standalone integration target executes this build-once workflow, exact
argument/status/data/boundary/denied-grant assertions, and all 18 offline replays:

```sh
python3 tools/buck2/prepare.py
.buck2/bin/buck2 targets 'root//:'
.buck2/bin/buck2 test //:hirsel-cli-test-httpaggregate_9
```

It must report one executed Rust test, plus a nonzero exact observation count
from the helper. Its source is private to this target under `tests/support`.

## Prior art

Request/body lifecycle and caller-owned status policy adapt Go net/http
`client.go:450–476`; loopback response/read/close testing adapts
`client_test.go:63–81`, commit `ebe2feefc400f813aefdc8a036e66d2619da6649`.
Copyright (c) 2009 The Go Authors, BSD-3-Clause; see `fixtures/GO-LICENSE.txt`.
This is a structural adaptation, with Hirsel resource drop replacing explicit
Body.Close. Hirsel grants, no redirects/retries, exact-200 policy and size limits
are application requirements. Existing std HTTP remains unchanged.
Explicit nested JSON conversion follows `examples/configctl/model.hir:1–65`
and bounded HTTP selection follows `stdlib/std@1/http.hir:1240–1267` at
`9f149f01207d15f7bb62c501e3c43823a62f16b5`. The low-level request path replaces
fetch because its read_all bound is explicit; no schema-lane dependency is added.
