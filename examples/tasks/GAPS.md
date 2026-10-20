# Gaps

Each line is a gap the app or the SDK met, with the message that routed it to
@agentlang-24. The workarounds still in the source are marked `// GAP:`.

## Open

- #513132: a program cannot observe SIGINT or SIGTERM, so the server stops gracefully when its granted stdin ends or reads `quit`.
- #514473: `time.Date` has no JSON form here, so due dates are stored as canonical `YYYY-MM-DD` text (`model.Item.due`).
- #520100: `http.respond` writes the body after a HEAD request and a `Content-Length` on a 204, so `httpd.send` answers HEAD, 204 and 304 with `respond_head` and `end_response_body`.

- #520546: an LLVM build of a program using std's HTTP client takes over 5 minutes and 2.3 GB, so the native end-to-end test builds only the server and runs the client interpreted.

## Fixed on main, workarounds removed

- #513862: a `@test` got no `Tasks`; `test.tasks` now runs the hub and the save-then-publish path as `@test`s.
- #513128: std http could not stream a server response; the app now uses `http.next_stream`, `respond` and, for event streams, `respond_head`/`write_response_body`/`end_response_body`.
- #513667: std's server request parser took at most 16 headers; `http.Config.max_headers` now sets the limit.
- #513157: native builds refused `String` literal patterns; the protocol-name parsers now `match` on literals.
- #513475: a `unique` copy of part of a mutex payload and a record holding `Mutex`/`Shared` handles were not sendable; `Services` now carries the board and the static files.
- #513531: a listener taken as a parameter could not move into a task; `main` now listens and `app.serve` takes the listener.
- #513641: listeners did not set SO_REUSEADDR; a restarted server binds its port again at once, and the end-to-end test restarts on the same port.
- #514255: a `match` yielding a borrowed `split_once` piece or the binding it came from failed TIR validation.
- #514323: natively, freezing a value moved into a task by a `take` capture tripped a runtime invariant.
- #514473: `hirsel fmt` removed the blank line after a file's header comment.
- #514473: `Inline` had no `[e; N]` repeat literal (the app's own request parser, which needed it, is gone).
- #514473: std had no HTML escaping; the app and the SDK use `std@1/html`.
- #514473: a derived JSON decoder looked for a nested record's decoder in the declaring module; the forwarding wrappers are gone.

Not routed, because these are language rules with clear diagnostics rather than gaps:

- a type cannot be named `Task`, a builtin type (LANG-7), so a task is `model.Item`;
- a local named like a module (`set`, `text`, `event`, `file`) cannot be used before `.`, and a local cannot shadow a top-level name of its own module (LANG-4).
