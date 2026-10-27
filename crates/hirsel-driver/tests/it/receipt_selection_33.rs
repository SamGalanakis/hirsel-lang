//! Original direct-call discovery through current source/V and CT-5 providers.
use hirsel_driver::{
    compiler_adapters,
    native::receipt_source::{provider::*, selection::*},
};
use hirsel_engine::*;
use hirsel_rc::instance::{Actuals, key_of};
use hirsel_rc::sealed_source::{SourceError, closed_storage_type};
use hirsel_syntax::{DefId, DefPath};
use hirsel_tir::InstanceBodies;
use hirsel_types::{GenericArgument, SignatureRef, TypeInterner, TypeMemo};
use std::sync::Arc;

struct Simulated<'a> {
    front: &'a Front<'a>,
    catalogue: InstanceBodies,
    stale: Option<DefPath>,
}
impl Provider for Simulated<'_> {
    fn types(&self) -> &TypeInterner {
        self.front.types()
    }
    fn current(&self, path: &DefPath) -> Result<Current, SourceError> {
        let mut current = self.front.current(path)?;
        if self.stale.as_ref() == Some(path) {
            current.revision = hirsel_syntax::ContentHash::of_bytes(b"stale target revision");
        }
        Ok(current)
    }
    fn signature(&self, path: &DefPath) -> Result<SignatureRef, SourceError> {
        self.front.signature(path)
    }
    fn original(&self, path: &DefPath) -> Result<Arc<NativeOriginalBody>, SourceError> {
        self.front.original(path)
    }
}
impl SelectionProvider for Simulated<'_> {
    fn selected_instance(
        &self,
        declaration: DefId,
        arguments: &[GenericArgument],
    ) -> Result<Option<DefId>, SourceError> {
        self.catalogue
            .resolve(declaration, self.types(), arguments)
            .map_err(|error| SourceError::Type(format!("{error:?}")))
    }
}

fn source(text: &str) -> (QueryEngine, NativeOriginalReader, DefPath) {
    let store: Arc<dyn store::ResultStore> = Arc::new(store::MemStore::default());
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
        .with_result_store(store.clone());
    let file = engine
        .sources
        .add("receipt-selection.hir", text.into())
        .unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let path = entry_path(&default_context());
    let facts = crate::native_front_fixture::products(&mut engine, &[path.clone()]).unwrap();
    let reader = facts
        .original_context
        .reader(
            facts.originals.iter().map(|product| (**product).clone()),
            store,
        )
        .unwrap();
    (engine, reader, path)
}
fn base(front: &Front<'_>, path: &DefPath) -> hirsel_layout::instance::InstanceKey {
    let original = front.original(path).unwrap();
    key_of(
        front.types(),
        path,
        original.definition().entry,
        &Actuals::none(),
        None,
        None,
    )
    .unwrap()
}
fn call_block(
    front: &Front<'_>,
    key: &hirsel_layout::instance::InstanceKey,
    actuals: &Actuals,
    name: &str,
) -> u32 {
    let observed = named(front, key, actuals, &TypeMemo::new()).unwrap();
    *observed.demand().projection().calls().iter().find(|(_, call)| {
        matches!(&call.callee, hirsel_tir::Callee::Definition {path, ..} if path.0.name == name)
    }).expect("the original named call exists").0
}

#[test]
fn original_direct_calls_close_caller_binders_and_keep_exact_source_observations() {
    with_compiler_stack(|| {
        let (_engine, reader, path) = source(
            r#"
fn id[T](take x:T)->T { take x }
fn wrap[T](take x:T)->T { id(take x) }
pub fn main()->u64 { wrap(7u64) }
"#,
        );
        let front = Front::new(&reader);
        let root = base(&front, &path);
        let mut simulated = Simulated {
            front: &front,
            catalogue: reader.instance_bodies().clone(),
            stale: None,
        };
        let block = call_block(&front, &root, &Actuals::none(), "wrap");
        let actual = direct(&front, &root, &Actuals::none(), block, &TypeMemo::new()).unwrap();
        let same = direct(&simulated, &root, &Actuals::none(), block, &TypeMemo::new()).unwrap();
        assert_eq!(actual.base, same.base);
        assert_eq!(actual.source.entry(), same.source.entry());
        assert_eq!(
            actual.source.demand().projection(),
            same.source.demand().projection()
        );
        assert!(actual.selection.valid());
        assert!(!actual.selection.generated);
        let [GenericArgument::Type(ty)] = actual.actuals.arguments.as_slice() else {
            panic!("one selected original type")
        };
        let original = front.original(actual.base.definition()).unwrap();
        let entry = &original.definition().bodies[original.definition().entry.0 as usize];
        let local = entry.parameters[0];
        let port = hirsel_rc::instance::substitute(
            front.types(),
            &TypeMemo::new(),
            entry.locals[local.0 as usize].ty,
            &actual.actuals,
            true,
        )
        .unwrap();
        assert_eq!(
            actual.source.entry().parameters()[0].ty,
            closed_storage_type(front.types(), port).unwrap()
        );
        // Generic actuals use the language's normalization; original ports
        // additionally retain their own receiving mode defaults.
        assert_eq!(
            actual.source.entry().parameters()[0].ty.ty,
            closed_storage_type(front.types(), *ty).unwrap().ty
        );
        let block = call_block(&front, &actual.base, &actual.actuals, "id");
        let nested = direct(
            &front,
            &actual.base,
            &actual.actuals,
            block,
            &TypeMemo::new(),
        )
        .unwrap();
        assert_eq!(
            nested.source.entry().result(),
            actual.source.entry().result()
        );
        assert_eq!(nested.base.definition().0.name, "id");
        assert!(nested.actuals.outer.is_empty());
        let [GenericArgument::Type(nested_type)] = nested.actuals.arguments.as_slice() else {
            panic!("one closed inner type actual")
        };
        assert_eq!(
            hirsel_types::CanonicalSpelling::closed(front.types(), *nested_type),
            hirsel_types::CanonicalSpelling::closed(front.types(), *ty)
        );
        let nested_original = front.original(nested.base.definition()).unwrap();
        let nested_body =
            &nested_original.definition().bodies[nested_original.definition().entry.0 as usize];
        assert_eq!(nested.actuals.binder, Some(nested_body.parameter_scope));
        assert_ne!(nested.actuals.binder, actual.actuals.binder);
        assert!(matches!(
            direct(
                &front,
                &actual.base,
                &Actuals::none(),
                block,
                &TypeMemo::new()
            ),
            Err(SourceError::Malformed(
                "original named actual arity differs"
            ))
        ));
        simulated.stale = Some(actual.base.definition().clone());
        assert!(matches!(
            direct(
                &simulated,
                &root,
                &Actuals::none(),
                call_block(&front, &root, &Actuals::none(), "wrap"),
                &TypeMemo::new()
            ),
            Err(SourceError::Malformed(
                "receipt source differs from current front selection"
            ))
        ));
        assert!(matches!(
            direct(&front, &root, &Actuals::none(), u32::MAX, &TypeMemo::new()),
            Err(SourceError::Malformed(
                "receipt direct-call source selector"
            ))
        ));
    });
}

#[test]
fn original_generated_calls_select_the_current_ct5_body_and_refuse_missing_selection() {
    with_compiler_stack(|| {
        let (_engine, reader, path) = source(
            r#"
type Point { x:u64 }
fn make[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String] {
  Ok(codegen.body("fn(v: standalone@0/Point) -> u64 uses {} { v.x }"))
}
fn generated[T](v:T)->u64 uses {} = comptime make(shape(T))
pub fn main()->u64 { generated(Point(x:7u64)) }
"#,
        );
        let front = Front::new(&reader);
        let root = base(&front, &path);
        let block = call_block(&front, &root, &Actuals::none(), "generated");
        let selected = direct(&front, &root, &Actuals::none(), block, &TypeMemo::new()).unwrap();
        assert!(selected.selection.valid());
        assert!(selected.selection.generated);
        assert_eq!(
            selected.selection.selected.as_ref(),
            Some(selected.base.definition())
        );
        assert_ne!(selected.base.definition(), &selected.selection.declaration);
        assert!(selected.actuals.arguments.is_empty());
        assert_eq!(
            selected.source.entry().reference().definition,
            *selected.base.definition()
        );
        let mut simulated = Simulated {
            front: &front,
            catalogue: reader.instance_bodies().clone(),
            stale: None,
        };
        let same = direct(&simulated, &root, &Actuals::none(), block, &TypeMemo::new()).unwrap();
        assert_eq!(selected.base, same.base);
        assert_eq!(selected.selection, same.selection);
        assert_eq!(selected.source.entry(), same.source.entry());
        let declaration = DefId::intern(&selected.selection.declaration);
        simulated.catalogue.instances.remove(&declaration);
        assert!(matches!(
            direct(&simulated, &root, &Actuals::none(), block, &TypeMemo::new()),
            Err(SourceError::Type(_))
        ));
    });
}
