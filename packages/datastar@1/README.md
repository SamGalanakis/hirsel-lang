# datastar@1

A server SDK for [Datastar](https://data-star.dev): it renders and writes the
`datastar-patch-elements` and `datastar-patch-signals` Server-Sent Events, and
reads the signals a browser sends. It follows the SDK specification in
starfederation/datastar (`sdk/ADR.md`), with the Go and Python SDKs' surface, and
passes the official golden SDK tests
(`conformance/PROG/PROG-1-datastar-sdk-golden.hir`).

The [API reference](api.md) is generated with
`hirsel doc packages/datastar@1/hirsel.json -o packages/datastar@1/api.md`.

## Using it

Depend on the package from a manifest and expose its module:

```json
{
  "manifest": "../../packages/datastar@1/hirsel.json",
  "package": {"name": "datastar", "version": "1"},
  "expose": [{"head": "datastar", "module": ["datastar"]}]
}
```

A handler that reads typed signals and answers with events:

```hirsel
@serializable
pub type Search {
  query: String,
}

fn search_from_json(v: json.Value) -> Result[unique Search, String] uses {} = comptime derive.from_json(
  shape(Search)
)

// GET /search?datastar={"query":"hay"}, just received on `connection`: answer with
// two events, then end the response.
fn search(take connection: http.ServerConnection, request: http.ServerRequest) -> Result[
  (),
  datastar.SseError
] {
  let found = match datastar.read_signals(request, search_from_json) {
    Ok(Search(query: query)) => take query,
    Error(_) => "",
  }
  let mut sse = datastar.new_sse(take connection)?
  let mut options = datastar.elements_options()
  options.selector = Some("#results")
  options.mode = Inner()
  datastar.patch_elements(mut sse, f"<li>{html.esc(found)}</li>", options)?
  datastar.patch_signals(mut sse, r#"{"searching":false}"#, datastar.signals_options())?
  let _ = datastar.close(take sse)?
  Ok(())
}
```

## What it provides

- **Events as text.** `patch_elements_event`, `remove_elements_event`,
  `patch_signals_event`, `patch_signals_value_event` (from a `json.Value`),
  `execute_script_event` and `redirect_event` return one event as a `String`, for
  a reply body or a test. Data lines are split on CRLF, LF and CR, so text cannot
  inject a field. Only options that differ from the specification's defaults are
  written: the event id, a retry other than 1000 ms, the selector, the mode,
  view transitions, the namespace and `onlyIfMissing`.
- **Options.** `elements_options()`, `signals_options()` and `script_options()`
  give the defaults, and their fields can be changed: `ElementsOptions`
  (`selector`, `mode: ElementPatchMode`, `use_view_transition`,
  `view_transition_selector`, `namespace: Namespace`, `event_id`, `retry_ms`),
  `SignalsOptions` (`only_if_missing`, `event_id`, `retry_ms`) and
  `ScriptOptions` (`auto_remove`, `attributes`, `event_id`, `retry_ms`).
- **The generator.** `new_sse(take connection)` starts the event stream on an
  `http.ServerConnection` that has just received its request (`http.receive_request`).
  It sends status 200 with `Content-Type: text/event-stream` and
  `Cache-Control: no-cache` at once. HTTP/1.1 frames the body in chunks; HTTP/1.0
  ends it by closing. `ServerSentEventGenerator` then writes events with
  `patch_elements`, `remove_elements`, `patch_signals`, `execute_script`,
  `redirect` and `send`. `close` ends the response and returns the connection,
  which can take the next request after HTTP/1.1.
- **Signals.** `read_signals_value(request)` reads the `datastar` query parameter
  of a GET or DELETE request, or else the JSON body. `read_signals(request,
  decode)` also decodes the result, typically with a derived
  `derive.from_json(shape(T))` of a `@serializable` record. Failures are
  `SignalsError`: `Missing`, `Query`, `Utf8`, `Json` or `Decode`.

## Limits

The SDK writes events uncompressed. Text in elements is the caller's to escape:
`std@1/html` provides `esc` for f-string holes. Script attributes are escaped by
the SDK.

## Tests

`hirsel test packages/datastar@1/hirsel.json` runs the package's own tests: line
splitting, option rendering, signal reading, and a generator over a scripted
connection. The golden suite runs in the conformance corpus.
