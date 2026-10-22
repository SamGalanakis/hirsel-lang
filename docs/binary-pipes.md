# A binary pipe with borrowed chunks

[`examples/binary-pipe`](../examples/binary-pipe/main.hir) copies stdin to stdout
without decoding bytes as text. It counts byte `0a` (LF) and reports totals on
stderr. CRLF stays CRLF; NUL, `80`, `ff`, invalid UTF-8 and a final unterminated
span all pass through unchanged. Empty input produces empty stdout and zero
counts. Counts measure LF bytes, not logical lines.

The optional argument is a buffer capacity from 1 through 65536 bytes; the default
is 4096. The package needs only stdin, stdout, stderr and the optional argument.
Its [grant file](../examples/binary-pipe/grants.json) grants no directory, network,
clock or environment access. All three projections are obtained before input is
read, so a missing projection produces no stdout prefix.

## Build, pipe, compare, replay

From the repository root, with `hirsel` on PATH:

```sh
hirsel check examples/binary-pipe/hirsel.json
hirsel test examples/binary-pipe/hirsel.json
hirsel build examples/binary-pipe/hirsel.json --backend llvm -o /tmp/binary-pipe
python3 - <<'PY'
from pathlib import Path
Path('/tmp/binary-input').write_bytes(
    bytes(range(256)) + b'\x00\x80\xff\xc0\xaf\r\n' + b'x' * 8193 + b'\xff')
PY
cat /tmp/binary-input | /tmp/binary-pipe \
  --hirsel-record /tmp/binary-pipe.tape \
  --hirsel-grants examples/binary-pipe/grants.json \
  > /tmp/binary-output 2> /tmp/binary-counts
cmp /tmp/binary-input /tmp/binary-output
sha256sum /tmp/binary-input /tmp/binary-output
cat /tmp/binary-counts
/tmp/binary-pipe --hirsel-replay /tmp/binary-pipe.tape \
  < /dev/null > /tmp/binary-replayed 2> /tmp/binary-replayed-counts
cmp /tmp/binary-output /tmp/binary-replayed
cmp /tmp/binary-counts /tmp/binary-replayed-counts
```

The two SHA-256 values must match. For this fixture, stderr is
`bytes=8457 newlines=2 max_buffered=N`, where `N` is at most 4096 and depends on
pipe read boundaries. Replay reproduces those boundaries and the exact counts
without grants or live input. To select a capacity explicitly, append `7` after the standalone grants path.
For an interpreted run, put `"args": ["7"]` in a copy of the grant file. The interpreter uses
`hirsel run examples/binary-pipe/hirsel.json --grants GRANTS --record TAPE` and
`--replay TAPE`; Cranelift builds use `--backend cranelift`.

Do not merge stderr into stdout (`2>&1`) when passing binary output onward.
The interpreter's source step budget still applies: use the built executable for
large streams. Record/replay stores observations on a tape; tape storage and the
runtime's own bookkeeping are separate from the scanner's input buffer.

## The refill, borrow, consume cycle

`scan` keeps a `bufio.Reader` and three integer counters. Each iteration:

1. Calls `bufio.read_more` explicitly on a fully consumed reader.
2. Borrows the available bytes with `bufio.peek`, which never refills.
3. Counts LF bytes and writes that exact span with `out.write`.
4. Leaves the span's lexical scope, ending its loan.
5. Calls `bufio.consume` for the entire span before the next refill.

Short reads are normal. A successful zero read ends the stream; data returned by
an earlier read is emitted before that EOF. The scanner retains at most one
capacity-sized unread chunk. It never accumulates the input, a line, or an output
copy. Refill can temporarily retain old backing storage while constructing new
storage, so allocation overhead is O(capacity), rather than a claim that the
whole process uses exactly capacity bytes. The counter `max_buffered` reports the
largest unread chunk and is asserted against the capacity in tests.

This discipline matters: `read_more` itself can append to unread data, so calling
it repeatedly without consumption would defeat the bound. A `peek` loan used
after `consume` is rejected, even when the span or consume count is zero. The
[rejected fixture](../conformance/SEM/SEM-3-binary-pipe-consume-bad.hir)
and its passing twin demonstrate that boundary.

## Failures and partial output

`out.write` returns `Result[(), OutputError]`: success confirms the entire span;
an error supplies no byte count. On a read error, the scanner stops before
emitting that refill, including any bytes the source appended before failing.
On a write error it stops immediately, without consuming or retrying the span.
Earlier successful writes remain visible, and the failed write may itself have
emitted a prefix. There is no rollback and no safe retry offset. An error writing
the final stderr counts can occur after all stdout bytes have succeeded. A failed
run therefore makes no whole-input copy guarantee; it exits unsuccessfully and
reports the existing error through the launcher.

## Independent checks and reference adaptations

Run the focused recipe oracle (Python standard library only):

```sh
python3 examples/binary-pipe/verify.py --hirsel "$(command -v hirsel)" \
  --output /tmp/binary-pipe-evidence
hirsel test examples/binary-pipe/hirsel.json --native --backend llvm
hirsel test examples/binary-pipe/hirsel.json --native --backend cranelift
```

The oracle builds each native backend once. Across H, LLVM and Cranelift, it
checks five independent binary vectors at capacities 1, 2, 7 and 4096: 60 real
OS-pipe runs and 60 grant-free replays, with exact bytes, LF counts and chunk
bounds. Nine missing-grant runs verify empty stdout; three `/dev/full` runs
exercise real output errors on Linux. Evidence retains commands, statuses, stdout,
stderr, SHA-256 values and tapes. Package tests additionally script short reads
independently of OS scheduling, check source/sink errors without retries, and
adapt zero/repeated peek plus partial/full/over-end consumption vectors.

The structure follows Go's BSD-licensed `src/bufio/bufio.go:135–207` and vectors
from `src/bufio/bufio_test.go:843–903,1565–1608`, pinned at
`ebe2feefc400f813aefdc8a036e66d2619da6649`. Hirsel-specific adaptations:

- `peek` never refills; `read_more` is explicit.
- `consume` clamps to buffered bytes, does not read, and reports no short-discard
  error. Counts are unsigned, so negative-size vectors do not apply.
- A borrowed span's loan must end before reader mutation.
- `Result[u64, E]` represents data and EOF in separate reads; Go's simultaneous
  data-plus-EOF vector becomes a nonzero read followed by zero, preserving data.
- Capacities below Go's 16-byte minimum are deliberate boundary tests. Output
  uses Hirsel's existing byte-write and capability contracts, without a Writer.

The recipe uses these API ideas and independently written fixtures; it contains
no copied Go implementation text.
