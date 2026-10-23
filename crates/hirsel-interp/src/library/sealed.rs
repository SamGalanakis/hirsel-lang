//! Public requests use the evaluator's storage and callback custody machinery.
use super::{Library, callbacks::Pending};
use crate::{Error, OperationCall, OperationResult, Value, invalid};
use hirsel_types::{BuiltinType, GenericArgument, TypeKind};

pub(super) fn seal(call: OperationCall<'_>) -> Result<OperationResult, Error> {
    let [Value::Handle { kind, id }, _] = call.arguments else {
        return Err(invalid("arena.seal requires a grant and an owned root"));
    };
    if kind != crate::checked::ARENA_HANDLE {
        return Err(invalid("arena.seal requires an explicit arena grant"));
    }
    let root_type = call
        .generics
        .iter()
        .find_map(|a| match a {
            GenericArgument::Type(ty) => Some(*ty),
            _ => None,
        })
        .or_else(|| match &call.types.get(call.result_type)?.kind {
            TypeKind::Builtin {
                kind: BuiltinType::Region,
                arguments,
            } => match arguments.first()? {
                GenericArgument::Type(ty) => Some(*ty),
                _ => None,
            },
            _ => None,
        })
        .ok_or_else(|| invalid("arena.seal missing closed root type"))?;
    Ok(OperationResult::SealArena {
        arena: *id,
        root_type,
    })
}
impl Library {
    pub(super) fn region_with(
        &mut self,
        call: OperationCall<'_>,
    ) -> Result<OperationResult, Error> {
        let [Value::Handle { kind, id }, body] = call.arguments else {
            return Err(invalid("region.with requires an owner and a callback"));
        };
        if kind != "Region" {
            return Err(invalid("region.with requires a Region owner"));
        }
        let root_type = call
            .generics
            .iter()
            .find_map(|a| match a {
                GenericArgument::Type(ty) => Some(*ty),
                _ => None,
            })
            .ok_or_else(|| invalid("region.with missing closed root type"))?;
        let result = self.schedule(
            Pending::Read,
            body.clone(),
            vec![Value::Unit],
            vec![],
            vec![],
        );
        Ok(OperationResult::BorrowRegion {
            owner: *id,
            root_type,
            result: Box::new(result),
        })
    }
}
