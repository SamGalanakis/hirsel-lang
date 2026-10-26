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
}
impl OperationDeclarations for ReadDeclarations {
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

fn check_read_body(source: &str, root_kind: TypeKind, root_type: NativeType) {
    let program = compile(source);
    let types = &program.types;
    let root = types.intern(Type::with_kind(root_kind));
    let result = types.intern(Type::with_kind(TypeKind::Primitive(
        hirsel_types::Primitive::U64,
    )));
    let path = DefPath::core("region.with");
    assert!(
        hirsel_rc::library::body_index(&path).is_none(),
        "public activation is separate"
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
    let checked = hirsel_lir::validate::validate(unit, &inputs);
    assert!(checked.is_ok(), "{:?}", checked.err().map(|e| e.violations));
}

#[test]
fn unregistered_region_with_unit_body_validates_its_owner_and_callback_loans() {
    check_read_body(
        include_str!("../../../../conformance/MEM/MEM-4-sealed-empty-unit-run.hir"),
        TypeKind::Tuple(vec![].into()),
        NativeType::Tuple(vec![]),
    );
}

#[test]
fn unregistered_region_with_scalar_body_validates_its_owner_and_callback_loans() {
    check_read_body(
        include_str!("../../../../conformance/MEM/MEM-4-sealed-scalar-zero-run.hir"),
        TypeKind::Primitive(hirsel_types::Primitive::U64),
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
