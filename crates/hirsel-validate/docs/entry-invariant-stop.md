# Entry-invariant experiment: stop before certificate checking

The proposed full-join absorption order is insufficient for arbitrary untrusted
entry states. Keep full V authoritative. The `cfg(test)` module
`src/flow_evidence_laws.rs` retains two counterexamples and bounded supporting
oracles. There is no production checker, evidence format, acceptance token,
cache or environment switch.

## A binding can justify its own premature use

This is hand-built TIR, not accepted source syntax. Let `p` be a borrowed Bool
parameter and `a` an alias place for `p`, with a local origin binding `n`:

```text
b0: goto b1
b1: branch copy(a) -> b2, b3
b2: BindOrigin Alias(source=p, place=a[n]); goto b3
b3: assign return_slot = unit; return return_slot; cleanup empty
```

Full V rejects `InvalidOrigin(a)` at body 0, block 1, terminator. Moving the
binding to b0 is an accepted twin. The raw local name and the location ordinal
are both legitimate in the same immutable definition; this is not a stale
artifact or a missing declaration.

Let S be V's independently initialized ordinary state, and C the state after
V's own transfer of b2 from S. C adds the nonempty `origins.locations[n]` and
`origins.bindings[n]` facts. The full State join absorbs S into C. Supply C at
all four entries: reporting-enabled transfer has no errors, all four actual
normal successor obligations hold, and a return observation is produced. There
are no backedges, closures, failure cleanups or audit transitions in this
witness. Derived cleanup sets are independently rebuilt before comparison.

Nevertheless S cannot use a: `binding_valid` requires an existing **nonempty**
location set. Union can create that precondition. Safety over these raw facts
is not downward closed under absorption. A concrete interpreter whose only
binding operation is b2 must reject the b1 access before it can reach b2.

This falsifies the proposed unconditional safety argument, not V's production
solver. Full inference never seeds the premature location. Validating row
shapes, names and finite domain correspondence does not prove introduction
history. A future design must explain the presence/absence semantics and
realizability of these facts; changing V's join or weakening this oracle is
outside this experiment.

## Tag widening changes the execution class

```text
b0: switch b: true -> b3, false -> b1
b1: switch b: true -> b2, false -> b3
b2: assign return_slot = unit; unreachable
b3: assign return_slot = unit; return return_slot
```

The state at b1 with known false is covered by one with unknown tags. On the
b1-to-b2 edge, V routes the former to audit and the latter to executable flow.
With exact audit classes, the wider edge does not simulate the narrower one.
Reporting `Unreachable` on the wider state also produces a violation absent
from the audit state. With an empty b2 sentinel, the contradictory edge instead
vanishes. Both cases are retained; merging classes to obtain a passing law
would change the obligations. Full V accepts the original repeated switch.

## Scope of the retained oracle

The reference uses actual full `State::join`, not a manufactured delta base.
Its exhaustive State destructuring requires new fields to be accounted for.
It rebuilds all five cleanup sets from rows and compares exact member lists,
never trie IDs. Dead-only must/defused bits are normalized away. Structured
rows outside the exercised slice remain identical: this is **not** a general
canonicalizer or decoder for arbitrary origin/tag/crossing representations.

The bounded row oracle checks 12,544 pairs (112 rows), nine valid sharing class
pairs, and all five guarantee bits, with an independent alternative-based
scalar join. All bit masks are nonzero and in range. Dead ownership-conflict
facts are included as raw-shaped inputs; a separate witness records why a
producer's cached cleanup roots cannot be trusted for them. V's same-cache
shortcut can leave such a newly live conflict absent from cached derived sets;
the reference reconstructs it. No reachability claim is made for that raw row.
The tag fixture also checks reporting-on/off state equality for eight
block/class combinations. An OpenJDK-shaped exceptional-cleanup twin keeps
normal flow valid while omitting a required failure release. The witnesses run
under salts 1, 2, 0x5eed and unsalted in real subprocesses, through the existing
salt harness. Public validation is rerun independently for positive/negative
twins; repeated full results and recorded reads match on the premature-binding
witness.

This does not establish general transfer monotonicity, arbitrary structured
fact normalization, cross-audit simulation, closure context soundness, unique
proof mode equivalence, decoder boundedness, or complete move/borrow/expiry
simulation. Those remain requirements of a separately designed checker.

## Source reconciliation and field inventory

The implementation base is architecture revision
`57e84aedccbc732b64e38925f621960dcfd31f62`. All validator sources are unchanged
from design revision `a9a96640b06bd9471a36d14809cd978e9fc83655` before this
experiment. No earlier entry-evidence implementation was present. Line
references below use the implementation base.

| Fields | Consumers / evidence treatment |
|---|---|
| `places`, `loans` | Availability, moves, conflicts, loan end, exit/cleanup; nonzero possibility masks, full join. |
| `must`, `defused`, `ownership_conflicts` | Live-conditioned guarantees and finalizer/ownership alternatives; `flow.rs:187-215`, cleanup at 961-1007. All five must bits matter. |
| `shared`, `borrow_only`, `borrowed_shares` | Alias/hand-back and retained result sharing; coupled equation at 216-237, never independent reversed Bool order. |
| `current_unique`, `crossing_parts` | Current content guarantees and prefix lookup; intersection/structured join at 238-243; `crossing.rs:23-95`. False and absent differ. |
| `tags` | Active payload access, invalidation, capture and edge refinement; unknown means missing carrier; `tags.rs:91-111,307-408`. Audit simulation fails. |
| `envelopes` | Creation/install/return obligations; `creation.rs:8-16`; unknown is not empty. Not a generally supported raw domain here. |
| `contract_readonly`, `sources`, `before` | Restriction/source union, contract writes, cleanup precedence; `flow.rs:157-161,244-253,3736-3854`. |
| `content_loans` | Held/expired installed-content dependencies by finite field path; `loan_content.rs:6-90`; sibling retention cannot be erased. |
| `origins.identities`, `origins.parts`, `origins.owners` | Value identity and exact/prefix projection; `flow_origins.rs:239-283`, `flow.rs:1930-2040`; raw set inclusion is not a general comparator. |
| `origins.ended`, `origins.dead`, `origins.expired` | Incarnation/brand expiration, including loop reinsertion; `flow_origins.rs:285-316,574-646`. |
| `origins.bindings`, `origins.locations` | Canonical brand substitution and access authority; `flow.rs:1025-1062,4550-4755`; nonempty location precondition defeats absorption safety. |
| `origins.retained`, `origins.environments`, `origins.proofs` | Retained names, hidden captures, sharing/crossing requirements and proof maps; `flow_origins.rs:30-66,285-316`. Requires complete context, not body text. |
| `audit` | Exact class, edge routing and suppressed exit/return obligations; `flow.rs:5971,6113,6287`. |
| `cleanup_roots`, `live_temporaries`, `cleanup_conflicts`, `cleanup_loans`, `cleanup_maybe_loans` | Derived only, rebuilt with `refresh_cleanup` / `refresh_cleanup_loan`; exact members/order required, no raw accepted IDs. |

Every current State field is included above. None is declared irrelevant to a
future checker. The harness supports only the stated finite slices; everything
else remains unsupported, rather than being silently ignored.

## Borrowed mechanisms and disposition

CompCert `e276a3f881669252e2a5c4b5fec06f51f7143161`,
`backend/Kildall.v:24-36,62-114`, supplies the entry/successor inequations and
the conditional invariant obligations. The concrete witness shows why its
semilattice theorem cannot be imported as Hirsel's theorem.

JVMS 25 section 4.10.1.6 and OpenJDK
`a0785e62626f6b4e9a8bf7e3ca1bbd2c4b6f2133`,
`src/hotspot/share/classfile/stackMapTable.cpp:88-151` and
`verifier.cpp:2013-2048`, require checking frames before use and every applicable
exceptional target. `test/hotspot/jtreg/runtime/verifier/VerifyStackForExceptionHandlers.java:25-48,83-111`
supplies the positive/negative test shape. The Hirsel twin omits an owned
failure release, not JVM stack capacity. No OpenJDK implementation is copied.

The codec Snapshot pattern (`hirsel-codec/src/lib.rs:1-15,30-68`) and V-owned
reads (`input.rs:28-46,120-180`) were inspected. No format or read mechanism is
needed after this stop condition. Capture/proof-mode binding, bounded sidecars,
adversarial decode and generation/check benchmarks are deliberately unimplemented.
A V-produced certificate cannot supply the cheaper producer gate even if this
soundness issue is later resolved. No performance gain is claimed.
