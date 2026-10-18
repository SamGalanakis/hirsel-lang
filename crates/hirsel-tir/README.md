# hirsel-tir

Own typed, ordered, ownership-explicit IR and deterministic runtime boundary data.

Allowed dependencies: hirsel-span, hirsel-syntax, hirsel-types, hirsel-diag; no implementation crates.

`Body::successors` is the exhaustive normal-edge vocabulary in terminator order,
including each remaining match candidate's guard or target. Failure cleanup chains
remain separate. An invalid match table names no successors for structural checking.

Invariants and boundary:

- All IDs address Vec arenas in the same Body, except BodyId which addresses Definition.bodies; TypeIds address Definition.types; match pattern IDs address their table.
- Instruction order is evaluation order. Effectful subexpressions are materialized into temporary locals before their consumer (LANG-3).
- Every operand says Borrow/Take/Copy/Mut; BeginLoan/EndLoan bound loans. Take clears the source live state; storage writes set it.
- Assign initializes dead storage. Replace atomically exchanges live writable storage into an owned displaced temporary; Drop of that temporary follows. Copy means the semantic copy, before Perceus optimization (OWN-4, DROP-2).
- Every instruction has an explicit failure cleanup; calls transfer take temporaries only on callee entry. Short-circuiting and ? use CFG edges.
- Cleanup actions are already ordered: evaluated temporaries in reverse, then locals in reverse declaration order. Return extracts its operand before cleanup (LANG-3, DROP-2).
- Cleanup continues after a failed destructor, with remaining field releases; destructor bodies are shielded. The interpreter does not infer drop points (FAIL-3).
- Contracts become explicit entry/exit checks and snapshots; drop/closure bodies have their own BodyIds. Result propagation is ordinary control flow, distinct from fail-stop failure.

Use RvalueId for Assign; the rvalue arena prevents a large expression enum from
inflating every instruction. Unary ownership variants from syntax must be lowered
to explicit Operand uses; logical And/Or must lower to branches, never eager binary
operations. Range values and loops can lower to checked scalar CFG operations;
no user iterator protocol is reconstructed by the interpreter.

Call arguments name already-evaluated temporary places. Failure evaluating a later
argument runs that instruction's cleanup (which includes earlier take temporaries).
A Call failure occurs after entry, so its cleanup excludes transferred temporaries.
Successful call results are initialized only on the next edge. A call whose
instantiated callee result is Never diverges: `Terminator::Call { returns: None }`
has no result slot and no normal successor, only its failure edge (TIR v3).
V checks both directions independently from the callee
signature: a diverging call needs a Never result, and a Never result needs a
diverging call. A diverging self-tail call (`fn serve() -> Never { ..; serve() }`)
is a loop site: V verifies it against the call's failure cleanup, which must hold
only plain, moved or loop-carried obligations. Failure cleanups
propagate after running their action list; RunCleanup executes the same list then
continues. Return moves/copies its operand before cleanup. Explicit EndLoan actions
are needed on every relevant edge. Before a self tail call, borrowed owned
arguments move into internal owned slots as required by LANG-10.

Logical frame bytes, resource/field order, scalar checked operations, snapshots,
source sites and failure edges are compiler output. The interpreter may maintain
slot-live bits for conditional drops; those bits implement explicit Take/Drop/
StorageLive actions, not liveness inference. M1b Perceus preserves these resource
effects while specializing ordinary value retain/release.

ConstructField.field and CallArgument.parameter explicitly map source-ordered
inputs to declaration-order storage/parameters. Labels may reorder fields without
reordering their evaluation. Sequence/tuple fields use their element ordinals.

Revision 1 defines its own LocalId: it indexes storage, while hirsel-types::LocalId
names a resolver binding. K maps these explicitly; numeric equality is irrelevant.
TirProvider returns Arc<Definition> through DefinitionRef, which driver caches and
interpreter frames retain. Construction/lowering still returns an owned Definition.

ToolingLimit { kind: Steps | CallDepth, limit, site: Option<Span> } carries artificial
evaluator guards independently of Failure. It contains no RuleId and does not
settle #34's accounting, site or cleanup semantics. It is not a semantic execution
observation record. Numeric conversions remain Callee::Definition calls dispatched
by the interpreter's operation registry. The IR checker, `hirsel-validate`,
checks structure, types, CFG, live slots, loans and cleanup dataflow before
publication/execution, reporting invariant failures separately from source
rejection diagnostics.

TIR v4 retains the sorted body set that inherits declaration caller attribution:
the entry and its own internal contract bodies. Ordinary closures and their
contracts remain separate invocation boundaries. Indirect
calls retain whether they are internal contract calls or source function-value
invocations. V derives this set from the independently supplied signature and clause producers.

TIR revision 5 adds `Constructor::SortedMap { by }`: the Order operand selects its brand,
its stored fields are empty,
and the ordinary Construct destination places its hidden empty entries storage.
V independently checks the dictionary type and result brand against its value identity.

TIR v6 adds size actuals to ReadConstant; it introduces no new instruction kind.
A constant signature can declare size parameters. ReadConstant has no runtime
operands: its size actuals are constants or lexical size references. The provider
materializes each selected instance, and native import emits the existing static
read using that instance's data.

`InstanceBodies` (CT-5) names every generic generated declaration and, per
declaration, its selected instances keyed by the canonical spelling of their
closed arguments (`InstanceBodies::key`). A declaration has no `Definition`;
`resolve` gives the monomorphic definition a call at closed arguments runs, and
`TirProvider::instance_body` serves the same lookup lazily (default: none).

