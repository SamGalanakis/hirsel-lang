//! Defining representation reads are private native inputs, never public signature facts.
use crate::*;
#[derive(Clone, Debug)]
pub(crate) struct OwnerRecord {
    pub text_hash: ContentHash,
    pub representation: hirsel_check_protocol::header::OwnerRepresentation,
    pub dependencies: Vec<SignatureDependency>,
    pub view: std::sync::OnceLock<(hirsel_types::SignatureRef, ContentHash)>,
}
impl OwnerRecord {
    pub fn body_view(
        &self,
        types: &hirsel_types::TypeInterner,
        public: &hirsel_types::SignatureRef,
    ) -> &(hirsel_types::SignatureRef, ContentHash) {
        self.view.get_or_init(|| {
            let view = std::sync::Arc::new(hirsel_types::encoding::owner_view(
                public,
                self.representation.declaration.clone(),
            ));
            let digest = hirsel_validate::digest(types, &view).0;
            (view, digest)
        })
    }
}
pub use hirsel_tir::representation::RepresentationRecord;
impl QueryEngine {
    /// `path`'s private shape as its signature's normalization recorded it,
    /// here or in the std image.
    pub(crate) fn owner_record(&self, path: &DefPath) -> Option<OwnerRecord> {
        if let Some(record) = self
            .signature_products(DefId::intern(path))
            .and_then(|products| products.representation.as_ref())
        {
            return Some(record.clone());
        }
        None
    }
    /// A body-local view, never published by item_signature or the public
    /// provider. PROG-2's privacy boundary is the exact package (version included).
    /// Reuse the shape from normalization; do not normalize a second signature.
    pub(crate) fn body_signature(
        &self,
        package: &hirsel_syntax::PackageId,
        public: hirsel_types::SignatureRef,
    ) -> Result<(hirsel_types::SignatureRef, Option<ContentHash>), QueryError> {
        if package != &public.definition.path().0.package
            || public.metadata.visibility != hirsel_syntax::Visibility::PublicOpaque
        {
            return Ok((public, None));
        }

        let Some(record) = self
            .signature_products(public.definition)
            .and_then(|products| products.representation.as_ref())
        else {
            // Absence is a private input too: adding a source shape must retry
            // an owner body's previously cached constructor rejection.
            let digest = self
                .implementation_hash(public.definition.path())
                .map(|_| hirsel_validate::digest(&self.types, &public).0);
            return Ok((public, digest));
        };
        if self.implementation_hash(public.definition.path()) != Some(record.text_hash) {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "body representation".into(),
                message: "stale private representation".into(),
            }));
        }
        let (private, digest) = record.body_view(&self.types, &public);
        Ok((private.clone(), Some(*digest)))
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
        let Some(record) = self.owner_record(definition) else {
            // Source-less capabilities/extern layouts need an explicit external record.
            return Err(QueryError::NotImplemented);
        };
        if self.declarations.sites(definition).len() != 1
            || self.implementation_hash(definition) != Some(record.text_hash)
        {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "owner representation".into(),
                message: "stale private representation".into(),
            }));
        }
        let finalizer = match &record.representation.declaration.drop {
            Some(path) => Some((path.clone(), self.lower_to_tir(path)?.value.entry)),
            None => None,
        };
        completed_owner_record(&self.types, definition, &public, record, finalizer)
    }
}

pub(crate) fn completed_owner_record(
    types: &hirsel_types::TypeInterner,
    definition: &DefPath,
    public: &hirsel_types::SignatureRef,
    record: OwnerRecord,
    finalizer: Option<(DefPath, hirsel_tir::BodyId)>,
) -> Result<RepresentationRecord, QueryError> {
    // Encoded as Private so the CAP-5 ceiling binds the revision.
    let mut private =
        hirsel_types::encoding::owner_view(public, record.representation.declaration.clone());
    private.metadata.visibility = hirsel_syntax::Visibility::Private;
    let shape = hirsel_types::encoding::canonical_encoding(types, &private, &record.dependencies)
        .map_err(|e| {
        QueryError::Operational(hirsel_types::OperationalFailure {
            operation: "owner representation".into(),
            message: format!("{e:?}"),
        })
    })?;
    let representation_revision =
        ContentHash::of_parts("hirsel.owner-representation.v1", [shape.as_slice()]);
    let declaration = record.representation.declaration;

    Ok(RepresentationRecord {
        definition: definition.clone(),
        representation_revision,
        generics: declaration.generics,
        private_shape: declaration.representation.expect("owner shape"),
        layout_promises: declaration.layout,
        resource: declaration.resource,
        finalizer,
        unsafe_ffi_package: public.metadata.unsafe_ffi,
    })
}
