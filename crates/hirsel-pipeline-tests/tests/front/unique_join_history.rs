//! Bounded ownership histories, independent of either production flow join.
use hirsel_engine::{
    DefinitionQueries, QueryEngine, check_progress, default_context, default_environment,
    with_compiler_stack,
};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug)]
enum Event {
    Share(usize),
    Fresh(usize),
    EndLoan,
}
use Event::*;

// Concrete graph identities, with all possible owners retained. A counted
// copy's scope ending does not release its static count fact (ownership.md).
// Slot 0 is the enclosing graph; slots 1/2 are disjoint owned fields. Replacing
// a field replaces only its graph, never the enclosing graph's lost receipt.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Owners {
    graphs: [u8; 3],
    owners: [BTreeSet<u8>; 16],
    next: u8,
}
impl Owners {
    fn after(history: &[Event]) -> Self {
        let mut state = Self {
            graphs: [0, 1, 2],
            owners: std::array::from_fn(|_| BTreeSet::from([0])),
            next: 3,
        };
        for event in history {
            match *event {
                Share(part) => {
                    state.owners[state.graphs[0] as usize].insert(1);
                    if part == 0 {
                        for graph in state.graphs {
                            state.owners[graph as usize].insert(1);
                        }
                    } else {
                        state.owners[state.graphs[part] as usize].insert(1);
                    }
                }
                Fresh(part) => {
                    let parts = if part == 0 { 0..3 } else { part..part + 1 };
                    for part in parts {
                        state.graphs[part] = state.next;
                        state.next += 1;
                    }
                }
                EndLoan => (),
            }
        }
        state
    }
    fn unique(&self) -> bool {
        self.graphs
            .iter()
            .all(|graph| self.owners[*graph as usize].len() == 1)
    }
}

struct Case {
    name: &'static str,
    body: &'static str,
    // Only histories reaching the consumer occur here: an early return or
    // contradictory edge is not an extra reachable, pristine predecessor.
    histories: &'static [&'static [Event]],
    unique: bool,
}

fn cases(pair: bool, cases: &[Case]) {
    evidence_cases(pair, cases, false);
}

fn evidence_cases(pair: bool, cases: &[Case], crossing: bool) {
    with_compiler_stack(|| {
        for case in cases {
            let alternatives: BTreeSet<_> =
                case.histories.iter().map(|h| Owners::after(h)).collect();
            assert!(!alternatives.is_empty(), "{}", case.name);
            assert_eq!(
                alternatives.iter().all(Owners::unique),
                case.unique,
                "{}",
                case.name
            );
            let (declaration, ty, initial) = if pair {
                (
                    "type Pair {left:List[u64],right:List[u64]}",
                    "Pair",
                    "Pair(left:[1u64],right:[2u64])",
                )
            } else {
                ("", "List[u64]", "[1u64]")
            };
            let source = |required: &str| {
                format!(
                    "{declaration}\nfn need(take value:{required}{ty}) {{}}\nfn f(flag:bool,n:u64) {{let mut xs:unique {ty}={initial};{};need(take xs)}}\nfn main(){{f(false,0u64)}}",
                    case.body,
                )
            };
            let mut strict =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let file = strict
                .sources
                .add(
                    format!("{}.hir", case.name),
                    source(if crossing { "sendable " } else { "unique " }),
                )
                .unwrap();
            let checked = check_progress(&mut strict, file, &default_context()).unwrap();
            if case.unique {
                assert!(
                    checked.diagnostics.is_empty(),
                    "{}: {:?}",
                    case.name,
                    checked.diagnostics
                );
            } else {
                assert!(!checked.diagnostics.is_empty(), "{} accepted", case.name);
                assert!(
                    checked
                        .diagnostics
                        .iter()
                        .all(|d| d.rule.as_str() == if crossing { "MODE-4" } else { "MODE-2" }),
                    "{}: {:?}",
                    case.name,
                    checked.diagnostics
                );
            }

            // Lower the permissive source twin, then strengthen only the
            // untrusted consumer signature. J never supplied a unique-call
            // receipt to V for this twin; V must reconstruct the whole history.
            let mut relaxed =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let file = relaxed
                .sources
                .add(format!("{}-v.hir", case.name), source(""))
                .unwrap();
            let checked = check_progress(&mut relaxed, file, &default_context()).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{} twin: {:?}",
                case.name,
                checked.diagnostics
            );
            let mut path = hirsel_engine::entry_path(&default_context());
            path.0.name = "f".into();
            let definition = relaxed.lower_to_tir(&path).unwrap().value;
            let input = relaxed.export_native_input().unwrap();
            let mut signatures = hirsel_validate::SignatureMap::new(relaxed.types().clone());
            for signature in input.signatures.values() {
                let mut signature = (**signature).clone();
                if signature.definition.path().0.name.as_str() == "need" {
                    let hirsel_types::SignatureKind::Function(function) = &mut signature.kind
                    else {
                        panic!()
                    };
                    let mut parameters = function.parameters.to_vec();
                    let mut row = relaxed.types().get(parameters[0].ty).unwrap().clone();
                    if crossing {
                        row.modes.sendability = hirsel_types::Sendability::Sendable;
                        row.sendability_requirement =
                            hirsel_types::SendabilityRequirement::Sendable;
                    } else {
                        row.modes.uniqueness = hirsel_types::Uniqueness::Unique;
                    }
                    parameters[0].ty = relaxed.types().intern(row);
                    function.parameters = parameters.into();
                }
                signatures.insert(signature.definition, signature);
            }
            let verdict =
                hirsel_validate::validate(&definition, relaxed.types(), &signatures, &mut vec![]);
            if case.unique {
                assert!(
                    verdict.violations.is_empty(),
                    "{} V: {:?}",
                    case.name,
                    verdict.violations
                );
            } else {
                assert!(
                    verdict.violations.iter().any(|v| v.kind
                        == hirsel_validate::ViolationKind::TypeMismatch {
                            entity: "call argument"
                        }),
                    "{} V: {:?}",
                    case.name,
                    verdict.violations
                );
            }
            eprintln!(
                "{}: {} histories, J/V {}",
                case.name,
                alternatives.len(),
                case.unique
            );
        }
    });
}

#[test]
fn branches_keep_reachable_shares_and_distinguish_returns() {
    cases(
        false,
        &[
            Case {
                name: "share-yes",
                body: "if flag {let alias=copy xs}",
                histories: &[&[], &[Share(0)]],
                unique: false,
            },
            Case {
                name: "share-no",
                body: "if flag {} else {let alias=copy xs}",
                histories: &[&[Share(0)], &[]],
                unique: false,
            },
            Case {
                name: "share-both",
                body: "if flag {let a=copy xs} else {let b=copy xs}",
                histories: &[&[Share(0)], &[Share(0)]],
                unique: false,
            },
            Case {
                name: "share-before-join",
                body: "{let alias=copy xs};if flag {}",
                histories: &[&[Share(0), EndLoan]],
                unique: false,
            },
            Case {
                name: "fresh-one-arm",
                body: "if flag {xs=[2u64]}",
                histories: &[&[], &[Fresh(0)]],
                unique: true,
            },
            Case {
                name: "fresh-after-share-join",
                body: "if flag {let alias=copy xs};xs=[2u64]",
                histories: &[&[Fresh(0)], &[Share(0), Fresh(0)]],
                unique: true,
            },
            Case {
                name: "fresh-other-arm",
                body: "if flag {let alias=copy xs} else {xs=[2u64]}",
                histories: &[&[Share(0)], &[Fresh(0)]],
                unique: false,
            },
            Case {
                name: "return-shared-arm",
                body: "if flag {let alias=copy xs;return}",
                histories: &[&[]],
                unique: true,
            },
            Case {
                name: "return-pristine-arm",
                body: "if flag {return} else {let alias=copy xs}",
                histories: &[&[Share(0)]],
                unique: false,
            },
        ],
    );
}

#[test]
fn loops_keep_zero_one_two_iteration_break_and_continue_histories() {
    cases(
        false,
        &[
            Case {
                // The original while-false/share precision probe is retained externally.
                name: "empty-zero-iterations",
                body: "while false {}",
                histories: &[&[]],
                unique: true,
            },
            Case {
                name: "one-iteration",
                body: "while true {let alias=copy xs;break}",
                histories: &[&[Share(0)]],
                unique: false,
            },
            Case {
                name: "unrolled-two-iterations",
                body: "{let a=copy xs};{let b=copy xs}",
                histories: &[&[Share(0), Share(0)]],
                unique: false,
            },
            Case {
                name: "loop-share",
                body: "let mut i=0u64;while i<n {let alias=copy xs;i=i+1u64}",
                histories: &[&[], &[Share(0)], &[Share(0), Share(0)]],
                unique: false,
            },
            Case {
                name: "loop-break",
                body: "while flag {let alias=copy xs;break}",
                histories: &[&[], &[Share(0)]],
                unique: false,
            },
            Case {
                name: "loop-continue",
                body: "let mut i=0u64;while i<n {i=i+1u64;let alias=copy xs;continue}",
                histories: &[&[], &[Share(0)], &[Share(0), Share(0)]],
                unique: false,
            },
            Case {
                name: "loop-fresh-each-time",
                body: "let mut i=0u64;while i<n {{let alias=copy xs};xs=[2u64];i=i+1u64}",
                histories: &[
                    &[],
                    &[Share(0), Fresh(0)],
                    &[Share(0), Fresh(0), Share(0), Fresh(0)],
                ],
                unique: true,
            },
            Case {
                name: "loop-fresh-after",
                body: "while flag {let alias=copy xs;break};xs=[3u64]",
                histories: &[&[Fresh(0)], &[Share(0), Fresh(0)]],
                unique: true,
            },
        ],
    );
}

#[test]
fn projected_replacement_cannot_reset_an_enclosing_or_sibling_graph() {
    cases(
        true,
        &[
            Case {
                name: "same-field",
                body: "{let alias=copy xs.left};xs.left=[3u64]",
                histories: &[&[Share(1), Fresh(1)]],
                unique: false,
            },
            Case {
                name: "disjoint-field",
                body: "{let alias=copy xs.left};xs.right=[3u64]",
                histories: &[&[Share(1), Fresh(2)]],
                unique: false,
            },
            Case {
                name: "both-fields",
                body: "{let alias=copy xs.left};xs.left=[3u64];xs.right=[4u64]",
                histories: &[&[Share(1), Fresh(1), Fresh(2)]],
                unique: false,
            },
            Case {
                name: "whole-pair",
                body: "{let alias=copy xs.left};xs=Pair(left:[3u64],right:[4u64])",
                histories: &[&[Share(1), Fresh(0)]],
                unique: true,
            },
            Case {
                name: "fresh-field-no-share",
                body: "if flag {xs.left=[3u64]} else {xs.right=[4u64]}",
                histories: &[&[Fresh(1)], &[Fresh(2)]],
                unique: true,
            },
        ],
    );
}

#[test]
fn crossing_evidence_is_not_recovered_from_a_join_or_ended_share() {
    evidence_cases(
        false,
        &[
            Case {
                name: "crossing-share-yes",
                body: "if flag {let alias=copy xs}",
                histories: &[&[], &[Share(0)]],
                unique: false,
            },
            Case {
                name: "crossing-share-no",
                body: "if flag {} else {let alias=copy xs}",
                histories: &[&[Share(0)], &[]],
                unique: false,
            },
            Case {
                name: "crossing-ended-share",
                body: "{let alias=copy xs};if flag {}",
                histories: &[&[Share(0), EndLoan]],
                unique: false,
            },
            Case {
                name: "crossing-fresh",
                body: "if flag {let alias=copy xs};xs=[2u64]",
                histories: &[&[Fresh(0)], &[Share(0), Fresh(0)]],
                unique: true,
            },
            Case {
                name: "crossing-continue",
                body: "let mut i=0u64;while i<n {i=i+1u64;let alias=copy xs;continue}",
                histories: &[&[], &[Share(0)], &[Share(0), Share(0)]],
                unique: false,
            },
        ],
        true,
    );
}
