# Public API

Generated from manifest-selected public declarations. Do not edit by hand.
Signatures retain modes, brands, attributes, relations and `uses` clauses.
Opaque representations stay hidden; bodies are not checked or evaluated.

10 exported definitions; 3 variants.

## api

<a id="def-doc_5fapi_401_2fapi_2fChoice"></a>

- `pub type api.Choice[T] = Empty | Value(T) | External(wire.Token)`

Selects a value, including a dependency payload.

References: [`doc_dep@2/model/Token`](../doc_dep@2/api.md#def-doc_5fdep_402_2fmodel_2fToken)

<a id="def-doc_5fapi_401_2fapi_2fChoice_23Empty"></a>

- `Empty -> api.Choice[T]`

Variant of api.Choice.

<a id="def-doc_5fapi_401_2fapi_2fChoice_23External"></a>

- `External(wire.Token) -> api.Choice[T]`

Variant of api.Choice.

References: [`doc_dep@2/model/Token`](../doc_dep@2/api.md#def-doc_5fdep_402_2fmodel_2fToken)

<a id="def-doc_5fapi_401_2fapi_2fChoice_23Value"></a>

- `Value(T) -> api.Choice[T]`

Variant of api.Choice.

<a id="def-doc_5fapi_401_2fapi_2fHandle"></a>

- `pub type api.Handle { dir: Dir, file: File of .dir }`

A directory and its branded file.

References: [`core@1/Dir`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fDir), [`core@1/File`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fFile)

<a id="def-doc_5fapi_401_2fapi_2fLIMIT"></a>

- `pub const api.LIMIT: u64`

The maximum count.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)

<a id="def-doc_5fapi_401_2fapi_2fLabel"></a>

- `@format(render) pub type api.Label { text: String }`

A label with explicit formatting.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString)

<a id="def-doc_5fapi_401_2fapi_2fSecret"></a>

- `pub opaque type api.Secret: inert uses {}`

Keeps its representation private.

<a id="def-doc_5fapi_401_2fapi_2fretain"></a>

- `@noalloc pub fn api.retain[T: data](source: Dir, mut target: Bytes, take value: T, callback: fn(T) -> bool uses {}) -> File of source uses {}`

Retains a source brand and all parameter modes.
A second sentence is kept, too.

The callback's authority is explicit.

    retain(source, target, value, callback)

References: [`core@1/Bytes`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fBytes), [`core@1/Dir`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fDir), [`core@1/File`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fFile), [`core@1/bool`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fbool)

<a id="def-doc_5fapi_401_2fapi_2ftoken"></a>

- `pub fn api.token(value: wire.Token) -> doc_dep@2/model.Token`

Accepts the selected dependency through its local alias and full identity.

References: [`doc_dep@2/model/Token`](../doc_dep@2/api.md#def-doc_5fdep_402_2fmodel_2fToken)

<a id="def-doc_5fapi_401_2fapi_2fundocumented"></a>

- `@test pub fn api.undocumented()`

<a id="def-doc_5fapi_401_2fapi_2fview"></a>

- `pub fn api.view(source: String) -> String in source uses {}`

Returns a view in its source region.

References: [`core@1/String`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fString)


## api.detail

<a id="def-doc_5fapi_401_2fapi_2fdetail_2fvalue"></a>

- `pub fn api.detail.value() -> u64`

Nested public modules retain their full definition path.

References: [`core@1/u64`](https://github.com/SamGalanakis/hirsel-lang/blob/main/docs/api.md#def-core_401_2fu64)
