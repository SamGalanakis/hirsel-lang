//! Call record validation.
use crate::invalid;
use hirsel_run_schema::{CallForm, CallRecord, RunRefusal};
pub(super) fn validate(call: &CallRecord) -> Result<(), RunRefusal> {
    if CallForm::parse(&call.form).is_none() {
        return Err(invalid("call.form", "expected compute or call"));
    }
    call.cause()?;
    call.argument()?;
    Ok(())
}
