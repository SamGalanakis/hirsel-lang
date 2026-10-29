//! Ownership traversal follows checked::resources_in and eval::release_fields,
//! with the latter's closure rule: only uninvoked owned captures are released.
//! Borrow snapshots and snapshots of an invoked once environment own nothing.
use crate::Value;

pub(super) fn children<'a>(value: &'a Value, mut visit: impl FnMut(u32, &'a Value)) {
    if let Value::Closure {
        captures,
        environment,
        invoked,
        ..
    } = value
    {
        if invoked.as_ref().is_some_and(|state| state.get().is_some()) {
            return;
        }
        for (index, (value, owned)) in captures.iter().zip(&environment.owned_captures).enumerate()
        {
            if *owned && let Some(value) = value {
                visit(index as u32, value);
            }
        }
    } else if let Some(fields) = value.fields() {
        for (index, value) in fields.iter().enumerate() {
            if let Some(value) = value {
                visit(index as u32, value);
            }
        }
    }
}

pub(super) fn resources_in<const OWNED: bool>(value: &Value) -> u64 {
    let mut count = 0;
    let mut work = vec![value];
    while let Some(value) = work.pop() {
        count += u64::from(
            OWNED && owned_data(value)
                || matches!(
                    value,
                    Value::Resource { .. } | Value::ResourceVariant { .. }
                )
                || matches!(value, Value::Handle { kind, .. } if kind == "Region"),
        );
        children(value, |_, child| work.push(child));
    }
    count
}

pub(super) fn owned_data(value: &Value) -> bool {
    matches!(value, Value::String(_) | Value::Bytes(_))
}
