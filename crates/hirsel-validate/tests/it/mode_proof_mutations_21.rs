//! Paired adversaries at the call, join and capture evidence boundaries.
//! No J/ownership producer is involved. Expected obligations are literal.
//!
//! Call/failure separation adapts rustc_mir_dataflow initialized.rs:255-303
//! and framework/tests.rs:1-68, revision 19833af7224424e8b53e288097d56b6af43d5bcf
//! (MIT, Rust Project Contributors). Hirsel rejects disconnected executable
//! blocks; these fixtures use connected branches and real call continuations.
use crate::support::*;
use hirsel_syntax::PassingMode;
use hirsel_tir::LocalId;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::{SignatureMap, ViolationKind, validate};

fn string() -> TypeKind {
    TypeKind::Primitive(Primitive::String)
}

fn row_at(f: &mut Fixture, place: u32, edit: impl FnOnce(&mut Type)) {
    let original = f.body().places[place as usize].ty;
    let mut row = interner().row(original).clone();
    edit(&mut row);
    let row = f.definition.types.push(row);
    let root = f.body().places[place as usize].root;
    f.body().locals[root.0 as usize].ty = row;
    f.body().places[place as usize].ty = row;
}

fn call(f: &mut Fixture, place: u32, required: TypeId, cleanup: u32) {
    let mut signature = function_signature(
        "consume",
        vec![(PassingMode::Take, string())],
        TypeKind::Tuple(vec![].into()),
    );
    let SignatureKind::Function(header) = &mut signature.kind else {
        unreachable!()
    };
    header.parameters[0].ty = required;
    f.signatures.insert(id("consume"), signature);
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("consume"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: take(place),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(cleanup),
    };
    f.body().blocks.push(block(vec![], ret(result, cleanup)));
}

fn unique_string() -> TypeId {
    let mut row = ty(string());
    row.modes.uniqueness = Uniqueness::Unique;
    interner().intern(row)
}

fn share(f: &mut Fixture, source: u32, cleanup: u32) {
    let kind = interner()
        .row(f.body().places[source as usize].ty)
        .kind
        .clone();
    let alias = f.slot(kind, Storage::Temporary);
    f.assign(alias, Rvalue::Use(copy(source)), cleanup);
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::Drop {
            place: PlaceId(alias),
            if_live: false,
        },
        cleanup,
    ));
}

// Change only the receiver's uniqueness claim after an explicit, ended share.
fn current_unique(forged: bool) -> Fixture {
    let mut f = Fixture::string();
    share(&mut f, 1, 1);
    let required = if forged {
        unique_string()
    } else {
        interner().intern(ty(string()))
    };
    call(&mut f, 1, required, 1);
    f
}

// Both sources have the same physical row. Only one has a fresh current value.
fn crossing_supplier(forged: bool) -> Fixture {
    let mut f = Fixture::string();
    let stale = f.slot(string(), Storage::Local);
    f.assign(stale, Rvalue::Constant(Constant::String("stale".into())), 1);
    let cleanup = f.cleanup(vec![drop(stale), drop(1)]);
    share(&mut f, stale, cleanup);
    let mut required = ty(string());
    required.modes.sendability = Sendability::Sendable;
    required.sendability_requirement = SendabilityRequirement::Sendable;
    call(
        &mut f,
        if forged { stale } else { 1 },
        interner().intern(required),
        cleanup,
    );
    f
}

// A field's current owner, rather than its static aliased row, supplies proof.
fn projection_owner(forged: bool) -> Fixture {
    let mut f = Fixture::unit();
    let field = f.definition.types.push(ty(string()));
    let kind = TypeKind::Tuple(vec![field].into());
    let pristine = param(&mut f, kind.clone());
    let shared = param(&mut f, kind);
    for p in [pristine, shared] {
        row_at(&mut f, p, |row| row.modes.uniqueness = Uniqueness::Unique);
    }
    owner_sig(&mut f, &[PassingMode::Take, PassingMode::Take]);
    let cleanup = f.cleanup(vec![drop(shared), drop(pristine)]);
    share(&mut f, shared, cleanup);
    let projected = f.body().places.len() as u32;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(if forged { shared } else { pristine }),
        projection: vec![Projection::Field(0)].into(),
        ty: field,
    });
    call(&mut f, projected, unique_string(), cleanup);
    f
}

// Both generic declarations exist. The same ordinal in the child binder has
// no sendability bound; a foreign/missing binder would only be malformed setup.
fn generic_binder(forged: bool) -> Fixture {
    let mut f = Fixture::unit();
    owner_sig(&mut f, &[]);
    let SignatureKind::Function(owner) = &mut f.signatures.get_mut(id("main")).unwrap().kind else {
        unreachable!()
    };
    owner.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![Bound::Sendable].into(),
    }]
    .into();
    let mut child = empty_body(&mut f);
    let scope = if forged {
        child.parameter_scope
    } else {
        f.body().parameter_scope
    };
    let actual = f
        .definition
        .types
        .push(ty(TypeKind::Parameter(TypeParameterRef::body(
            scope,
            TypeParamId(0),
        ))));
    let unit = child.result;
    let result = add_capture(&mut child, unit);
    child.locals[result as usize].storage = Storage::Temporary;
    child.blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("bounded"),
            arguments: vec![GenericArgument::Type(actual)],
        },
        arguments: vec![],
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    child.blocks.push(block(vec![], ret(result, 0)));
    f.definition.bodies.push(child);
    let unit = f.body().result;
    let mut header = fn_type(vec![], unit);
    header.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    }]
    .into();
    let closure = f.slot(TypeKind::Function(header), Storage::Temporary);
    f.assign(
        closure,
        Rvalue::Closure {
            context: CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![],
            destination: Destination::Inferred,
        },
        0,
    );
    let cleanup = f.cleanup(vec![drop(closure)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    let mut bounded = function_signature("bounded", vec![], TypeKind::Tuple(vec![].into()));
    let SignatureKind::Function(header) = &mut bounded.kind else {
        unreachable!()
    };
    header.generics = vec![GenericParameter::Type {
        name: "S".into(),
        bounds: vec![Bound::Sendable].into(),
    }]
    .into();
    f.signatures.insert(id("bounded"), bounded);
    f
}

// The altered edge admits a shared predecessor to a previously pristine call.
// No constant-condition pruning or producer reachability fact is assumed.
fn incoming_branch(forged: bool, reversed: bool) -> Fixture {
    let mut f = Fixture::string();
    let flag = param(&mut f, TypeKind::Primitive(Primitive::Bool));
    owner_sig(&mut f, &[PassingMode::Borrow]);
    call(&mut f, 1, unique_string(), 1);
    f.body().blocks[1].terminator = ret(0, 1);
    let call_block = f.body().blocks[0].terminator.clone();
    // The shared block is assembled with the same ordinary transfer as above.
    let start = f.body().blocks[0].instructions.len();
    share(&mut f, 1, 1);
    let sharing = f.body().blocks[0].instructions.split_off(start);
    f.body().blocks[0].terminator = Terminator::Branch {
        condition: copy(flag),
        yes: BlockId(if reversed { 3 } else { 2 }),
        no: BlockId(if reversed { 2 } else { 3 }),
    };
    // block 1 is the call continuation. Both branches and both exits remain
    // connected even when the shared edge changes from exit to call.
    f.body().blocks.push(block(
        sharing,
        Terminator::Goto(BlockId(if forged { 4 } else { 1 })),
    ));
    f.body()
        .blocks
        .push(block(vec![], Terminator::Goto(BlockId(4))));
    f.body().blocks.push(block(vec![], call_block));
    f
}

// The normal continuation is unchanged; only call-failure cleanup is forged.
fn cleanup_obligation(forged: bool) -> Fixture {
    let mut f = Fixture::string();
    let argument = f.slot(string(), Storage::Local);
    f.assign(
        argument,
        Rvalue::Constant(Constant::String("argument".into())),
        1,
    );
    let cleanup = f.cleanup(vec![drop(argument), drop(1)]);
    call(&mut f, argument, interner().intern(ty(string())), cleanup);
    if forged {
        let Terminator::Call { on_failure, .. } = &mut f.body().blocks[0].terminator else {
            unreachable!()
        };
        *on_failure = CleanupId(0);
    }
    f
}

// Reuse the taken-capture shape from checks::taken_slot_capture_fixture. Keep
// current source content shared; strengthen exactly one child capture fact.
fn capture_claim(unique: bool, forged: bool) -> Fixture {
    let mut f = Fixture::string();
    share(&mut f, 1, 1);
    let mut child = empty_body(&mut f);
    let mut row = ty(string());
    if forged {
        if unique {
            row.modes.uniqueness = Uniqueness::Unique;
        } else {
            row.modes.sendability = Sendability::Sendable;
        }
    }
    let row = f.definition.types.push(row);
    let captured = add_capture(&mut child, row);
    let cleanup = child.add_cleanup(vec![drop(captured)]);
    child.blocks[0].terminator = ret(0, cleanup.0);
    f.definition.bodies.push(child);
    let unit = f.body().result;
    let closure = f.slot(
        TypeKind::Function(fn_type(vec![], unit)),
        Storage::Temporary,
    );
    row_at(&mut f, closure, |row| {
        row.modes.multiplicity = Multiplicity::Once
    });
    f.assign(
        closure,
        Rvalue::Closure {
            context: CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![take(1)],
            destination: Destination::Inferred,
        },
        1,
    );
    let cleanup = f.cleanup(vec![drop(closure), drop(1)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f
}

/// Canonical type tables plus Definition/Signature serde, with a fresh type
/// interner on decode. No engine export/store/cache implementation is involved.
fn round_trip(f: &Fixture) -> (Definition, std::sync::Arc<TypeInterner>, SignatureMap) {
    let definition = tabled(&f.definition);
    let signatures: Vec<_> = f.signatures.values().cloned().collect();
    let payload = (&definition, &signatures);
    let (_, roots) = with_table_collection(|| serde_json::to_vec(&payload).unwrap());
    let (table, _) = encode_table(interner(), &roots);
    assert!(table.valid_indices());
    let bytes = with_table_encoding(interner(), &roots, || serde_json::to_vec(&payload).unwrap());
    let table: TypeTable = serde_json::from_slice(&serde_json::to_vec(&table).unwrap()).unwrap();
    let types = TypeInterner::new(None);
    // Perturb row positions in addition to the subprocess's global ID salt.
    types.intern(ty(TypeKind::Primitive(Primitive::U16)));
    let ids = decode_table(&types, &table);
    let (definition, signatures): (Definition, Vec<Signature>) =
        with_table_decoding(&ids, || serde_json::from_slice(&bytes).unwrap());
    let mut map = SignatureMap::new(types.clone());
    map.extend_signatures(signatures);
    (definition, types, map)
}

#[derive(Clone, Copy, Debug)]
enum Expected {
    Valid,
    Mismatch(&'static str),
    Cleanup,
}

fn check_encoding(f: &Fixture, expected: Expected) {
    let (definition, types, signatures) = round_trip(f);
    for (definition, types, signatures) in [
        (&tabled(&f.definition), interner(), &f.signatures),
        (&definition, types.as_ref(), &signatures),
    ] {
        let errors = validate(definition, types, signatures, &mut vec![]).violations;
        match expected {
            Expected::Valid => assert!(errors.is_empty(), "{errors:?}"),
            Expected::Mismatch(entity) => {
                assert!(!errors.is_empty(), "forged {entity} accepted");
                assert!(
                    errors
                        .iter()
                        .all(|v| v.kind == ViolationKind::TypeMismatch { entity }),
                    "expected only {entity}, got {errors:?}"
                );
            }
            Expected::Cleanup => {
                assert!(!errors.is_empty(), "forged cleanup accepted");
                assert!(
                    errors.iter().all(|v| matches!(
                        v.kind,
                        ViolationKind::CleanupMismatch { .. } | ViolationKind::LeakedSlot(_)
                    )),
                    "{errors:?}"
                );
                assert!(
                    errors
                        .iter()
                        .all(|v| v.block == Some(BlockId(0)) && v.instruction.is_none()),
                    "must reach call failure: {errors:?}"
                );
            }
        }
    }
}

fn check(f: &Fixture, expected: Expected) {
    check_encoding(f, expected);
    if f.definition.bodies.len() > 1 {
        let mut permuted = Fixture {
            definition: f.definition.clone(),
            signatures: f.signatures.clone(),
        };
        let last = permuted.definition.bodies.len() as u32 - 1;
        permuted.definition.bodies.reverse();
        permuted.definition.entry = BodyId(last - permuted.definition.entry.0);
        for body in &mut permuted.definition.bodies {
            for rvalue in &mut body.rvalues {
                if let Rvalue::Closure { body, .. } = rvalue {
                    *body = BodyId(last - body.0);
                }
            }
        }
        check_encoding(&permuted, expected);
    }
}

fn matrix() -> String {
    let cases: [(&str, fn(bool) -> Fixture, Expected); 8] = [
        (
            "current unique after share",
            current_unique,
            Expected::Mismatch("call argument"),
        ),
        (
            "crossing supplier",
            crossing_supplier,
            Expected::Mismatch("call argument"),
        ),
        (
            "projection owner",
            projection_owner,
            Expected::Mismatch("call argument"),
        ),
        (
            "generic binder",
            generic_binder,
            Expected::Mismatch("generic bound"),
        ),
        (
            "incoming branch",
            |forged| incoming_branch(forged, false),
            Expected::Mismatch("call argument"),
        ),
        ("cleanup obligation", cleanup_obligation, Expected::Cleanup),
        (
            "capture unique",
            |forged| capture_claim(true, forged),
            Expected::Mismatch("value modes"),
        ),
        (
            "capture crossing",
            |forged| capture_claim(false, forged),
            Expected::Mismatch("closure capture"),
        ),
    ];
    let mut out = String::new();
    for (name, build, negative) in cases {
        eprintln!("checking {name}");
        check(&build(false), Expected::Valid);
        check(&build(true), negative);
        out += &format!("{name}: valid twin; {negative:?}; direct + round trip\n");
    }
    check(&incoming_branch(false, true), Expected::Valid);
    check(
        &incoming_branch(true, true),
        Expected::Mismatch("call argument"),
    );
    out += "9 pairs; 18 fixtures; 48 direct/decoded/body-permuted verdicts\n";
    out
}

#[test]
fn paired_receipts_survive_salts_and_tir_round_trips() {
    let Some(runs) = hirsel_intern::under_salts(
        "mode_proof_mutations_21::paired_receipts_survive_salts_and_tir_round_trips",
        &[1, 2, 0x5eed],
        matrix,
    ) else {
        return;
    };
    assert_eq!(runs.len(), 3);
    let ordinary = matrix();
    assert!(runs.iter().all(|run| run == &ordinary), "{runs:#?}");
    eprintln!("unsalted and 3 salted matrices: {ordinary}");
}
