# Gaps

Each line is a gap the app or the SDK met, with the message number that tracked it.
Fixed entries name their implementation and cleanup commits.

## Open

- #520546: `961c96e0` fixed repeated resource cleanup, but the unchanged native client build currently peaks at 1554 MiB against its 1536 MiB regression bound. The original `748d47a2` std/client with its matching precheck and the same compiler also exceeds the bound (1587 MiB). The client remains interpreted in e2e runs; this compiler-memory gap stays open.

## Fixed on main, workarounds removed

- #513132: `d79c14bd` added granted, recorded and replayed signals; `64f6e20c` makes the server race `signal.wait` for SIGINT/SIGTERM with its accept loop. Stdin stop remains because the CLI e2e suite and benchmark helpers close it to test shutdown with open event streams.
- #514473: `64f6e20c` adds `time.Date` JSON hooks using canonical date strings and stores `model.Item.due` as `Option[time.Date]`; validation, sorting and rendering now use dates directly. The JSON snapshot shape stays string-or-null; dates are now validated on load.
- #520100: `5bc46741` fixed HEAD/204/304 framing (`bf84c84d` completed coverage); `64f6e20c` makes `httpd.send` call `respond` directly.

- #513862: `af2d37fb`, `98a38054`: a `@test` got no `Tasks`; `test.tasks` now runs the hub and the save-then-publish path as `@test`s.
- #513128: `cbfa45b9`, `98a38054`: std http could not stream a server response; the app now uses `http.next_stream`, `respond` and, for event streams, `respond_head`/`write_response_body`/`end_response_body`.
- #513667: `9e317263`, `98a38054`: std's server request parser took at most 16 headers; `http.Config.max_headers` now sets the limit.
- #513157: `30eb10f1`: native builds refused `String` literal patterns; the protocol-name parsers now `match` on literals.
- #513475: `4465ab6d`, `98a38054`: a `unique` copy of part of a mutex payload and a record holding `Mutex`/`Shared` handles were not sendable; `Services` now carries the board and the static files.
- #513531: `4465ab6d`, `98a38054`: a listener taken as a parameter could not move into a task; `main` now listens and `app.serve` takes the listener.
- #513641: `2b035f0d`, `98a38054`: listeners did not set SO_REUSEADDR; a restarted server binds its port again at once, and the end-to-end test restarts on the same port.
- #514255: `582e03fe`: a `match` yielding a borrowed `split_once` piece or the binding it came from failed TIR validation.
- #514323: `80980c5c`: natively, freezing a value moved into a task by a `take` capture tripped a runtime invariant.
- #514473: `597505ae`: `hirsel fmt` removed the blank line after a file's header comment.
- #514473: `597505ae`: `Inline` had no `[e; N]` repeat literal (the app's own request parser, which needed it, is gone).
- #514473: `597505ae`, `98a38054`: std had no HTML escaping; the app and the SDK use `std@1/html`.
- #514473: `597505ae`, `98a38054`: a derived JSON decoder looked for a nested record's decoder in the declaring module; the forwarding wrappers are gone.

Not routed, because these are language rules with clear diagnostics rather than gaps:

- a type cannot be named `Task`, a builtin type (LANG-7), so a task is `model.Item`;
- a local named like a module (`set`, `text`, `event`, `file`) cannot be used before `.`, and a local cannot shadow a top-level name of its own module (LANG-4).
