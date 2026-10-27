//! Actual V-owned original closure sites, independent of native environments.
use hirsel_driver::{compiler_adapters, native::receipt_source};
use hirsel_engine::*;
use hirsel_layout::instance::InstanceKey;
use hirsel_rc::sealed_source::{SourceEntryKind, SourceError};
use hirsel_tir::{BlockId, BodyId, InstructionKind, Rvalue};
use std::sync::Arc;

const SOURCE: &str = r#"
fn probe(a:u64,b:u64)->u64 {
 let f:fn()->u64=fn()->u64 {a}
 let g:fn()->u64=fn()->u64 {b}
 f()+g()
}
pub fn main()->u64 {probe(7u64,9u64)}
"#;

fn original(
    verify: Verify,
) -> (
    Arc<SignatureSnapshot>,
    NativeOriginalReader,
    NativeOriginalBody,
) {
    let store: Arc<dyn store::ResultStore> = Arc::new(store::MemStore::default());
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
        .with_result_store(store.clone());
    engine.verify = verify;
    let file = engine
        .sources
        .add("receipt-closure-source.hir", SOURCE.into())
        .unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    let mut probe = entry_path(&default_context());
    probe.0.name = "probe".into();
    let facts = crate::native_front_fixture::products(&mut engine, &[probe.clone()]).unwrap();
    let reader = facts
        .original_context
        .reader(
            facts.originals.iter().map(|product| (**product).clone()),
            store,
        )
        .unwrap();
    let original = reader.read(&probe).unwrap();
    (facts.signatures.clone(), reader, original)
}

fn sites(definition: &hirsel_tir::Definition) -> Vec<(BodyId, BlockId, u32, BodyId)> {
    let mut sites = vec![];
    for (parent, body) in definition.bodies.iter().enumerate() {
        for (block, row) in body.blocks.iter().enumerate() {
            for (statement, instruction) in row.instructions.iter().enumerate() {
                if let InstructionKind::Assign { value, .. } = instruction.kind
                    && let Rvalue::Closure { body: child, .. } = body.rvalues[value.0 as usize]
                {
                    sites.push((
                        BodyId(parent as u32),
                        BlockId(block as u32),
                        statement as u32,
                        child,
                    ));
                }
            }
        }
    }
    sites
}

fn key(
    original: &NativeOriginalBody,
    child: BodyId,
    types: &hirsel_types::TypeInterner,
) -> InstanceKey {
    InstanceKey::new(
        original.definition().path.path(),
        child.0,
        &[],
        types,
        None,
        None,
    )
    .unwrap()
}

#[test]
fn actual_closure_sites_keep_distinct_capture_accounts_without_retaining_the_body() {
    with_compiler_stack(|| {
        let (engine, reader, original) = original(Verify::On);
        let source = original.output().unwrap().observation_identity();
        let revision = original.implementation_revision();
        let types = engine.types();
        let sites = sites(original.definition());
        assert_eq!(sites.len(), 2);
        let weak = Arc::downgrade(original.definition());
        let mut observed = vec![];
        for &(parent, block, statement, child) in &sites {
            let base = key(&original, child, types);
            let account =
                receipt_source::closure(&original, &base, types, parent, block, statement).unwrap();
            assert_eq!(account.entry().source_identity(), source);
            assert_eq!(account.demand().source_identity(), source);
            assert_eq!(account.entry().implementation_revision(), revision);
            assert_eq!(account.entry().reference(), account.demand().reference());
            assert!(account.entry().parameters().is_empty());
            assert_eq!(account.entry().captures().len(), 1);
            assert_eq!(account.entry().captures()[0].ordinal, 0);
            let SourceEntryKind::Closure { producer, .. } = account.entry().kind() else {
                panic!("original closure kind")
            };
            assert_eq!(
                (producer.body, producer.block, producer.statement),
                (parent.0, block.0, statement)
            );
            observed.push(account);
        }
        assert_eq!(observed[0].entry().result(), observed[1].entry().result());
        assert_eq!(
            observed[0].entry().captures()[0].ty,
            observed[1].entry().captures()[0].ty
        );
        assert_ne!(observed[0].entry().kind(), observed[1].entry().kind());
        assert_ne!(
            observed[0].entry().captures()[0].place,
            observed[1].entry().captures()[0].place
        );
        drop(original);
        drop(reader);
        drop(engine);
        assert!(weak.upgrade().is_none());
        assert_eq!(observed[1].entry().source_identity(), source);
    });
}

#[test]
fn a_closure_site_rejects_another_target_statement_or_derived_native_key() {
    with_compiler_stack(|| {
        let (engine, _reader, original) = original(Verify::On);
        let sites = sites(original.definition());
        let (parent, block, statement, child) = sites[0];
        let types = engine.types();
        let base = key(&original, child, types);
        assert!(receipt_source::closure(&original, &base, types, parent, block, statement).is_ok());
        let wrong = key(&original, sites[1].3, types);
        assert_eq!(
            receipt_source::closure(&original, &wrong, types, parent, block, statement).err(),
            Some(SourceError::Malformed(
                "original closure target body differs"
            ))
        );
        assert_eq!(
            receipt_source::closure(&original, &base, types, parent, block, u32::MAX).err(),
            Some(SourceError::Malformed("original closure statement"))
        );
        let derived = base.derived(
            "hirsel.test.derived-closure.v1",
            &[b"another consuming context"],
        );
        assert_eq!(
            receipt_source::closure(&original, &derived, types, parent, block, statement).err(),
            Some(SourceError::Unsupported(
                "closure source observation needs an exact nongeneric lexical base"
            ))
        );
    });
}

#[test]
fn a_closure_without_the_actual_v_output_stays_unavailable() {
    with_compiler_stack(|| {
        let (engine, _reader, original) = original(Verify::Off);
        assert!(original.output().is_none());
        let (parent, block, statement, child) = sites(original.definition())[0];
        let types = engine.types();
        let base = key(&original, child, types);
        assert_eq!(
            receipt_source::closure(&original, &base, types, parent, block, statement).err(),
            Some(SourceError::Unsupported(
                "receipt source observation needs the current original V output"
            ))
        );
    });
}
