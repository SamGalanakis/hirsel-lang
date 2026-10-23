# invoicecheck

Reconcile a supplier invoice or expense export without binary floating point.
Each record has `invoice_id,quantity,unit_price,expected_total` in that exact
header order. `expected_total` is the supplier's **line** total. Multiply the
quantity by the exact price, round that product once to two fractional places
with HalfEven, then sum the rounded lines per invoice. Compare amounts with
`decimal.compare`; `1.0` and `1.00` agree numerically.

From the repository root, with a built `hirsel` on PATH:

```sh
hirsel check examples/invoicecheck/hirsel.json
hirsel export --tir examples/invoicecheck/hirsel.json -o /tmp/invoicecheck.tir
hirsel run examples/invoicecheck/hirsel.json --grants examples/invoicecheck/grants.json
hirsel run --native --backend llvm examples/invoicecheck/hirsel.json --grants examples/invoicecheck/grants.json
hirsel run --native --backend cranelift examples/invoicecheck/hirsel.json --grants examples/invoicecheck/grants.json
hirsel run examples/invoicecheck/hirsel.json --grants examples/invoicecheck/grants.json -- --help
hirsel run examples/invoicecheck/hirsel.json --grants examples/invoicecheck/grants.json -- invalid.csv
hirsel test examples/invoicecheck/hirsel.json
```

The success fixture reconciles a three-item charge at 19.995, a negative credit,
and a second invoice with zero quantity. It reports invoice totals `54.98` and
`2.45`. `invalid.csv` models an expense export containing a currency prefix in
its numeric column; it prints
`{"error":{"record":2,"reason":"decimal_syntax"}}` and exits 2.
To use another file, put it in the granted fixture directory and supply its base
filename. Directory hosts are relative to the grants file. The checked-in grant
allows only reads in `fixtures` and stdout. No write, stderr, process, socket,
clock, randomness or task authority is requested. `--help` needs stdout only.

Policies and bounds:

- At most 65,536 input bytes. The bounded file reader probes one extra byte to
  distinguish exact length from excess and rejects excess without parsing it.
- At most 64 data records, 64 invoice accumulators, 64 mismatch indices and 64
  report entries. The reader retains one record at a time; record 65 is rejected
  before decoding or adding it to a collection. Blank lines are skipped by std.
- Four fields per record, with at most 512 normalized CSV bytes per record.
  Std's strict CSV quoting and CRLF normalization apply. Header-only input is valid.
- Invoice IDs are 1–64 UTF-8 bytes with no ASCII controls or DEL. Quantity is 1–7
  ASCII digits representing 0 through 1,000,000, including leading zeros.
- Both decimal fields are at most 64 bytes, with at most 6 fractional digits;
  grammar is `-?[0-9]+(\.[0-9]+)?`. Currency symbols, exponents, plus signs and
  surrounding whitespace are rejected. Negative prices represent credits.
- Duplicate rows count as separate charges: without a line ID there is no safe
  basis to deduplicate. All counts and mismatch positions include duplicates.
- Report invoices follow first appearance; mismatches are one-based data-record
  indices, excluding the header and blank lines. Record 0 identifies an error
  before data decoding. Group totals always have two places; summed expected
  amounts are normalized strings. A mismatched line stays reported even when
  opposing discrepancies cancel in the invoice sum. `matches` compares group sums.

Every completed reconciliation prints one compact JSON object and a newline.
Invalid input prints one compact error object using the model's typed
`InvalidInput { record, reason: InputError }`; no partial reconciliation is emitted.
Exit 0 means reconciled or help, 1 means line mismatches, and 2 means invalid
input, usage, read failure or missing directory grant. Missing stdout or output
failure follows the launcher's `Result` error policy (exit 1 and a launcher error
on stderr). Successful, mismatched and structured invalid-input runs have empty
stderr. Launcher admission errors can have their own CLI status.

`model.hir` owns validation, typed decoding, exact arithmetic and JSON encoding;
`main.hir` owns argument and filesystem I/O. The integration test compares exact
stdout, stderr and status on H, LLVM and Cranelift against Python `Decimal` with
explicit quantization, hashes the read-only fixtures, and asserts private-file
cleanup. A separate test runs pure model controls through the existing checked-H
runner. Package tests contain the upstream HalfEven vectors and exact-add/value
comparison controls; the harness asserts their nonzero count.

Prior art: rust_decimal `b23fb077e301e77cb0ac36cc1ac4df1fba6c45d5`,
`src/ops/add.rs:1–44` (exact fold usage) and
`tests/decimal_tests/rounding.rs:73–97` (6.5 → 6, 7.5 → 8, 1.2250 → 1.22).
Copyright (c) 2016 Paul Mason, MIT; the license text is in
[third-party/NOTICES.md](../../third-party/NOTICES.md).
This application calls the shipped decimal implementation; it ports no 96-bit
arithmetic internals. Hirsel's arbitrary-size coefficients replace that storage
limit, input caps bound the application, and rescale always writes the requested
number of places. The Python oracle independently implements the application
policy. See [FRICTION.md](FRICTION.md) for authoring repairs.

See the [tested JSON, text and time recipes](../../docs/std-api-recipes.md)
for public module names, contextual variants and ownership at each call.
