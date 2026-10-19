# Gaps

Each line is a gap the app or the SDK works around today, with the message that
routed it to @agentlang-24. The workarounds are marked `// GAP:` in the source.

- #513128: std http cannot stream a server response, so `httpd.hir` serves HTTP/1.1 itself over a raw `Stream` and `datastar.new_sse` writes the response head by hand.
- #513667: std's server request parser takes at most 16 headers and browsers send more, so `httpd.parse` calls `http_wire.parse_request` with room for 64.
- #513132: a program cannot observe SIGINT or SIGTERM, so the server stops gracefully when its granted stdin ends or reads `quit`.
- #513157: native builds refuse `String` literal patterns, so the status, priority, sort, mode and namespace parsers compare in turn.
- #513475: a `unique` copy of part of a mutex payload is not sendable, and a record holding a `Mutex` or `Shared` handle is not sendable. So `app.board_snapshot` rebuilds the board with its constructor, and handlers take `state` and `assets` as separate parameters instead of inside `Services`.
- #513531: a resource taken as a parameter and branded by a parameter `Net` cannot move into a task, so `app.serve` listens itself instead of taking a listener.
- #513641: listeners do not set SO_REUSEADDR, so a restarted server cannot bind the port it just closed while its connections linger in TIME_WAIT. The end-to-end test runs on two ports.
- #513862: a `@test` gets no `Tasks` or clock, so the server's concurrent parts (handlers, hub, writer, event streams) are tested by the end-to-end program in `e2e/`, not by `@test`s.
- #514255: a `match` whose arms yield a borrowed piece of a `split_once` result or the binding it was split from fails TIR validation (LoanConflict), so `e2e/main.hir` copies with `unique`.
- #514323: natively, `shared.freeze` of a value moved into a task by a `take` capture trips a runtime invariant. An earlier end-to-end harness that ran the server inside a task hit it. The suite now drives the server process over HTTP, so nothing works around it any more.
- #514473: `hirsel fmt` removes the blank line after a file's header comment.
- #514473: there is no `[e; N]` repeat literal for `Inline`, so `httpd.parse` writes out its 64 empty headers.
- #514473: std has no HTML escaping, so `html.hir` provides `esc` and `escape_into`.
- #514473: `time.Date` has no serialized form, so due dates are stored as canonical `YYYY-MM-DD` text.
- #514473: a derived JSON decoder looks for a nested record's decoder in the declaring module, so `app.hir` forwards `draft_from_json` and `query_from_json` from `model`.

Not routed, because these are language rules with clear diagnostics rather than gaps:

- a type cannot be named `Task`, a builtin type (LANG-7), so a task is `model.Item`;
- a local named like a module (`set`, `text`, `event`, `file`) cannot be used before `.`, and a local cannot shadow a top-level name of its own module (LANG-4).
