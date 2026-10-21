# Small Hirsel tools

Each package selects `std@1` through its `hirsel.json`; a standalone source without
a sidecar selects its toolchain's std. From the repository root:

```sh
hirsel run examples/wordscan/hirsel.json --grants '{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":[]}'
hirsel run --native examples/wordscan/hirsel.json --grants '{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":[]}'
hirsel run --simulation examples/wordscan/demo.json examples/wordscan/hirsel.json
```

No arguments selects a built-in demonstration. To process a file, copy `demo.json`,
put its UTF-8 contents in `files` under a relative path, and set `args` to that path.
For example, `{"grants":["Io","Args"],"projections":["stdout","dir"],
"args":["in.txt"],"files":{"in.txt":"hello\n"}}`. These are explicit simulation
launch configs; they grant no host filesystem access. Native supplied-file tests
use the explicit driver fixture boundary. To grant real host inputs to either
CLI backend, pass a grants template with `--grants grants.json` (the same `io`
and `args` fields below, without semantic binding fields), or export a bound
record and fill its grants and argument snapshot:

```sh
hirsel export --run examples/wordscan/hirsel.json -o /tmp/wordscan-run.json
python3 - <<'PY_RECORD'
import json
from pathlib import Path
path = Path('/tmp/wordscan-run.json')
record = json.loads(path.read_text())
record.update(io={"stdout": "stdout.bytes@1", "dirs": [
    {"path": "", "host": str(Path.cwd()), "access": "read"}
]}, args=["README.md", "Hirsel"])
path.write_text(json.dumps(record))
PY_RECORD
hirsel run examples/wordscan/hirsel.json --run /tmp/wordscan-run.json
hirsel run --native examples/wordscan/hirsel.json --run /tmp/wordscan-run.json
```

The calculator accepts an expression file, or `-` for a stdin session. For example:

```sh
printf '1 + 2\n9 / 3\n' | hirsel run examples/calculator/hirsel.json --grants examples/calculator/stdin-grants.json
printf '1 + 2\n9 / 3\n' | hirsel run --native examples/calculator/hirsel.json --grants examples/calculator/stdin-grants.json
```

Each expression produces a line immediately; blank lines are ignored and malformed
expressions produce `error`. EOF ends the session, including a final unterminated line.
Interpreted CLI runs without `--run` or `--grants` default to one billion execution steps; `--max-steps <n>`
selects an explicit bound.

| Program | Job and optional second argument |
| --- | --- |
| [wordscan](wordscan/main.hir) | Count newline characters, ASCII-whitespace words and UTF-8 bytes; print lines containing a substring (default `red`). |
| [csvjson](csvjson/main.hir) | Bounded std CSV reader to JSON objects, with `--help`, optional file/`-` stdin, and `--base64-field NAME` to decode/re-encode a field and append its SHA-256 digest. Reject unequal widths and duplicate headers; explicit stderr grants enable escaped JSON error records. |
| [jsonquery](jsonquery/main.hir) | Extract a top-level JSON object member (default `flock`) and emit compact JSON. |
| [logsummary](logsummary/main.hir) | Count the first whitespace-delimited severity token, case-insensitively, in first-seen order. |
| [calculator](calculator/main.hir) | Evaluate one whitespace-separated integer expression per line, left to right; report malformed expressions and continue. Checked arithmetic retains language overflow failures. |
| [markdown](markdown/main.hir) | Render paragraphs, `#`/`##` headings and `-` lists; escape HTML metacharacters. |
| [todo](todo/main.hir) | Execute `add TEXT`, `done NUMBER`, `list`; missing state starts empty; load and atomically save newline-separated tasks (second argument is state path, default `todo.txt`). |
| [adventure](adventure/main.hir) | Play a command-file state machine with a key, cave and locked gate. |
| [udp-query](udp-query/main.hir) | Send one datagram and receive a bounded reply; `demo.json` and `grants.json` supply a scripted echo peer. |
| [ledger-service](ledger-service/main.hir) | A ledger store at a `sim@1` place with two transaction entries, a client that re-sends in-doubt calls on a call stream, crash and lost-reply faults, replay and an upgrade trial; see [Replay a service](../docs/replay-a-service.md). |
| [unix-http](unix-http/README.md) | Serve and fetch one HTTP exchange over a Unix domain socket granted by exact path, with the std client's `unix_socket` setting as `curl --unix-socket` uses one. |
| [httpget](httpget/main.hir) | Fetch a UTF-8 HTTP response from an explicitly granted endpoint; `demo.json` supplies a scripted local peer. |
| [fetchjson](fetchjson/README.md) | POST two JSON values and decode the replies over one persistent Client connection; `demo.json` supplies a deterministic echo peer. |
| [race](race/main.hir) | Two tasks add one to a shared atomic counter by loading it and storing one more, which loses an addition when both loads run first. Its test passes under the default `fifo@1` schedule; `hirsel test race/hirsel.json --explore 20` finds a losing schedule and prints the command that replays it, as `race/README.md` shows. |
| [testrun](testrun/main.hir) | Run one test program, print each line it writes, prefixed, as it arrives, and report `PASS`, `FAIL` or `TIMEOUT`: at the deadline (second argument, milliseconds, default 5000) `task.scope_until` cancels the stream on the program's monotonic clock, which kills the test. Grant the test's exact path, cooperative tasks and the clock with `--grants`, as `testrun/README.md` shows. |
| [filesystem](filesystem/README.md) | Create nested directories, list typed entries and metadata, rename within a grant and remove files and empty directories. |
| [sortuniq](sortuniq/main.hir) | Sort distinct UTF-8 lines by byte order; empty fields, including the final field after a newline, are retained. |
| [logq](logq/main.hir) | Query a JSON-lines or plain log: `[--level L] [--match TEXT] [--field KEY=VALUE] [--top N] FILE` prints matches per level and the most frequent messages; bad input exits 1 with `error: ...` on stderr. Takes flags and the file, with no demo. |
| [mdtoc](mdtoc/main.hir) | Print a nested Markdown table of contents for `FILE`: ATX headings outside code fences, GitHub-style anchors over NFC text with `-1`, `-2` for repeats; non-UTF-8 input exits 1 with `error: ...`. |
| [bench-lite](bench-lite/main.hir) | `[WORDS [SEED [ARENA_BYTES]]]`: count the words of a seeded generated corpus in a `List`/`Map` on the heap and again in a bounded arena, print the summary and its checksum, and check the two agree. Deterministic, with no timing. |
| [kvstore](kvstore/main.hir) | `[PORT]`: serve `GET /kv`, `GET`/`PUT`/`DELETE /kv/<key>` with percent-decoded UTF-8 keys, one task per connection; entries live in a mutex and a writer task replaces `data/kv.json` atomically before each reply. |
| [tasks](tasks/README.md) | `tasks/run [PORT]`: a task manager over HTTP with [Datastar](../packages/datastar@1/README.md). Server-rendered HTML with inline editing, filters, search and sorting; every open browser updates live over an event stream; snapshots in `data/` survive restarts. Unit tests and a Hirsel end-to-end suite (`tasks/e2e`). |
| [wc2](wc2/README.md) | Count lines, ASCII words and bytes across granted files/stdin; `-u` sorts unique UTF-8 lines, with bounded line reads and explicit stderr errors. |
| [csvstats](csvstats/README.md) | Group `group,value` CSV rows into a derived JSON report of counts, sums and means. |
| [linkcheck](linkcheck/README.md) | Check URLs in bounded concurrent batches, with redirects, channel results and whole-request deadlines; scripted peers cover 200, 404 and timeout. |
| [kvd](kvd/README.md) | Durable std-store key/value TCP service, a Mutex request counter and graceful SIGTERM shutdown; scripted session grants exercise four commands and shutdown. |
| [logtail](logtail/README.md) | Follow a growing log with a parent-directory watcher, bounded regex searches, partial-line retention and UTC timestamps. |
| [mdsite](mdsite/README.md) | Recursively render Markdown to HTML with an escaped, path-preserving index and safe default rendering. |

The E2E tests are in `crates/hirsel-cli/tests/examples.rs`. They check exact output
through both CLI backends and additional supplied-file cases, including a
4,096-line sort input. These examples deliberately use only documented core and
`std@1` APIs. The HTTP test uses the existing driver simulation and native launch
fixtures, including an actual local TCP server. File-input native tests also use
that explicit fixture boundary; wordscan also checks both production CLI engines
with real host file grants. The commands above exercise bound run records.

The CSV tool uses `std@1/csv.read_record` over buffered files/stdin. Its
`examples/csvjson/stdin-grants.json` template grants stdin, stdout and stderr and
supplies `--base64-field body -` as the argument snapshot:

```sh
printf 'id,body\n1,YWJj\n' | hirsel run examples/csvjson/hirsel.json --grants examples/csvjson/stdin-grants.json
```

It validates canonical standard padded base64 in `body`, emits that field in
canonical base64 and adds `body_sha256` as lowercase hex. An optional file path
uses a granted directory instead. No arguments retains the built-in demo;
`--help` prints the declared options. Records are bounded at 1 MiB; quoted CRLF is
preserved, field counts must match headers, and a generated digest key may not
collide with a header. Output is one JSON array, so the converted output itself
remains proportional to the input row count. Errors return exit 1 after an
explicit escaped JSON-lines log record when stderr is granted. The launcher
then reports the returned `Invalid` error, as specified by PROG-3.

The tools with parsing, transformation or state logic include `@test` suites next
to those helpers. For example, run `hirsel test examples/calculator/hirsel.json`
or `hirsel test examples/kvstore/hirsel.json`; no host grants are needed.
`hirsel test examples/jsonquery/hirsel.json --native` runs the same assertions
in a compiled executable. `hirsel doc examples/kvstore/hirsel.json` documents
the package's public declarations. The filesystem and HTTP wrappers use their
interpreted and native E2Es to exercise capability behavior.

The six first-contact examples above each include `FRICTION.md` with copied diagnostics,
minimal reproducers and fixes. Their five-line READMEs show interpreted/native runs and
record/replay commands; all launch authority lives in the checked-in grants templates.

For a new single-source example, register its tests with `Example::new` and
explicit `include_str!` inputs for both source and manifest in the Rust harness.
That carries the files into remote Buck2 tests; a dynamically constructed
`repository_manifest` path alone does not register test data.

## Four first-contact workflow examples

| Program | Workflow |
| --- | --- |
| [zipinspect](zipinspect/README.md) | Bounded ZIP directory inspection and raw binary member output; standalone source without a sidecar. |
| [snapshift](snapshift/README.md) | Library plus CLI driver: direct typed JSON decode, checked Date and direct compact output. |
| [httpmirror](httpmirror/README.md) | Bounded loopback HTTP response saved under a confined directory grant. |
| [logdigest](logdigest/README.md) | Borrowed bufio chunks, severity counters and a watcher with graceful stop. |

All four started with `hirsel new`. Each has explicit grants and package tests.
`python3 examples/verify-dogfood6.py --hirsel /absolute/path/to/hirsel --tool NAME`
checks one tool's formatting, launch, successful and failing records, byte-exact
replay on all three engines and fresh LLVM/Cranelift runs. Add `--output DIR` to
retain fixtures, tapes, stdout/stderr bytes, exact commands and exit statuses.
The four `dogfood6_` CLI registrations also assert nonzero language test counts
on the interpreter and both native backends. Tapes live outside directory grants.
