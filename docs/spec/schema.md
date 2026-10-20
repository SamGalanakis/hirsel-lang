# Pure representation boundaries

`std@1/schema` relates an explicit wire type `W: data + inert` to an inert
validated domain `D`. `Boundary[D, W]` stores pure fallible `decode` and
`encode` callbacks. `Decoder[D, W]` stores only `decode`; it has no synthesized
reverse operation. Neither dictionary is serializable. HSC1 admission,
descriptors, bytes and errors remain governed by [SER-1/2](serialization.md).
For HSC1, explicitly mark the wire DTO serializable, apply `codec.decode` to
that DTO, then `schema.decode` for domain construction; encode in reverse.
The schema dictionary and domain do not gain canonical admission.

Callbacks borrow their input and one mutable `CheckState`; their result owns
its data globally and cannot retain an input loan. A base dictionary promises
neither uniqueness nor sendability. Domain types can be opaque, fact constrained
or noncopyable. Retained policy and callback environments must be inert. A
callback's `uses {}` also excludes captured authority; domain and wire bounds
exclude authority supplied as arguments or exercised during destruction.

`decode`, `encode` and `decode_only` create one state from explicit `Options`.
`then(take outer, take inner)` decodes W → B → A and encodes A → B → W.
Its second conversion runs only when its prerequisite succeeds. Combinators
own their retained dictionaries, so their escaping closures retain no caller
loan. A returned error exposes no partially converted domain value.

## Construction and guarantees

Use the same named pure predicate in a distinct type's `where` clause and its
owner-written fallible decoder. A normally false predicate returns an
`Invalid` issue before construction. Successful construction still runs the
ordinary FACT-2 check; it may evaluate the shared predicate twice. Failure or
divergence while evaluating the predicate is an ordinary language outcome,
never a caught validation issue. Private multi-field invariants remain in
fallible owner constructors. No reflection discovers facts or bypasses opacity.

Structural constructors have their documented representation laws. `identity`
is available for plain inert data; it copies plain bits. `u8_json` accepts JSON
integer lexemes that `u8.parse` accepts (0 through 255), rejecting fractions,
exponents, negatives and overflow; it writes decimal integer text. `text_json`
relates String to JSON Text. Both directions of `list` use element index order
and prefix child errors with `Index`. `array_json` relates a JSON Array to its
DOM element list; `list_json` composes it with `list`. These constructors return
owned representations; none introduces a new static domain type.

Arbitrary paired callbacks provide typed endpoints, normal domain invariants
and the checks they actually perform. They do not establish inverse,
preservation or normalization laws. `write_json` formats the declared encoder's
output. A custom mapping may produce output its decoder rejects. Stronger
checked writes, explicit projection law helpers, structural generation and
evolution APIs build on this seam separately. Existing `derive.from_json`
acceptance is unchanged. New structural schema generation must reject unknown
fields, alias collisions and non-exact variant tags by default.

## Structured rejection

`SchemaError` is `Invalid(Issues)` or `Limit(LimitError)`. Each `Issue` carries a
stable `code`, `stage`, relative `path`, `schema_site`, optional safe `expected`
label and child issues. Stages are Syntax, Wire, Fact, Transform and Migration.
Segments distinguish Field, Index, Variant, MapEntry, Key and Value; map
locations never require retaining or stringifying the rejected key.

`invalid` makes a bounded leaf issue. `prefix` consumes an issue collection and
prepends one segment to each top-level path while unwinding. Child paths stay
relative to their parent. `prefix_error` leaves a limit cause unchanged.
`append_issues` consumes already charged issues in producer order; it does not
charge their slots again. `empty_issues` starts an aggregate. These low-level
helpers also support owner-written traversals; public issue records may be
constructed directly, so arbitrary callbacks must honor the same protocol.
`format` is a separate human renderer of top-level paths/codes/sites; its
punctuation is not the machine protocol. It does not disclose raw input.

Record traversals use declaration order, sequences index order and maps
insertion order. Unknown fields follow declared fields in incoming order.
Collect only independent siblings; dependent transforms stop on failure.
`First` stops after the first issue and does not mark it truncated. `All`
stops when the issue allowance is reached and marks the retained collection
`truncated`, conservatively including a limit reached on the last sibling.
Paths on retained issues remain complete. `list` discards all temporary
successful elements if any child rejects, even an empty Invalid collection.

No issue field contains a byte offset. DOM-derived paths are structural.
`parse_json` retains the original `json.JsonError` including genuine syntax
and duplicate-key offsets; it never invents a source position for a DOM leaf.

## Limits and format buffers

`Limits` fixes `max_depth`, `max_issues`, `max_work`, `max_input_bytes` and
`max_output_bytes` for one operation. Defaults are 128 levels, 32 issues,
1,000,000 work visits and 16,777,216 bytes for each buffer direction, with
`First` mode. Zero depth or issues returns `Limit(InvalidOptions, 0)` before
callbacks or parsing; zero work or byte budgets are valid limits.

`CheckState` is opaque, plain, inert and nonallocating to destroy. Only its
helpers mutate the private counters. `charge_check` charges one work visit;
`enter_node` charges one work visit then enters one depth level; `leave_node`
releases that level. Both successful and returned-error paths must leave a
successfully entered node. `then` and `list` enter one level around their
complete operation, and leaves such as the scalar/DOM view constructors charge
one check. A predicate adapter charges one check before invoking its predicate.
The check performed by ordinary FACT-2 construction is not a second schema
charge. `should_stop` tests the shared issue allowance.

Limits are tested before incrementing, so u64 counters cannot wrap. Depth or
work exhaustion returns `Limit` rather than truncated success. Code, schema
site and expected labels are capped at `MAX_LABEL_BYTES` (256 UTF-8 bytes);
`invalid` rejects an oversized label with `DiagnosticBytes`. Limit causes
retain their kind and configured limit. Prefixing preserves complete paths.

`parse_json` borrows its text and checks input bytes before copying it for
JSON parsing. `write_json` buffers the
encoder's DOM, preflights the exact compact JSON output size, then formats it.
Preflight counts UTF-8 bytes, punctuation and the writer's escapes without
allocating candidate text, using checked subtraction against the output limit.
It enters one schema node for each DOM value, sharing the encoder's work and
depth budget. JSON's own 128-container nesting limit remains a distinct
`BoundaryError.Json` cause. Format byte refusal is `BoundaryError.FormatLimit`;
schema refusal is `BoundaryError.Schema`. No candidate text is returned on
failure. These functions perform no I/O and publish no tape events.

These are logical visit counts, preserved by optimization. Schema limits do
not bound an already allocated input DOM, JSON parser work, or arbitrary
callback scratch allocation, work, omitted charges, failure or divergence.
A callback must pass the same mutable state through recursive/composed work;
creating another state would reset its own bookkeeping. This library is not a
sandbox or preemption mechanism. File/network authorization and exact replay
continue under their existing contracts. A changed schema/predicate body is a
changed program revision.

## Reference vectors

Path composition follows Effect v4's SchemaIssue.Pointer
(`4cddd3c41e71fc4990d37c42b1bcf2ce3780b41d`) and serde_path_to_error's parent
chains (`7aad3f59acd6d239615ee8b315e0c666915a13bf`). The list wrong-element
vector is adapted from serde_path_to_error `tests/deserialize.rs::test_vec`;
the nested record/fact fixture preserves its containing field/index/leaf
structure. JSON escaping/preflight follows the existing std JSON writer,
which is adapted from serde_json. No dynamic schema registry, unchecked
constructor, implicit policy or callback inverse theorem is introduced.
