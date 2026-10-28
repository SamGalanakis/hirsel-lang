//! Forged controls for V's typing, matching and witness checks that no test
//! reached. Each rejected program has an accepted twin one edit away.
use super::*;

fn scalar(f: &mut Fixture, primitive: Primitive, value: u128) -> u32 {
    let p = f.slot(TypeKind::Primitive(primitive), Storage::Temporary);
    f.assign(p, Rvalue::Constant(Constant::Integer(value)), 0);
    p
}

fn mismatch(entity: &'static str) -> Vec<ViolationKind> {
    vec![ViolationKind::TypeMismatch { entity }]
}

/// `let r: <result> = -x` for `x: i8`.
#[test]
fn a_unary_result_has_its_operand_type() {
    let build = |result: Primitive| {
        let mut f = Fixture::unit();
        let x = scalar(&mut f, Primitive::I8, 1);
        let r = f.slot(TypeKind::Primitive(result), Storage::Temporary);
        f.assign(
            r,
            Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Neg,
                value: copy(x),
            },
            0,
        );
        f
    };
    build(Primitive::I8).valid();
    build(Primitive::I16).broken(mismatch("unary result"));
}

/// `let r: <result> = x == x` for `x: u8`.
#[test]
fn a_comparison_result_is_bool() {
    let build = |result: Primitive| {
        let mut f = Fixture::unit();
        let x = scalar(&mut f, Primitive::U8, 1);
        let r = f.slot(TypeKind::Primitive(result), Storage::Temporary);
        f.assign(
            r,
            Rvalue::Binary {
                op: BinaryOp::Eq,
                left: copy(x),
                right: copy(x),
            },
            0,
        );
        f
    };
    build(Primitive::Bool).valid();
    build(Primitive::U8).broken(mismatch("comparison result"));
}

/// `let g = callee; g()` through a borrowed view of `g`, or the same call
/// through a u8.
#[test]
fn an_indirect_callee_is_a_function_value() {
    let build = |callable: bool| {
        let mut f = Fixture::unit();
        let unit = unit_ty(&mut f);
        let g = f.slot(TypeKind::Function(fn_type(vec![], unit)), Storage::Local);
        let signature = function_signature("callee", vec![], TypeKind::Tuple(vec![].into()));
        f.signatures.insert(id("callee"), signature);
        f.assign(
            g,
            Rvalue::FunctionReference {
                definition: path("callee"),
                arguments: vec![],
            },
            0,
        );
        f.emit(InstructionKind::BeginLoan {
            id: LoanId(0),
            place: PlaceId(g),
            mutable: false,
            if_inactive: false,
        });
        let via = f.through_loan(g, 0);
        let value = if callable {
            operand(via, UseKind::Borrow)
        } else {
            copy(scalar(&mut f, Primitive::U8, 0))
        };
        let exit = f.cleanup(vec![
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            drop(g),
        ]);
        let r = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        f.body().blocks[0].terminator = Terminator::Call {
            callee: Callee::Indirect {
                forward_caller: false,
                value,
                arguments: vec![],
            },
            arguments: vec![],
            returns: Some(CallReturn {
                destination: PlaceId(r),
                next: BlockId(1),
            }),
            on_failure: CleanupId(exit),
        };
        f.body().blocks.push(block(vec![], ret(0, exit)));
        f
    };
    build(true).valid();
    build(false).broken(mismatch("indirect callee"));
}

/// One input per field ordinal: `(a, a)` with both inputs at field 0.
#[test]
fn each_constructor_input_names_a_distinct_field() {
    let build = |second: u32| {
        let mut f = Fixture::unit();
        let byte = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::U8)));
        let a = scalar(&mut f, Primitive::U8, 1);
        let b = scalar(&mut f, Primitive::U8, 2);
        let pair = f.slot(TypeKind::Tuple(vec![byte, byte].into()), Storage::Temporary);
        f.assign(
            pair,
            Rvalue::Construct {
                constructor: Constructor::Tuple,
                fields: vec![
                    ConstructField {
                        field: 0,
                        value: copy(a),
                    },
                    ConstructField {
                        field: second,
                        value: copy(b),
                    },
                ],
                destination: Destination::Inferred,
            },
            0,
        );
        f
    };
    build(1).valid();
    build(0).broken(vec![ViolationKind::Duplicate {
        entity: "constructor field",
        index: 0,
    }]);
}

/// A closure body declares exactly the parameters of its function type.
#[test]
fn a_closure_body_has_its_function_types_parameters() {
    let mut f = closure_fixture();
    f.valid();
    let string = f.body().places[1].ty;
    let nested = &mut f.definition.bodies[1];
    let p = add_capture(nested, string);
    nested.locals[p as usize].storage = Storage::Parameter;
    nested.parameters.push(LocalId(p));
    f.broken(vec![ViolationKind::Arity {
        entity: "closure parameters",
        expected: 0,
        actual: 1,
    }]);
}

/// MEM-5: a closure typed with a no-allocation cleanup owns only captures
/// whose own cleanup cannot allocate. Here it takes a callable whose cleanup may.
#[test]
fn a_noalloc_closure_cleanup_owns_only_noalloc_captures() {
    let build = |closure_drop: DropAllocation| {
        let mut f = Fixture::unit();
        let unit = unit_ty(&mut f);
        let mut held = ty(TypeKind::Function(fn_type(vec![], unit)));
        held.drop_allocation = DropAllocation::MayAllocate;
        let held = f.definition.types.push(held);
        let g = f.slot(TypeKind::Tuple(vec![].into()), Storage::Local);
        f.body().locals[g as usize].ty = held;
        f.body().places[g as usize].ty = held;
        let signature = function_signature("callee", vec![], TypeKind::Tuple(vec![].into()));
        f.signatures.insert(id("callee"), signature);
        f.assign(
            g,
            Rvalue::FunctionReference {
                definition: path("callee"),
                arguments: vec![],
            },
            0,
        );
        let mut nested = empty_body(&mut f);
        add_capture(&mut nested, held);
        // A once call consumes the environment, so the body releases it.
        edit_cleanup(&mut nested, 0, |actions| *actions = vec![drop(1)]);
        f.definition.bodies.push(nested);
        let mut call = fn_type(vec![], unit);
        call.allocation = Allocation::MayAllocate;
        let mut closure = ty(TypeKind::Function(call));
        // Taking a callable that is not copyable makes the closure once.
        closure.modes.multiplicity = Multiplicity::Once;
        closure.drop_allocation = closure_drop;
        let closure = f.definition.types.push(closure);
        let c = f.slot(TypeKind::Tuple(vec![].into()), Storage::Local);
        f.body().locals[c as usize].ty = closure;
        f.body().places[c as usize].ty = closure;
        let failure = f.cleanup(vec![drop(g)]);
        f.assign(
            c,
            Rvalue::Closure {
                context: hirsel_types::CaptureContext::OwningScoped,
                body: BodyId(1),
                captures: vec![take(g)],
                destination: Destination::Inferred,
            },
            failure,
        );
        let exit = f.cleanup(vec![drop(c)]);
        f.body().blocks[0].terminator = ret(0, exit);
        // The owner's own cleanup may allocate, so it may drop either closure.
        owner_signature(&mut f, PassingMode::Take);
        let SignatureKind::Function(owner) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
        else {
            unreachable!()
        };
        owner.allocation = Allocation::MayAllocate;
        f
    };
    build(DropAllocation::MayAllocate).valid();
    build(DropAllocation::NoAlloc).broken(mismatch("closure cleanup"));
}

/// FACT-1: a contract clause is a predicate, `fn(borrowed formals) -> bool`.
fn contract_clause(result: Primitive) -> Fixture {
    let mut f = Fixture::unit();
    let string = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::String)));
    let result_ty = f.definition.types.push(ty(TypeKind::Primitive(result)));
    let mut clause = empty_body(&mut f);
    clause.locals[0].ty = result_ty;
    clause.places[0].ty = result_ty;
    clause.result = result_ty;
    clause.rvalues[0] = match result {
        Primitive::Bool => Rvalue::Constant(Constant::Bool(true)),
        _ => Rvalue::Constant(Constant::Integer(1)),
    };
    let parameter = add_capture(&mut clause, string);
    clause.locals[parameter as usize].storage = Storage::Parameter;
    clause.parameters.push(LocalId(parameter));
    f.definition.bodies.push(clause);
    let function = f.slot(
        TypeKind::Function(fn_type(
            vec![Parameter {
                name: None,
                label: None,
                passing: PassingMode::Borrow,
                ty: string,
            }],
            result_ty,
        )),
        Storage::Local,
    );
    f.assign(
        function,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![],
            destination: Destination::Inferred,
        },
        0,
    );
    let cleanup = f.cleanup(vec![drop(function)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f.definition.contracts.push((0, BodyId(1)));
    f
}

#[test]
fn a_contract_clause_returns_bool() {
    contract_clause(Primitive::Bool).valid();
    contract_clause(Primitive::U8).broken(mismatch("contract result"));
}

/// `let g = generic[<arguments>]` for `fn generic[T]() -> ()`.
fn function_reference(arguments: fn(TypeId) -> Vec<GenericArgument>) -> Fixture {
    let mut f = Fixture::unit();
    let unit = unit_ty(&mut f);
    let arguments = arguments(unit);
    let mut signature = function_signature("generic", vec![], TypeKind::Tuple(vec![].into()));
    let SignatureKind::Function(fun) = &mut signature.kind else {
        unreachable!()
    };
    fun.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    }]
    .into();
    f.signatures.insert(id("generic"), signature);
    let g = f.slot(TypeKind::Function(fn_type(vec![], unit)), Storage::Local);
    f.assign(
        g,
        Rvalue::FunctionReference {
            definition: path("generic"),
            arguments,
        },
        0,
    );
    let exit = f.cleanup(vec![drop(g)]);
    f.body().blocks[0].terminator = ret(0, exit);
    f
}

#[test]
fn a_function_reference_instantiates_each_generic_with_its_kind() {
    function_reference(|t| vec![GenericArgument::Type(t)]).valid();
    function_reference(|t| vec![GenericArgument::Type(t), GenericArgument::Type(t)]).broken(vec![
        ViolationKind::Arity {
            entity: "function reference generics",
            expected: 1,
            actual: 2,
        },
    ]);
    function_reference(|_| vec![GenericArgument::Size(Size::Constant(1))])
        .broken(mismatch("function reference generic kind"));
}

/// `struct Cell[T]` held as a borrowed parameter `Cell[<arguments>]`.
fn nominal(arguments: fn(TypeId) -> Vec<GenericArgument>) -> Fixture {
    let mut f = Fixture::unit();
    let unit = unit_ty(&mut f);
    let arguments = arguments(unit);
    let mut s = capability_signature("Cell");
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.capability = None;
    d.authority = AuthoritySummary::inert();
    d.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    }];
    f.signatures.insert(id("Cell"), s);
    param(
        &mut f,
        TypeKind::Named {
            definition: id("Cell"),
            arguments: arguments.into(),
        },
    );
    owner_signature(&mut f, PassingMode::Borrow);
    f
}

#[test]
fn a_nominal_type_has_one_argument_of_each_declared_kind() {
    nominal(|t| vec![GenericArgument::Type(t)]).valid();
    nominal(|_| vec![]).broken(mismatch("nominal generic arguments"));
    nominal(|_| vec![GenericArgument::Size(Size::Constant(1))])
        .broken(mismatch("nominal generic argument kind"));
}

/// A bound on `Wrapper[U]` inside `fn[U: <inner>](Wrapper[U])` is discharged
/// by `U`'s own binder, never by the body's same-ordinal `T: <outer>`.
#[test]
fn nominal_bounds_inside_a_generic_function_type_use_its_own_binder() {
    let build = |inner: bool, outer: bool| {
        let mut f = Fixture::unit();
        let unit = unit_ty(&mut f);
        let mut s = capability_signature("Wrapper");
        let rep = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let SignatureKind::Type(d) = &mut s.kind else {
            unreachable!()
        };
        d.capability = None;
        d.authority = AuthoritySummary::inert();
        d.representation = Some(rep);
        d.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Plain].into(),
        }];
        f.signatures.insert(id("Wrapper"), s);
        let u = f
            .definition
            .types
            .push(ty(TypeKind::Parameter(TypeParameterRef::bound(
                TypeParamId(0),
            ))));
        let wrapped = f.definition.types.push(ty(TypeKind::Named {
            definition: id("Wrapper"),
            arguments: vec![GenericArgument::Type(u)].into(),
        }));
        let bounds = |plain: bool| {
            if plain {
                vec![Bound::Plain].into()
            } else {
                vec![].into()
            }
        };
        let mut callback = fn_type(
            vec![Parameter {
                name: None,
                label: None,
                passing: PassingMode::Borrow,
                ty: wrapped,
            }],
            unit,
        );
        callback.generics = vec![GenericParameter::Type {
            name: "U".into(),
            bounds: bounds(inner),
        }]
        .into();
        param(&mut f, TypeKind::Function(callback));
        owner_signature(&mut f, PassingMode::Borrow);
        let SignatureKind::Function(owner) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
        else {
            unreachable!()
        };
        owner.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: bounds(outer),
        }]
        .into();
        f
    };
    build(true, false).valid();
    build(false, true).broken(mismatch("nominal generic bound"));
}

/// `match p { (false, x) => .., _ => .. }` over a borrowed `(bool, bool)`,
/// binding `x` by copy in the first arm's target.
fn binding_match(bind: bool) -> Fixture {
    let mut f = Fixture::unit();
    let boolean = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::Bool)));
    let p = param(&mut f, TypeKind::Tuple(vec![boolean, boolean].into()));
    let root = f.body().places[p as usize].root;
    let field = |f: &mut Fixture, i| {
        f.body().places.push(Place {
            loan: None,
            binding: None,
            root,
            projection: vec![Projection::Field(i)].into(),
            ty: boolean,
        });
        f.body().places.len() as u32 - 1
    };
    let left = field(&mut f, 0);
    let right = field(&mut f, 1);
    let x = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Local);
    let pattern = |place, kind| MatchPattern {
        place: PlaceId(place),
        kind,
    };
    f.body().matches.push(MatchTable {
        scrutinee: PlaceId(p),
        patterns: vec![
            pattern(left, MatchPatternKind::Literal(Constant::Bool(false))),
            pattern(
                right,
                MatchPatternKind::Binding {
                    usage: UseKind::Copy,
                    guard_usage: UseKind::Copy,
                },
            ),
            pattern(
                p,
                MatchPatternKind::Tuple(vec![(0, MatchPatternId(0)), (1, MatchPatternId(1))]),
            ),
            pattern(p, MatchPatternKind::Wildcard),
        ],
        arms: vec![
            MatchArm {
                pattern: MatchPatternId(2),
                guard: None,
                target: BlockId(2),
            },
            MatchArm {
                pattern: MatchPatternId(3),
                guard: None,
                target: BlockId(1),
            },
        ],
    });
    f.body().blocks[0].terminator = Terminator::Match {
        table: MatchTableId(0),
        from: 0,
    };
    f.body().blocks.push(block(vec![], ret(0, 0)));
    let read = RvalueId(f.body().rvalues.len() as u32);
    f.body().rvalues.push(Rvalue::Use(copy(right)));
    let bound = if bind {
        vec![inst(
            InstructionKind::Assign {
                destination: PlaceId(x),
                value: read,
            },
            0,
        )]
    } else {
        // The rvalue stays referenced, by an unrelated temporary.
        let other = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
        f.body().rvalues[read.0 as usize] = Rvalue::Use(copy(left));
        vec![inst(
            InstructionKind::Assign {
                destination: PlaceId(other),
                value: read,
            },
            0,
        )]
    };
    f.body().blocks.push(block(bound, ret(0, 0)));
    owner_signature(&mut f, PassingMode::Borrow);
    f
}

#[test]
fn match_arm_targets_establish_their_bindings() {
    binding_match(true).valid();
    let right = binding_match(false).body_place(1);
    binding_match(false).broken(vec![ViolationKind::InvalidUse(right)]);
}

impl Fixture {
    /// The place of the `index`th pattern of the first match table.
    fn body_place(&mut self, index: usize) -> PlaceId {
        self.body().matches[0].patterns[index].place
    }
}

/// `StorageLive` begins a local's storage; on a local that is already live
/// (here a parameter, reached only through its fields) it would discard it.
#[test]
fn storage_live_starts_only_dead_storage() {
    let build = |restart_parameter: bool| {
        let mut f = Fixture::unit();
        let byte = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::U8)));
        let pair = f
            .definition
            .types
            .push(ty(TypeKind::Tuple(vec![byte, byte].into())));
        let local = |f: &mut Fixture, storage| {
            let id = LocalId(f.body().locals.len() as u32);
            f.body().locals.push(Local {
                ty: pair,
                storage,
                scope: ScopeId(0),
                span: Span::default(),
            });
            id
        };
        let parameter = local(&mut f, Storage::Parameter);
        let fresh = local(&mut f, Storage::Local);
        f.body().parameters.push(parameter);
        f.body().places.push(Place {
            loan: None,
            binding: None,
            root: parameter,
            projection: vec![Projection::Field(0)].into(),
            ty: byte,
        });
        let field = f.body().places.len() as u32 - 1;
        let read = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
        f.assign(read, Rvalue::Use(copy(field)), 0);
        f.emit(InstructionKind::StorageLive(if restart_parameter {
            parameter
        } else {
            fresh
        }));
        owner_signature(&mut f, PassingMode::Borrow);
        f
    };
    build(false).valid();
    build(true).broken(vec![ViolationKind::WrongKind {
        entity: "StorageLive on initialized local",
        index: 1,
    }]);
}

/// `fn main(x: u32) -> () { main(x) }`, published as a self-tail call.
fn self_tail() -> Fixture {
    let mut f = Fixture::unit();
    let x = param(&mut f, TypeKind::Primitive(Primitive::U32));
    let r = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    let body = f.body();
    body.blocks[0].instructions.clear();
    body.rvalues.clear();
    body.blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("main"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: take(x),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(r),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    body.blocks.push(block(vec![], ret(r, 0)));
    body.tail_sites.push(TailSite {
        owner_body: BodyId(0),
        call_block: BlockId(0),
        kind: TailKind::SelfTail,
        owned_loop_arguments: vec![],
    });
    owner_signature(&mut f, PassingMode::Take);
    f
}

#[test]
fn a_self_tail_witness_names_one_self_call_with_nothing_pending() {
    self_tail().valid();
    let witness = || {
        vec![ViolationKind::WrongKind {
            entity: "tail witness",
            index: 0,
        }]
    };
    let mut twice = self_tail();
    let site = twice.body().tail_sites[0].clone();
    twice.body().tail_sites.push(site);
    twice.broken(witness());
    let mut constructor = self_tail();
    constructor.body().tail_sites[0].kind = TailKind::ConstructorHole(vec![]);
    constructor.broken(witness());
}

/// `let g: fn(String) -> R = || ..` where `R` may claim the argument's captures.
fn promised_closure(promise: bool) -> Fixture {
    let mut f = Fixture::unit();
    let string = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::String)));
    let unit = unit_ty(&mut f);
    let mut result = ty(TypeKind::Function(fn_type(vec![], unit)));
    if promise {
        result.erased_captures = Some(ErasedCaptures::FromArguments(
            vec![ParameterRef::bound(ParamId(0)).into()].into(),
        ));
    }
    let result = f.definition.types.push(result);
    let mut clause = empty_body(&mut f);
    clause.locals[0].ty = result;
    clause.places[0].ty = result;
    clause.result = result;
    clause.rvalues[0] = Rvalue::FunctionReference {
        definition: path("callee"),
        arguments: vec![],
    };
    let signature = function_signature("callee", vec![], TypeKind::Tuple(vec![].into()));
    f.signatures.insert(id("callee"), signature);
    let parameter = add_capture(&mut clause, string);
    clause.locals[parameter as usize].storage = Storage::Parameter;
    clause.parameters.push(LocalId(parameter));
    f.definition.bodies.push(clause);
    let g = f.slot(
        TypeKind::Function(fn_type(
            vec![Parameter {
                name: None,
                label: None,
                passing: PassingMode::Borrow,
                ty: string,
            }],
            result,
        )),
        Storage::Local,
    );
    f.assign(
        g,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![],
            destination: Destination::Inferred,
        },
        0,
    );
    let cleanup = f.cleanup(vec![drop(g)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f
}

/// A callable result claiming its argument's captures has no flow proof yet,
/// so its creation fails closed.
#[test]
fn a_closure_with_a_callable_capture_promise_fails_closed() {
    promised_closure(false).valid();
    promised_closure(true).broken(vec![ViolationKind::UnprovedStatePromise]);
}

/// `fn main[T]()` building a capture-free closure whose type claims a
/// crossing mode.
fn claimed_sendability(claim: Sendability) -> Fixture {
    let mut f = Fixture::unit();
    let unit = unit_ty(&mut f);
    let nested = empty_body(&mut f);
    f.definition.bodies.push(nested);
    let mut closure = ty(TypeKind::Function(fn_type(vec![], unit)));
    closure.modes.sendability = claim;
    let closure = f.definition.types.push(closure);
    let c = f.slot(TypeKind::Tuple(vec![].into()), Storage::Local);
    f.body().locals[c as usize].ty = closure;
    f.body().places[c as usize].ty = closure;
    f.assign(
        c,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![],
            destination: Destination::Inferred,
        },
        0,
    );
    let exit = f.cleanup(vec![drop(c)]);
    f.body().blocks[0].terminator = ret(0, exit);
    owner_signature(&mut f, PassingMode::Take);
    let SignatureKind::Function(owner) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
    else {
        unreachable!()
    };
    owner.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    }]
    .into();
    f
}

/// A closure with no captures crosses unconditionally; a type claiming it
/// crosses only when `T` does is not the proved mode.
#[test]
fn a_closure_type_claims_the_crossing_mode_of_its_captures() {
    let t = TypeParameterRef::body(scoped(NameSite::Item), TypeParamId(0));
    claimed_sendability(Sendability::Sendable).valid();
    claimed_sendability(Sendability::DependsOn(vec![t].into()))
        .broken(mismatch("closure sendability"));
}

/// A symbolic mismatch on a dead branch still needs an independent proof;
/// moving live receipts out of typing must not silently omit that construction.
#[test]
fn a_dead_closure_construction_keeps_its_symbolic_obligation() {
    let build = |claim| {
        let mut f = claimed_sendability(claim);
        let original = f.body().blocks[0].clone();
        let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
        f.body().blocks[0] = block(
            vec![],
            Terminator::Branch {
                condition: copy(condition),
                yes: BlockId(1),
                no: BlockId(2),
            },
        );
        f.assign(condition, Rvalue::Constant(Constant::Bool(true)), 0);
        f.body()
            .blocks
            .push(block(vec![original.instructions[0].clone()], ret(0, 0)));
        f.body().blocks.push(original);
        f
    };
    let t = TypeParameterRef::body(scoped(NameSite::Item), TypeParamId(0));
    build(Sendability::Sendable).valid();
    build(Sendability::DependsOn(vec![t].into())).broken(mismatch("closure sendability"));
}

/// `match n { start..=end => .., .. }` over a borrowed scalar of `primitive`,
/// one returning arm per range, in order.
fn range_match(primitive: Primitive, ranges: &[(Constant, Constant)]) -> Fixture {
    let mut f = Fixture::unit();
    let p = param(&mut f, TypeKind::Primitive(primitive));
    let mut patterns = vec![];
    let mut arms = vec![];
    for (index, (start, end)) in ranges.iter().enumerate() {
        patterns.push(MatchPattern {
            place: PlaceId(p),
            kind: MatchPatternKind::Range {
                start: start.clone(),
                end: end.clone(),
            },
        });
        arms.push(MatchArm {
            pattern: MatchPatternId(index as u32),
            guard: None,
            target: BlockId(1 + index as u32),
        });
        f.body().blocks.push(block(vec![], ret(0, 0)));
    }
    f.body().matches.push(MatchTable {
        scrutinee: PlaceId(p),
        patterns,
        arms,
    });
    f.body().blocks[0].terminator = Terminator::Match {
        table: MatchTableId(0),
        from: 0,
    };
    owner_signature(&mut f, PassingMode::Borrow);
    f
}

#[test]
fn range_patterns_are_ordered_typed_and_cover_by_interval() {
    let n = |n: u128| Constant::Integer(n);
    // u8 split in two halves, and every Unicode scalar around the surrogates.
    range_match(Primitive::U8, &[(n(0), n(127)), (n(128), n(255))]).valid();
    range_match(
        Primitive::Char,
        &[(n(0), n(0xD7FF)), (n(0xE000), n(0x10FFFF))],
    )
    .valid();
    // i8 `-128..=-1` and `0..=127` are the bits 0x80..=0xFF and 0..=0x7F.
    range_match(Primitive::I8, &[(n(0x80), n(0xFF)), (n(0), n(0x7F))]).valid();
    // Bounds order by the place's signedness: i8 `1..=-1` is empty.
    range_match(Primitive::I8, &[(n(1), n(0xFF)), (n(0x80), n(0x7F))])
        .broken(mismatch("match pattern"));
    // An empty unsigned range, a bound outside the type and a non-scalar bound.
    range_match(Primitive::U8, &[(n(9), n(0)), (n(0), n(255))]).broken(mismatch("match pattern"));
    range_match(Primitive::U8, &[(n(0), n(256))]).broken(mismatch("match pattern"));
    range_match(
        Primitive::Bool,
        &[(Constant::Bool(false), Constant::Bool(true))],
    )
    .broken(mismatch("match pattern"));
    // A gap at 127 leaves the match uncovered.
    range_match(Primitive::U8, &[(n(0), n(126)), (n(128), n(255))])
        .broken(vec![ViolationKind::MatchCoverage]);
}

/// A generic result instantiated as Result derives its error channel. The
/// forged reference omits that channel while keeping the same result type.
#[test]
fn a_function_reference_derives_its_instantiated_error_channel() {
    let build = |channel: bool| {
        let mut f = Fixture::unit();
        let mut result_declaration = capability_signature("Result");
        let SignatureKind::Type(result) = &mut result_declaration.kind else {
            unreachable!()
        };
        result.capability = None;
        result.authority = AuthoritySummary::inert();
        result.generics = ["T", "E"]
            .into_iter()
            .map(|name| GenericParameter::Type {
                name: name.into(),
                bounds: vec![].into(),
            })
            .collect();
        f.signatures.insert(id("Result"), result_declaration);
        let byte = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::U8)));
        let result = f.definition.types.push(ty(TypeKind::Named {
            definition: id("Result"),
            arguments: vec![GenericArgument::Type(byte), GenericArgument::Type(byte)].into(),
        }));
        let mut signature = function_signature(
            "generic",
            vec![],
            TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
        );
        std::sync::Arc::make_mut(&mut signature.metadata.roles).result_type = Some(path("Result"));
        let SignatureKind::Function(function) = &mut signature.kind else {
            unreachable!()
        };
        function.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![].into(),
        }]
        .into();
        f.signatures.insert(id("generic"), signature);
        let mut function = fn_type(vec![], result);
        function.error = channel.then_some(byte);
        let g = f.slot(TypeKind::Function(function), Storage::Local);
        f.assign(
            g,
            Rvalue::FunctionReference {
                definition: path("generic"),
                arguments: vec![GenericArgument::Type(result)],
            },
            0,
        );
        let exit = f.cleanup(vec![drop(g)]);
        f.body().blocks[0].terminator = ret(0, exit);
        f
    };
    build(false).broken(mismatch("function reference"));
    build(true).valid();
}

#[test]
fn caller_body_sets_follow_entry_contract_producers_and_exclude_nested_contracts() {
    let mut f = contract_clause(Primitive::Bool);
    let mut signature = function_signature("main", vec![], TypeKind::Tuple(vec![].into()));
    signature.metadata.track_caller = true;
    f.signatures.insert(signature.definition.clone(), signature);
    f.definition.caller_bodies = vec![BodyId(0), BodyId(1)];
    f.valid();
    f.definition.caller_bodies = vec![BodyId(0)];
    f.broken(mismatch("caller attribution"));
    // Move that contract's producer into an ordinary nested function.
    let entry = empty_body(&mut f);
    let nested = std::mem::replace(&mut f.definition.bodies[0], entry);
    f.definition.bodies.push(nested);
    let unit = f.body().result;
    let function = f.slot(TypeKind::Function(fn_type(vec![], unit)), Storage::Local);
    f.assign(
        function,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(2),
            captures: vec![],
            destination: Destination::Inferred,
        },
        0,
    );
    let cleanup = f.cleanup(vec![drop(function)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f.valid();
    f.definition.caller_bodies.push(BodyId(1));
    f.broken(mismatch("caller attribution"));
}

#[test]
fn only_contract_function_values_forward_the_caller() {
    fn build(contract: bool, forward_caller: bool) -> Fixture {
        let mut f = Fixture::unit();
        let boolean = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::Bool)));
        let mut clause = empty_body(&mut f);
        clause.locals[0].ty = boolean;
        clause.places[0].ty = boolean;
        clause.result = boolean;
        clause.rvalues[0] = Rvalue::Constant(Constant::Bool(true));
        f.definition.bodies.push(clause);
        if contract {
            f.definition.contracts.push((0, BodyId(1)));
        }
        let function = f.slot(TypeKind::Function(fn_type(vec![], boolean)), Storage::Local);
        f.assign(
            function,
            Rvalue::Closure {
                context: hirsel_types::CaptureContext::OwningScoped,
                body: BodyId(1),
                captures: vec![],
                destination: Destination::Inferred,
            },
            0,
        );
        f.emit(InstructionKind::BeginLoan {
            id: LoanId(0),
            place: PlaceId(function),
            mutable: false,
            if_inactive: false,
        });
        let via = f.through_loan(function, 0);
        let cleanup = f.cleanup(vec![
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            drop(function),
        ]);
        let result = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
        f.body().blocks[0].terminator = Terminator::Call {
            callee: Callee::Indirect {
                forward_caller,
                value: operand(via, UseKind::Borrow),
                arguments: vec![],
            },
            arguments: vec![],
            returns: Some(CallReturn {
                destination: PlaceId(result),
                next: BlockId(1),
            }),
            on_failure: CleanupId(cleanup),
        };
        f.body().blocks.push(block(vec![], ret(0, cleanup)));
        f
    }
    build(false, false).valid();
    build(false, true).broken(mismatch("caller forwarding"));
    build(true, true).valid();
    build(true, false).broken(mismatch("caller forwarding"));
}

/// RUN-3: `let e: Entry[fn() -> <result>] = callee` for `fn callee() -> ()`.
/// An entry names a non-generic, non-extern, complete definition of the
/// entry's callable shape.
fn entry_of(result: Primitive, edit: fn(&mut FunctionType)) -> Fixture {
    let mut f = Fixture::unit();
    let result = f.definition.types.push(ty(TypeKind::Primitive(result)));
    let signature = f
        .definition
        .types
        .push(ty(TypeKind::Function(fn_type(vec![], result))));
    let e = f.slot(
        TypeKind::Named {
            definition: DefId::intern(&DefPath::core("Entry")),
            arguments: vec![GenericArgument::Type(signature)].into(),
        },
        Storage::Local,
    );
    let mut callee = function_signature("callee", vec![], TypeKind::Primitive(Primitive::Bool));
    if let SignatureKind::Function(function) = &mut callee.kind {
        edit(function);
    }
    f.signatures.insert(id("callee"), callee);
    f.assign(
        e,
        Rvalue::FunctionReference {
            definition: path("callee"),
            arguments: vec![],
        },
        0,
    );
    let exit = f.cleanup(vec![drop(e)]);
    f.body().blocks[0].terminator = ret(0, exit);
    f
}

#[test]
fn an_entry_names_a_definition_of_exactly_its_type() {
    entry_of(Primitive::Bool, |_| ()).valid();
    entry_of(Primitive::U8, |_| ()).broken(mismatch("function reference"));
    entry_of(Primitive::Bool, |f| f.incomplete = true).broken(mismatch("function reference"));
    entry_of(Primitive::Bool, |f| {
        f.foreign = Some(ForeignContract {
            abi: "c".into(),
            blocking: false,
            aliasing_contract: String::new(),
        });
    })
    .broken(mismatch("function reference"));
}
