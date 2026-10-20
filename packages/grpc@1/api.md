# Public API

Generated from manifest-selected public declarations. Do not edit by hand.
Signatures retain modes, brands, attributes, relations and `uses` clauses.
Opaque representations stay hidden; bodies are not checked or evaluated.

53 exported definitions; 17 variants.

## grpc

<a id="def-grpc_401_2fgrpc_2fCallOptions"></a>

- `pub type grpc.CallOptions { metadata: Metadata, deadline: Option[Deadline], compress: bool, max_message: u64, }`

Per-call settings: metadata to send, an optional deadline, gzip compression
of request messages, and the largest response message accepted.

References: [`core@1/Deadline`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fDeadline), [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`grpc@1/grpc/Metadata`](#def-grpc_401_2fgrpc_2fMetadata)

<a id="def-grpc_401_2fgrpc_2fClientCall"></a>

- `pub type grpc.ClientCall { id: u32, deadline: Option[Deadline], compress: bool, gzip_in: bool, max_message: u64, headers: Option[Metadata], done: Option[Status], }`

One call on a channel. `headers` holds the response header metadata once
it arrives.

References: [`core@1/Deadline`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fDeadline), [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`core@1/u32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu32), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`grpc@1/grpc/Metadata`](#def-grpc_401_2fgrpc_2fMetadata), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus)

<a id="def-grpc_401_2fgrpc_2fClientConn"></a>

- `pub opaque resource type grpc.ClientConn: () uses {Net, Tasks, MonotonicClock}`

A connection to one server. `tasks` bounds blocking waits by call deadlines
(`task.scope_until`); without it deadlines are only sent as `grpc-timeout`.

References: [`core@1/MonotonicClock`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fMonotonicClock), [`core@1/Net`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fNet), [`core@1/Tasks`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fTasks)

<a id="def-grpc_401_2fgrpc_2fCode"></a>

- `pub type grpc.Code = OK | CANCELLED | UNKNOWN | INVALID_ARGUMENT | DEADLINE_EXCEEDED | NOT_FOUND | ALREADY_EXISTS | PERMISSION_DENIED | RESOURCE_EXHAUSTED | FAILED_PRECONDITION | ABORTED | OUT_OF_RANGE | UNIMPLEMENTED | INTERNAL | UNAVAILABLE | DATA_LOSS | UNAUTHENTICATED`

The canonical gRPC status codes, numbered 0 to 16 in this order.

<a id="def-grpc_401_2fgrpc_2fCode_23ABORTED"></a>

- `ABORTED -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fCode_23ALREADY_5fEXISTS"></a>

- `ALREADY_EXISTS -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fCode_23CANCELLED"></a>

- `CANCELLED -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fCode_23DATA_5fLOSS"></a>

- `DATA_LOSS -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fCode_23DEADLINE_5fEXCEEDED"></a>

- `DEADLINE_EXCEEDED -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fCode_23FAILED_5fPRECONDITION"></a>

- `FAILED_PRECONDITION -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fCode_23INTERNAL"></a>

- `INTERNAL -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fCode_23INVALID_5fARGUMENT"></a>

- `INVALID_ARGUMENT -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fCode_23NOT_5fFOUND"></a>

- `NOT_FOUND -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fCode_23OK"></a>

- `OK -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fCode_23OUT_5fOF_5fRANGE"></a>

- `OUT_OF_RANGE -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fCode_23PERMISSION_5fDENIED"></a>

- `PERMISSION_DENIED -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fCode_23RESOURCE_5fEXHAUSTED"></a>

- `RESOURCE_EXHAUSTED -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fCode_23UNAUTHENTICATED"></a>

- `UNAUTHENTICATED -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fCode_23UNAVAILABLE"></a>

- `UNAVAILABLE -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fCode_23UNIMPLEMENTED"></a>

- `UNIMPLEMENTED -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fCode_23UNKNOWN"></a>

- `UNKNOWN -> grpc.Code`

Variant of grpc.Code.

<a id="def-grpc_401_2fgrpc_2fDEFAULT_5fMAX_5fMESSAGE"></a>

- `pub const grpc.DEFAULT_MAX_MESSAGE: u64`

The largest message a peer may send unless options say otherwise: 4 MiB.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-grpc_401_2fgrpc_2fEntry"></a>

- `pub type grpc.Entry { key: String, value: Bytes, }`

One metadata entry. Keys are lowercase; a key ending in `-bin` holds arbitrary
bytes, sent base64-encoded; other values are printable ASCII.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString)

<a id="def-grpc_401_2fgrpc_2fMetadata"></a>

- `pub type grpc.Metadata = List[Entry]`

Metadata in the order it was added or received; a key may repeat.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`grpc@1/grpc/Entry`](#def-grpc_401_2fgrpc_2fEntry)

<a id="def-grpc_401_2fgrpc_2fReply"></a>

- `pub type grpc.Reply[T] { message: T, headers: Metadata, trailers: Metadata, }`

A unary call's result: the response and the server's header and trailer metadata.

References: [`grpc@1/grpc/Metadata`](#def-grpc_401_2fgrpc_2fMetadata)

<a id="def-grpc_401_2fgrpc_2fServerCall"></a>

- `pub opaque resource type grpc.ServerCall: () uses {Net, MonotonicClock}`

One call being served. `method` is the request path, such as
`/helloworld.Greeter/SayHello`; `metadata` holds the request's custom headers.
Set `headers` and `trailers` to send response metadata.

References: [`core@1/MonotonicClock`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fMonotonicClock), [`core@1/Net`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fNet)

<a id="def-grpc_401_2fgrpc_2fServerOptions"></a>

- `pub type grpc.ServerOptions { max_message: u64, compress: bool, clock: Option[MonotonicClock], }`

Server settings: the largest request message accepted, whether responses are
gzip-compressed when the client accepts it, and the clock that turns a
request's `grpc-timeout` into `ServerCall.deadline`.

References: [`core@1/MonotonicClock`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fMonotonicClock), [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-grpc_401_2fgrpc_2fStatus"></a>

- `pub type grpc.Status { code: Code, message: String, trailers: Metadata, }`

A call's outcome: its code, a message, and the metadata the peer sent with it
(the response trailers on a client; trailers to send on a server).

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`grpc@1/grpc/Code`](#def-grpc_401_2fgrpc_2fCode), [`grpc@1/grpc/Metadata`](#def-grpc_401_2fgrpc_2fMetadata)

<a id="def-grpc_401_2fgrpc_2fTransport"></a>

- `pub opaque resource type grpc.Transport: () uses {Net}`

One HTTP/2 connection carrying gRPC calls.

References: [`core@1/Net`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fNet)

<a id="def-grpc_401_2fgrpc_2fadd"></a>

- `pub fn grpc.add(mut md: Metadata, key: String, value: Bytes) -> () uses {}`

Appends an entry.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`grpc@1/grpc/Metadata`](#def-grpc_401_2fgrpc_2fMetadata)

<a id="def-grpc_401_2fgrpc_2fcall_5foptions"></a>

- `pub fn grpc.call_options() -> CallOptions uses {}`

No metadata, no deadline, no compression, 4 MiB responses.

References: [`grpc@1/grpc/CallOptions`](#def-grpc_401_2fgrpc_2fCallOptions)

<a id="def-grpc_401_2fgrpc_2fcancel"></a>

- `pub fn grpc.cancel(mut ch: ClientConn, mut call: ClientCall) -> ()`

Cancels the call: resets its stream with CANCEL.

References: [`grpc@1/grpc/ClientCall`](#def-grpc_401_2fgrpc_2fClientCall), [`grpc@1/grpc/ClientConn`](#def-grpc_401_2fgrpc_2fClientConn)

<a id="def-grpc_401_2fgrpc_2fcanceled"></a>

- `pub fn grpc.canceled(c: ServerCall) -> bool uses {}`

Whether the client reset the call.

References: [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`grpc@1/grpc/ServerCall`](#def-grpc_401_2fgrpc_2fServerCall)

<a id="def-grpc_401_2fgrpc_2fclose_5fsend"></a>

- `pub fn grpc.close_send(mut ch: ClientConn, call: ClientCall) -> Result[(), Status]`

Half-closes the call: the server sees the end of the requests.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`grpc@1/grpc/ClientCall`](#def-grpc_401_2fgrpc_2fClientCall), [`grpc@1/grpc/ClientConn`](#def-grpc_401_2fgrpc_2fClientConn), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus)

<a id="def-grpc_401_2fgrpc_2fcode_5fname"></a>

- `pub fn grpc.code_name(code: Code) -> unique String uses {}`

The canonical name of a code, such as `DEADLINE_EXCEEDED`.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`grpc@1/grpc/Code`](#def-grpc_401_2fgrpc_2fCode)

<a id="def-grpc_401_2fgrpc_2fcode_5fnumber"></a>

- `pub fn grpc.code_number(code: Code) -> u32 uses {}`

The number of a code.

References: [`core@1/u32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu32), [`grpc@1/grpc/Code`](#def-grpc_401_2fgrpc_2fCode)

<a id="def-grpc_401_2fgrpc_2fcode_5fof"></a>

- `pub fn grpc.code_of(number: u32) -> Code uses {}`

The code with a number; numbers above 16 are `UNKNOWN`, as gRPC specifies.

References: [`core@1/u32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu32), [`grpc@1/grpc/Code`](#def-grpc_401_2fgrpc_2fCode)

<a id="def-grpc_401_2fgrpc_2fdeadline"></a>

- `pub fn grpc.deadline(c: ServerCall) -> Option[Deadline] uses {}`

The request's deadline on the server's clock, when the client sent
`grpc-timeout` and the server has a clock. Bound work with
`task.scope_until(tasks, deadline, ...)`.

References: [`core@1/Deadline`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fDeadline), [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`grpc@1/grpc/ServerCall`](#def-grpc_401_2fgrpc_2fServerCall)

<a id="def-grpc_401_2fgrpc_2fdecode_5fmessage"></a>

- `pub fn grpc.decode_message(data: Bytes) -> unique String uses {}`

Decodes `grpc-message`; a malformed escape stays as written, and invalid
UTF-8 is replaced, as grpc-go does.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString)

<a id="def-grpc_401_2fgrpc_2fdecode_5ftimeout"></a>

- `pub fn grpc.decode_timeout(value: Bytes) -> Option[u64] uses {}`

The interval of a `grpc-timeout` value in nanoseconds, saturating.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-grpc_401_2fgrpc_2fdial"></a>

- `pub fn grpc.dial(n: Net, host: String, port: u16, tasks: Option[Tasks]) -> ClientConn`

A channel to `host:port` over plaintext HTTP/2 (h2c with prior knowledge),
connecting on its first call.

References: [`core@1/Net`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fNet), [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/Tasks`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fTasks), [`core@1/u16`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu16), [`grpc@1/grpc/ClientConn`](#def-grpc_401_2fgrpc_2fClientConn)

<a id="def-grpc_401_2fgrpc_2fencode_5fmessage"></a>

- `pub fn grpc.encode_message(message: String) -> unique Bytes uses {}`

The `grpc-message` encoding of a status message: bytes outside printable
ASCII, and `%`, are percent-encoded.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString)

<a id="def-grpc_401_2fgrpc_2fencode_5ftimeout"></a>

- `pub fn grpc.encode_timeout(nanoseconds: u64) -> unique String uses {}`

The `grpc-timeout` value for a remaining interval: at most eight digits and
the finest unit that fits, rounding up.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-grpc_401_2fgrpc_2ffield"></a>

- `pub fn grpc.field(fields: List[hpack.Field], name: String) -> Option[Bytes] uses {}`

The value of a header field.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`std@1/hpack/Field`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fhpack_2fField)

<a id="def-grpc_401_2fgrpc_2ffinish_5fcall"></a>

- `pub fn grpc.finish_call(mut ch: ClientConn, mut call: ClientCall) -> Status`

The status of a finished call, reading and discarding any remaining responses.

References: [`grpc@1/grpc/ClientCall`](#def-grpc_401_2fgrpc_2fClientCall), [`grpc@1/grpc/ClientConn`](#def-grpc_401_2fgrpc_2fClientConn), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus)

<a id="def-grpc_401_2fgrpc_2fframe"></a>

- `pub fn grpc.frame(message: Bytes, gzip: bool) -> unique Bytes uses {}`

The length-prefixed form of a message, gzip-compressed when `gzip`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool)

<a id="def-grpc_401_2fgrpc_2ffrom_5ffields"></a>

- `pub fn grpc.from_fields(fields: List[hpack.Field]) -> Result[Metadata, Status] uses {}`

The metadata among received header fields: every non-reserved field, with
`-bin` values base64-decoded (padded or not).

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`grpc@1/grpc/Metadata`](#def-grpc_401_2fgrpc_2fMetadata), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus), [`std@1/hpack/Field`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fhpack_2fField)

<a id="def-grpc_401_2fgrpc_2fget"></a>

- `pub fn grpc.get(md: Metadata, key: String) -> Option[Bytes] uses {}`

The first value of `key`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`grpc@1/grpc/Metadata`](#def-grpc_401_2fgrpc_2fMetadata)

<a id="def-grpc_401_2fgrpc_2fget_5fall"></a>

- `pub fn grpc.get_all(md: Metadata, key: String) -> List[Bytes] uses {}`

Every value of `key`, in order.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`grpc@1/grpc/Metadata`](#def-grpc_401_2fgrpc_2fMetadata)

<a id="def-grpc_401_2fgrpc_2fheader_5ffields"></a>

- `pub fn grpc.header_fields(md: Metadata) -> Result[List[hpack.Field], Status] uses {}`

HTTP/2 header fields for metadata, refusing reserved or malformed keys and
non-printable values of non-binary keys.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`grpc@1/grpc/Metadata`](#def-grpc_401_2fgrpc_2fMetadata), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus), [`std@1/hpack/Field`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fhpack_2fField)

<a id="def-grpc_401_2fgrpc_2finternal"></a>

- `pub fn grpc.internal(message: String) -> Status uses {}`

An `INTERNAL` status.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus)

<a id="def-grpc_401_2fgrpc_2fmethod"></a>

- `pub fn grpc.method(c: ServerCall) -> String in c uses {}`

The request path, `/package.Service/Method`.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`grpc@1/grpc/ServerCall`](#def-grpc_401_2fgrpc_2fServerCall)

<a id="def-grpc_401_2fgrpc_2freceive_5fone"></a>

- `pub fn grpc.receive_one(mut ch: ClientConn, mut call: ClientCall) -> Result[Bytes, Status]`

The only response of a call with one response: an error when the call ends
without one or sends a second.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`grpc@1/grpc/ClientCall`](#def-grpc_401_2fgrpc_2fClientCall), [`grpc@1/grpc/ClientConn`](#def-grpc_401_2fgrpc_2fClientConn), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus)

<a id="def-grpc_401_2fgrpc_2freceive_5frequest"></a>

- `pub fn grpc.receive_request(mut c: ServerCall) -> Result[Option[Bytes], Status]`

The next request message, or `None` once the client has finished sending.
A reset stream is `CANCELLED`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`grpc@1/grpc/ServerCall`](#def-grpc_401_2fgrpc_2fServerCall), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus)

<a id="def-grpc_401_2fgrpc_2freceive_5fresponse"></a>

- `pub fn grpc.receive_response(mut ch: ClientConn, mut call: ClientCall) -> Result[ Option[Bytes], Status ]`

The next response message, or `None` when the server has finished with
status OK. A non-OK status is the `Error`; it carries the trailers.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`grpc@1/grpc/ClientCall`](#def-grpc_401_2fgrpc_2fClientCall), [`grpc@1/grpc/ClientConn`](#def-grpc_401_2fgrpc_2fClientConn), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus)

<a id="def-grpc_401_2fgrpc_2freceive_5funary"></a>

- `pub fn grpc.receive_unary(mut c: ServerCall) -> Result[Bytes, Status]`

The only request of a call with one request: an error when the client sends
none or more than one.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`grpc@1/grpc/ServerCall`](#def-grpc_401_2fgrpc_2fServerCall), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus)

<a id="def-grpc_401_2fgrpc_2frequest_5fmetadata"></a>

- `pub fn grpc.request_metadata(c: ServerCall) -> Metadata in c uses {}`

The request's custom metadata.

References: [`grpc@1/grpc/Metadata`](#def-grpc_401_2fgrpc_2fMetadata), [`grpc@1/grpc/ServerCall`](#def-grpc_401_2fgrpc_2fServerCall)

<a id="def-grpc_401_2fgrpc_2fresponse_5fheaders"></a>

- `pub fn grpc.response_headers(call: ClientCall) -> Metadata uses {}`

The response header metadata, once the server's headers have arrived.

References: [`grpc@1/grpc/ClientCall`](#def-grpc_401_2fgrpc_2fClientCall), [`grpc@1/grpc/Metadata`](#def-grpc_401_2fgrpc_2fMetadata)

<a id="def-grpc_401_2fgrpc_2fresponse_5ftrailers"></a>

- `pub fn grpc.response_trailers(call: ClientCall) -> Metadata uses {}`

The response trailer metadata, once the call has finished.

References: [`grpc@1/grpc/ClientCall`](#def-grpc_401_2fgrpc_2fClientCall), [`grpc@1/grpc/Metadata`](#def-grpc_401_2fgrpc_2fMetadata)

<a id="def-grpc_401_2fgrpc_2fsend_5frequest"></a>

- `pub fn grpc.send_request(mut ch: ClientConn, call: ClientCall, message: Bytes) -> Result[ (), Status ]`

Sends one request message, gzip-compressed when the call's options say so.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`grpc@1/grpc/ClientCall`](#def-grpc_401_2fgrpc_2fClientCall), [`grpc@1/grpc/ClientConn`](#def-grpc_401_2fgrpc_2fClientConn), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus)

<a id="def-grpc_401_2fgrpc_2fsend_5fresponse"></a>

- `pub fn grpc.send_response(mut c: ServerCall, message: Bytes) -> Result[(), Status]`

Sends one response message.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`grpc@1/grpc/ServerCall`](#def-grpc_401_2fgrpc_2fServerCall), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus)

<a id="def-grpc_401_2fgrpc_2fserve"></a>

- `pub fn grpc.serve( n: Net, take stream: Stream of n, options: ServerOptions, handler: fn(mut ServerCall) -> Result[(), Status] ) -> Result[(), Status]`

Serves the calls of one accepted connection until the client closes it,
running `handler` for each call in turn. The handler's `Error` becomes the
call's status; `Ok` sends status `OK`. Messages for other calls that arrive
while one is served wait in their stream's inbox.

References: [`core@1/Net`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fNet), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/Stream`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fStream), [`grpc@1/grpc/ServerCall`](#def-grpc_401_2fgrpc_2fServerCall), [`grpc@1/grpc/ServerOptions`](#def-grpc_401_2fgrpc_2fServerOptions), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus)

<a id="def-grpc_401_2fgrpc_2fserver_5foptions"></a>

- `pub fn grpc.server_options() -> ServerOptions uses {}`

Defaults: 4 MiB requests, no response compression, no deadlines.

References: [`grpc@1/grpc/ServerOptions`](#def-grpc_401_2fgrpc_2fServerOptions)

<a id="def-grpc_401_2fgrpc_2fset_5fheader"></a>

- `pub fn grpc.set_header(mut c: ServerCall, key: String, value: Bytes) -> () uses {}`

Adds response header metadata; it is sent with the first message.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`grpc@1/grpc/ServerCall`](#def-grpc_401_2fgrpc_2fServerCall)

<a id="def-grpc_401_2fgrpc_2fset_5ftrailer"></a>

- `pub fn grpc.set_trailer(mut c: ServerCall, key: String, value: Bytes) -> () uses {}`

Adds response trailer metadata, sent with the status.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`grpc@1/grpc/ServerCall`](#def-grpc_401_2fgrpc_2fServerCall)

<a id="def-grpc_401_2fgrpc_2fstart"></a>

- `pub fn grpc.start(mut ch: ClientConn, method_path: String, options: CallOptions) -> Result[ ClientCall, Status ]`

Starts a call of `method` (`/package.Service/Method`), sending its headers.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`grpc@1/grpc/CallOptions`](#def-grpc_401_2fgrpc_2fCallOptions), [`grpc@1/grpc/ClientCall`](#def-grpc_401_2fgrpc_2fClientCall), [`grpc@1/grpc/ClientConn`](#def-grpc_401_2fgrpc_2fClientConn), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus)

<a id="def-grpc_401_2fgrpc_2fstatus"></a>

- `pub fn grpc.status(code: Code, message: String) -> Status uses {}`

A status with `code` and `message` and no trailers.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`grpc@1/grpc/Code`](#def-grpc_401_2fgrpc_2fCode), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus)

<a id="def-grpc_401_2fgrpc_2ftimeout"></a>

- `pub fn grpc.timeout(c: ServerCall) -> Option[u64] uses {}`

The interval the client allowed, in nanoseconds, as sent in `grpc-timeout`.

References: [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`grpc@1/grpc/ServerCall`](#def-grpc_401_2fgrpc_2fServerCall)

<a id="def-grpc_401_2fgrpc_2funary"></a>

- `pub fn grpc.unary( mut ch: ClientConn, method_path: String, request: Bytes, options: CallOptions ) -> Result[Reply[Bytes], Status]`

A unary call: one request, one response.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`grpc@1/grpc/CallOptions`](#def-grpc_401_2fgrpc_2fCallOptions), [`grpc@1/grpc/ClientConn`](#def-grpc_401_2fgrpc_2fClientConn), [`grpc@1/grpc/Reply`](#def-grpc_401_2fgrpc_2fReply), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus)

<a id="def-grpc_401_2fgrpc_2funknown_5fmethod"></a>

- `pub fn grpc.unknown_method(path: String, service: String) -> Status uses {}`

The `UNIMPLEMENTED` status of a request path that `service` does not serve,
worded as grpc-go words it.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`grpc@1/grpc/Status`](#def-grpc_401_2fgrpc_2fStatus)
