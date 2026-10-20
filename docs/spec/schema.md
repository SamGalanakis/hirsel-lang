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
output. A custom mapping may produce output its decoder rejects. Checked
writes and explicit projection law helpers below test stronger properties without inferring inverse laws. Existing `derive.from_json`
acceptance is unchanged. New structural schema generation must reject unknown
fields, alias collisions and non-exact variant tags by default.

## Checked writes and law helpers

`write_checked(b, value, projection, options)` (also named
`write_json_checked`) is the explicit stronger JSON operation. It buffers the
encoder's **actual formatted text**, parses that candidate with `json.parse`,
and passes the resulting DOM to the same boundary's decoder. Only then does it
compare `codec.encode(projection(value))` with
`codec.encode(projection(decoded))`, returning the original candidate on equal
canonical HSC1 bytes. `P: serializable` is the projection's DTO type; `D` needs
only `inert`, so opaque domains can expose an owner-approved projection without
universal equality or canonical admission of the domain itself.

`CheckedWriteError` is `Boundary(BoundaryError) | ProjectionMismatch`.
Encoder/preflight/parser/decoder refusal preserves its original typed cause in
`Boundary`; unequal projection bytes return `ProjectionMismatch`. Neither arm
retains rejected input or candidate output. Callbacks, projections and ordinary
FACT checks may fail or diverge under the usual language rules. No candidate is
published on a returned error. This is a pure buffered operation; a caller may
publish its successful String through ordinary authorized I/O. It cannot retract
an already emitted streaming prefix.

One shared CheckState charges encoding, JSON size preflight and decoding.
Reparsing checks candidate bytes against `max_input_bytes` before copying them;
preflight checks `max_output_bytes` before allocating candidate text. The JSON
parser's own work and projections/canonical encodings are outside schema visit
counts, just as arbitrary callback scratch work is. Invalid Options refuse
before any callback. There is no new tape event or format encoding.

A successful checked write establishes **only the supplied projection's
preservation for that value**. A constant projection deliberately ignores all
domain state. Canonical equivalence includes HSC1's canonical NaN and set
rules; it is not IEEE equality, lexeme equality or an inverse theorem. A
non-idempotent encoder can preserve a projection on this write without
establishing normalization laws for other inputs. Ordinary mappings retain
typed endpoints, domain invariants and their supplied validation; the
dictionary adds no independent validator around arbitrary user callbacks.

`check_round_trip(b, value, projection, options)` checks one generated domain
case: encode, decode and compare canonical projection bytes. It works for any
wire `W: data + inert`, without formatting it. `check_normalization(b, wire,
projection, options)` checks one admitted wire case under `N = encode ∘ decode`:
compute `N(w)` then `N(N(w))` and compare their projected canonical bytes. The
wire projection states the intended normalization equivalence; use an identity
DTO projection for exact representation equality. Each helper shares one state
across its stages and short-circuits on failure. `LawError` is
`Schema(SchemaError) | ProjectionMismatch`, preserving refusal/limit causes.
These helpers check examples, not all possible values; a generator must produce
values in the claimed law's domain. They do not treat refusal as a passing law.

Compose the per-case helper with the existing seeded/shrinking property runner:

```hirsel
fn project(v: u8) -> u8 uses {} { v }
fn generate(mut source: test.Source) -> u8 uses {} {
  int.narrow(test.choice(mut source, 255))
}
fn law(v: u8) -> bool uses {} {
  match schema.check_round_trip(schema.u8_json(), v, project, schema.default_options()) {
    Ok(()) => true,
    Error(_) => false,
  }
}
// In a Test entry: test.property_cases(t, 100, generate, law, u8.to_string)
```

Trimming on decode with passthrough encode normalizes whitespace idempotently,
but a domain round trip preserves only already trimmed strings. A callback
encoding every u8 as zero has typed valid endpoints: `write_json` succeeds,
while `write_checked` and `check_round_trip` with the identity projection refuse
nonzero values. This distinction is intentional.

JSON retains Number lexemes, including integers beyond binary64 precision and
large exponents; generated numeric leaves instead use their declared scalar's
parse/range contract. Finite floats use JSON Number and nonfinite floats use
the existing explicit Text spellings `NaN`, `inf`, `-inf`. Invalid Number
lexemes refuse in the format writer. A format-valid DOM alone says nothing
about numeric preservation. HSC1-through-DTO continues to use exact canonical
descriptors/bytes and explicit reconstruction. No schema adapter for CBOR or
MessagePack is enabled here: their existing DOM codecs preserve supported
integers, normalize decimal/exponent spelling through binary64, reject integer
overflow and reject nonfinite Number lexemes. YAML and TOML have separate value
domains and are not implicit JSON adapters.

## Paired structural generation

`derive.schema_decode(shape(T), policy)` and
`derive.schema_encode(shape(T), policy)` generate monomorphic CT-5 bodies.
Both declarations and their complete headers remain written in source. Pass
the same explicit `schema.Policy` to both directions. The generator checks
the policy in both directions, including unknown/duplicate member policies,
colliding canonical names/aliases and paired named leaf conversions. Separate
declarations with different policies are a caller error, checked by round-trip
and golden tests rather than a type-system theorem.

```hirsel
fn item_schema_decode(wire: json.Value, mut check: schema.CheckState)
  -> Result[unique Item, schema.SchemaError] uses {}
  = comptime derive.schema_decode(shape(Item), schema.strict_policy())
fn item_schema_encode(value: Item, mut check: schema.CheckState)
  -> Result[unique json.Value, schema.SchemaError] uses {}
  = comptime derive.schema_encode(shape(Item), schema.strict_policy())
```

The generated decoder constructs owned values and checks under the written
`unique` header. Inputs are borrowed. Structural results are fresh, including
when a custom leaf callback returns shared inert data. A caller can also write
headers without `unique`; this does not change the generated representation.
Function-value uniqueness qualifiers must match exactly. To store a generated
`Result[unique T, SchemaError]` function in Boundary's ordinary-result callback
field, write a pure adapter that calls it and moves the successful value into
`Ok`; alternatively generate an additional declaration with an ordinary result
header. Generated nested converters include this adapter themselves.

`Policy` contains `fields: List[FieldPolicy]`, `tags: List[TagPolicy]` and
`leaves: List[Leaf]`. Empty `strict_policy()` keeps declaration names, requires
every field (including Option fields), accepts null only where the field's
converter does, and always rejects unknown record fields and non-exact tags.
`FieldPolicy` names a declared field with `name`, chooses its canonical
`wire_name`, supplies accepted `aliases`, `missing` and `nulls`. `Missing` is
`Required`, `NoneValue` (only for Option fields), or `Default(json.Value)`.
Defaults are wire values passed through the same field converter and null
check as a present value; they cannot bypass facts or owner constructors.
`NoneValue` synthesizes None for absence, while `Nulls.Reject` rejects explicit
null. Its encoder omits None fields. Other Option fields encode None as null;
rejecting null without an absence policy makes that encode fallible.
Nulls never triggers a default. A default equal to null still undergoes the
field's null check. The writer otherwise emits every field in declaration
order under its canonical name, including fields equal to defaults.

Aliases are accepted only on read. Simultaneous alias/canonical or two alias
members produce `AliasCollision`, even with equal values. Record traversal
decodes declared fields first, then rejects unknowns in incoming insertion
order. Error field paths use declaration names even when an alias was read;
unknown paths use the bounded unknown member name. Neither error retains the
rejected value. A field converter never runs after its member lookup fails.
No partially initialized record escapes an error.

`TagPolicy` has `name`, `wire_name` and `aliases`. Nullary variants use JSON
strings. Payload variants use an object containing exactly one recognized
member, whose value is an array of exactly the declared payload count.
The encoder uses the canonical tag; the decoder dispatches only that tag,
without trying alternative arms. Payload errors prefix `Variant` then `Index`.
Unknown tags, extra members and multiple recognized members produce `Tag`.
All field/tag wire names and aliases must be disjoint in their containing
record/variant; malformed policies fail generation in either direction.

Built-in leaves are bool, String, integer scalars and floats, following the
existing derive numeric domain and nonfinite string spellings. Lists use
arrays; tuples use exact-length arrays. Option uses null/element, and a nested
Option is refused during generation in either direction because this wire
form cannot distinguish its Nones. String-keyed maps use JSON objects; every
other key type uses arrays of `[key, value]` entries. Both preserve insertion
order, reject duplicate decoded keys and report `MapEntry(index), Key` or
`Value` without retaining/rendering the key. JSON parsing rejects duplicate
object members before schema traversal. Custom String key encoders must return
Text and may not produce duplicate object names. Unsupported builtins
(including Set and box) require an explicit leaf
pair rather than receiving an inferred representation.

`Leaf` supplies canonical type spelling `ty` and named pure `decode`/`encode`
functions. These receive the shared mutable CheckState and return Result with
SchemaError. Their signatures and visibility are checked in the generated
body. There is no source-expression injection in the function-name seam.
Distinct/opaque leaves use their owners' explicit conversions. Other nominal
record/variant leaves call `type_name_schema_decode` and
`type_name_schema_encode` in the type's declaring module, using the existing
derive snake-case naming rule. Those declarations can themselves be generated
or written, including recursive functions; no runtime recursive closure graph
is created. Custom leaf pairs carry the ordinary weaker mapping guarantee.

Each generated record, variant and tuple enters/leaves one node, including on
returned errors. Scalars charge one check; Option enters one node. List
conversion enters one node around its elements. Maps enter one node
and charge one check per entry in addition to leaf visits. Record member lookup
and each unknown-member scan charge one check. Independent siblings collect
under the operation's First/All policy; dependent conversion stops on error.
All nested calls borrow the original CheckState. A limit outcome takes
precedence over already collected invalidity and never becomes partial success.

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

Strict generated policy follows Serde's renamed/aliased fields and unknown-field
rejection (`0db9b3b504bcb0a7fa9cf001429869519afd3e9a`,
`serde_derive/src/internals/attr.rs:277,345,454` and `src/de.rs:435`).
The alias collision/unknown vectors adapt `test_suite/tests/test_annotations.rs:494`
and its alias vectors; nested list and variant paths adapt
serde_path_to_error `tests/deserialize.rs:43,119`. Differences are explicit
shared directional policy, bounded deterministic issue accumulation, entry/key/
value paths for all maps, and defaults validated through the field boundary.

Law/normalization fixtures adapt Effect v4
`packages/effect/test/schema/toDifferJsonPatch.test.ts:27,181` (explicit generated
round trips) and `Schema.test.ts:5184` (trim/passthrough). The trim vector
`" asdf " → "asdf"` also follows Zod
`packages/zod/src/v4/classic/tests/codec.test.ts:392` at
`afdac65e0f80c0ccf9e8909da97c411567908b57`. The library adds explicit canonical
projection comparison because these references do not prove inverses. Seeded
inputs/shrinking reuse Hirsel's existing test helpers; formatting reuses its
existing JSON codec. Cross-engine real-I/O replay follows the existing CLI
replay fixture, recording natively and removing host data before interpreter
replay of identical structured issue fields and checked output bytes.
