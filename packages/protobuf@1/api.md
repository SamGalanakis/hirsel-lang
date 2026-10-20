# Public API

Generated from manifest-selected public declarations. Do not edit by hand.
Signatures retain modes, brands, attributes, relations and `uses` clauses.
Opaque representations stay hidden; bodies are not checked or evaluated.

290 exported definitions; 17 variants.

## protobuf

<a id="def-protobuf_401_2fprotobuf_2fDecodeError"></a>

- `pub type protobuf.DecodeError = Truncated(u64) | Overflow(u64) | FieldNumber(u64) | WireType(u64) | EndGroup( u64 ) | Utf8(u64) | Depth(u64)`

Why binary input failed to decode, with the byte offset where it was found.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fDecodeError_23Depth"></a>

- `Depth(u64) -> protobuf.DecodeError`

Variant of protobuf.DecodeError.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fDecodeError_23EndGroup"></a>

- `EndGroup( u64 ) -> protobuf.DecodeError`

Variant of protobuf.DecodeError.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fDecodeError_23FieldNumber"></a>

- `FieldNumber(u64) -> protobuf.DecodeError`

Variant of protobuf.DecodeError.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fDecodeError_23Overflow"></a>

- `Overflow(u64) -> protobuf.DecodeError`

Variant of protobuf.DecodeError.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fDecodeError_23Truncated"></a>

- `Truncated(u64) -> protobuf.DecodeError`

Variant of protobuf.DecodeError.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fDecodeError_23Utf8"></a>

- `Utf8(u64) -> protobuf.DecodeError`

Variant of protobuf.DecodeError.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fDecodeError_23WireType"></a>

- `WireType(u64) -> protobuf.DecodeError`

Variant of protobuf.DecodeError.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fEND_5fGROUP"></a>

- `pub const protobuf.END_GROUP: u64`

The deprecated end-group wire type.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fFIXED32"></a>

- `pub const protobuf.FIXED32: u64`

The 32-bit wire type: fixed32, sfixed32, float.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fFIXED64"></a>

- `pub const protobuf.FIXED64: u64`

The 64-bit wire type: fixed64, sfixed64, double.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fJsonError"></a>

- `pub type protobuf.JsonError = Json(json.JsonError) | Invalid(String)`

Why a message could not be converted to or from JSON.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`std@1/json/JsonError`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fJsonError)

<a id="def-protobuf_401_2fprotobuf_2fJsonError_23Invalid"></a>

- `Invalid(String) -> protobuf.JsonError`

Variant of protobuf.JsonError.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString)

<a id="def-protobuf_401_2fprotobuf_2fJsonError_23Json"></a>

- `Json(json.JsonError) -> protobuf.JsonError`

Variant of protobuf.JsonError.

References: [`std@1/json/JsonError`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fJsonError)

<a id="def-protobuf_401_2fprotobuf_2fJsonOptions"></a>

- `pub type protobuf.JsonOptions { ignore_unknown_fields: bool, registry: Registry, }`

JSON parsing options: `ignore_unknown_fields` skips unknown members and enum
names; `registry` resolves `Any`.

References: [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry)

<a id="def-protobuf_401_2fprotobuf_2fLEN"></a>

- `pub const protobuf.LEN: u64`

The length-delimited wire type: string, bytes, messages, packed repeated fields.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fMAX_5fDEPTH"></a>

- `pub const protobuf.MAX_DEPTH: u64`

The deepest message nesting a decoder accepts, as in the C++ and Go runtimes.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fMAX_5fFIELD"></a>

- `pub const protobuf.MAX_FIELD: u64`

The largest field number, 2^29 - 1.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fMessageType"></a>

- `pub type protobuf.MessageType { name: String, special: bool, to_json: fn(Bytes, Registry) -> Result[json.Value, JsonError] uses {}, from_json: fn(json.Value, bool, Registry) -> Result[unique Bytes, JsonError] uses {}, }`

One message type known to a `Registry`: its full proto name, whether its JSON
is not an object of fields (a well-known type with its own mapping), and its
JSON conversions over the binary encoding. `from_json` takes
`ignore_unknown_fields` and the registry.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fRegistry"></a>

- `pub type protobuf.Registry { types: Map[String, MessageType], }`

The message types an `Any` can hold, by full name.

References: [`core@1/Map`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fMap), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fSTART_5fGROUP"></a>

- `pub const protobuf.START_GROUP: u64`

The deprecated start-group wire type, skipped as an unknown field.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fVARINT"></a>

- `pub const protobuf.VARINT: u64`

The varint wire type: int32, int64, uint32, uint64, sint32, sint64, bool, enum.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fany_5fof"></a>

- `pub fn protobuf.any_of(v: json.Value, o: JsonOptions) -> Result[(String, Bytes), JsonError] uses {}`

The type URL and binary value of an `Any` from its JSON.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fany_5fto_5fjson"></a>

- `pub fn protobuf.any_to_json(type_url: String, value: Bytes, r: Registry) -> Result[ json.Value, JsonError ] uses {}`

The JSON of an `Any`: the embedded message's members with `@type` first, or
`@type` and `value` for a well-known type with its own mapping.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fbool_5fkey"></a>

- `pub fn protobuf.bool_key(k: String) -> Result[bool, JsonError] uses {}`

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError)

<a id="def-protobuf_401_2fprotobuf_2fbool_5fof"></a>

- `pub fn protobuf.bool_of(v: json.Value) -> Result[bool, JsonError] uses {}`

A bool from JSON `true` or `false`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fbytes_5fof"></a>

- `pub fn protobuf.bytes_of(v: json.Value) -> Result[unique Bytes, JsonError] uses {}`

Bytes from a base64 JSON string: the standard or URL-safe alphabet, padded or not.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fcheck_5fdepth"></a>

- `pub fn protobuf.check_depth(depth: u64) -> Result[(), DecodeError] uses {}`

Errors when `depth` exceeds `MAX_DEPTH`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fconcat"></a>

- `pub fn protobuf.concat(a: Bytes, b: Bytes) -> unique Bytes uses {}`

`a` followed by `b`: the encoding of a message merged from both, since
parsing concatenated encodings merges them.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes)

<a id="def-protobuf_401_2fprotobuf_2fdecode_5ffailed"></a>

- `pub fn protobuf.decode_failed(e: DecodeError) -> JsonError uses {}`

References: [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError)

<a id="def-protobuf_401_2fprotobuf_2fduration_5fof"></a>

- `pub fn protobuf.duration_of(v: json.Value) -> Result[(i64, i32), JsonError] uses {}`

A Duration from its JSON string: an optional sign, digits with an optional
fraction of up to 9 digits, and `s`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fduration_5ftext"></a>

- `pub fn protobuf.duration_text(seconds: i64, nanos: i32) -> Result[unique String, JsonError] uses {}`

The JSON text of a Duration: seconds with 0, 3, 6 or 9 fraction digits and `s`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError)

<a id="def-protobuf_401_2fprotobuf_2fexpected"></a>

- `pub fn protobuf.expected(what: String, kind: String) -> JsonError uses {}`

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError)

<a id="def-protobuf_401_2fprotobuf_2ff32_5fof"></a>

- `pub fn protobuf.f32_of(v: json.Value) -> Result[f32, JsonError] uses {}`

A float from JSON, as `f64_of`; a number beyond the float range is an error.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/f32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2ff32), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2ff64_5fof"></a>

- `pub fn protobuf.f64_of(v: json.Value) -> Result[f64, JsonError] uses {}`

A double from a JSON number, a numeric string, or `"NaN"`, `"Infinity"` and
`"-Infinity"`. A number beyond the double range is an error.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/f64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2ff64), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2ffield_5fmask_5fof"></a>

- `pub fn protobuf.field_mask_of(v: json.Value) -> Result[List[String], JsonError] uses {}`

A FieldMask's paths from its JSON string.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2ffield_5fmask_5ftext"></a>

- `pub fn protobuf.field_mask_text(paths: List[String]) -> Result[unique String, JsonError] uses {}`

The JSON text of a FieldMask: lowerCamelCase paths joined by commas. A path
that would not convert back to itself is an error.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError)

<a id="def-protobuf_401_2fprotobuf_2ffirst_5fmember"></a>

- `pub fn protobuf.first_member(mut seen: Set[u64], id: u64, key: String) -> Result[(), JsonError] uses {}`

`Ok` the first time `id` is seen; a second member for one field, or a second
member of one oneof, is an error.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/Set`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fSet), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError)

<a id="def-protobuf_401_2fprotobuf_2fi32_5fkey"></a>

- `pub fn protobuf.i32_key(k: String) -> Result[i32, JsonError] uses {}`

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError)

<a id="def-protobuf_401_2fprotobuf_2fi32_5fof"></a>

- `pub fn protobuf.i32_of(v: json.Value) -> Result[i32, JsonError] uses {}`

An int32 from a JSON number or numeric string.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fi64_5fkey"></a>

- `pub fn protobuf.i64_key(k: String) -> Result[i64, JsonError] uses {}`

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError)

<a id="def-protobuf_401_2fprotobuf_2fi64_5fof"></a>

- `pub fn protobuf.i64_of(v: json.Value) -> Result[i64, JsonError] uses {}`

An int64 from a JSON number or numeric string.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fint32_5fvarint"></a>

- `pub fn protobuf.int32_varint(v: i32) -> u64 uses {}`

The varint of an int32: negative values sign-extend to ten bytes.

References: [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fint64_5fvarint"></a>

- `pub fn protobuf.int64_varint(v: i64) -> u64 uses {}`

The varint of an int64.

References: [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fis_5fnull"></a>

- `pub fn protobuf.is_null(v: json.Value) -> bool uses {}`

Whether a JSON value is `null`.

References: [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fjson_5fbool"></a>

- `pub fn protobuf.json_bool(x: bool) -> json.Value uses {}`

References: [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fjson_5fbytes"></a>

- `pub fn protobuf.json_bytes(x: Bytes) -> json.Value uses {}`

Bytes as standard padded base64.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fjson_5ff32"></a>

- `pub fn protobuf.json_f32(x: f32) -> json.Value uses {}`

A float as its shortest round-trip decimal, or `"NaN"`, `"Infinity"`, `"-Infinity"`.

References: [`core@1/f32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2ff32), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fjson_5ff64"></a>

- `pub fn protobuf.json_f64(x: f64) -> json.Value uses {}`

A double as its shortest round-trip decimal, or `"NaN"`, `"Infinity"`, `"-Infinity"`.

References: [`core@1/f64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2ff64), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fjson_5fi32"></a>

- `pub fn protobuf.json_i32(x: i32) -> json.Value uses {}`

References: [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fjson_5fi64"></a>

- `pub fn protobuf.json_i64(x: i64) -> json.Value uses {}`

64-bit integers are JSON strings.

References: [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fjson_5fnumber"></a>

- `pub fn protobuf.json_number(x: f64) -> Result[json.Value, JsonError] uses {}`

A `google.protobuf.Value` number, which JSON cannot hold when not finite.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/f64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2ff64), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fjson_5foptions"></a>

- `pub fn protobuf.json_options(r: Registry) -> JsonOptions uses {}`

Options that reject unknown fields, resolving `Any` through `r`.

References: [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry)

<a id="def-protobuf_401_2fprotobuf_2fjson_5fstring"></a>

- `pub fn protobuf.json_string(x: String) -> json.Value uses {}`

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fjson_5ftext"></a>

- `pub fn protobuf.json_text(v: json.Value) -> Result[unique String, JsonError] uses {}`

Compact JSON text of a generated `to_json_` result.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fjson_5fu32"></a>

- `pub fn protobuf.json_u32(x: u32) -> json.Value uses {}`

References: [`core@1/u32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu32), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fjson_5fu64"></a>

- `pub fn protobuf.json_u64(x: u64) -> json.Value uses {}`

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fkeep"></a>

- `@preserves_unique(unknown) pub fn protobuf.keep(data: Bytes, start: u64, end: u64, mut unknown: List[u8]) -> () uses {}`

Appends `data[start..end]`, an unknown field's tag and value, to `unknown`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fkey_5fbool"></a>

- `pub fn protobuf.key_bool(k: bool) -> unique String uses {}`

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool)

<a id="def-protobuf_401_2fprotobuf_2fkey_5fi32"></a>

- `pub fn protobuf.key_i32(k: i32) -> unique String uses {}`

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32)

<a id="def-protobuf_401_2fprotobuf_2fkey_5fi64"></a>

- `pub fn protobuf.key_i64(k: i64) -> unique String uses {}`

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64)

<a id="def-protobuf_401_2fprotobuf_2fkey_5fstring"></a>

- `pub fn protobuf.key_string(k: String) -> unique String uses {}`

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString)

<a id="def-protobuf_401_2fprotobuf_2fkey_5fu32"></a>

- `pub fn protobuf.key_u32(k: u32) -> unique String uses {}`

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/u32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu32)

<a id="def-protobuf_401_2fprotobuf_2fkey_5fu64"></a>

- `pub fn protobuf.key_u64(k: u64) -> unique String uses {}`

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fparse_5fjson"></a>

- `pub fn protobuf.parse_json(take text: String) -> Result[json.Value, JsonError] uses {}`

Parses JSON text for a generated `from_json_` function.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fput_5fbool"></a>

- `@preserves_unique(out) pub fn protobuf.put_bool(field: u64, v: bool, mut out: List[u8]) -> () uses {}`

Appends a bool field.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5fbytes"></a>

- `@preserves_unique(out) pub fn protobuf.put_bytes(field: u64, v: Bytes, mut out: List[u8]) -> () uses {}`

Appends a bytes field, or an embedded message from its encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5fdouble"></a>

- `@preserves_unique(out) pub fn protobuf.put_double(field: u64, v: f64, mut out: List[u8]) -> () uses {}`

Appends a double field.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/f64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2ff64), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5fenum"></a>

- `@preserves_unique(out) pub fn protobuf.put_enum(field: u64, v: i32, mut out: List[u8]) -> () uses {}`

Appends an enum field from its number.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5ffixed32"></a>

- `@preserves_unique(out) pub fn protobuf.put_fixed32(field: u64, v: u32, mut out: List[u8]) -> () uses {}`

Appends a fixed32 field.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu32), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5ffixed64"></a>

- `@preserves_unique(out) pub fn protobuf.put_fixed64(field: u64, v: u64, mut out: List[u8]) -> () uses {}`

Appends a fixed64 field.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5ffloat"></a>

- `@preserves_unique(out) pub fn protobuf.put_float(field: u64, v: f32, mut out: List[u8]) -> () uses {}`

Appends a float field.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/f32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2ff32), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5fint32"></a>

- `@preserves_unique(out) pub fn protobuf.put_int32(field: u64, v: i32, mut out: List[u8]) -> () uses {}`

Appends an int32 field.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5fint64"></a>

- `@preserves_unique(out) pub fn protobuf.put_int64(field: u64, v: i64, mut out: List[u8]) -> () uses {}`

Appends an int64 field.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5fmessage"></a>

- `@preserves_unique(out) pub fn protobuf.put_message(field: u64, body: List[u8], mut out: List[u8]) -> () uses {}`

Appends an embedded message field from its encoded fields, even when empty.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5fpacked"></a>

- `@preserves_unique(out) pub fn protobuf.put_packed(field: u64, run: List[u8], mut out: List[u8]) -> () uses {}`

Appends a packed run, given the concatenated encodings of its elements.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5fsfixed32"></a>

- `@preserves_unique(out) pub fn protobuf.put_sfixed32(field: u64, v: i32, mut out: List[u8]) -> () uses {}`

Appends a sfixed32 field.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5fsfixed64"></a>

- `@preserves_unique(out) pub fn protobuf.put_sfixed64(field: u64, v: i64, mut out: List[u8]) -> () uses {}`

Appends a sfixed64 field.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5fsint32"></a>

- `@preserves_unique(out) pub fn protobuf.put_sint32(field: u64, v: i32, mut out: List[u8]) -> () uses {}`

Appends a sint32 field.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5fsint64"></a>

- `@preserves_unique(out) pub fn protobuf.put_sint64(field: u64, v: i64, mut out: List[u8]) -> () uses {}`

Appends a sint64 field.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5fstring"></a>

- `@preserves_unique(out) pub fn protobuf.put_string(field: u64, v: String, mut out: List[u8]) -> () uses {}`

Appends a string field.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5fuint32"></a>

- `@preserves_unique(out) pub fn protobuf.put_uint32(field: u64, v: u32, mut out: List[u8]) -> () uses {}`

Appends a uint32 field.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu32), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fput_5fuint64"></a>

- `@preserves_unique(out) pub fn protobuf.put_uint64(field: u64, v: u64, mut out: List[u8]) -> () uses {}`

Appends a uint64 field.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fread_5fbool"></a>

- `pub fn protobuf.read_bool(data: Bytes, mut pos: u64) -> Result[bool, DecodeError] uses {}`

Reads a bool: any nonzero varint is true.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5fbytes"></a>

- `pub fn protobuf.read_bytes(data: Bytes, mut pos: u64) -> Result[unique Bytes, DecodeError] uses {}`

Reads length-delimited bytes as a fresh copy.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5fdouble"></a>

- `pub fn protobuf.read_double(data: Bytes, mut pos: u64) -> Result[f64, DecodeError] uses {}`

Reads a double from its little-endian IEEE 754 bits.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/f64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2ff64), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5fenum"></a>

- `pub fn protobuf.read_enum(data: Bytes, mut pos: u64) -> Result[i32, DecodeError] uses {}`

Reads an enum number: an int32 varint.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5ffixed32"></a>

- `pub fn protobuf.read_fixed32(data: Bytes, mut pos: u64) -> Result[u32, DecodeError] uses {}`

Reads four little-endian bytes.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu32), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5ffixed64"></a>

- `pub fn protobuf.read_fixed64(data: Bytes, mut pos: u64) -> Result[u64, DecodeError] uses {}`

Reads eight little-endian bytes.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5ffloat"></a>

- `pub fn protobuf.read_float(data: Bytes, mut pos: u64) -> Result[f32, DecodeError] uses {}`

Reads a float from its little-endian IEEE 754 bits.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/f32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2ff32), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5fint32"></a>

- `pub fn protobuf.read_int32(data: Bytes, mut pos: u64) -> Result[i32, DecodeError] uses {}`

Reads an int32: the low 32 bits of a varint, so a sign-extended ten-byte
encoding and a five-byte one decode alike.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5fint64"></a>

- `pub fn protobuf.read_int64(data: Bytes, mut pos: u64) -> Result[i64, DecodeError] uses {}`

Reads an int64 varint as its two's complement value.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5flen"></a>

- `pub fn protobuf.read_len(data: Bytes, mut pos: u64) -> Result[Bytes, DecodeError] in data uses {}`

Reads a length prefix and returns the delimited bytes as a slice of `data`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5fpacked"></a>

- `pub fn protobuf.read_packed[T]( data: Bytes, mut pos: u64, read: fn(Bytes, mut u64) -> Result[T, DecodeError] uses {}, mut out: List[T] ) -> Result[(), DecodeError] uses {}`

Reads every element of a packed run (the bytes of one length-delimited
field) with `read`, appending them to `out`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5fsfixed32"></a>

- `pub fn protobuf.read_sfixed32(data: Bytes, mut pos: u64) -> Result[i32, DecodeError] uses {}`

Reads a sfixed32.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5fsfixed64"></a>

- `pub fn protobuf.read_sfixed64(data: Bytes, mut pos: u64) -> Result[i64, DecodeError] uses {}`

Reads a sfixed64.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5fsint32"></a>

- `pub fn protobuf.read_sint32(data: Bytes, mut pos: u64) -> Result[i32, DecodeError] uses {}`

Reads a zigzag-encoded sint32.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5fsint64"></a>

- `pub fn protobuf.read_sint64(data: Bytes, mut pos: u64) -> Result[i64, DecodeError] uses {}`

Reads a zigzag-encoded sint64.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5fstring"></a>

- `pub fn protobuf.read_string(data: Bytes, mut pos: u64) -> Result[unique String, DecodeError] uses {}`

Reads a length-delimited UTF-8 string. Invalid UTF-8 is `Utf8`, as proto3 requires.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5ftag"></a>

- `pub fn protobuf.read_tag(data: Bytes, mut pos: u64) -> Result[(u64, u64), DecodeError] uses {}`

Reads a field tag as `(field number, wire type)`. Field number 0, a number above
`MAX_FIELD` and the unused wire types 6 and 7 are errors.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5fuint32"></a>

- `pub fn protobuf.read_uint32(data: Bytes, mut pos: u64) -> Result[u32, DecodeError] uses {}`

Reads a uint32: the low 32 bits of a varint.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu32), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5fuint64"></a>

- `pub fn protobuf.read_uint64(data: Bytes, mut pos: u64) -> Result[u64, DecodeError] uses {}`

Reads a uint64 varint.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fread_5fvarint"></a>

- `pub fn protobuf.read_varint(data: Bytes, mut pos: u64) -> Result[u64, DecodeError] uses {}`

Reads one base-128 varint. A tenth byte above 1 overflows 64 bits.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fregistry"></a>

- `pub fn protobuf.registry(types: List[MessageType]) -> Registry uses {}`

A registry of `types`, such as `list.concat(protobuf.wkt.message_types(), message_types())`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry)

<a id="def-protobuf_401_2fprotobuf_2fskip"></a>

- `pub fn protobuf.skip(data: Bytes, mut pos: u64, field: u64, wire: u64) -> Result[(), DecodeError] uses {}`

Skips the value of a field whose tag was just read, including a whole group.
An end-group tag here has no matching start, so it is `EndGroup`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError)

<a id="def-protobuf_401_2fprotobuf_2fstring_5fkey"></a>

- `pub fn protobuf.string_key(k: String) -> Result[unique String, JsonError] uses {}`

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError)

<a id="def-protobuf_401_2fprotobuf_2fstring_5fof"></a>

- `pub fn protobuf.string_of(v: json.Value) -> Result[unique String, JsonError] uses {}`

A string from a JSON string.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2ftimestamp_5fof"></a>

- `pub fn protobuf.timestamp_of(v: json.Value) -> Result[(i64, i32), JsonError] uses {}`

A Timestamp from its RFC 3339 JSON string: `YYYY-MM-DDTHH:MM:SS`, an optional
fraction of 1 to 9 digits, and `Z` or a `+HH:MM`/`-HH:MM` offset.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2ftimestamp_5ftext"></a>

- `pub fn protobuf.timestamp_text(seconds: i64, nanos: i32) -> Result[unique String, JsonError] uses {}`

The RFC 3339 JSON text of a Timestamp in UTC, with 0, 3, 6 or 9 fraction digits.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError)

<a id="def-protobuf_401_2fprotobuf_2fu32_5fkey"></a>

- `pub fn protobuf.u32_key(k: String) -> Result[u32, JsonError] uses {}`

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/u32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu32), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError)

<a id="def-protobuf_401_2fprotobuf_2fu32_5fof"></a>

- `pub fn protobuf.u32_of(v: json.Value) -> Result[u32, JsonError] uses {}`

A uint32 from a JSON number or numeric string.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu32), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fu64_5fkey"></a>

- `pub fn protobuf.u64_key(k: String) -> Result[u64, JsonError] uses {}`

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError)

<a id="def-protobuf_401_2fprotobuf_2fu64_5fof"></a>

- `pub fn protobuf.u64_of(v: json.Value) -> Result[u64, JsonError] uses {}`

A uint64 from a JSON number or numeric string.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2funknown_5fenum"></a>

- `pub fn protobuf.unknown_enum(name: String, full: String) -> JsonError uses {}`

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError)

<a id="def-protobuf_401_2fprotobuf_2funknown_5ffield"></a>

- `pub fn protobuf.unknown_field(o: JsonOptions, key: String) -> Result[(), JsonError] uses {}`

An unknown member: an error unless the options ignore unknown fields.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions)

<a id="def-protobuf_401_2fprotobuf_2funzigzag32"></a>

- `pub fn protobuf.unzigzag32(v: u32) -> i32 uses {}`

The two's complement value of a zigzag-encoded 32-bit integer.

References: [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/u32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu32)

<a id="def-protobuf_401_2fprotobuf_2funzigzag64"></a>

- `pub fn protobuf.unzigzag64(v: u64) -> i64 uses {}`

The two's complement value of a zigzag-encoded 64-bit integer.

References: [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5fbool"></a>

- `@preserves_unique(out) pub fn protobuf.write_bool(v: bool, mut out: List[u8]) -> () uses {}`

Appends a bool value without a tag.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5fdouble"></a>

- `@preserves_unique(out) pub fn protobuf.write_double(v: f64, mut out: List[u8]) -> () uses {}`

Appends a double value without a tag.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/f64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2ff64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5fenum"></a>

- `@preserves_unique(out) pub fn protobuf.write_enum(v: i32, mut out: List[u8]) -> () uses {}`

Appends an enum number without a tag.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5ffixed32"></a>

- `@preserves_unique(out) pub fn protobuf.write_fixed32(v: u32, mut out: List[u8]) -> () uses {}`

Appends four little-endian bytes.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu32), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5ffixed64"></a>

- `@preserves_unique(out) pub fn protobuf.write_fixed64(v: u64, mut out: List[u8]) -> () uses {}`

Appends eight little-endian bytes.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5ffloat"></a>

- `@preserves_unique(out) pub fn protobuf.write_float(v: f32, mut out: List[u8]) -> () uses {}`

Appends a float value without a tag.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/f32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2ff32), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5fint32"></a>

- `@preserves_unique(out) pub fn protobuf.write_int32(v: i32, mut out: List[u8]) -> () uses {}`

Appends an int32 value without a tag, as in a packed run.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5fint64"></a>

- `@preserves_unique(out) pub fn protobuf.write_int64(v: i64, mut out: List[u8]) -> () uses {}`

Appends an int64 value without a tag.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5flen"></a>

- `@preserves_unique(out) pub fn protobuf.write_len(data: Bytes, mut out: List[u8]) -> () uses {}`

Appends a length prefix and the bytes it delimits.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5fraw"></a>

- `@preserves_unique(out) pub fn protobuf.write_raw(data: Bytes, mut out: List[u8]) -> () uses {}`

Appends bytes verbatim, such as preserved unknown fields.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5fsfixed32"></a>

- `@preserves_unique(out) pub fn protobuf.write_sfixed32(v: i32, mut out: List[u8]) -> () uses {}`

Appends a sfixed32 value without a tag.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5fsfixed64"></a>

- `@preserves_unique(out) pub fn protobuf.write_sfixed64(v: i64, mut out: List[u8]) -> () uses {}`

Appends a sfixed64 value without a tag.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5fsint32"></a>

- `@preserves_unique(out) pub fn protobuf.write_sint32(v: i32, mut out: List[u8]) -> () uses {}`

Appends a sint32 value without a tag.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5fsint64"></a>

- `@preserves_unique(out) pub fn protobuf.write_sint64(v: i64, mut out: List[u8]) -> () uses {}`

Appends a sint64 value without a tag.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5ftag"></a>

- `@preserves_unique(out) pub fn protobuf.write_tag(field: u64, wire: u64, mut out: List[u8]) -> () uses {}`

Appends a field tag.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5fuint32"></a>

- `@preserves_unique(out) pub fn protobuf.write_uint32(v: u32, mut out: List[u8]) -> () uses {}`

Appends a uint32 value without a tag.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu32), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5fuint64"></a>

- `@preserves_unique(out) pub fn protobuf.write_uint64(v: u64, mut out: List[u8]) -> () uses {}`

Appends a uint64 value without a tag.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fwrite_5fvarint"></a>

- `@preserves_unique(out) pub fn protobuf.write_varint(v: u64, mut out: List[u8]) -> () uses {}`

Appends a base-128 varint.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8)

<a id="def-protobuf_401_2fprotobuf_2fzigzag32"></a>

- `pub fn protobuf.zigzag32(v: i32) -> u32 uses {}`

The zigzag encoding of a 32-bit integer.

References: [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/u32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu32)

<a id="def-protobuf_401_2fprotobuf_2fzigzag64"></a>

- `pub fn protobuf.zigzag64(v: i64) -> u64 uses {}`

The zigzag encoding of a 64-bit integer.

References: [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)


## protobuf.wkt

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fAny"></a>

- `pub type protobuf.wkt.Any { type_url: String, value: Bytes, unknown_fields: Bytes, }`

The proto message `google.protobuf.Any`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fBoolValue"></a>

- `pub type protobuf.wkt.BoolValue { value: bool, unknown_fields: Bytes, }`

The proto message `google.protobuf.BoolValue`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fBytesValue"></a>

- `pub type protobuf.wkt.BytesValue { value: Bytes, unknown_fields: Bytes, }`

The proto message `google.protobuf.BytesValue`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fDoubleValue"></a>

- `pub type protobuf.wkt.DoubleValue { value: f64, unknown_fields: Bytes, }`

The proto message `google.protobuf.DoubleValue`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/f64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2ff64)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fDuration"></a>

- `pub type protobuf.wkt.Duration { seconds: i64, nanos: i32, unknown_fields: Bytes, }`

The proto message `google.protobuf.Duration`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fEmpty"></a>

- `pub type protobuf.wkt.Empty { unknown_fields: Bytes, }`

The proto message `google.protobuf.Empty`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fFieldMask"></a>

- `pub type protobuf.wkt.FieldMask { paths: List[String], unknown_fields: Bytes, }`

The proto message `google.protobuf.FieldMask`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fFloatValue"></a>

- `pub type protobuf.wkt.FloatValue { value: f32, unknown_fields: Bytes, }`

The proto message `google.protobuf.FloatValue`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/f32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2ff32)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fInt32Value"></a>

- `pub type protobuf.wkt.Int32Value { value: i32, unknown_fields: Bytes, }`

The proto message `google.protobuf.Int32Value`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fInt64Value"></a>

- `pub type protobuf.wkt.Int64Value { value: i64, unknown_fields: Bytes, }`

The proto message `google.protobuf.Int64Value`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fListValue"></a>

- `pub type protobuf.wkt.ListValue { values: List[Value], unknown_fields: Bytes, }`

The proto message `google.protobuf.ListValue`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`protobuf@1/protobuf/wkt/Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fNullValue"></a>

- `pub type protobuf.wkt.NullValue = NULL_VALUE | Unrecognized(i32)`

The proto enum `google.protobuf.NullValue`.

References: [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fNullValue_23NULL_5fVALUE"></a>

- `NULL_VALUE -> protobuf.wkt.NullValue`

Variant of protobuf.wkt.NullValue.

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fNullValue_23Unrecognized"></a>

- `Unrecognized(i32) -> protobuf.wkt.NullValue`

Variant of protobuf.wkt.NullValue.

References: [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fStringValue"></a>

- `pub type protobuf.wkt.StringValue { value: String, unknown_fields: Bytes, }`

The proto message `google.protobuf.StringValue`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fStruct"></a>

- `pub type protobuf.wkt.Struct { fields: Map[String, Value], unknown_fields: Bytes, }`

The proto message `google.protobuf.Struct`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Map`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fMap), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`protobuf@1/protobuf/wkt/Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fTimestamp"></a>

- `pub type protobuf.wkt.Timestamp { seconds: i64, nanos: i32, unknown_fields: Bytes, }`

The proto message `google.protobuf.Timestamp`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`core@1/i64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi64)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fUInt32Value"></a>

- `pub type protobuf.wkt.UInt32Value { value: u32, unknown_fields: Bytes, }`

The proto message `google.protobuf.UInt32Value`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/u32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu32)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fUInt64Value"></a>

- `pub type protobuf.wkt.UInt64Value { value: u64, unknown_fields: Bytes, }`

The proto message `google.protobuf.UInt64Value`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fValue"></a>

- `pub type protobuf.wkt.Value { kind: Option[Value_kind], unknown_fields: Bytes, }`

The proto message `google.protobuf.Value`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`protobuf@1/protobuf/wkt/Value_kind`](#def-protobuf_401_2fprotobuf_2fwkt_2fValue_5fkind)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fValue_5fkind"></a>

- `pub type protobuf.wkt.Value_kind = null_value(NullValue) | number_value(f64) | string_value(String) | bool_value( bool ) | struct_value(Struct) | list_value(ListValue)`

The oneof `kind` of `Value`.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`core@1/f64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2ff64), [`protobuf@1/protobuf/wkt/ListValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fListValue), [`protobuf@1/protobuf/wkt/NullValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fNullValue), [`protobuf@1/protobuf/wkt/Struct`](#def-protobuf_401_2fprotobuf_2fwkt_2fStruct)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fValue_5fkind_23bool_5fvalue"></a>

- `bool_value( bool ) -> protobuf.wkt.Value_kind`

Variant of protobuf.wkt.Value_kind.

References: [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fValue_5fkind_23list_5fvalue"></a>

- `list_value(ListValue) -> protobuf.wkt.Value_kind`

Variant of protobuf.wkt.Value_kind.

References: [`protobuf@1/protobuf/wkt/ListValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fListValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fValue_5fkind_23null_5fvalue"></a>

- `null_value(NullValue) -> protobuf.wkt.Value_kind`

Variant of protobuf.wkt.Value_kind.

References: [`protobuf@1/protobuf/wkt/NullValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fNullValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fValue_5fkind_23number_5fvalue"></a>

- `number_value(f64) -> protobuf.wkt.Value_kind`

Variant of protobuf.wkt.Value_kind.

References: [`core@1/f64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2ff64)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fValue_5fkind_23string_5fvalue"></a>

- `string_value(String) -> protobuf.wkt.Value_kind`

Variant of protobuf.wkt.Value_kind.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fValue_5fkind_23struct_5fvalue"></a>

- `struct_value(Struct) -> protobuf.wkt.Value_kind`

Variant of protobuf.wkt.Value_kind.

References: [`protobuf@1/protobuf/wkt/Struct`](#def-protobuf_401_2fprotobuf_2fwkt_2fStruct)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fAny"></a>

- `pub fn protobuf.wkt.decode_Any(data: Bytes) -> Result[Any, protobuf.DecodeError] uses {}`

Decodes a `Any` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/Any`](#def-protobuf_401_2fprotobuf_2fwkt_2fAny)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fBoolValue"></a>

- `pub fn protobuf.wkt.decode_BoolValue(data: Bytes) -> Result[BoolValue, protobuf.DecodeError] uses {}`

Decodes a `BoolValue` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/BoolValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fBoolValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fBytesValue"></a>

- `pub fn protobuf.wkt.decode_BytesValue(data: Bytes) -> Result[BytesValue, protobuf.DecodeError] uses {}`

Decodes a `BytesValue` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/BytesValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fBytesValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fDoubleValue"></a>

- `pub fn protobuf.wkt.decode_DoubleValue(data: Bytes) -> Result[DoubleValue, protobuf.DecodeError] uses {}`

Decodes a `DoubleValue` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/DoubleValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fDoubleValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fDuration"></a>

- `pub fn protobuf.wkt.decode_Duration(data: Bytes) -> Result[Duration, protobuf.DecodeError] uses {}`

Decodes a `Duration` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/Duration`](#def-protobuf_401_2fprotobuf_2fwkt_2fDuration)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fEmpty"></a>

- `pub fn protobuf.wkt.decode_Empty(data: Bytes) -> Result[Empty, protobuf.DecodeError] uses {}`

Decodes a `Empty` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/Empty`](#def-protobuf_401_2fprotobuf_2fwkt_2fEmpty)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fFieldMask"></a>

- `pub fn protobuf.wkt.decode_FieldMask(data: Bytes) -> Result[FieldMask, protobuf.DecodeError] uses {}`

Decodes a `FieldMask` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/FieldMask`](#def-protobuf_401_2fprotobuf_2fwkt_2fFieldMask)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fFloatValue"></a>

- `pub fn protobuf.wkt.decode_FloatValue(data: Bytes) -> Result[FloatValue, protobuf.DecodeError] uses {}`

Decodes a `FloatValue` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/FloatValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fFloatValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fInt32Value"></a>

- `pub fn protobuf.wkt.decode_Int32Value(data: Bytes) -> Result[Int32Value, protobuf.DecodeError] uses {}`

Decodes a `Int32Value` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/Int32Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fInt32Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fInt64Value"></a>

- `pub fn protobuf.wkt.decode_Int64Value(data: Bytes) -> Result[Int64Value, protobuf.DecodeError] uses {}`

Decodes a `Int64Value` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/Int64Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fInt64Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fListValue"></a>

- `pub fn protobuf.wkt.decode_ListValue(data: Bytes) -> Result[ListValue, protobuf.DecodeError] uses {}`

Decodes a `ListValue` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/ListValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fListValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fStringValue"></a>

- `pub fn protobuf.wkt.decode_StringValue(data: Bytes) -> Result[StringValue, protobuf.DecodeError] uses {}`

Decodes a `StringValue` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/StringValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fStringValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fStruct"></a>

- `pub fn protobuf.wkt.decode_Struct(data: Bytes) -> Result[Struct, protobuf.DecodeError] uses {}`

Decodes a `Struct` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/Struct`](#def-protobuf_401_2fprotobuf_2fwkt_2fStruct)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fTimestamp"></a>

- `pub fn protobuf.wkt.decode_Timestamp(data: Bytes) -> Result[Timestamp, protobuf.DecodeError] uses {}`

Decodes a `Timestamp` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/Timestamp`](#def-protobuf_401_2fprotobuf_2fwkt_2fTimestamp)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fUInt32Value"></a>

- `pub fn protobuf.wkt.decode_UInt32Value(data: Bytes) -> Result[UInt32Value, protobuf.DecodeError] uses {}`

Decodes a `UInt32Value` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/UInt32Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fUInt32Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fUInt64Value"></a>

- `pub fn protobuf.wkt.decode_UInt64Value(data: Bytes) -> Result[UInt64Value, protobuf.DecodeError] uses {}`

Decodes a `UInt64Value` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/UInt64Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fUInt64Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdecode_5fValue"></a>

- `pub fn protobuf.wkt.decode_Value(data: Bytes) -> Result[Value, protobuf.DecodeError] uses {}`

Decodes a `Value` from its binary encoding.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fAny"></a>

- `pub fn protobuf.wkt.default_Any() -> Any uses {}`

A `Any` with every field at its default.

References: [`protobuf@1/protobuf/wkt/Any`](#def-protobuf_401_2fprotobuf_2fwkt_2fAny)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fBoolValue"></a>

- `pub fn protobuf.wkt.default_BoolValue() -> BoolValue uses {}`

A `BoolValue` with every field at its default.

References: [`protobuf@1/protobuf/wkt/BoolValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fBoolValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fBytesValue"></a>

- `pub fn protobuf.wkt.default_BytesValue() -> BytesValue uses {}`

A `BytesValue` with every field at its default.

References: [`protobuf@1/protobuf/wkt/BytesValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fBytesValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fDoubleValue"></a>

- `pub fn protobuf.wkt.default_DoubleValue() -> DoubleValue uses {}`

A `DoubleValue` with every field at its default.

References: [`protobuf@1/protobuf/wkt/DoubleValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fDoubleValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fDuration"></a>

- `pub fn protobuf.wkt.default_Duration() -> Duration uses {}`

A `Duration` with every field at its default.

References: [`protobuf@1/protobuf/wkt/Duration`](#def-protobuf_401_2fprotobuf_2fwkt_2fDuration)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fEmpty"></a>

- `pub fn protobuf.wkt.default_Empty() -> Empty uses {}`

A `Empty` with every field at its default.

References: [`protobuf@1/protobuf/wkt/Empty`](#def-protobuf_401_2fprotobuf_2fwkt_2fEmpty)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fFieldMask"></a>

- `pub fn protobuf.wkt.default_FieldMask() -> FieldMask uses {}`

A `FieldMask` with every field at its default.

References: [`protobuf@1/protobuf/wkt/FieldMask`](#def-protobuf_401_2fprotobuf_2fwkt_2fFieldMask)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fFloatValue"></a>

- `pub fn protobuf.wkt.default_FloatValue() -> FloatValue uses {}`

A `FloatValue` with every field at its default.

References: [`protobuf@1/protobuf/wkt/FloatValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fFloatValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fInt32Value"></a>

- `pub fn protobuf.wkt.default_Int32Value() -> Int32Value uses {}`

A `Int32Value` with every field at its default.

References: [`protobuf@1/protobuf/wkt/Int32Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fInt32Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fInt64Value"></a>

- `pub fn protobuf.wkt.default_Int64Value() -> Int64Value uses {}`

A `Int64Value` with every field at its default.

References: [`protobuf@1/protobuf/wkt/Int64Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fInt64Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fListValue"></a>

- `pub fn protobuf.wkt.default_ListValue() -> ListValue uses {}`

A `ListValue` with every field at its default.

References: [`protobuf@1/protobuf/wkt/ListValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fListValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fStringValue"></a>

- `pub fn protobuf.wkt.default_StringValue() -> StringValue uses {}`

A `StringValue` with every field at its default.

References: [`protobuf@1/protobuf/wkt/StringValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fStringValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fStruct"></a>

- `pub fn protobuf.wkt.default_Struct() -> Struct uses {}`

A `Struct` with every field at its default.

References: [`protobuf@1/protobuf/wkt/Struct`](#def-protobuf_401_2fprotobuf_2fwkt_2fStruct)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fTimestamp"></a>

- `pub fn protobuf.wkt.default_Timestamp() -> Timestamp uses {}`

A `Timestamp` with every field at its default.

References: [`protobuf@1/protobuf/wkt/Timestamp`](#def-protobuf_401_2fprotobuf_2fwkt_2fTimestamp)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fUInt32Value"></a>

- `pub fn protobuf.wkt.default_UInt32Value() -> UInt32Value uses {}`

A `UInt32Value` with every field at its default.

References: [`protobuf@1/protobuf/wkt/UInt32Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fUInt32Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fUInt64Value"></a>

- `pub fn protobuf.wkt.default_UInt64Value() -> UInt64Value uses {}`

A `UInt64Value` with every field at its default.

References: [`protobuf@1/protobuf/wkt/UInt64Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fUInt64Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fdefault_5fValue"></a>

- `pub fn protobuf.wkt.default_Value() -> Value uses {}`

A `Value` with every field at its default.

References: [`protobuf@1/protobuf/wkt/Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fAny"></a>

- `pub fn protobuf.wkt.encode_Any(m: Any) -> unique Bytes uses {}`

The binary encoding of a `Any`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/Any`](#def-protobuf_401_2fprotobuf_2fwkt_2fAny)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fBoolValue"></a>

- `pub fn protobuf.wkt.encode_BoolValue(m: BoolValue) -> unique Bytes uses {}`

The binary encoding of a `BoolValue`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/BoolValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fBoolValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fBytesValue"></a>

- `pub fn protobuf.wkt.encode_BytesValue(m: BytesValue) -> unique Bytes uses {}`

The binary encoding of a `BytesValue`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/BytesValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fBytesValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fDoubleValue"></a>

- `pub fn protobuf.wkt.encode_DoubleValue(m: DoubleValue) -> unique Bytes uses {}`

The binary encoding of a `DoubleValue`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/DoubleValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fDoubleValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fDuration"></a>

- `pub fn protobuf.wkt.encode_Duration(m: Duration) -> unique Bytes uses {}`

The binary encoding of a `Duration`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/Duration`](#def-protobuf_401_2fprotobuf_2fwkt_2fDuration)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fEmpty"></a>

- `pub fn protobuf.wkt.encode_Empty(m: Empty) -> unique Bytes uses {}`

The binary encoding of a `Empty`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/Empty`](#def-protobuf_401_2fprotobuf_2fwkt_2fEmpty)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fFieldMask"></a>

- `pub fn protobuf.wkt.encode_FieldMask(m: FieldMask) -> unique Bytes uses {}`

The binary encoding of a `FieldMask`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/FieldMask`](#def-protobuf_401_2fprotobuf_2fwkt_2fFieldMask)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fFloatValue"></a>

- `pub fn protobuf.wkt.encode_FloatValue(m: FloatValue) -> unique Bytes uses {}`

The binary encoding of a `FloatValue`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/FloatValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fFloatValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fInt32Value"></a>

- `pub fn protobuf.wkt.encode_Int32Value(m: Int32Value) -> unique Bytes uses {}`

The binary encoding of a `Int32Value`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/Int32Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fInt32Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fInt64Value"></a>

- `pub fn protobuf.wkt.encode_Int64Value(m: Int64Value) -> unique Bytes uses {}`

The binary encoding of a `Int64Value`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/Int64Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fInt64Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fListValue"></a>

- `pub fn protobuf.wkt.encode_ListValue(m: ListValue) -> unique Bytes uses {}`

The binary encoding of a `ListValue`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/ListValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fListValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fStringValue"></a>

- `pub fn protobuf.wkt.encode_StringValue(m: StringValue) -> unique Bytes uses {}`

The binary encoding of a `StringValue`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/StringValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fStringValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fStruct"></a>

- `pub fn protobuf.wkt.encode_Struct(m: Struct) -> unique Bytes uses {}`

The binary encoding of a `Struct`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/Struct`](#def-protobuf_401_2fprotobuf_2fwkt_2fStruct)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fTimestamp"></a>

- `pub fn protobuf.wkt.encode_Timestamp(m: Timestamp) -> unique Bytes uses {}`

The binary encoding of a `Timestamp`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/Timestamp`](#def-protobuf_401_2fprotobuf_2fwkt_2fTimestamp)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fUInt32Value"></a>

- `pub fn protobuf.wkt.encode_UInt32Value(m: UInt32Value) -> unique Bytes uses {}`

The binary encoding of a `UInt32Value`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/UInt32Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fUInt32Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fUInt64Value"></a>

- `pub fn protobuf.wkt.encode_UInt64Value(m: UInt64Value) -> unique Bytes uses {}`

The binary encoding of a `UInt64Value`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/UInt64Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fUInt64Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fencode_5fValue"></a>

- `pub fn protobuf.wkt.encode_Value(m: Value) -> unique Bytes uses {}`

The binary encoding of a `Value`.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`protobuf@1/protobuf/wkt/Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fAny"></a>

- `pub fn protobuf.wkt.from_json_Any(v: json.Value, o: protobuf.JsonOptions) -> Result[ Any, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `Any`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/Any`](#def-protobuf_401_2fprotobuf_2fwkt_2fAny), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fBoolValue"></a>

- `pub fn protobuf.wkt.from_json_BoolValue(v: json.Value, o: protobuf.JsonOptions) -> Result[ BoolValue, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `BoolValue`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/BoolValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fBoolValue), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fBytesValue"></a>

- `pub fn protobuf.wkt.from_json_BytesValue(v: json.Value, o: protobuf.JsonOptions) -> Result[ BytesValue, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `BytesValue`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/BytesValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fBytesValue), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fDoubleValue"></a>

- `pub fn protobuf.wkt.from_json_DoubleValue(v: json.Value, o: protobuf.JsonOptions) -> Result[ DoubleValue, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `DoubleValue`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/DoubleValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fDoubleValue), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fDuration"></a>

- `pub fn protobuf.wkt.from_json_Duration(v: json.Value, o: protobuf.JsonOptions) -> Result[ Duration, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `Duration`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/Duration`](#def-protobuf_401_2fprotobuf_2fwkt_2fDuration), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fEmpty"></a>

- `pub fn protobuf.wkt.from_json_Empty(v: json.Value, o: protobuf.JsonOptions) -> Result[ Empty, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `Empty`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/Empty`](#def-protobuf_401_2fprotobuf_2fwkt_2fEmpty), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fFieldMask"></a>

- `pub fn protobuf.wkt.from_json_FieldMask(v: json.Value, o: protobuf.JsonOptions) -> Result[ FieldMask, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `FieldMask`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/FieldMask`](#def-protobuf_401_2fprotobuf_2fwkt_2fFieldMask), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fFloatValue"></a>

- `pub fn protobuf.wkt.from_json_FloatValue(v: json.Value, o: protobuf.JsonOptions) -> Result[ FloatValue, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `FloatValue`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/FloatValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fFloatValue), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fInt32Value"></a>

- `pub fn protobuf.wkt.from_json_Int32Value(v: json.Value, o: protobuf.JsonOptions) -> Result[ Int32Value, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `Int32Value`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/Int32Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fInt32Value), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fInt64Value"></a>

- `pub fn protobuf.wkt.from_json_Int64Value(v: json.Value, o: protobuf.JsonOptions) -> Result[ Int64Value, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `Int64Value`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/Int64Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fInt64Value), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fListValue"></a>

- `pub fn protobuf.wkt.from_json_ListValue(v: json.Value, o: protobuf.JsonOptions) -> Result[ ListValue, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `ListValue`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/ListValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fListValue), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fNullValue"></a>

- `pub fn protobuf.wkt.from_json_NullValue(v: json.Value, ignore_unknown: bool) -> Result[ Option[NullValue], protobuf.JsonError ] uses {}`

References: [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/wkt/NullValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fNullValue), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fStringValue"></a>

- `pub fn protobuf.wkt.from_json_StringValue(v: json.Value, o: protobuf.JsonOptions) -> Result[ StringValue, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `StringValue`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/StringValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fStringValue), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fStruct"></a>

- `pub fn protobuf.wkt.from_json_Struct(v: json.Value, o: protobuf.JsonOptions) -> Result[ Struct, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `Struct`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/Struct`](#def-protobuf_401_2fprotobuf_2fwkt_2fStruct), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fTimestamp"></a>

- `pub fn protobuf.wkt.from_json_Timestamp(v: json.Value, o: protobuf.JsonOptions) -> Result[ Timestamp, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `Timestamp`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/Timestamp`](#def-protobuf_401_2fprotobuf_2fwkt_2fTimestamp), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fUInt32Value"></a>

- `pub fn protobuf.wkt.from_json_UInt32Value(v: json.Value, o: protobuf.JsonOptions) -> Result[ UInt32Value, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `UInt32Value`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/UInt32Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fUInt32Value), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fUInt64Value"></a>

- `pub fn protobuf.wkt.from_json_UInt64Value(v: json.Value, o: protobuf.JsonOptions) -> Result[ UInt64Value, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `UInt64Value`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/UInt64Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fUInt64Value), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fjson_5fValue"></a>

- `pub fn protobuf.wkt.from_json_Value(v: json.Value, o: protobuf.JsonOptions) -> Result[ Value, protobuf.JsonError ] uses {}`

Parses the proto3 JSON of a `Value`.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/JsonOptions`](#def-protobuf_401_2fprotobuf_2fJsonOptions), [`protobuf@1/protobuf/wkt/Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fValue), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fname_5fNullValue"></a>

- `pub fn protobuf.wkt.from_name_NullValue(s: String) -> Option[NullValue] uses {}`

References: [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`protobuf@1/protobuf/wkt/NullValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fNullValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ffrom_5fnumber_5fNullValue"></a>

- `pub fn protobuf.wkt.from_number_NullValue(n: i32) -> NullValue uses {}`

References: [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`protobuf@1/protobuf/wkt/NullValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fNullValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fmessage_5ftypes"></a>

- `pub fn protobuf.wkt.message_types() -> List[protobuf.MessageType] uses {}`

Registry entries for every message of this module.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fname_5fNullValue"></a>

- `pub fn protobuf.wkt.name_NullValue(e: NullValue) -> Option[String] uses {}`

References: [`core@1/Option`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fOption), [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString), [`protobuf@1/protobuf/wkt/NullValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fNullValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fnumber_5fNullValue"></a>

- `pub fn protobuf.wkt.number_NullValue(e: NullValue) -> i32 uses {}`

References: [`core@1/i32`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fi32), [`protobuf@1/protobuf/wkt/NullValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fNullValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fAny"></a>

- `pub fn protobuf.wkt.read_Any(data: Bytes, depth: u64) -> Result[Any, protobuf.DecodeError] uses {}`

Decodes a `Any` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/Any`](#def-protobuf_401_2fprotobuf_2fwkt_2fAny)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fBoolValue"></a>

- `pub fn protobuf.wkt.read_BoolValue(data: Bytes, depth: u64) -> Result[ BoolValue, protobuf.DecodeError ] uses {}`

Decodes a `BoolValue` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/BoolValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fBoolValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fBytesValue"></a>

- `pub fn protobuf.wkt.read_BytesValue(data: Bytes, depth: u64) -> Result[ BytesValue, protobuf.DecodeError ] uses {}`

Decodes a `BytesValue` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/BytesValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fBytesValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fDoubleValue"></a>

- `pub fn protobuf.wkt.read_DoubleValue(data: Bytes, depth: u64) -> Result[ DoubleValue, protobuf.DecodeError ] uses {}`

Decodes a `DoubleValue` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/DoubleValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fDoubleValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fDuration"></a>

- `pub fn protobuf.wkt.read_Duration(data: Bytes, depth: u64) -> Result[Duration, protobuf.DecodeError] uses {}`

Decodes a `Duration` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/Duration`](#def-protobuf_401_2fprotobuf_2fwkt_2fDuration)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fEmpty"></a>

- `pub fn protobuf.wkt.read_Empty(data: Bytes, depth: u64) -> Result[Empty, protobuf.DecodeError] uses {}`

Decodes a `Empty` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/Empty`](#def-protobuf_401_2fprotobuf_2fwkt_2fEmpty)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fFieldMask"></a>

- `pub fn protobuf.wkt.read_FieldMask(data: Bytes, depth: u64) -> Result[ FieldMask, protobuf.DecodeError ] uses {}`

Decodes a `FieldMask` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/FieldMask`](#def-protobuf_401_2fprotobuf_2fwkt_2fFieldMask)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fFloatValue"></a>

- `pub fn protobuf.wkt.read_FloatValue(data: Bytes, depth: u64) -> Result[ FloatValue, protobuf.DecodeError ] uses {}`

Decodes a `FloatValue` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/FloatValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fFloatValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fInt32Value"></a>

- `pub fn protobuf.wkt.read_Int32Value(data: Bytes, depth: u64) -> Result[ Int32Value, protobuf.DecodeError ] uses {}`

Decodes a `Int32Value` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/Int32Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fInt32Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fInt64Value"></a>

- `pub fn protobuf.wkt.read_Int64Value(data: Bytes, depth: u64) -> Result[ Int64Value, protobuf.DecodeError ] uses {}`

Decodes a `Int64Value` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/Int64Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fInt64Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fListValue"></a>

- `pub fn protobuf.wkt.read_ListValue(data: Bytes, depth: u64) -> Result[ ListValue, protobuf.DecodeError ] uses {}`

Decodes a `ListValue` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/ListValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fListValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fStringValue"></a>

- `pub fn protobuf.wkt.read_StringValue(data: Bytes, depth: u64) -> Result[ StringValue, protobuf.DecodeError ] uses {}`

Decodes a `StringValue` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/StringValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fStringValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fStruct"></a>

- `pub fn protobuf.wkt.read_Struct(data: Bytes, depth: u64) -> Result[Struct, protobuf.DecodeError] uses {}`

Decodes a `Struct` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/Struct`](#def-protobuf_401_2fprotobuf_2fwkt_2fStruct)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fTimestamp"></a>

- `pub fn protobuf.wkt.read_Timestamp(data: Bytes, depth: u64) -> Result[ Timestamp, protobuf.DecodeError ] uses {}`

Decodes a `Timestamp` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/Timestamp`](#def-protobuf_401_2fprotobuf_2fwkt_2fTimestamp)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fUInt32Value"></a>

- `pub fn protobuf.wkt.read_UInt32Value(data: Bytes, depth: u64) -> Result[ UInt32Value, protobuf.DecodeError ] uses {}`

Decodes a `UInt32Value` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/UInt32Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fUInt32Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fUInt64Value"></a>

- `pub fn protobuf.wkt.read_UInt64Value(data: Bytes, depth: u64) -> Result[ UInt64Value, protobuf.DecodeError ] uses {}`

Decodes a `UInt64Value` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/UInt64Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fUInt64Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fread_5fValue"></a>

- `pub fn protobuf.wkt.read_Value(data: Bytes, depth: u64) -> Result[Value, protobuf.DecodeError] uses {}`

Decodes a `Value` nested `depth` messages deep.

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64), [`protobuf@1/protobuf/DecodeError`](#def-protobuf_401_2fprotobuf_2fDecodeError), [`protobuf@1/protobuf/wkt/Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fAny"></a>

- `pub fn protobuf.wkt.to_json_Any(m: Any, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `Any`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/Any`](#def-protobuf_401_2fprotobuf_2fwkt_2fAny), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fBoolValue"></a>

- `pub fn protobuf.wkt.to_json_BoolValue(m: BoolValue, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `BoolValue`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/BoolValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fBoolValue), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fBytesValue"></a>

- `pub fn protobuf.wkt.to_json_BytesValue(m: BytesValue, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `BytesValue`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/BytesValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fBytesValue), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fDoubleValue"></a>

- `pub fn protobuf.wkt.to_json_DoubleValue(m: DoubleValue, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `DoubleValue`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/DoubleValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fDoubleValue), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fDuration"></a>

- `pub fn protobuf.wkt.to_json_Duration(m: Duration, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `Duration`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/Duration`](#def-protobuf_401_2fprotobuf_2fwkt_2fDuration), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fEmpty"></a>

- `pub fn protobuf.wkt.to_json_Empty(m: Empty, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `Empty`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/Empty`](#def-protobuf_401_2fprotobuf_2fwkt_2fEmpty), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fFieldMask"></a>

- `pub fn protobuf.wkt.to_json_FieldMask(m: FieldMask, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `FieldMask`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/FieldMask`](#def-protobuf_401_2fprotobuf_2fwkt_2fFieldMask), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fFloatValue"></a>

- `pub fn protobuf.wkt.to_json_FloatValue(m: FloatValue, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `FloatValue`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/FloatValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fFloatValue), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fInt32Value"></a>

- `pub fn protobuf.wkt.to_json_Int32Value(m: Int32Value, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `Int32Value`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/Int32Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fInt32Value), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fInt64Value"></a>

- `pub fn protobuf.wkt.to_json_Int64Value(m: Int64Value, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `Int64Value`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/Int64Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fInt64Value), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fListValue"></a>

- `pub fn protobuf.wkt.to_json_ListValue(m: ListValue, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `ListValue`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/ListValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fListValue), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fNullValue"></a>

- `pub fn protobuf.wkt.to_json_NullValue(e: NullValue) -> json.Value uses {}`

References: [`protobuf@1/protobuf/wkt/NullValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fNullValue), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fStringValue"></a>

- `pub fn protobuf.wkt.to_json_StringValue(m: StringValue, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `StringValue`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/StringValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fStringValue), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fStruct"></a>

- `pub fn protobuf.wkt.to_json_Struct(m: Struct, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `Struct`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/Struct`](#def-protobuf_401_2fprotobuf_2fwkt_2fStruct), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fTimestamp"></a>

- `pub fn protobuf.wkt.to_json_Timestamp(m: Timestamp, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `Timestamp`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/Timestamp`](#def-protobuf_401_2fprotobuf_2fwkt_2fTimestamp), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fUInt32Value"></a>

- `pub fn protobuf.wkt.to_json_UInt32Value(m: UInt32Value, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `UInt32Value`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/UInt32Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fUInt32Value), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fUInt64Value"></a>

- `pub fn protobuf.wkt.to_json_UInt64Value(m: UInt64Value, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `UInt64Value`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/UInt64Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fUInt64Value), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fto_5fjson_5fValue"></a>

- `pub fn protobuf.wkt.to_json_Value(m: Value, r: protobuf.Registry) -> Result[ json.Value, protobuf.JsonError ] uses {}`

The canonical proto3 JSON of a `Value`; `r` resolves `Any` types.

References: [`core@1/Result`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fResult), [`protobuf@1/protobuf/JsonError`](#def-protobuf_401_2fprotobuf_2fJsonError), [`protobuf@1/protobuf/Registry`](#def-protobuf_401_2fprotobuf_2fRegistry), [`protobuf@1/protobuf/wkt/Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fValue), [`std@1/json/Value`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-std_401_2fjson_2fValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fAny"></a>

- `pub fn protobuf.wkt.type_Any() -> protobuf.MessageType uses {}`

The registry entry of `Any`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fBoolValue"></a>

- `pub fn protobuf.wkt.type_BoolValue() -> protobuf.MessageType uses {}`

The registry entry of `BoolValue`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fBytesValue"></a>

- `pub fn protobuf.wkt.type_BytesValue() -> protobuf.MessageType uses {}`

The registry entry of `BytesValue`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fDoubleValue"></a>

- `pub fn protobuf.wkt.type_DoubleValue() -> protobuf.MessageType uses {}`

The registry entry of `DoubleValue`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fDuration"></a>

- `pub fn protobuf.wkt.type_Duration() -> protobuf.MessageType uses {}`

The registry entry of `Duration`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fEmpty"></a>

- `pub fn protobuf.wkt.type_Empty() -> protobuf.MessageType uses {}`

The registry entry of `Empty`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fFieldMask"></a>

- `pub fn protobuf.wkt.type_FieldMask() -> protobuf.MessageType uses {}`

The registry entry of `FieldMask`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fFloatValue"></a>

- `pub fn protobuf.wkt.type_FloatValue() -> protobuf.MessageType uses {}`

The registry entry of `FloatValue`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fInt32Value"></a>

- `pub fn protobuf.wkt.type_Int32Value() -> protobuf.MessageType uses {}`

The registry entry of `Int32Value`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fInt64Value"></a>

- `pub fn protobuf.wkt.type_Int64Value() -> protobuf.MessageType uses {}`

The registry entry of `Int64Value`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fListValue"></a>

- `pub fn protobuf.wkt.type_ListValue() -> protobuf.MessageType uses {}`

The registry entry of `ListValue`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fStringValue"></a>

- `pub fn protobuf.wkt.type_StringValue() -> protobuf.MessageType uses {}`

The registry entry of `StringValue`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fStruct"></a>

- `pub fn protobuf.wkt.type_Struct() -> protobuf.MessageType uses {}`

The registry entry of `Struct`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fTimestamp"></a>

- `pub fn protobuf.wkt.type_Timestamp() -> protobuf.MessageType uses {}`

The registry entry of `Timestamp`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fUInt32Value"></a>

- `pub fn protobuf.wkt.type_UInt32Value() -> protobuf.MessageType uses {}`

The registry entry of `UInt32Value`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fUInt64Value"></a>

- `pub fn protobuf.wkt.type_UInt64Value() -> protobuf.MessageType uses {}`

The registry entry of `UInt64Value`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2ftype_5fValue"></a>

- `pub fn protobuf.wkt.type_Value() -> protobuf.MessageType uses {}`

The registry entry of `Value`, for `Any` in JSON.

References: [`protobuf@1/protobuf/MessageType`](#def-protobuf_401_2fprotobuf_2fMessageType)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fAny"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_Any(m: Any, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `Any`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/Any`](#def-protobuf_401_2fprotobuf_2fwkt_2fAny)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fBoolValue"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_BoolValue(m: BoolValue, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `BoolValue`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/BoolValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fBoolValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fBytesValue"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_BytesValue(m: BytesValue, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `BytesValue`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/BytesValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fBytesValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fDoubleValue"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_DoubleValue(m: DoubleValue, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `DoubleValue`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/DoubleValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fDoubleValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fDuration"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_Duration(m: Duration, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `Duration`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/Duration`](#def-protobuf_401_2fprotobuf_2fwkt_2fDuration)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fEmpty"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_Empty(m: Empty, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `Empty`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/Empty`](#def-protobuf_401_2fprotobuf_2fwkt_2fEmpty)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fFieldMask"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_FieldMask(m: FieldMask, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `FieldMask`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/FieldMask`](#def-protobuf_401_2fprotobuf_2fwkt_2fFieldMask)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fFloatValue"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_FloatValue(m: FloatValue, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `FloatValue`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/FloatValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fFloatValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fInt32Value"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_Int32Value(m: Int32Value, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `Int32Value`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/Int32Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fInt32Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fInt64Value"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_Int64Value(m: Int64Value, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `Int64Value`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/Int64Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fInt64Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fListValue"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_ListValue(m: ListValue, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `ListValue`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/ListValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fListValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fStringValue"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_StringValue(m: StringValue, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `StringValue`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/StringValue`](#def-protobuf_401_2fprotobuf_2fwkt_2fStringValue)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fStruct"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_Struct(m: Struct, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `Struct`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/Struct`](#def-protobuf_401_2fprotobuf_2fwkt_2fStruct)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fTimestamp"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_Timestamp(m: Timestamp, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `Timestamp`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/Timestamp`](#def-protobuf_401_2fprotobuf_2fwkt_2fTimestamp)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fUInt32Value"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_UInt32Value(m: UInt32Value, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `UInt32Value`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/UInt32Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fUInt32Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fUInt64Value"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_UInt64Value(m: UInt64Value, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `UInt64Value`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/UInt64Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fUInt64Value)

<a id="def-protobuf_401_2fprotobuf_2fwkt_2fwrite_5fValue"></a>

- `@preserves_unique(out) pub fn protobuf.wkt.write_Value(m: Value, mut out: List[u8]) -> () uses {}`

Appends the binary encoding of a `Value`.

References: [`core@1/List`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fList), [`core@1/u8`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu8), [`protobuf@1/protobuf/wkt/Value`](#def-protobuf_401_2fprotobuf_2fwkt_2fValue)
