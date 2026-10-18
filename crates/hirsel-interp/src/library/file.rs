//! Incremental file operations use the opened identity, never its current path.
use super::{
    Library,
    dispatch::*,
    simulation::{Fault, Grant},
};
use crate::{Error, Integer, OperationContext, OperationResult, Value, invalid};
use hirsel_types::{BuiltinType, Primitive};

impl Library {
    pub(super) fn file_stream(
        &mut self,
        name: &str,
        a: &[Value],
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let (tree, _, dir) = self.simulation.directory(arg(a, 0)?)?;
        let id = self.simulation.id(arg(a, 1)?, "File")?;
        let Some(Grant::File {
            tree: owner_tree,
            dir: owner,
            identity,
            position,
            writable,
            ..
        }) = self.simulation.grants.get(&id).cloned()
        else {
            return Err(invalid("file stream grant"));
        };
        if tree != owner_tree || dir != owner {
            return Err(invalid("file stream directory"));
        }
        let writing = matches!(name, "file.write" | "file.append");
        let denied = writing && !self.simulation.writable(dir);
        let wrong_mode = (writing && !writable) || (name == "file.read" && writable);
        let fault = if denied {
            None
        } else {
            self.tasks
                .canceled()
                .map(Fault::Canceled)
                .or_else(|| self.simulation.fault(name))
        };
        let mut updates = vec![];
        let value = if denied {
            error(variant(2, vec![]))
        } else if wrong_mode {
            file_fault(self.simulation.config.invalid_path_code)
        } else if writing {
            let bytes = bytes(arg(a, 2)?)?;
            let n = match &fault {
                Some(Fault::Canceled(_)) => 0,
                Some(Fault::Fault { partial, .. } | Fault::File { partial, .. }) => {
                    (*partial).min(bytes.len())
                }
                None => bytes.len(),
            };
            if n > 0 {
                self.simulation
                    .file_contents
                    .get_mut(&identity)
                    .unwrap()
                    .extend_from_slice(&bytes[..n]);
                self.simulation.sync_file(tree, identity);
            }
            if let Some(f) = fault {
                fault_result(f)
            } else {
                ok(if name == "file.write" {
                    u64_value(n as u64)
                } else {
                    Value::Unit
                })
            }
        } else if let Some(f) = fault {
            fault_result(f)
        } else if name == "file.seek" {
            let offset = integer_u64(arg(a, 2)?)?;
            if offset > i64::MAX as u64 {
                file_fault(self.simulation.config.invalid_path_code)
            } else {
                if let Some(Grant::File { position, .. }) = self.simulation.grants.get_mut(&id) {
                    *position = offset;
                }
                ok(Value::Unit)
            }
        } else {
            let max = integer_u64(arg(a, 3)?)?;
            let data = &self.simulation.file_contents[&identity];
            let start = position.min(data.len() as u64) as usize;
            let n = max.min(65536).min((data.len() - start) as u64) as usize;
            let delivered = &data[start..start + n];
            let mut buffer = arg(a, 2)?.clone();
            let Value::Sequence {
                kind: BuiltinType::List,
                elements,
                ..
            } = &mut buffer
            else {
                return Err(invalid("file read buffer"));
            };
            elements.extend(delivered.iter().map(|b| {
                Some(Value::Integer(
                    Integer::new(Primitive::U8, (*b).into()).unwrap(),
                ))
            }));
            let bytes = delivered.to_vec();
            if let Some(Grant::File { position, .. }) = self.simulation.grants.get_mut(&id) {
                *position += n as u64;
            }
            updates.push((2, buffer));
            // DET-1 records the bytes, not merely their count.
            self.simulation.record(
                "Dir",
                name,
                encode_arguments(a),
                encode_value(&ok(Value::Bytes(bytes))),
                context,
            )?;
            return Ok(OperationResult::Complete {
                value: ok(u64_value(n as u64)),
                arguments: updates,
                consumed: vec![],
                released: vec![],
            });
        };
        self.simulation.record(
            "Dir",
            name,
            encode_arguments(a),
            encode_value(&value),
            context,
        )?;
        Ok(OperationResult::returned(value))
    }
}
fn integer_u64(value: &Value) -> Result<u64, Error> {
    match value {
        Value::Integer(n) => u64::try_from(n.bits()).map_err(|_| invalid("file count")),
        _ => Err(invalid("file count")),
    }
}
