use super::*;

fn function(name: &str) -> Function {
    (
        hirsel_syntax::DefPath::core(name),
        hirsel_syntax::ContentHash([0; 32]),
    )
}

fn local(name: &str, dependencies: &[Option<(Function, u32)>]) -> LocalArenaEquations {
    let mut callees = BTreeSet::new();
    let candidates = dependencies
        .iter()
        .enumerate()
        .map(|(parameter, dep)| {
            let e = match dep {
                None => Equation::Constant(true),
                Some((f, p)) => {
                    callees.insert(f.clone());
                    Equation::Call {
                        target: Some(f.clone()),
                        fallback: None,
                        arguments: vec![(*p, 1)],
                    }
                }
            };
            Candidate {
                parameter: parameter as u32,
                equations: vec![e, Equation::Constant(true)],
                requests: vec![0],
                returns_void: false,
            }
        })
        .collect();
    LocalArenaEquations {
        owner: function(name),
        candidates,
        callees: callees.into_iter().collect(),
    }
}

#[test]
fn chosen_uses_own_candidates_and_never_replaces_a_lost_minimum() {
    // Parent DESIGN's mandatory two-node counterexample. f0 <- g1,
    // f1 <- g0, both g candidates valid: chosen f is empty, g is {0}.
    let f = local("f", &[Some((function("g"), 1)), Some((function("g"), 0))]);
    let g = local("g", &[None, None]);
    let g0 = solve_component(&[&g], None, &[]).unwrap();
    let f0 = solve_component(&[&f], None, &g0).unwrap();
    assert_eq!(f0[0].1, [0, 1]);
    let g1 = solve_component(&[&g], Some(&g0), &[]).unwrap();
    let f1 = solve_component(&[&f], Some(&f0), &g1).unwrap();
    assert_eq!(g1[0].1, [0]);
    assert!(f1[0].1.is_empty());
    // Re-solving Candidates against g's Chosen would incorrectly choose f1.
    assert_eq!(solve_component(&[&f], None, &g1).unwrap()[0].1, [1]);
}

#[test]
fn cyclic_candidates_delete_through_the_component() {
    let f = local("f", &[Some((function("g"), 0))]);
    let g = local("g", &[Some((function("f"), 0))]);
    let settled = solve_component(&[&f, &g], None, &[]).unwrap();
    assert_eq!(
        settled.iter().map(|(_, xs)| xs.clone()).collect::<Vec<_>>(),
        [vec![0], vec![0]]
    );
    let mut changed = g.clone();
    changed.candidates[0].equations[0] = Equation::Constant(false);
    assert!(
        solve_component(&[&f, &changed], None, &[])
            .unwrap()
            .iter()
            .all(|(_, xs)| xs.is_empty())
    );
}

#[test]
fn active_local_cycles_remain_false_with_request_order_preserved() {
    let c = Candidate {
        parameter: 0,
        equations: vec![Equation::All(vec![1]), Equation::All(vec![0])],
        requests: vec![0, 1],
        returns_void: false,
    };
    assert!(!c.proves(&BTreeMap::new()));
    let mut c = c;
    c.equations[1] = Equation::Constant(true);
    assert!(c.proves(&BTreeMap::new()));
}

#[test]
fn missing_or_noncanonical_dependencies_and_foreign_candidates_refuse() {
    let f = local("f", &[Some((function("g"), 0))]);
    assert!(solve_component(&[&f], None, &[]).is_err());
    assert!(solve_component(&[&f], None, &[(function("g"), vec![1, 0])]).is_err());
    assert!(
        solve_component(
            &[&f],
            Some(&[(function("g"), vec![0])]),
            &[(function("g"), vec![0])]
        )
        .is_err()
    );
    let mut malformed = f.clone();
    malformed.candidates[0].requests.push(u32::MAX);
    assert!(malformed.validate().is_err());
}

#[test]
fn stratified_components_match_global_deletion_for_five_thousand_equation_graphs() {
    // Parent oracle seed and sizes; positive conjunction/disjunction equations,
    // independently settled by a whole-graph deletion reference below.
    let mut seed = 191919u64;
    let mut random = |bound: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed as usize) % bound
    };
    for size in [1, 2, 3, 4, 8] {
        for _ in 0..1000 {
            let mut locals = Vec::new();
            for i in 0..size {
                let mut candidates = Vec::new();
                let mut callees = BTreeSet::new();
                for parameter in 0..2 {
                    if random(3) == 0 {
                        continue;
                    }
                    let clauses = random(3);
                    let mut equations =
                        vec![Equation::All((1..=clauses).map(|n| n as u32).collect())];
                    let mut extra = Vec::new();
                    for _ in 0..clauses {
                        let target = function(&format!("f{}", random(size)));
                        callees.insert(target.clone());
                        let mut arguments = Vec::new();
                        for p in 0..2 {
                            if random(2) == 1 {
                                arguments.push((p, (1 + clauses + extra.len()) as u32));
                                extra.push(Equation::Constant(true));
                            }
                        }
                        equations.push(Equation::Call {
                            target: Some(target),
                            fallback: None,
                            arguments,
                        });
                    }
                    equations.extend(extra);
                    candidates.push(Candidate {
                        parameter,
                        equations,
                        requests: vec![0],
                        returns_void: false,
                    });
                }
                locals.push(LocalArenaEquations {
                    owner: function(&format!("f{i}")),
                    candidates,
                    callees: callees.into_iter().collect(),
                });
            }
            let mut expected: BTreeMap<_, BTreeSet<_>> = locals
                .iter()
                .map(|m| (m.owner.clone(), m.candidates().into_iter().collect()))
                .collect();
            for choose in [false, true] {
                if choose {
                    for xs in expected.values_mut() {
                        *xs = xs.first().copied().into_iter().collect();
                    }
                }
                loop {
                    let mut changed = false;
                    for m in &locals {
                        let invalid: Vec<_> = m
                            .candidates
                            .iter()
                            .filter(|c| {
                                expected[&m.owner].contains(&c.parameter) && !c.proves(&expected)
                            })
                            .map(|c| c.parameter)
                            .collect();
                        for p in invalid {
                            expected.get_mut(&m.owner).unwrap().remove(&p);
                            changed = true;
                        }
                    }
                    if !changed {
                        break;
                    }
                }
            }
            let expected: BTreeMap<_, Vec<_>> = expected
                .into_iter()
                .map(|(f, xs)| (f, xs.into_iter().collect()))
                .collect();
            assert_eq!(settle(&locals, &[]).unwrap(), expected);
        }
    }
}
