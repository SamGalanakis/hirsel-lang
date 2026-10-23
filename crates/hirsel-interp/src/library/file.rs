//! Incremental file operations use the opened identity, never its current path.
use super::{
    Library,
    dispatch::*,
    simulation::{Fault, Grant},
};
use crate::{Error, Integer, OperationCall, OperationContext, OperationResult, Value, invalid};
use hirsel_types::{BuiltinType, Primitive};

impl Library {
    pub(super) fn file_stream_call(
        &mut self,
        name: &str,
        call: OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        if let Some(result) = self.file_stream(name, call.arguments, context)? {
            return Ok(result);
        }
        let token = self.next_callback;
        self.next_callback += 1;
        let file = self.simulation.id(arg(call.arguments, 1)?, "File")?;
        let anchor = context
            .anchor()
            .cloned()
            .unwrap_or(crate::observation::Anchor {
                definition: call.path.clone().into(),
                body: hirsel_tir::BodyId(0),
                cleanup: None,
                site: call.site,
            });
        self.simulation.record(
            "Dir",
            "file.read.pending",
            encode_arguments(&[
                call.arguments[0].clone(),
                call.arguments[1].clone(),
                call.arguments[3].clone(),
            ]),
            vec![],
            context,
        )?;
        // Own the exclusive buffer while parked, as a completed read owns its
        // writeback. Polls must not clone accumulated bytes or force COW.
        let arguments = call
            .arguments
            .iter_mut()
            .enumerate()
            .map(|(index, value)| {
                if index == 2 {
                    std::mem::replace(value, Value::Unit)
                } else {
                    value.clone()
                }
            })
            .collect();
        self.file_waits.insert(token, arguments);
        Ok(OperationResult::Park {
            token,
            wait: crate::BlockedWait {
                spawn_path: self.tasks.path(),
                anchor,
                kind: name.into(),
                targets: vec![file],
            },
        })
    }

    pub(super) fn file_stream(
        &mut self,
        name: &str,
        a: &mut [Value],
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let (tree, _, dir) = self.simulation.directory(arg(a, 0)?)?;
        let id = self.simulation.id(arg(a, 1)?, "File")?;
        let Some(Grant::File {
            tree: owner_tree,
            dir: owner,
            identity,
            position,
            path: open,
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
        let stale = self.simulation.stale.contains(&id) && name != "file.seek";
        let wrong_mode = (writing && !writable)
            || (name == "file.read" && writable)
            || (name == "file.seek" && integer_u64(arg(a, 2)?)? > i64::MAX as u64);
        if name == "file.read"
            && !wrong_mode
            && !stale
            && self.tasks.canceled().is_none()
            && self.simulation.file_readiness.blocked(tree, &open)
        {
            return Ok(None);
        }
        let fault = if denied || wrong_mode {
            None
        } else {
            self.tasks
                .canceled()
                .map(Fault::Canceled)
                .or_else(|| self.simulation.fault(name))
        };
        // A FileRead request is (directory, file, max), as in the host/native
        // protocol. The caller's accumulated output buffer is not an input.
        let request = if name == "file.read" {
            encode_arguments(&[a[0].clone(), a[1].clone(), a[3].clone()])
        } else {
            encode_arguments(a)
        };
        let mut updates = vec![];
        let value = if denied {
            error(variant(2, vec![]))
        } else if stale && fault.is_none() {
            file_fault(1)
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
                let contents = self.simulation.file_contents.get_mut(&identity).unwrap();
                let start = contents.len();
                contents.extend_from_slice(&bytes[..n]);
                self.simulation.sync_file(tree, identity);
                self.simulation
                    .durable_write(tree, identity, start, &bytes[..n]);
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
        } else if name == "file.sync" {
            // A detached or unresolvable open path still syncs the file's bytes.
            let open = self
                .simulation
                .resolve_stored(tree, dir, &open, true)
                .unwrap_or_default();
            self.simulation.durable_sync(tree, identity, &open);
            ok(Value::Unit)
        } else {
            let max = integer_u64(arg(a, 3)?)?;
            let data = &self.simulation.file_contents[&identity];
            let start = position.min(data.len() as u64) as usize;
            let n = max.min(65536).min((data.len() - start) as u64) as usize;
            let delivered = &data[start..start + n];
            if !matches!(
                arg(a, 2)?,
                Value::Sequence {
                    kind: BuiltinType::List,
                    ..
                }
            ) {
                return Err(invalid("file read buffer"));
            }
            let bytes = delivered.to_vec();
            if let Some(Grant::File { position, .. }) = self.simulation.grants.get_mut(&id) {
                *position += n as u64;
            }
            // DET-1 records the bytes, not merely their count.
            self.simulation.record(
                "Dir",
                name,
                request,
                encode_value(&ok(Value::Bytes(bytes.clone()))),
                context,
            )?;
            // Mut arguments have already moved out of the caller. Move the
            // block into the writeback rather than forcing a COW copy per read.
            // Replay validation above completes before touching the buffer.
            let mut buffer = std::mem::replace(&mut a[2], Value::Unit);
            let Value::Sequence { elements, .. } = &mut buffer else {
                unreachable!()
            };
            elements.extend(bytes.into_iter().map(|b| {
                Some(Value::Integer(
                    Integer::new(Primitive::U8, b.into()).unwrap(),
                ))
            }));
            updates.push((2, buffer));
            return Ok(Some(OperationResult::Complete {
                value: ok(u64_value(n as u64)),
                arguments: updates,
                consumed: vec![],
                released: vec![],
            }));
        };
        self.simulation
            .record("Dir", name, request, encode_value(&value), context)?;
        Ok(Some(OperationResult::returned(value)))
    }
}
fn integer_u64(value: &Value) -> Result<u64, Error> {
    match value {
        Value::Integer(n) => u64::try_from(n.bits()).map_err(|_| invalid("file count")),
        _ => Err(invalid("file count")),
    }
}
