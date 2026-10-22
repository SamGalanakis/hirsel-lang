//! Regression evidence for audit gaps G02–G04; no synthetic graph is attested.
use crate::rc_support::*;

#[test]
fn borrow_cursor_evidence_13_environment_diagnostic_preserves_ordinary_bindings() {
    for (source, rule) in [
        (
            "fn f(a:List[u64])->u64 {let mut c:List[u64] in env=a; c=a; list.len(c)}",
            "LANG-15",
        ),
        (
            "fn f(a:List[u64])->u64 {let alias=a; let mut c:List[u64] in env=alias; list.len(c)}",
            "LANG-15",
        ),
        (
            "fn f(a:List[u64])->u64 {let mut c:List[u64] in env=copy a; list.len(c)}",
            "MODE-1",
        ),
        (
            "fn f(a:List[u64])->u64 {let c:List[u64] in env=a; list.len(c)}",
            "MODE-1",
        ),
        (
            "fn f()->u64 {let mut c:List[u64] in env=[1]; list.len(c)}",
            "MODE-1",
        ),
        ("fn f()->u64 {let mut c:u64 in env=1; c}", "MODE-1"),
        (
            "fn f()->u64 {let mut c:(fn()->u64) in env=fn()->u64 {1}; c()}",
            "MODE-1",
        ),
        ("fn f(a:List[u64] in env)->u64 {list.len(a)}", "MODE-1"),
        (
            "fn f(a:List[u64])->u64 {let mut _:List[u64] in env=a; 0}",
            "MODE-1",
        ),
        (
            "fn f(a:(u64,List[u64]))->u64 {let mut (n,_):(u64,List[u64]) in env=a; n}",
            "MODE-1",
        ),
    ] {
        hirsel_engine::with_compiler_stack(|| {
            let mut engine = hirsel_engine::QueryEngine::new(
                hirsel_engine::default_environment(),
                hirsel_driver::compiler_adapters(),
            );
            let file = engine
                .sources
                .add("cursor.hir", format!("{source}\nfn main()->u64 {{0}}"))
                .unwrap();
            let output =
                hirsel_engine::check_progress(&mut engine, file, &hirsel_engine::default_context())
                    .unwrap();
            let rules: Vec<_> = output.diagnostics.iter().map(|d| d.rule.as_str()).collect();
            assert_eq!(rules, [rule], "{source}: {:?}", output.diagnostics);
        });
    }
}

#[test]
fn borrow_cursor_evidence_13_environment_diagnostic_repair_lowers() {
    let (program, lowered) = lower_valid(
        "fn f(a:List[u64])->u64{let mut c:List[u64] in a=a;c=a;list.len(c)}\nfn main()->u64 {f([1])}",
    );
    let definition = &program.definitions[&path("f")];
    assert_eq!(
        definition.bodies[definition.entry.0 as usize]
            .borrow_cursors
            .len(),
        1
    );
    assert!(!lowered.units.is_empty());
}

#[test]
fn borrow_cursor_evidence_13_native_import_rejects_owned_refill_with_old_audit() {
    use hirsel_driver::*;
    use hirsel_engine::*;
    use hirsel_tir::*;
    use hirsel_types::UseKind;
    use std::sync::Arc;

    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("cursor-attestation.hir", "fn f(a:List[u64])->u64 {let mut c=a; c=a; list.len(c)}\npub fn main()->Result[(),u64] {if f([1])==1 {Ok(())}else{Error(1)}}".into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let original = engine.export_native_input().unwrap();
        assert_eq!(original.completed_source_stages, NativeStatus::Complete);
        engine.import_native_input(original.clone()).unwrap();

        let mut forged = original.clone();
        let definition = Arc::make_mut(forged.definitions.get_mut(&path("f")).unwrap());
        let body = &mut definition.bodies[definition.entry.0 as usize];
        let [cursor] = body.borrow_cursors.as_slice() else {
            panic!("expected a checked cursor")
        };
        // The second read of parameter a stages the rebind's RHS. Changing
        // Borrow to Copy is precisely the forbidden source refill `c=copy a`.
        let root = body.parameters[0];
        let mut reads = body.rvalues.iter_mut().enumerate().filter(|(_, value)| {
            matches!(value, Rvalue::Use(op) if op.usage == UseKind::Borrow
                && body.places[op.place.0 as usize].root == root)
        });
        let (initializer, _) = reads.next().expect("initial borrow of a");
        assert_eq!(RvalueId(initializer as u32), cursor.initializer);
        let (_, Rvalue::Use(refill)) = reads.next().expect("refill borrow of a") else {
            unreachable!()
        };
        refill.usage = UseKind::Copy;
        assert!(reads.next().is_none(), "exactly two reads of a");
        assert_eq!(
            forged.audits, original.audits,
            "retain old source attestation"
        );
        assert!(Arc::ptr_eq(
            &forged.source_owners[&path("f")].syntax,
            &original.source_owners[&path("f")].syntax,
        ));
        let error = engine
            .import_native_input(forged)
            .err()
            .expect("owned refill with stale audit must be rejected");
        assert!(
            format!("{error:?}").contains("forged implementation"),
            "{error:?}"
        );
        engine.import_native_input(original).unwrap();
    });
}

pub(super) const BOUNDS: &str = r#"fn f(a:List[u64],rows:List[List[u64]])->u64 {
 let mut c:List[u64] in {a,rows}=a
 c=rows[3]
 list.len(c)
}
pub fn main()->Result[(),u64] {let n=f([1],[[2]]); if n==1 {Ok(())}else{Error(1)}}
"#;

/// Replay the straight-line witness's storage and permission state up to the
/// indexed RHS load. Inspect the failing edge before executing its cleanup,
/// rather than inferring a holding from the final MEM-14 observation.
fn old_cursor_at_failure(body: &hirsel_tir::Body) -> Result<(), &'static str> {
    use hirsel_tir::*;
    use hirsel_types::UseKind;
    use std::collections::BTreeSet;
    let [cursor] = body.borrow_cursors.as_slice() else {
        return Err("expected exactly one cursor");
    };
    let Rvalue::Use(ref initializer) = body.rvalues[cursor.initializer.0 as usize] else {
        return Err("expected borrowed initializer");
    };
    assert_eq!(initializer.usage, UseKind::Borrow);
    let loan = body.places[initializer.place.0 as usize].loan.unwrap();
    let mut active = BTreeSet::new();
    let mut holdings = vec![None; body.locals.len()];
    let root = |place: PlaceId| body.places[place.0 as usize].root.0 as usize;
    let indexed = |place: PlaceId| {
        body.places[place.0 as usize]
            .projection
            .iter()
            .any(|p| matches!(p, Projection::Index(_)))
    };
    let cleanup = |id, holdings: &mut Vec<Option<RvalueId>>, active: &mut BTreeSet<u32>| {
        for action in body.cleanup_actions(id) {
            match *action {
                CleanupAction::Drop { place, .. } => holdings[root(place)] = None,
                CleanupAction::EndLoan { loan, .. } => {
                    active.remove(&loan.0);
                }
                _ => panic!("unexpected cleanup in bounds witness: {action:?}"),
            }
        }
    };
    for instruction in &body.blocks[body.entry.0 as usize].instructions {
        let failure = match instruction.kind {
            InstructionKind::Assign { value, .. } => matches!(
                &body.rvalues[value.0 as usize], Rvalue::Use(op) if indexed(op.place)
            ),
            _ => false,
        };
        if failure {
            if holdings[cursor.carrier.0 as usize] != Some(cursor.initializer) {
                return Err("old cursor holding was released before RHS failure");
            }
            if !active.contains(&loan.0) {
                return Err("old cursor loan ended before RHS failure");
            }
            cleanup(instruction.on_failure, &mut holdings, &mut active);
            if active.contains(&loan.0) {
                return Err("normal failure cleanup did not end the old cursor loan");
            }
            return Ok(());
        }
        match instruction.kind {
            InstructionKind::BeginLoan { id, .. } => {
                active.insert(id.0);
            }
            InstructionKind::EndLoan { loan, .. } => {
                active.remove(&loan.0);
            }
            InstructionKind::Assign { destination, value } => {
                if let Rvalue::Use(ref op) = body.rvalues[value.0 as usize]
                    && op.usage == UseKind::Take
                {
                    holdings[root(op.place)] = None;
                }
                holdings[root(destination)] = Some(value);
            }
            InstructionKind::Drop { place, .. } | InstructionKind::StorageDead { place, .. } => {
                holdings[root(place)] = None;
            }
            InstructionKind::StorageLive(local) => holdings[local.0 as usize] = None,
            InstructionKind::EndScope(scope) => {
                for (local, holding) in body.locals.iter().zip(&mut holdings) {
                    let mut current = Some(local.scope);
                    while let Some(s) = current {
                        if s == scope {
                            *holding = None;
                            break;
                        }
                        current = body.scopes[s.0 as usize].parent;
                    }
                }
            }
            InstructionKind::RunCleanup(id) => cleanup(id, &mut holdings, &mut active),
            InstructionKind::BindOrigin { .. } => {}
            _ => panic!("unexpected instruction before bounds failure: {instruction:?}"),
        }
    }
    Err("no failing indexed RHS in the entry block")
}

#[test]
fn borrow_cursor_evidence_13_bounds_failure_keeps_old_holding_until_cleanup() {
    bounds_failure_evidence();
}

pub(super) fn bounds_failure_evidence() {
    use hirsel_driver::*;
    use hirsel_engine::*;
    use hirsel_tir::*;
    let program = with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("cursor-bounds.hir", BOUNDS.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let execution = run_observed_checked(
            &mut engine,
            &RunConfiguration {
                entry: entry_path(&default_context()),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let RunOutcome::Failed { primary, causes } = execution.outcome.unwrap() else {
            panic!("expected bounds failure")
        };
        assert_eq!(primary.rule, "MEM-14");
        assert_eq!((primary.site.line, primary.site.column), (3, 4));
        assert!(causes.is_empty());
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        crate::rc_support::program(validated.snapshot())
    });
    let definition = &program.definitions[&path("f")];
    let body = &definition.bodies[definition.entry.0 as usize];
    assert_eq!(old_cursor_at_failure(body), Ok(()));
    let cursor = &body.borrow_cursors[0];
    let place = PlaceId(
        body.places
            .iter()
            .position(|p| p.root == cursor.carrier && p.projection.is_empty())
            .unwrap() as u32,
    );
    let Rvalue::Use(ref initializer) = body.rvalues[cursor.initializer.0 as usize] else {
        unreachable!()
    };
    let loan = body.places[initializer.place.0 as usize].loan.unwrap();
    // Both controls edit the real lowered prefix after cursor installation,
    // retaining the original bounds access and its ordinary failure cleanup.
    for (kind, expected) in [
        (
            InstructionKind::Drop {
                place,
                if_live: false,
            },
            "old cursor holding was released before RHS failure",
        ),
        (
            InstructionKind::EndLoan {
                loan,
                if_active: false,
            },
            "old cursor loan ended before RHS failure",
        ),
    ] {
        let mut early = body.clone();
        let instructions = &mut early.blocks[body.entry.0 as usize].instructions;
        let installed = instructions
            .iter()
            .position(|i| {
                matches!(i.kind,
            InstructionKind::Assign { value, .. } if value == cursor.initializer)
            })
            .unwrap();
        let mut release = instructions[installed].clone();
        release.kind = kind;
        instructions.insert(installed + 1, release);
        assert_eq!(old_cursor_at_failure(&early), Err(expected));
    }
}
