//! Checked execution's declared facts over one immutable signature snapshot.
//! Reading these facts neither checks a body nor lowers a finalizer.
use super::*;
impl SignatureSnapshot {
    pub fn checked_runtime(
        &self,
        forget: Option<&DefPath>,
    ) -> Result<hirsel_interp::checked::CheckedRuntime, QueryError> {
        let mut checked = hirsel_interp::checked::CheckedRuntime {
            arena_with: Some(DefPath::core("arena.with")),
            ..Default::default()
        };
        let mut definitions: Vec<_> = self.definitions();
        definitions.sort_by(|a, b| a.path().cmp(b.path()));
        for definition in definitions {
            let signature = self
                .signature(definition)
                .ok_or(QueryError::NotImplemented)?
                .map_err(Clone::clone)?;
            if let hirsel_types::SignatureKind::Type(declaration) = &signature.kind {
                if declaration.layout.flat {
                    checked.flat.insert(signature.definition.path().clone());
                }
                if declaration.loop_source.is_some() {
                    checked
                        .operations
                        .insert(signature.definition.path().clone(), signature.clone());
                }
                if declaration.destructor_allocation == hirsel_types::Allocation::NoAlloc {
                    checked
                        .noalloc_drop
                        .insert(signature.definition.path().clone());
                    checked.noalloc.insert(signature.definition.path().clone());
                }
            }
            let hirsel_types::SignatureKind::Function(function) = &signature.kind else {
                continue;
            };
            if function.allocation == hirsel_types::Allocation::NoAlloc {
                checked.noalloc.insert(signature.definition.path().clone());
            }
            checked
                .operations
                .insert(signature.definition.path().clone(), signature.clone());
        }
        if let Some(forget) = forget {
            checked.noalloc.insert(forget.clone());
        }
        Ok(checked)
    }
}
