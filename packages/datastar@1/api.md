# Public API

Generated from manifest-selected public declarations. Do not edit by hand.
Signatures retain modes, brands, attributes, relations and `uses` clauses.
Opaque representations stay hidden; bodies are not checked or evaluated.

36 exported definitions; 19 variants.

## datastar

<a id="def-datastar_401_2fdatastar_2fDATASTAR_5fKEY"></a>

- `pub const datastar.DATASTAR_KEY: String`

The query parameter that carries signals on GET and DELETE requests.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString)

<a id="def-datastar_401_2fdatastar_2fDEFAULT_5fRETRY_5fMS"></a>

- `pub const datastar.DEFAULT_RETRY_MS: u64`

The reconnection delay a browser assumes when an event names none, in milliseconds.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-datastar_401_2fdatastar_2fElementPatchMode"></a>

- `pub type datastar.ElementPatchMode = Outer | Inner | Remove | Replace | Prepend | Append | Before | After`

How patched elements enter the DOM. `Outer` and `Inner` morph, keeping element
state; the others replace, insert or remove without morphing.

<a id="def-datastar_401_2fdatastar_2fElementPatchMode_23After"></a>

- `After -> datastar.ElementPatchMode`

Variant of datastar.ElementPatchMode.

<a id="def-datastar_401_2fdatastar_2fElementPatchMode_23Append"></a>

- `Append -> datastar.ElementPatchMode`

Variant of datastar.ElementPatchMode.

<a id="def-datastar_401_2fdatastar_2fElementPatchMode_23Before"></a>

- `Before -> datastar.ElementPatchMode`

Variant of datastar.ElementPatchMode.

<a id="def-datastar_401_2fdatastar_2fElementPatchMode_23Inner"></a>

- `Inner -> datastar.ElementPatchMode`

Variant of datastar.ElementPatchMode.

<a id="def-datastar_401_2fdatastar_2fElementPatchMode_23Outer"></a>

- `Outer -> datastar.ElementPatchMode`

Variant of datastar.ElementPatchMode.

<a id="def-datastar_401_2fdatastar_2fElementPatchMode_23Prepend"></a>

- `Prepend -> datastar.ElementPatchMode`

Variant of datastar.ElementPatchMode.

<a id="def-datastar_401_2fdatastar_2fElementPatchMode_23Remove"></a>

- `Remove -> datastar.ElementPatchMode`

Variant of datastar.ElementPatchMode.

<a id="def-datastar_401_2fdatastar_2fElementPatchMode_23Replace"></a>

- `Replace -> datastar.ElementPatchMode`

Variant of datastar.ElementPatchMode.

<a id="def-datastar_401_2fdatastar_2fElementsOptions"></a>

- `pub type datastar.ElementsOptions { selector: Option[String], mode: ElementPatchMode, use_view_transition: bool, view_transition_selector: Option[String], namespace: Namespace, event_id: Option[String], retry_ms: u64, }`

The options of `patch_elements`; start from `elements_options()` and change fields.
`selector` targets elements by CSS selector instead of by each element's id.
`view_transition_selector` is sent only with `use_view_transition`.

References: [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`datastar@1/datastar/ElementPatchMode`](#def-datastar_401_2fdatastar_2fElementPatchMode), [`datastar@1/datastar/Namespace`](#def-datastar_401_2fdatastar_2fNamespace)

<a id="def-datastar_401_2fdatastar_2fEventType"></a>

- `pub type datastar.EventType = PatchElements | PatchSignals`

The two event types of the Datastar SSE protocol.

<a id="def-datastar_401_2fdatastar_2fEventType_23PatchElements"></a>

- `PatchElements -> datastar.EventType`

Variant of datastar.EventType.

<a id="def-datastar_401_2fdatastar_2fEventType_23PatchSignals"></a>

- `PatchSignals -> datastar.EventType`

Variant of datastar.EventType.

<a id="def-datastar_401_2fdatastar_2fNamespace"></a>

- `pub type datastar.Namespace = Html | Svg | MathMl`

The namespace new elements are created in.

<a id="def-datastar_401_2fdatastar_2fNamespace_23Html"></a>

- `Html -> datastar.Namespace`

Variant of datastar.Namespace.

<a id="def-datastar_401_2fdatastar_2fNamespace_23MathMl"></a>

- `MathMl -> datastar.Namespace`

Variant of datastar.Namespace.

<a id="def-datastar_401_2fdatastar_2fNamespace_23Svg"></a>

- `Svg -> datastar.Namespace`

Variant of datastar.Namespace.

<a id="def-datastar_401_2fdatastar_2fScriptOptions"></a>

- `pub type datastar.ScriptOptions { auto_remove: bool, attributes: List[(String, String)], event_id: Option[String], retry_ms: u64, }`

The options of `execute_script`; start from `script_options()`.
`auto_remove` removes the script element once it has run. `attributes` are written
on the script tag in order, each value HTML-escaped.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-datastar_401_2fdatastar_2fServerSentEventGenerator"></a>

- `pub opaque resource type datastar.ServerSentEventGenerator: noalloc_drop uses {Net}`

An open `text/event-stream` response on an HTTP connection. Each operation writes
one complete event and returns once it is flushed, so events arrive in call
order. A failed write means the browser has gone; the generator is then of no
further use. Dropping it closes the connection.

References: [`core@1/Net`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fNet)

<a id="def-datastar_401_2fdatastar_2fSignalsError"></a>

- `pub type datastar.SignalsError = Missing | Query(uri.QueryError) | Utf8 | Json(json.JsonError) | Decode( String )`

Why signals could not be read: the request carried no JSON in the expected place
(`Missing`), the query or body was not valid (`Query`, `Utf8`, `Json`), or the
caller's decoder rejected the value (`Decode`).

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`std@1/json/JsonError`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fJsonError), [`std@1/uri/QueryError`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2furi_2fQueryError)

<a id="def-datastar_401_2fdatastar_2fSignalsError_23Decode"></a>

- `Decode( String ) -> datastar.SignalsError`

Variant of datastar.SignalsError.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString)

<a id="def-datastar_401_2fdatastar_2fSignalsError_23Json"></a>

- `Json(json.JsonError) -> datastar.SignalsError`

Variant of datastar.SignalsError.

References: [`std@1/json/JsonError`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fJsonError)

<a id="def-datastar_401_2fdatastar_2fSignalsError_23Missing"></a>

- `Missing -> datastar.SignalsError`

Variant of datastar.SignalsError.

<a id="def-datastar_401_2fdatastar_2fSignalsError_23Query"></a>

- `Query(uri.QueryError) -> datastar.SignalsError`

Variant of datastar.SignalsError.

References: [`std@1/uri/QueryError`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2furi_2fQueryError)

<a id="def-datastar_401_2fdatastar_2fSignalsError_23Utf8"></a>

- `Utf8 -> datastar.SignalsError`

Variant of datastar.SignalsError.

<a id="def-datastar_401_2fdatastar_2fSignalsOptions"></a>

- `pub type datastar.SignalsOptions { only_if_missing: bool, event_id: Option[String], retry_ms: u64, }`

The options of `patch_signals`; start from `signals_options()`.
`only_if_missing` patches only signals the browser does not have yet.

References: [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-datastar_401_2fdatastar_2fSseError"></a>

- `pub type datastar.SseError = Http(http.HttpError)`

Why an event could not be sent: the connection failed, so the browser has gone.

References: [`std@1/http/HttpError`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fhttp_2fHttpError)

<a id="def-datastar_401_2fdatastar_2fSseError_23Http"></a>

- `Http(http.HttpError) -> datastar.SseError`

Variant of datastar.SseError.

References: [`std@1/http/HttpError`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fhttp_2fHttpError)

<a id="def-datastar_401_2fdatastar_2fclose"></a>

- `pub fn datastar.close(take sse: ServerSentEventGenerator) -> Result[ http.ServerConnection, SseError ]`

Ends the response, which the browser sees as the stream closing, and returns the
connection: after HTTP/1.1 it can receive the next request.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`datastar@1/datastar/ServerSentEventGenerator`](#def-datastar_401_2fdatastar_2fServerSentEventGenerator), [`datastar@1/datastar/SseError`](#def-datastar_401_2fdatastar_2fSseError), [`std@1/http/ServerConnection`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fhttp_2fServerConnection)

<a id="def-datastar_401_2fdatastar_2felements_5foptions"></a>

- `pub fn datastar.elements_options() -> ElementsOptions uses {}`

The defaults: target by id, morph the outer element, no view transition, HTML.

References: [`datastar@1/datastar/ElementsOptions`](#def-datastar_401_2fdatastar_2fElementsOptions)

<a id="def-datastar_401_2fdatastar_2fevent_5ftext"></a>

- `pub fn datastar.event_text( kind: EventType, data: List[String], event_id: Option[String], retry_ms: u64 ) -> unique String uses {}`

One complete SSE event, in the specification's order: `event`, `id` when given,
`retry` unless it is the default, a `data` line per entry of `data`, and the blank
line that ends the event. A data entry must not contain a line terminator; the
`_event` functions below split their text so none does.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`datastar@1/datastar/EventType`](#def-datastar_401_2fdatastar_2fEventType)

<a id="def-datastar_401_2fdatastar_2fevent_5ftype_5ftext"></a>

- `pub fn datastar.event_type_text(kind: EventType) -> String uses {}`

The protocol name of an event type.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/EventType`](#def-datastar_401_2fdatastar_2fEventType)

<a id="def-datastar_401_2fdatastar_2fexecute_5fscript"></a>

- `pub fn datastar.execute_script( mut sse: ServerSentEventGenerator, script: String, options: ScriptOptions ) -> Result[(), SseError]`

Writes `execute_script_event(script, options)`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/ScriptOptions`](#def-datastar_401_2fdatastar_2fScriptOptions), [`datastar@1/datastar/ServerSentEventGenerator`](#def-datastar_401_2fdatastar_2fServerSentEventGenerator), [`datastar@1/datastar/SseError`](#def-datastar_401_2fdatastar_2fSseError)

<a id="def-datastar_401_2fdatastar_2fexecute_5fscript_5fevent"></a>

- `pub fn datastar.execute_script_event(script: String, options: ScriptOptions) -> unique String uses {}`

The event that runs `script` in the browser: a script element appended to `body`,
with the attributes in order, then `data-effect="el.remove()"` when `auto_remove`.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/ScriptOptions`](#def-datastar_401_2fdatastar_2fScriptOptions)

<a id="def-datastar_401_2fdatastar_2fmode_5ffrom_5ftext"></a>

- `pub fn datastar.mode_from_text(text: String) -> Option[ElementPatchMode] uses {}`

The patch mode a protocol name denotes.

References: [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/ElementPatchMode`](#def-datastar_401_2fdatastar_2fElementPatchMode)

<a id="def-datastar_401_2fdatastar_2fmode_5ftext"></a>

- `pub fn datastar.mode_text(mode: ElementPatchMode) -> String uses {}`

The protocol name of a patch mode.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/ElementPatchMode`](#def-datastar_401_2fdatastar_2fElementPatchMode)

<a id="def-datastar_401_2fdatastar_2fnamespace_5ffrom_5ftext"></a>

- `pub fn datastar.namespace_from_text(text: String) -> Option[Namespace] uses {}`

The namespace a protocol name denotes.

References: [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/Namespace`](#def-datastar_401_2fdatastar_2fNamespace)

<a id="def-datastar_401_2fdatastar_2fnamespace_5ftext"></a>

- `pub fn datastar.namespace_text(namespace: Namespace) -> String uses {}`

The protocol name of a namespace.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/Namespace`](#def-datastar_401_2fdatastar_2fNamespace)

<a id="def-datastar_401_2fdatastar_2fnew_5fsse"></a>

- `pub fn datastar.new_sse(take connection: http.ServerConnection) -> Result[ ServerSentEventGenerator, SseError ]`

Starts the SSE response to the request `connection` has just received: status
200 with `Cache-Control: no-cache` and `Content-Type: text/event-stream`, sent at
once so the browser does not time out. HTTP/1.1 frames the body in chunks and
keeps the connection; HTTP/1.0 ends it by closing.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`datastar@1/datastar/ServerSentEventGenerator`](#def-datastar_401_2fdatastar_2fServerSentEventGenerator), [`datastar@1/datastar/SseError`](#def-datastar_401_2fdatastar_2fSseError), [`std@1/http/ServerConnection`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fhttp_2fServerConnection)

<a id="def-datastar_401_2fdatastar_2fpatch_5felements"></a>

- `pub fn datastar.patch_elements( mut sse: ServerSentEventGenerator, elements: String, options: ElementsOptions ) -> Result[(), SseError]`

Writes `patch_elements_event(elements, options)`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/ElementsOptions`](#def-datastar_401_2fdatastar_2fElementsOptions), [`datastar@1/datastar/ServerSentEventGenerator`](#def-datastar_401_2fdatastar_2fServerSentEventGenerator), [`datastar@1/datastar/SseError`](#def-datastar_401_2fdatastar_2fSseError)

<a id="def-datastar_401_2fdatastar_2fpatch_5felements_5fevent"></a>

- `pub fn datastar.patch_elements_event(elements: String, options: ElementsOptions) -> unique String uses {}`

The `datastar-patch-elements` event for `elements`, one or more complete HTML
elements; each top-level element needs an id unless `options.selector` is set.
Only options that differ from the defaults are written. Empty `elements` writes no
elements line, as `Remove` needs.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/ElementsOptions`](#def-datastar_401_2fdatastar_2fElementsOptions)

<a id="def-datastar_401_2fdatastar_2fpatch_5fsignals"></a>

- `pub fn datastar.patch_signals( mut sse: ServerSentEventGenerator, signals: String, options: SignalsOptions ) -> Result[(), SseError]`

Writes `patch_signals_event(signals, options)`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/ServerSentEventGenerator`](#def-datastar_401_2fdatastar_2fServerSentEventGenerator), [`datastar@1/datastar/SignalsOptions`](#def-datastar_401_2fdatastar_2fSignalsOptions), [`datastar@1/datastar/SseError`](#def-datastar_401_2fdatastar_2fSseError)

<a id="def-datastar_401_2fdatastar_2fpatch_5fsignals_5fevent"></a>

- `pub fn datastar.patch_signals_event(signals: String, options: SignalsOptions) -> unique String uses {}`

The `datastar-patch-signals` event for `signals`, JSON text applied as an RFC 7386
merge patch: a `null` member removes that signal.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/SignalsOptions`](#def-datastar_401_2fdatastar_2fSignalsOptions)

<a id="def-datastar_401_2fdatastar_2fpatch_5fsignals_5fvalue_5fevent"></a>

- `pub fn datastar.patch_signals_value_event(signals: json.Value, options: SignalsOptions) -> Result[ unique String, json.JsonError ] uses {}`

`patch_signals` for a JSON value, written compactly.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/SignalsOptions`](#def-datastar_401_2fdatastar_2fSignalsOptions), [`std@1/json/JsonError`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fJsonError), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-datastar_401_2fdatastar_2fread_5fsignals"></a>

- `pub fn datastar.read_signals[T]( request: http.ServerRequest, decode: fn(json.Value) -> Result[T, String] ) -> Result[T, SignalsError]`

The signals a request carries, decoded by `decode`, usually a derived
`from_json` (`fn form_from_json(v: json.Value) -> Result[unique Form, String]
uses {} = comptime derive.from_json(shape(Form))`).

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/SignalsError`](#def-datastar_401_2fdatastar_2fSignalsError), [`std@1/http/ServerRequest`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fhttp_2fServerRequest), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-datastar_401_2fdatastar_2fread_5fsignals_5fvalue"></a>

- `pub fn datastar.read_signals_value(request: http.ServerRequest) -> Result[ json.Value, SignalsError ] uses {}`

The signals a request carries, as JSON: the `datastar` query parameter of a GET or
DELETE request, or the body of any other method. A request without them is
`Missing`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`datastar@1/datastar/SignalsError`](#def-datastar_401_2fdatastar_2fSignalsError), [`std@1/http/ServerRequest`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fhttp_2fServerRequest), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-datastar_401_2fdatastar_2fredirect"></a>

- `pub fn datastar.redirect(mut sse: ServerSentEventGenerator, url: String) -> Result[(), SseError]`

Writes `redirect_event(url)`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/ServerSentEventGenerator`](#def-datastar_401_2fdatastar_2fServerSentEventGenerator), [`datastar@1/datastar/SseError`](#def-datastar_401_2fdatastar_2fSseError)

<a id="def-datastar_401_2fdatastar_2fredirect_5fevent"></a>

- `pub fn datastar.redirect_event(url: String) -> unique String uses {}`

The event that sends the browser to `url`, as the Go SDK's `Redirect` does.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString)

<a id="def-datastar_401_2fdatastar_2fremove_5felements"></a>

- `pub fn datastar.remove_elements( mut sse: ServerSentEventGenerator, selector: String, options: ElementsOptions ) -> Result[(), SseError]`

Writes `remove_elements_event(selector, options)`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/ElementsOptions`](#def-datastar_401_2fdatastar_2fElementsOptions), [`datastar@1/datastar/ServerSentEventGenerator`](#def-datastar_401_2fdatastar_2fServerSentEventGenerator), [`datastar@1/datastar/SseError`](#def-datastar_401_2fdatastar_2fSseError)

<a id="def-datastar_401_2fdatastar_2fremove_5felements_5fevent"></a>

- `pub fn datastar.remove_elements_event(selector: String, options: ElementsOptions) -> unique String uses {}`

The event that removes the elements `selector` matches: `patch_elements` with no
elements, the selector and mode `Remove`. Other options are kept.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/ElementsOptions`](#def-datastar_401_2fdatastar_2fElementsOptions)

<a id="def-datastar_401_2fdatastar_2fscript_5foptions"></a>

- `pub fn datastar.script_options() -> ScriptOptions uses {}`

The defaults: remove the script after it runs, no attributes.

References: [`datastar@1/datastar/ScriptOptions`](#def-datastar_401_2fdatastar_2fScriptOptions)

<a id="def-datastar_401_2fdatastar_2fsend"></a>

- `pub fn datastar.send(mut sse: ServerSentEventGenerator, text: String) -> Result[(), SseError]`

Writes `event`, complete event text from one of the `_event` functions. Rendering
once and sending the same text to many generators is how a server broadcasts.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`datastar@1/datastar/ServerSentEventGenerator`](#def-datastar_401_2fdatastar_2fServerSentEventGenerator), [`datastar@1/datastar/SseError`](#def-datastar_401_2fdatastar_2fSseError)

<a id="def-datastar_401_2fdatastar_2fsignals_5foptions"></a>

- `pub fn datastar.signals_options() -> SignalsOptions uses {}`

The defaults: patch every signal, no event id, the default retry.

References: [`datastar@1/datastar/SignalsOptions`](#def-datastar_401_2fdatastar_2fSignalsOptions)
