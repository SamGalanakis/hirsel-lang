# argvplan

Turn a batch of proposed commands into an inspectable JSON argv plan. This is
useful before choosing which program and arguments to pass to a process tool.
The joined text is a canonical display representation; it grants no execution
authority. The program only reads its input and writes stdout/stderr.

From the repository root, with a built `hirsel` and its matching std image:

```sh
hirsel run examples/argvplan/hirsel.json --grants examples/argvplan/grants.json
hirsel export --tir examples/argvplan/hirsel.json -o /tmp/argvplan.tir
hirsel build examples/argvplan/hirsel.json --backend llvm -o /tmp/argvplan
/tmp/argvplan --hirsel-grants examples/argvplan/grants.json review.txt
```

Cranelift uses the same build command with `--backend cranelift`. The grant
contains only stdout, stderr, and read access to `examples/argvplan/fixtures`.
Its host path assumes invocation from the repository root. To inspect other
inputs, place them in that directory or change that one read-only host mapping.
For the interpreter, set `args` in a copy of the grant to `["--help"]`,
`["--comments", "review.txt"]`, or `["invalid.txt"]`. Native binaries receive
those application arguments directly. `-h` also prints help. Help needs only
stdout/stderr grants and does not open a file.

Each LF-delimited physical line is parsed independently. Spaces, tabs and CR
are shlex whitespace; blank lines produce no command. No multiline quotes or
backslash-newline continuation are accepted. One final LF terminates the last
line without introducing another record. Empty files yield an empty plan.
`--comments` explicitly enables unquoted, unescaped `#` comments; by default
`#` is literal. Quotes concatenate, empty quoted arguments remain empty,
Unicode remains UTF-8, and dollar signs, backticks, globs, pipes and redirects
are literal argument text. NUL is retained as data; consumers deciding to
execute argv must perform their own platform validation.

Success writes one compact JSON object and LF to stdout, with no stderr:

```json
{"schema":1,"commands":[{"line":1,"argv":["git","diff","--","src/agent notes.hir"],"joined":"git diff -- 'src/agent notes.hir'"}]}
```

The realistic `review.txt` fixture has three commands, including an empty
argument and literal shell syntax. `invalid.txt` contains an unterminated
quote on its second line: the whole plan is rejected with no stdout and this
typed `model.InvalidInput` report on stderr:

```json
{"kind":"unclosed_quote","line":2,"byte":12}
```

Line numbers start at 1 and byte offsets at 0 within that line, including
multibyte UTF-8 preceding the error. Shlex errors identify the opening quote
or trailing backslash. File, UTF-8 and usage errors use line 0 and byte 0;
input-size errors use line 0 and byte 65536. A line-size error points to byte
4096. Other collection limits identify the rejected line and byte 0. Invalid
UTF-8 rejects the file, with no claim to locate the malformed byte.

Bounds are application policy: 65536 file bytes, 64 physical lines (including
blank/comment lines), 4096 bytes per line excluding LF but including CR,
64 arguments per line, 512 arguments total. The bounded file reader probes
at most one byte beyond its cap. The model repeats the file-size check and
validates UTF-8 before allocating line builders. It checks record and line
caps before appending, and argument caps before retaining a command. Shlex
may temporarily produce at most 2048 words from a bounded 4096-byte line;
there is no unbounded token collection. At most 64 command records and 512
arguments reach output conversion. JSON escaping/join expansion is therefore
bounded by admitted input. Filename arguments must be single basenames of
1–256 bytes: no `.`, `..`, slash, backslash, colon, NUL or initial dash.

Exit statuses: 0 for a complete plan or help; 2 for a typed invalid-input
report (including missing directory authority, missing file or usage);
3 if stdout or stderr authority is unavailable. Unexpected output/JSON errors
return the existing typed `AppError` and runner status 1. Outputs are
deterministic, with input order retained and no time, random or process data.

The integration test invokes actual `run` and both `build --backend` commands,
then checks stdout, stderr, status, unchanged fixture hashes, absence of a
shell-created sentinel, and removal of its private input directory and grants.
A separate test uses the existing checked-H helper on the pure model. Run:

```sh
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=argvplan_23
```

Prior art: CPython `Lib/shlex.py:305–336` and
`Lib/test/test_shlex.py:95–125`, revision
`036d309cf542986a39eeba5ca805b2ec7619731d`. The implementation reuses shipped
`std@1/shlex`; fixtures port all 31 rows of that range into JSON source/argv
pairs. Python's independent `shlex.split(posix=True, comments=...)` and
`shlex.join` supply application expectations. Deviations: physical lines,
bounded input/collections, basename-only input selection, typed errors with
byte offsets, and a JSON plan replace CPython's stream/API interface. No full
shell grammar or expansion is added. PSF-2.0 copyright and license are retained
in [fixtures/PSF-LICENSE.txt](fixtures/PSF-LICENSE.txt); see also the repository
[third-party notice](../../third-party/NOTICES.md#python-software-foundation-license-version-2-cpython).
See [FRICTION.md](FRICTION.md) for authoring attempts and remaining limitations.

Nested JSON conversion uses explicit `command_plan_to_json` helpers. See the
[complete nested JSON helper recipe](../../docs/spec/std-library.md#nested-json-helpers)
for naming, module visibility and directional signatures. Missing or mistyped
helpers are labelled at the enclosing source field or payload.
