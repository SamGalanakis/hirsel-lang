use super::{NativeDeclarations, RootInputs};
use hirsel_engine::NativeMetadataWeight as Weight;
use hirsel_rc::instance::Requirement;
use std::sync::Arc;

fn requirement(w: &mut Weight, value: &Requirement) {
    let Requirement {
        owner,
        owner_binder: _,
        site: _,
        kind: _,
        callee,
        binder: _,
        arguments,
    } = value;
    w.path(owner);
    w.path(callee);
    w.flat_vec(arguments);
}
impl NativeDeclarations {
    /// Retained allocation bound for this copied, bodyless declaration snapshot.
    /// Packs its private ordered maps and CT-5 tables before measuring; no source/Value/
    /// Ready body is demanded. Charge this value for the snapshot's lifetime.
    /// Shared front syntax, signatures, summaries, source maps, V and interned
    /// rows still require their original owners. Mutable memo growth is separate
    /// work storage; this covers its initial inline/Arc storage only.
    pub fn retained_metadata_bytes(&mut self) -> Result<u64, String> {
        if !self.program.definitions.is_empty() {
            return Err("native metadata accounting requires bodyless declarations".into());
        }
        let Self {
            roots,
            program,
            context,
            sources,
            source_anchors,
            discoveries,
            constant_definitions,
        } = self;
        let shared = match roots {
            RootInputs::Front(front) => front
                .catalogue
                .iter()
                .map(|product| Arc::as_ptr(&product.source))
                .collect::<std::collections::HashSet<_>>(),
            RootInputs::Owned(_) => Default::default(),
        };
        let mut w = Weight::default();
        w.add(size_of::<Self>());
        if let Some(roots) = roots.owned_mut() {
            w.add(size_of::<hirsel_engine::NativeSourceRoots>() + 2 * size_of::<usize>());
            w.roots(roots);
        }
        // Shared front roots pin the producer reservation through FrontNativeProducts.
        // Only the new program/context/source maps below are copied ownership.
        w.arc(&program.memo);
        w.packed_map(&mut program.definitions, |w, path, _| w.path(path));
        w.packed_map(&mut program.signatures, |w, path, _| w.path(path));
        w.packed_map(&mut program.revisions, |w, path, _| w.path(path));
        w.packed_map(&mut program.binders, |w, path, _| w.path(path));
        w.packed_map(&mut program.operations, |w, path, operation| {
            w.path(path);
            w.path(&operation.definition);
        });
        w.vec(&program.requirements, requirement);
        w.packed_map(&mut program.constants, |w, path, value| {
            w.path(path);
            w.materialized(value);
        });
        w.packed_map(&mut program.owners.nominals, |w, _, value| {
            if let hirsel_layout::layout::OwnerRepresentation::Defined(value) = value {
                w.representation(value);
            }
        });
        w.packed_map(&mut program.owners.containers, |w, _, value| {
            w.representation(value)
        });
        w.instance_bodies(&mut program.instance_bodies);
        w.packed_map(&mut context.sources, |w, path, source| {
            w.path(path);
            w.flat_vec(&source.entry_parameters);
            w.arc(&source.declaration);
            let hirsel_layout::instance::InstanceDeclaration {
                definition,
                signature: _,
                body_count: _,
                caller_bodies,
                scoped_parameters,
            } = &*source.declaration;
            w.path(definition);
            w.flat_vec(caller_bodies);
            w.vec(scoped_parameters, |w, parameters| {
                w.vec(parameters, |w, p| w.flat_vec(&p.fields))
            });
        });
        w.packed_map(&mut context.requirements, |w, path, values| {
            w.path(path);
            w.vec(values, requirement);
        });
        w.packed_map(sources, |w, path, value| {
            w.path(path);
            if !shared.contains(&Arc::as_ptr(value)) {
                w.arc(value);
                w.source(value);
            }
        });
        w.packed_map(source_anchors, |w, path, anchors| {
            w.path(path);
            w.flat_vec(anchors);
        });
        w.flat_vec(discoveries);
        w.vec(constant_definitions, Weight::path);
        Ok(w.bytes())
    }
}
