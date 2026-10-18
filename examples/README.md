# Small Hirsel tools

Every tool selects `std@1` through its `hirsel.json`. From the repository root:

```sh
hirsel run examples/wordscan/hirsel.json
hirsel run --native examples/wordscan/hirsel.json
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
Interpreted CLI runs default to one billion execution steps; `--max-steps <n>`
selects an explicit bound.

| Program | Job and optional second argument |
| --- | --- |
| [wordscan](wordscan/main.hir) | Count newline characters, ASCII-whitespace words and UTF-8 bytes; print lines containing a substring (default `red`). |
| [csvjson](csvjson/main.hir) | Header-based CSV to JSON objects; quoted commas/newlines, doubled quotes, CRLF and empty fields. Reject unequal row widths and duplicate headers. |
| [jsonquery](jsonquery/main.hir) | Extract a top-level JSON object member (default `flock`) and emit compact JSON. |
| [logsummary](logsummary/main.hir) | Count the first whitespace-delimited severity token, case-insensitively, in first-seen order. |
| [calculator](calculator/main.hir) | Evaluate one whitespace-separated integer expression per line, left to right; report malformed expressions and continue. Checked arithmetic retains language overflow failures. |
| [markdown](markdown/main.hir) | Render paragraphs, `#`/`##` headings and `-` lists; escape HTML metacharacters. |
| [todo](todo/main.hir) | Execute `add TEXT`, `done NUMBER`, `list`; missing state starts empty; load and atomically save newline-separated tasks (second argument is state path, default `todo.txt`). |
| [adventure](adventure/main.hir) | Play a command-file state machine with a key, cave and locked gate. |
| [httpget](httpget/main.hir) | Fetch a UTF-8 HTTP response from an explicitly granted endpoint; `demo.json` supplies a scripted local peer. |
| [filesystem](filesystem/README.md) | Create nested directories, list typed entries and metadata, rename within a grant and remove files and empty directories. |
| [sortuniq](sortuniq/main.hir) | Sort distinct UTF-8 lines by byte order; empty fields, including the final field after a newline, are retained. |
| [logq](logq/main.hir) | Query a JSON-lines or plain log: `[--level L] [--match TEXT] [--field KEY=VALUE] [--top N] FILE` prints matches per level and the most frequent messages; bad input exits 1 with `error: ...` on stderr. Takes flags and the file, with no demo. |

The E2E tests are in `crates/hirsel-cli/tests/examples.rs`. They check exact output
through both CLI backends and additional supplied-file cases, including a
4,096-line sort input. These examples deliberately use only documented core and
`std@1` APIs. The HTTP test uses the existing driver simulation and native launch
fixtures, including an actual local TCP server. File-input native tests also use
that explicit fixture boundary; wordscan also checks both production CLI engines
with real host file grants. The commands above exercise bound run records.
