//! Project ordinary source-query contracts into native declaration metadata.
//! This creates no source V token, native acceptance, or implementation body.
use hirsel_engine::NativeDiscoveryProduct;
use hirsel_layout::instance::InstanceDeclaration;
use hirsel_lir::abi_types::ParameterPath;
use hirsel_rc::instance::SourceDeclaration;
use hirsel_tir::BodyId;
use std::sync::Arc;

/// A type-only declaration has no executable entry. Executable source results
/// carry the complete lexical caller/scoped contracts and entry parameter types
/// needed to register functions and finalizers before a body is demanded.
pub fn source_declaration(
    product: &NativeDiscoveryProduct,
) -> Result<Option<SourceDeclaration>, String> {
    let Some(contracts) = &product.contracts else {
        return Ok(None);
    };
    let body_count = u32::try_from(contracts.scoped_parameters.len())
        .map_err(|_| "source contract body limit exceeded")?;
    if contracts.entry >= body_count
        || contracts.caller_bodies.iter().any(|b| *b >= body_count)
        || product.source.signature.definition.path() != &product.source.definition
    {
        return Err("source contract declaration identity differs".into());
    }
    Ok(Some(SourceDeclaration {
        entry: BodyId(contracts.entry),
        entry_parameters: contracts.entry_parameters.clone(),
        declaration: Arc::new(InstanceDeclaration {
            definition: product.source.definition.clone(),
            signature: product.source.signature.fingerprint,
            body_count,
            caller_bodies: contracts.caller_bodies.iter().map(|b| BodyId(*b)).collect(),
            scoped_parameters: contracts
                .scoped_parameters
                .iter()
                .map(|parameters| {
                    parameters
                        .iter()
                        .map(|(parameter, fields)| ParameterPath {
                            parameter: *parameter,
                            fields: fields.clone(),
                        })
                        .collect()
                })
                .collect(),
        }),
    }))
}
