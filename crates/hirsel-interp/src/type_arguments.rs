//! Concrete invocation types retained for type-directed operations in generic TIR.
use crate::{Error, invalid};
use hirsel_types::*;

pub use hirsel_types::Actuals;
pub type Bindings = ActualBindings;

pub(crate) fn concrete(
    runtime: &crate::aggregate::RuntimeTypes,
    arguments: &[GenericArgument],
    bindings: &Bindings,
) -> Result<Actuals, Error> {
    close_arguments(&runtime.types, &runtime.memo, arguments, bindings)
        .map_err(|e| invalid(format!("invocation type substitution: {e:?}")))
}

/// Nongeneric frames carry no incidental signature-store snapshot.
pub(crate) fn bind(bindings: &mut Bindings, scope: SiteRef, actuals: Actuals) {
    if !actuals.arguments.is_empty() {
        bindings.insert(scope, actuals);
    }
}
