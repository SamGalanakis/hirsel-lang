# JSON Pointer

`std@1/json_pointer.parse(pointer: String) -> Result[Pointer, PointerError]`
borrows the input while producing an owned, reusable set of decoded tokens.
The pointer can outlive the input. `get(value: json.Value, pointer: Pointer)
-> Option[json.Value] in value` borrows the selected value from the document;
it does not clone or mutate the document. The result may outlive the pointer,
but cannot escape a borrowed document parameter or grant mutation of it. A live
selection freezes its source against mutation. As with existing copyable
`json.Value`, a heap-owned local can publish a counted share; this is safe
ownership transfer, with no deep clone. Arena and borrowed-parameter bounds
remain attached to the result.

The empty string selects the entire value; `/` selects the empty object key.
Tokens decode `~1` to `/` and `~0` to `~` exactly once (`~01` selects key `~1`).
Keys compare exact Unicode strings without normalization. Percent sequences
remain literal; URI fragments are not supported. `MissingSlash(0)` rejects a
nonempty string without a leading slash. `InvalidEscape(offset)` reports the
zero-based UTF-8 byte position of an invalid or trailing tilde.

Array tokens must be `0` or `[1-9][0-9]*` and fit u64. Overflow, out-of-range
indexes, leading zeroes, signs, `-`, absent members and scalar descent yield
`None`. These are valid pointers with no target, distinct from parse errors.
Object keys such as `00` and `-` remain ordinary keys. Number spelling is
preserved through the existing copyable `json.Value` representation.

For a configuration `{"agents":[{"transport/http":{"retry~delay":1.2300E+004}}]}`,
parse `/agents/0/transport~1http/retry~0delay` once and call `get` for each
configuration. It selects `Some(Number("1.2300E+004"))`. No JSON Patch,
append, setter or removal operation is provided.

Parsing takes linear time and storage in pointer bytes. Lookup walks tokens
using the existing object-map lookup and constant-time checked array access;
index parsing reads each array token's bytes. Lookup allocates no token or
document copies. Recursive borrowed descent uses one stack frame per traversed
document level: a child view keeps its parent alive under Hirsel's loan rules,
so it cannot replace a mutable cursor while borrowing that cursor.

The implementation ports serde_json `src/value/mod.rs:259–264,779–800` and
its RFC6901 object and lookup vectors from `tests/test.rs:1830–1864`, revision
`20572a6d066958542d79cdd6cf1a7cd9f0c8e1f8`, under MIT/Apache-2.0 (existing
license texts in `third-party/NOTICES.md`). Adaptations: parsing is separate
and owns tokens for repeated lookup; decoding is a single byte pass with typed
syntax errors (serde_json leaves other tilde spellings literal); checked u64
indexes replace machine-sized usize; Hirsel's `in value` relation replaces
Rust references; recursive borrowed descent replaces `try_fold` because local
child results freeze cursor storage. Independent tests cover Unicode, literal percent/NUL keys,
single decoding, index limits, byte offsets, nested configuration extraction,
input/pointer lifetimes and refused document escape/mutation. The same fixed
vector fixture executes on H, LLVM and Cranelift through a focused Rust test.
