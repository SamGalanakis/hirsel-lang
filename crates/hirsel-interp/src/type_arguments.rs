//! Concrete invocation types retained for type-directed operations in generic TIR.
use crate::{Error, invalid};
use hirsel_types::*;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Actuals {
    pub types: Arc<TypeStore>,
    pub arguments: Vec<GenericArgument>,
}
pub type Bindings = BTreeMap<NameSite, Actuals>;

pub(crate) fn concrete(
    source: &TypeStore,
    arguments: &[GenericArgument],
    bindings: &Bindings,
) -> Result<Actuals, Error> {
    let mut store = source.clone();
    let mut result = arguments.to_vec();
    for (&scope, actuals) in bindings.iter().filter(|_| !arguments.is_empty()) {
        let imported = actuals
            .arguments
            .iter()
            .map(|a| match a {
                GenericArgument::Type(t) => {
                    import_type(&actuals.types, *t, &mut store).map(GenericArgument::Type)
                }
                GenericArgument::Size(n) => Ok(GenericArgument::Size(n.clone())),
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| invalid("invocation type import"))?;
        let authorities = vec![AuthoritySummary::Unknown; imported.len()];
        let substitution = Substitution {
            binder: Some(GenericBinder::Body(scope)),
            arguments: &imported,
            authorities: &authorities,
            ..Default::default()
        };
        for argument in &mut result {
            match argument {
                GenericArgument::Type(t) => {
                    *t = substitute_type_in_place(&mut store, *t, substitution)
                        .map_err(|e| invalid(format!("invocation type substitution: {e:?}")))?
                }
                GenericArgument::Size(Size::Parameter(SizeParameterRef::Body {
                    scope: owner,
                    parameter,
                })) if *owner == scope => {
                    *argument = imported
                        .get(parameter.0 as usize)
                        .cloned()
                        .ok_or_else(|| invalid("invocation size argument"))?;
                }
                _ => (),
            }
        }
    }
    Ok(Actuals {
        types: Arc::new(store),
        arguments: result,
    })
}

/// Nongeneric frames carry no incidental signature-store snapshot.
pub(crate) fn bind(bindings: &mut Bindings, scope: NameSite, actuals: Actuals) {
    if !actuals.arguments.is_empty() {
        bindings.insert(scope, actuals);
    }
}
