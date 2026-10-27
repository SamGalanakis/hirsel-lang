//! Exercise the real unregistered read body before public native activation.
use crate::rc_support::{compile, manifest, target, try_compile};
use hirsel_layout::{layout::LayoutBuilder, operation::OperationRegistry};
use hirsel_lir::operation::{NativeType, OperationArgument, OperationDeclarations};
use hirsel_lir::sealed::{SealDescriptor, SourceEvent, SourceEventKind};
use hirsel_rc::{
    instance::Instances,
    library::{Cx, Request},
};
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_types::{GenericArgument, Type, TypeKind};

struct ReadDeclarations {
    registry: OperationRegistry,
    definition: DefPath,
    instance: ContentHash,
    expected: Vec<SourceEvent>,
    receipt: Option<hirsel_lir::sealed_payload_source::ReceiptSource>,
}
impl OperationDeclarations for ReadDeclarations {
    fn receipt_source(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<hirsel_lir::sealed_payload_source::ReceiptSource> {
        self.receipt
            .as_ref()
            .filter(|s| &s.original.definition == definition && s.instance == instance)
            .cloned()
    }
    fn derive(
        &self,
        operation: &hirsel_core_contract::OperationId,
        arguments: &[OperationArgument],
        target: &hirsel_lir::TargetSpec,
    ) -> Result<(hirsel_lir::FnAbi, hirsel_lir::LayoutTable), hirsel_lir::layout::LayoutError> {
        self.registry.derive(operation, arguments, target)
    }
    fn seal_descriptor(
        &self,
        root: &NativeType,
        table: &hirsel_lir::LayoutTable,
        target: &hirsel_lir::TargetSpec,
    ) -> Result<SealDescriptor, hirsel_lir::layout::LayoutError> {
        self.registry.seal_descriptor(root, table, target)
    }
    fn seal_descriptor_for_source(
        &self,
        definition: &DefPath,
        instance: ContentHash,
        root: &NativeType,
        table: &hirsel_lir::LayoutTable,
        target: &hirsel_lir::TargetSpec,
    ) -> Result<SealDescriptor, hirsel_lir::layout::LayoutError> {
        if definition != &self.definition
            || instance != self.instance
            || self.expected.iter().any(|event| &event.root_type != root)
        {
            return Err(hirsel_lir::layout::LayoutError::Invalid(
                "unselected library read declaration".into(),
            ));
        }
        self.registry.seal_descriptor(root, table, target)
    }
    fn seal_events(&self, definition: &DefPath, instance: ContentHash) -> Option<Vec<SourceEvent>> {
        (definition == &self.definition && instance == self.instance).then(|| self.expected.clone())
    }
}

fn check_read_body(
    source: &str,
    root_kind: impl FnOnce(&hirsel_types::TypeInterner) -> TypeKind,
    root_type: NativeType,
) {
    let program = compile(source);
    let types = &program.types;
    let root = types.intern(Type::with_kind(root_kind(types)));
    let result = types.intern(Type::with_kind(TypeKind::Primitive(
        hirsel_types::Primitive::U64,
    )));
    let path = DefPath::core("region.with");
    assert!(
        hirsel_rc::library::body_index(&path).is_some(),
        "sealed library body is registered"
    );
    let target = target();
    let registry = program.operation_registry();
    let mut layouts = LayoutBuilder::new(&target, &registry.owners);
    let mut instances = Instances::new(program.types.clone(), program.memo.clone());
    // Build the unregistered entry explicitly; its body ordinal is never dispatched.
    let request = Request::operation(
        &program,
        usize::MAX,
        &path,
        vec![GenericArgument::Type(root), GenericArgument::Type(result)],
    )
    .unwrap();
    let id = hirsel_rc::library::instantiate(&mut layouts, &mut instances, request).unwrap();
    let mut unit = (hirsel_rc::library::sealed_region::WITH.build)(&mut Cx::new(
        &program,
        &mut layouts,
        &mut instances,
        id,
    ))
    .unwrap();
    let table = layouts.finish();
    hirsel_rc::import::prune_pretransfer(std::slice::from_mut(&mut unit), &table, &[]);
    let declarations = ReadDeclarations {
        receipt: None,
        registry,
        definition: path,
        instance: instances.view(id).key.digest(),
        expected: vec![SourceEvent {
            anchor: hirsel_lir::ir::SourceAnchor {
                body: 0,
                block: 0,
                statement: 0,
                site: hirsel_lir::ir::SiteRange {
                    source: 0,
                    start: 0,
                    end: 0,
                },
            },
            kind: SourceEventKind::BorrowRoot,
            root_type,
        }],
    };
    let manifest = manifest(&program);
    let inputs = hirsel_lir::validate::Inputs {
        target: &target,
        layouts: &table,
        operations: &declarations,
        manifest: &manifest,
        discharge: None,
        memo: Default::default(),
    };
    let strict = hirsel_lir::validate::check_scoped_transport(&unit, &inputs);
    assert!(strict.is_empty(), "strict scoped: {strict:?}");
    hirsel_lir::validate::ScopedQualification::new(&target, &manifest, &declarations, None)
        .qualify(std::slice::from_ref(&unit), &table)
        .expect("linked callback contract");
    hirsel_lir::validate::validate(unit.clone(), &inputs)
        .unwrap_or_else(|e| panic!("Value C: {:?}", e.violations));
    let placed = hirsel_rc::placement::place(unit, &table, &Default::default()).unwrap();
    let counted = hirsel_rc::insert::insert(placed, &table, &Default::default()).unwrap();
    let ready = hirsel_rc::ready::ready(counted, &table, &Default::default()).unwrap();
    hirsel_lir::validate::validate_ready(ready, &inputs)
        .unwrap_or_else(|e| panic!("Ready C: {:?}", e.violations));
}

#[test]
fn unregistered_region_with_unit_body_validates_its_owner_and_callback_loans() {
    check_read_body(
        include_str!("../../../../conformance/MEM/MEM-4-sealed-empty-unit-run.hir"),
        |_| TypeKind::Tuple(vec![].into()),
        NativeType::Tuple(vec![]),
    );
}

#[test]
fn unregistered_region_with_scalar_body_validates_its_owner_and_callback_loans() {
    check_read_body(
        include_str!("../../../../conformance/MEM/MEM-4-sealed-scalar-zero-run.hir"),
        |_| TypeKind::Primitive(hirsel_types::Primitive::U64),
        NativeType::Primitive(hirsel_lir::operation::Primitive::U64),
    );
}

#[test]
fn unqualified_callback_parameter_is_not_a_storage_proof() {
    let rejected = try_compile(
        r#"
        pub fn read(mut r: Region[()], body: fn(root: ()) -> u64) -> u64 {
            region.with(mut r, body)
        }
    "#,
    )
    .expect_err("an unknown callback cannot establish complete storage provenance");
    assert!(
        rejected.contains("MODE-1")
            && rejected.contains("sealed-root callback has no complete storage provenance"),
        "{rejected}"
    );
}

fn check_seal_body(source: &str, root_kind: TypeKind, root_type: NativeType) {
    let program = compile(source);
    let root = program.types.intern(Type::with_kind(root_kind));
    let path = DefPath::core("arena.seal");
    assert!(hirsel_rc::library::body_index(&path).is_some());
    let target = target();
    let registry = program.operation_registry();
    let mut layouts = LayoutBuilder::new(&target, &registry.owners);
    let mut instances = Instances::new(program.types.clone(), program.memo.clone());
    let request = Request::operation(
        &program,
        usize::MAX,
        &path,
        vec![GenericArgument::Type(root)],
    )
    .unwrap();
    let id = hirsel_rc::library::instantiate(&mut layouts, &mut instances, request).unwrap();
    let unit = (hirsel_rc::library::sealed_region::SEAL.build)(&mut Cx::new(
        &program,
        &mut layouts,
        &mut instances,
        id,
    ))
    .unwrap();
    let mut table = layouts.finish();
    let declarations = ReadDeclarations {
        receipt: None,
        registry,
        definition: path,
        instance: instances.view(id).key.digest(),
        expected: vec![SourceEvent {
            anchor: hirsel_lir::ir::SourceAnchor {
                body: 0,
                block: 0,
                statement: 0,
                site: hirsel_lir::ir::SiteRange {
                    source: 0,
                    start: 0,
                    end: 0,
                },
            },
            kind: SourceEventKind::Seal,
            root_type,
        }],
    };
    let manifest = manifest(&program);
    {
        let inputs = hirsel_lir::validate::Inputs {
            target: &target,
            layouts: &table,
            operations: &declarations,
            manifest: &manifest,
            discharge: None,
            memo: Default::default(),
        };
        let strict = hirsel_lir::validate::check_scoped_transport(&unit, &inputs);
        assert!(strict.is_empty(), "{strict:?}");
        hirsel_lir::validate::ScopedQualification::new(&target, &manifest, &declarations, None)
            .qualify(std::slice::from_ref(&unit), &table)
            .unwrap();
        assert!(hirsel_lir::validate::validate(unit.clone(), &inputs).is_ok());
    }
    // Re-annotating the same body as a borrowed entry must not grant consumption.
    table.fn_abis[unit.header.abi.0 as usize].parameters[0].convention =
        hirsel_core_contract::Passing::Borrow;
    let inputs = hirsel_lir::validate::Inputs {
        target: &target,
        layouts: &table,
        operations: &declarations,
        manifest: &manifest,
        discharge: None,
        memo: Default::default(),
    };
    let rejected = hirsel_lir::validate::validate(unit, &inputs)
        .err()
        .expect("borrowed grant entry is not consumable");
    assert!(rejected.violations.iter().any(|v| matches!(&v.kind,
        hirsel_lir::validate::ViolationKind::Sealed(reason) if reason.contains("borrow-only arena"))),
        "{:?}", rejected.violations);
}

#[test]
fn unregistered_arena_seal_unit_body_preserves_consumable_grant_entry() {
    check_seal_body(
        include_str!("../../../../conformance/MEM/MEM-4-sealed-empty-unit-run.hir"),
        TypeKind::Tuple(vec![].into()),
        NativeType::Tuple(vec![]),
    );
}

#[test]
fn unregistered_arena_seal_scalar_body_preserves_consumable_grant_entry() {
    check_seal_body(
        include_str!("../../../../conformance/MEM/MEM-4-sealed-scalar-zero-run.hir"),
        TypeKind::Primitive(hirsel_types::Primitive::U64),
        NativeType::Primitive(hirsel_lir::operation::Primitive::U64),
    );
}

#[test]
fn unregistered_region_with_list_body_reaches_ready_under_its_owner_loan() {
    check_read_body(
        "pub fn read(mut r: Region[List[u64]]) -> u64 {region.with(mut r, fn(root: List[u64]) -> u64 {list.len(root)})}",
        |types| {
            let element = types.intern(Type::with_kind(TypeKind::Primitive(
                hirsel_types::Primitive::U64,
            )));
            TypeKind::Builtin {
                kind: hirsel_types::BuiltinType::List,
                arguments: vec![GenericArgument::Type(element)].into(),
            }
        },
        NativeType::Builtin(
            hirsel_lir::operation::BuiltinType::List,
            vec![OperationArgument::Type(NativeType::Primitive(
                hirsel_lir::operation::Primitive::U64,
            ))],
        ),
    );
}

#[test]
fn sealed_data_callback_sum_is_independently_proved_by_v_and_c() {
    let program = compile(
        "pub fn read(mut r: Region[List[u64]]) -> u64 {region.with(mut r, fn(root: List[u64]) -> u64 {root[0] + root[1]})}",
    );
    let mut signatures = hirsel_validate::SignatureMap::new(program.types.clone());
    for (path, signature) in &program.signatures {
        signatures.insert(hirsel_syntax::DefId::intern(path), (**signature).clone());
    }
    let definition = &program.definitions[&crate::rc_support::path("read")];
    let proof = hirsel_validate::qualify_scoped_transport(
        definition,
        &program.types,
        &signatures,
        &mut vec![],
    );
    assert!(proof.violations.is_empty(), "V: {:?}", proof.violations);
    let target = target();
    let mut layouts = LayoutBuilder::new(&target, &program.owners);
    layouts.prepare_native_controls();
    let mut instances = hirsel_rc::instance::expand(&program, &mut layouts).unwrap();
    let id = instances
        .ids()
        .find(|id| {
            matches!(
                instances.roles[id.index()],
                hirsel_rc::instance::InstanceRole::Closure { .. }
            )
        })
        .expect("source callback");
    let imported =
        hirsel_rc::import::import_instance(&program, &mut instances, id, None, &mut layouts)
            .unwrap();
    assert!(imported.required.is_empty(), "reader only observes bits");
    let table = layouts.finish();
    let abi = &table.fn_abis[imported.unit.header.abi.0 as usize];
    assert_eq!(
        abi.nonretaining,
        [hirsel_lir::abi_types::ParameterPath {
            parameter: 0,
            fields: vec![]
        }]
    );
    let manifest = manifest(&program);
    let mut declarations = program.operation_registry();
    let selected = hirsel_lir::ir::Symbol {
        kind: hirsel_lir::ir::SymbolKind::Function,
        definition: imported.unit.header.definition.clone(),
        instance: Some(imported.unit.header.instance),
    };
    let evidence = hirsel_rc::invocations::bind_source(
        definition,
        declarations.invocations[definition.path.path()].clone(),
        &selected,
        instances.view(id).body,
    )
    .unwrap();
    declarations
        .invocations
        .insert(evidence.body.definition.clone(), evidence.invocations);
    declarations.invocation_bodies.push(evidence.body);
    let inputs = hirsel_lir::validate::Inputs {
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: &declarations,
        discharge: None,
        memo: Default::default(),
    };
    hirsel_lir::validate::qualify_scoped_transport(std::slice::from_ref(&imported.unit), &inputs)
        .expect("C proves callback body");
    let placed = hirsel_rc::placement::place(imported.unit, &table, &Default::default()).unwrap();
    let counted = hirsel_rc::insert::insert(placed, &table, &Default::default()).unwrap();
    let ready = hirsel_rc::ready::ready(counted, &table, &Default::default()).unwrap();
    hirsel_lir::validate::qualify_scoped_transport(&[ready], &inputs)
        .expect("Ready C proves callback body");
}

#[test]
fn sealed_heap_body_95_keeps_take_receipts_until_the_seal_transaction_finishes() {
    let program = compile(
        "pub fn read(mut r: Region[List[u64]]) -> u64 {region.with(mut r, fn(root: List[u64])->u64 {list.len(root)})}",
    );
    let scalar = program.types.intern(Type::with_kind(TypeKind::Primitive(
        hirsel_types::Primitive::U64,
    )));
    let root = program.types.intern(Type::with_kind(TypeKind::Builtin {
        kind: hirsel_types::BuiltinType::List,
        arguments: vec![GenericArgument::Type(scalar)].into(),
    }));
    let path = DefPath::core("arena.seal");
    assert!(hirsel_rc::library::body_index(&path).is_some());
    let target = target();
    let registry = program.operation_registry();
    let mut layouts = LayoutBuilder::new(&target, &registry.owners);
    let mut instances = Instances::new(program.types.clone(), program.memo.clone());
    let request = Request::operation(
        &program,
        usize::MAX,
        &path,
        vec![GenericArgument::Type(root)],
    )
    .unwrap();
    let id = hirsel_rc::library::instantiate(&mut layouts, &mut instances, request).unwrap();
    let source =
        hirsel_rc::library::sealed_region::receipt_source(&program, root, &instances.view(id).key)
            .unwrap();
    let profile = source.profiles[0].profile.clone();
    assert_eq!(
        layouts.table().fn_abis[instances.abis[id.index()].0 as usize]
            .receipts
            .as_ref(),
        Some(&source.contract)
    );
    let instance = instances.view(id).key.digest();
    let declarations = ReadDeclarations {
        registry: registry.clone(),
        definition: path,
        instance,
        receipt: Some(source),
        expected: vec![SourceEvent {
            anchor: hirsel_lir::ir::SourceAnchor {
                body: 0,
                block: 0,
                statement: 0,
                site: hirsel_lir::ir::SiteRange {
                    source: 0,
                    start: 0,
                    end: 0,
                },
            },
            kind: SourceEventKind::Seal,
            root_type: profile.root.ty,
        }],
    };
    let unit = (hirsel_rc::library::sealed_region::SEAL.build)(&mut Cx::new(
        &program,
        &mut layouts,
        &mut instances,
        id,
    ))
    .unwrap();
    let table = layouts.finish();
    let manifest = manifest(&program);
    let inputs = hirsel_lir::validate::Inputs {
        target: &target,
        layouts: &table,
        operations: &declarations,
        manifest: &manifest,
        discharge: None,
        memo: Default::default(),
    };
    hirsel_lir::validate::validate(unit.clone(), &inputs)
        .unwrap_or_else(|e| panic!("Value C: {:?}", e.violations));
    let placed = hirsel_rc::placement::place(unit, &table, &Default::default()).unwrap();
    let counted = hirsel_rc::insert::insert(placed, &table, &Default::default()).unwrap();
    hirsel_lir::validate::validate(counted.clone(), &inputs)
        .unwrap_or_else(|e| panic!("Rc C: {:?}", e.violations));
    let ready = hirsel_rc::ready::ready(counted, &table, &Default::default()).unwrap();
    hirsel_lir::validate::validate_ready(ready, &inputs)
        .unwrap_or_else(|e| panic!("Ready C: {:?}", e.violations));
}
