//! Shared roots (MEM-11). Freezing moves a unique value into a frozen graph with
//! one root; thawing the last root moves it back out, and any other thaw gives the
//! root back unchanged. Root counting lives in `value::SharedRoot`.
use super::{error, ok};
use crate::value::SharedRoot;
use crate::{Error, OperationResult, Value, invalid};

pub(super) fn call(name: &str, args: &[Value], next: &mut u64) -> Result<OperationResult, Error> {
    let [argument] = args else {
        return Err(invalid("shared operation arity"));
    };
    let value = if name == "shared.freeze" {
        *next += 1;
        Value::Shared(SharedRoot::freeze(*next, argument.clone()))
    } else {
        let Value::Shared(root) = argument else {
            return Err(invalid("Shared representation"));
        };
        match root.clone().thaw()? {
            Ok(value) => ok(value),
            Err(root) => error(Value::Shared(root)),
        }
    };
    // The taken argument moves into the result either way.
    Ok(OperationResult::Complete {
        value,
        arguments: vec![],
        consumed: vec![0],
        released: vec![],
    })
}
