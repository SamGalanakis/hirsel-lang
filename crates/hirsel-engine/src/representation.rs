//! Defining representation reads are private native inputs, never public signature facts.
use crate::*;
#[derive(Clone, Debug)]
pub(crate) struct OwnerRecord {
    pub text_hash: ContentHash,
    pub representation: hirsel_check_protocol::header::OwnerRepresentation,
    pub dependencies: Vec<SignatureDependency>,
}
pub use hirsel_tir::representation::RepresentationRecord;
impl QueryEngine {
    /// A body-local view, never published by item_signature or the public
    /// provider. PROG-2's privacy boundary is the exact package (version included).
    /// Reuse the shape from normalization; do not normalize a second signature.
    pub(crate) fn body_signature(
        &self,
        package: &hirsel_syntax::PackageId,
        public: hirsel_types::SignatureRef,
    ) -> Result<(hirsel_types::SignatureRef, Option<ContentHash>), QueryError> {
        if package != &public.definition.0.package
            || public.metadata.visibility != hirsel_syntax::Visibility::PublicOpaque
        {
            return Ok((public, None));
        }
        let Some(state) = &self.adapters.compiler_state else {
            return Ok((public, None));
        };
        let state = state.borrow();
        let Some(record) = state.representations.get(&public.definition) else {
            // Absence is a private input too: adding a source shape must retry
            // an owner body's previously cached constructor rejection.
            let revision = self.implementation_hash(&public.definition);
            return Ok((public, revision));
        };
        if self.implementation_hash(&public.definition) != Some(record.text_hash) {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "body representation".into(),
                message: "stale private representation".into(),
            }));
        }
        let private = hirsel_types::encoding::owner_view(
            &public,
            record.representation.types.clone(),
            record.representation.declaration.clone(),
        );
        Ok((std::sync::Arc::new(private), Some(record.text_hash)))
    }

    /// Only the defining owner can obtain the private shape. Consumers must select
    /// an owner artifact explicitly; SignatureProvider continues to expose opacity.
    pub fn owner_representation(
        &mut self,
        owner: &DefPath,
        definition: &DefPath,
    ) -> Result<RepresentationRecord, QueryError> {
        if owner != definition {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "owner representation".into(),
                message: "private shape requested by a different owner".into(),
            }));
        }
        let public = self.item_signature(definition)?.value;
        let Some(state) = &self.adapters.compiler_state else {
            return Err(QueryError::NotImplemented);
        };
        let Some(record) = state.borrow().representations.get(definition).cloned() else {
            // Source-less capabilities/extern layouts need an explicit external record.
            return Err(QueryError::NotImplemented);
        };
        let source = self.definition_items(definition)?;
        if source.len() != 1 || source[0].text_hash != record.text_hash {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "owner representation".into(),
                message: "stale private representation".into(),
            }));
        }
        // Encoded as Private so the CAP-5 ceiling binds the revision.
        let mut private = hirsel_types::encoding::owner_view(
            &public,
            record.representation.types.clone(),
            record.representation.declaration.clone(),
        );
        private.metadata.visibility = hirsel_syntax::Visibility::Private;
        let shape = hirsel_types::encoding::canonical_encoding(&private, &record.dependencies)
            .map_err(|e| {
                QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "owner representation".into(),
                    message: format!("{e:?}"),
                })
            })?;
        let representation_revision =
            ContentHash::of_parts("hirsel.owner-representation.v1", [shape.as_slice()]);
        let declaration = record.representation.declaration;
        let finalizer = match &declaration.drop {
            Some(path) => Some((path.clone(), self.lower_to_tir(path)?.value.entry)),
            None => None,
        };
        Ok(RepresentationRecord {
            definition: definition.clone(),
            representation_revision,
            generics: declaration.generics,
            types: record.representation.types,
            private_shape: declaration.representation.expect("owner shape"),
            layout_promises: declaration.layout,
            resource: declaration.resource,
            finalizer,
            unsafe_ffi_package: public.metadata.unsafe_ffi,
        })
    }
}
