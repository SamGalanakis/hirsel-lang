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
launch configs; they grant no host filesystem access. The current CLI cannot pass
such inputs to `--native`; native E2E tests run the demonstrations. The calculator currently reads an expression file; stdin support is being added
separately. Host `--run` records are being wired through the CLI separately.

| Program | Job and optional second argument |
| --- | --- |
| [wordscan](wordscan/main.hir) | Count newline characters, ASCII-whitespace words and UTF-8 bytes; print lines containing a substring (default `red`). |
| [csvjson](csvjson/main.hir) | Header-based CSV to JSON objects; quoted commas/newlines, doubled quotes, CRLF and empty fields. Reject unequal row widths and duplicate headers. |
| [jsonquery](jsonquery/main.hir) | Extract a top-level JSON object member (default `flock`) and emit compact JSON. |
| [logsummary](logsummary/main.hir) | Count the first whitespace-delimited severity token, case-insensitively, in first-seen order. |
| [calculator](calculator/main.hir) | Evaluate one whitespace-separated integer expression per line, left to right; report malformed expressions and continue. Checked arithmetic retains language overflow failures. |
| [markdown](markdown/main.hir) | Render paragraphs, `#`/`##` headings and `-` lists; escape HTML metacharacters. |
| [todo](todo/main.hir) | Execute `init`, `add TEXT`, `done NUMBER`, `list`; use leading `init` to create/reset state, otherwise load and atomically save newline-separated tasks (second argument is state path, default `todo.txt`). |
| [adventure](adventure/main.hir) | Play a command-file state machine with a key, cave and locked gate. |
| [httpget](httpget/main.hir) | Fetch a UTF-8 HTTP response from an explicitly granted endpoint; `demo.json` supplies a scripted local peer. |
| [sortuniq](sortuniq/main.hir) | Sort UTF-8 lines by byte order and remove adjacent duplicates; empty fields, including the final field after a newline, are retained. |

The E2E tests are in `crates/hirsel-cli/tests/examples.rs`. They check exact output
through both CLI backends and additional supplied-file cases, including a
4,096-line sort input. These examples deliberately use only documented core and
`std@1` APIs. The HTTP test uses the existing driver simulation and native launch
fixtures, including an actual local TCP server. File-input native tests also use
that explicit fixture boundary until the production CLI accepts run records.
