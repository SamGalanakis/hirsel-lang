# Diagnostics

A diagnostic reports a failed judgment and cites exactly one rule. When a program violates
several rules, this part fixes which diagnostic comes first and where its primary span is,
so that every implementation reports the same first diagnostic. The policy is an oracle for
diagnostic and driver tests, never an additional reason to reject a valid program.

## Order

Obligations are built from syntax and declared signatures, in source evaluation order
([LANG-3](language.md#lang-3-evaluation)), including both branches for static checking.
Prerequisites are checked before the judgments that depend on them: lexical validity,
syntactic form, name and binder availability, signature well-formedness, then expression
typing and flow. Judgments whose operands or signatures are invalid are suppressed; mode,
brand or allocation errors are never manufactured from recovery types.

Across independent obligations, the first diagnostic is the minimum by (source-unit
manifest path as UTF-8 bytes, primary start byte, primary end byte, rule prefix as ASCII,
numeric rule suffix). File input order, phase scheduling and topology do not affect this
order, so a one-file program reports its earliest source site, not whichever check finishes
first.

## Owners

Each failed obligation has exactly one owner before sorting. A specialized construct owns
the extra restriction it introduces:

| Construct | Owner |
|---|---|
| resource finalization | DROP-3 |
| arena, flat, `Packed` and `Shared` construction | MEM-4, MEM-12, MEM-13, MEM-11 |
| scoped-capability escape | MODE-6 |
| task capture | CLO-3 |
| mutex payload, capture and result | CONC-5 |
| channel payload | CONC-6 |
| iteration marker and place | SEM-4 |
| contracts and facts | FACT-1, FACT-2 |
| other closure construction and escape | CLO-1 for scoped values, CLO-4 for escaping capture and authority |
| a restricted scope-forming callback's authority | CAP-3 |
| a transparent scoped call's unknown or joined authority | CLO-2 |
| entry formation from a definition name (planned) | RUN-3 |
| table key schema (planned) | STORE-6 |

These override the general properties they restate, not independent operand errors.

## Expression obligations

For the remaining expression obligations, in this order:

1. **Availability:** OWN-2 use after move; MODE-3 repeated invocation.
2. **Access:** OWN-1 borrowed parameters, LANG-15 aliases of `mut` parameters, LANG-5
   immutable bindings; OWN-8 partial moves.
3. **Copying:** OWN-5 for a concrete noncopyable value; GEN-1 when a generic copyable proof
   is absent.
4. **Exclusive call loans:** OWN-3. A conflict with a live loan that a local result holds is
   MODE-1. A value outside a required relation is PROV-4 when every bound loan it holds
   outside the relation is on an arena, and MODE-1 otherwise
   ([PROV-4](brands.md#prov-4-arena-placement)).
5. **Capability provision:** MEM-9 for `Unsafe` operations, CAP-4 for synchronization,
   CAP-2 for handle operations, otherwise CAP-1.
6. **Identity and placement:** PROV-5 sibling construction and owner mutation; PROV-2
   union, erased or closure-own origins; otherwise PROV-1 concrete brands and PROV-4
   placement.
7. **Required modes:** MODE-1, MODE-2, MODE-3, MODE-4, in that order.
8. **Authority and bounds:** CAP-2 for `inert`, GEN-1 for other bounds.
9. **Allocation:** MEM-5.

A specialized restriction is tested at the corresponding stage, so an already-moved operand
fails before its crossing or layout properties are considered. A missing capability
argument uses the operation's capability owner rather than a derived arity or type
mismatch. Every other typing failure uses the rule that defines the form; LANG-7 owns type
incompatibility without a narrower rule. Independent failures at the same stage use the
source and rule ordering above.

A failed binding annotation is the primary site for its mode error. Later uses of
that binding suppress mode diagnostics that depend on the failed annotation;
independent availability, access and copying errors remain obligations.

Complete actual generic bounds are prerequisites of the parameter defaults that depend on
them; a failed instantiation suppresses only those derived checks, never independent
availability, access, copy, loan or parameter obligations. `sendable` bounds report MODE-4,
`inert` CAP-2 and other bounds GEN-1. A concrete borrowed noncopyable result extraction is
OWN-1, before required modes. Returning local content through an inferred closure result is
MODE-1 only when environment specialization cannot establish a global result; not every
return of a borrowed capture is rejected. A `mut` constructor operand is OWN-3, a `mut`
error mapper LANG-8, and bare non-value type names and duplicate top-level paths LANG-7.

**Sendability.** Failure of a written sendability requirement, of an occurrence-specific
sendability proof, or of a crossing-contract hand-back is MODE-4 at the receiving flow or
the use requiring that proof. Losing evidence through an otherwise legal copy is not itself
an error. An independent earlier availability, access, copy, loan, identity or locality
failure is diagnosed first, and a specialized crossing construct keeps its own
restriction. When the other type, access and locality obligations are met, failure to
instantiate or transport a written sendability requirement is MODE-4 at the call or
receiving flow; the diagnostic identifies the qualified requirement and, when relevant, the
signature's copying permission or invariant mutable contract that prevents preserving it.

## Sequence prefixes

Missing sequence context is LANG-2. An empty sequence cites the whole `[]` and
explains that there is no first element; repairs show `List[u64]`, `Array[u64]`,
and `Inline[u64, 0]` binding annotations. An undetermined first numeric literal
or constructor keeps its existing primary site (the literal or constructor call)
and explains that later elements cannot provide its context. Guidance shows
complete sequence annotations and, for a numeric first element, a suffix that
can establish the List element type. These are examples for the author's intended
type, never a default integer type or expression type arguments.

Repeat literals explain that an Array or correctly sized Inline annotation is
required; a suffix alone cannot select their kind. A non-sequence explicit
context is identified as such and never discarded. An invalid prerequisite
suppresses derived sequence errors. Later heterogeneity and wrong Inline lengths
keep LANG-7 and their existing primary sites. Human and JSON diagnostics carry
the same guidance; no automatic edit chooses an author's kind, element type or size.

## Explicit error mappers

A failed explicit `e ? mapper` keeps LANG-8 and the complete propagation expression
as its primary site. It names the actual source error, the enclosing error channel,
and the required `fn(SourceError) -> EnclosingError` signature. Labels identify the
source expression and enclosing function or closure result annotation. The mapper
declaration is labelled when available in the same source file; otherwise a note
names it and its signature. Variant failures show the declared payloads. A nullary
variant cannot discard an error payload: help may show a named unary helper that
intentionally discards it and returns that variant, followed by `? helper`. This is
a recipe to check, not an automatic edit or a change to the error contract.

## Causes

An ownership-flow diagnostic reports a provided fact (a value's loans, locality,
placement, brand, uniqueness or multiplicity) that fails a required fact. Every such
diagnostic (MODE-1/2/3/4, OWN-1/3, LANG-15, PROV-1/2/4, CLO-1/3/4, MEM-4/11, and
CONC-5/6 for captures and payloads) names the cause of the provided fact and the cause of
the required fact. Each cause is a kind from the closed vocabulary below, naming a program
entity (a binding, parameter, field path, callee and parameter, or closure) with its site,
as a label or a note. A provided cause may chain through steps; steps that do not change
the fact are not printed. A loan conflict names the loan's creation and the use that keeps
it live. Signature well-formedness errors (a misplaced or malformed relation, modifier or
brand) cite the written form instead. Wording, label order and repair suggestions are not
specified; the vocabulary changes only with this section.

### Cause vocabulary

| Side | Kinds |
|---|---|
| Provided fact | borrowed parameter `x`; `mut` parameter `x`; taken parameter `x` and its relation; local result of `f` and its relation; written relation, brand or modifier of a binding or parameter; temporary `e`, ending with its statement; environment of closure `c` through capture `x`; arena placement in `a` (by annotation, relation or construction); brand `of p` (by annotation or operation); brand erased (by computation or join); identity ended (at the ending action); loan on place `q` and its kind, created at a site; share or copy that ended uniqueness; first call of a once function; storage of this body (binding `x` or the frame), ending with its scope; no placement (declared without one, or joined from different placements); moved or not initialised (moved at a site, or uninitialised on some path); access (a binding without `mut`, a read-only alias, a place passed without `mut` or `take`); last use of a binding; published storage (`Shared` or a sealed region); once function value (written, or forced by its captures); structural property of a type (noncopyable, a resource or a finalizer's own resource, not sendable such as a Receiver part, `Packed` layout); authority (of a capability, a scoped capability, or a function value or capture, against a `uses` restriction); computed operand that only borrows other storage; part `x.f` of an owned value; owner that cannot hold a brand or placement (a closure's own parameter, a sibling field); header of a function value; current content not proved sendable, including the share or replacement that lost its evidence, or the unsatisfied generic or erased crossing obligation |
| Step | stored at a place; read from field `p.f`; captured by closure `c`; joined at a branch or loop |
| Required fact | result without a relation (global); result relation `in S`; caller storage `mut x` and its relation; receiving contract of binding `x` (written or inferred at declaration); argument for parameter `p` of `f` and its declared type; escaping store (record field, returned or stored closure, task capture, channel or mutex payload, `shared.freeze`, `arena.seal`); end of a temporary scope; exit of a scope or arena; exclusive action on place `q` (write, move, `mut` or `take`); uniqueness requirement; sendability requirement; many function value (multiplicity); brand or placement required by a use, parameter or annotation (`of p`, arena `a`, the heap or no placement; a branded use needing a live owner; a parameter type naming no brand); closure result outside its own parameters; access requirement (read, copy, deep copy or index); authority bound (a declared `uses` or `inert`); arena contents without resources; written sendable receiving type, generic sendable bound, or exact mutable crossing contract, naming its declaration site |

```text
error[MODE-1]: this operation requires a global value: `xs` is local to the caller, but the result must be global
 --> escape.hir:1:41
   1 | fn escape(xs: List[u64]) -> List[u64] { xs }
     |                                         ^^
```

Here the provided fact is "borrowed parameter `xs`" and the required fact "result without
a relation".

## Folding

Failed obligations of one definition that share a root cause (the same provided cause at the
same site, whatever requirements they fail) fold into one diagnostic before the first
diagnostic is selected: the member of the group that the ordering selects first is reported,
and the others become its notes. Diagnostics with different root causes never fold, so
folding cannot change which diagnostic is first.

## Primary sites

Runtime failure reports, including LANG-9 test-check failures, identify the complete
source site: package name and version, module and definition, file path, and one-based
line and Unicode-scalar column. Human and JSON reports preserve the same location in
interpreted and native execution. `hirsel run --json` writes a JSON failure report to
standard error, with the rule and `site` fields `definition`, `file`, `line` and `column`;
program output keeps its ordinary streams. When the primary attribution differs from
the operation that failed, a human report adds an `in` line and the JSON failure adds
an `operation` object with the same complete location fields. The primary attribution
and the original operation are retained independently through the failure chain.

The primary span is the smallest complete syntactic operation that imposes the failed
obligation: a call, arguments included, for argument, mode, brand and loan checks; a
projection or read for use after move; an assignment for write access; a constructor for
construction; a type or field annotation for an invalid declaration; the closure literal for
capture mode and escape at construction. A prohibited authority capture instead cites its
first free use inside the callback, and nested callbacks repeat this policy. Returning or
storing an already scoped closure cites that use. An unused task handle cites its binding;
a definition containing `todo` without `@incomplete` cites the first `todo`, and a complete
caller cites the incomplete call. Implicit cleanup sites follow
[MEM-5](memory.md#mem-5-allocation); recursive cleanup notes point to fields and
destructors. A missing closing token uses an empty span at the expected boundary.
Invalidated brands are reported at their first incompatible use, with the invalidation as a
note, never as an earlier primary error.

Reserved identifiers cite the keyword (LANG-14), forbidden labels, patterns and facts cite
their form's rule (LANG-12, LANG-13, FACT-2), and other malformed syntax cites LANG-1, except
for these missing or malformed declaration components, owned by their own rules:

| Missing or malformed declaration component | Owner |
|---|---|
| Parameter, record-field or distinct-field colon; constant type colon or initializer `=`; `fn` after `extern` | LANG-6 |
| Required ABI string after `extern` | PROG-4 |
| Opening brace after `uses` | CLO-4 |
| Structural bound after a bound colon or `+`; closing parenthesis of explicit empty bounds | GEN-1 |
| Distinct payload parentheses; missing nonopaque type fields or right-hand side; malformed type, mode or growable marker | LANG-7 |
| Leading bar where the first variant name is required | LANG-4 |
| Drop binding parentheses | DROP-2 |
| `comptime` after a function body's `=` ([CT-5](compile-time.md#ct-5-generation)) | CT-1 |

Required identifier names in declaration and path positions use LANG-4 when absent or
malformed; list separators, ordinary grouping delimiters and block delimiters keep LANG-1;
a reserved keyword in a name position keeps LANG-14. These are syntax owners, not
permission for an unsupported feature.

A growing instantiation cycle cites the earliest type-growing call edge of the cycle by the
same source ordering ([GEN-5](generics.md#gen-5-polymorphic-recursion)); a comptime
dependency cycle cites its earliest dependency reference
([CT-4](compile-time.md#ct-4-order)). A well-typed comptime operation that fails during
evaluation cites CT-1 at the comptime expression, with the underlying failure as a note;
step exhaustion cites CT-1 at the comptime expression too. Static errors inside comptime keep
their ordinary owners. Run-time failures keep their operative rule. Their primary location follows
[`@track_caller`](failure.md#fail-1-kinds); the original operation remains available
independently. A false `ensures` has the declared clause as its operation site. Run-time order is
execution order, except for the spawn-order selection of
[FAIL-2](failure.md#fail-2-boundary).

## Capability and schema declarations

Missing implementation authority is CAP-5 at the seal declaration; representation access
outside the exact owning package and version is PROG-2. Ordinary opaque summaries keep
CAP-2. Exported structural-bound failures keep GEN-1 (`sendable` MODE-4, `inert` CAP-2);
serialization schema admission and opt-in failures use SER-1 at the declaration or the
encoding use (a runner call's argument, result and error schemas at the call,
[RUN-5](runners.md#run-5-placement-independence)), while malformed bytes are an expected decode error under SER-2. Declaration
eligibility precedes dependent uses. Outside the owner, authority diagnostics name the seal
and its declaration, never private representation paths or a suggested hidden grant. Owner
ceiling failures show the field and destruction chain. Scoped lifetime, capture and
defining-module destruction obligations keep their prerequisite order and primary sites.
Launch, header and admission refusals are tooling errors before entry, not source
diagnostics or language failures.

## Repair metadata and nested crossing causes

Replacement spans retain their semantic anchors inside the compiler. Edit consumers resolve
`Fix.span` through the diagnostic's `SourceMap` before indexing UTF-8 source bytes; JSON
locations already contain resolved file offsets. Moving a declaration or generating source
does not turn an item-relative offset into a file-relative offset.

LANG-14 replacement names use the lexer's reserved-word classifier. Before resolution, the
parser conservatively avoids all identifiers in the file, including later declarations and
references that a rename could capture. A machine edit is emitted only for a single occurrence;
otherwise the help asks for the identifier and its references to be renamed together.

MODE-4 callback result explanations name the rejected field/variant path and the declared
result's sendability requirement. For a callback with one simple result expression, a
`unique` result expression is suggested only when its copyability and structural crossing
proofs succeed and allocation is permitted. Changing an ordinary-returning call's result
annotation alone is not offered as a repair. These explanations preserve the rejection rule
and primary site.

Borrowed captures crossing global storage are explained at the capturing closure,
with a separate label for the source owner's declaration and the receiving contract.
A failed `take` remains OWN-1 at the complete operation; when its checked call row
requires a global argument, that requirement is included even though access fails
before the mode check. Changing the enclosing parameter to `take` does not change
the closure's borrowed capture.

For concrete copyable counted sequences (`String`, `Bytes`, and nested
`List`/`Array`/`Inline` values of concrete sequence or scalar types), the diagnostic
may suggest `unique value` when the failed value has no arena placement or scoped
content bound. This removes binding loans by deep copying; it may allocate and
is not permitted under `@noalloc`. It preserves arena/region bounds and borrowed
callable captures. A copied `Shared` root owns an independent global handle and
uses `copy value` instead. Generic, resource, callable and arena-bound captures
receive conditional guidance to keep the borrow local or construct independent
owned inputs, without promising that `unique` or `copy` makes them global.
Concrete suggestions have accepted source twins and the csvjoin callback repair
runs through H, LLVM and Cranelift. These are diagnostic changes only; ownership
judgments and first rule, primary span and source ordering remain unchanged.
