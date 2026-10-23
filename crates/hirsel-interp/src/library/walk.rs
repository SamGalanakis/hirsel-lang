//! Private iterator state, represented as ordinary evaluator data. No tree snapshot.
use super::{collection, dispatch::*, ok, path};
use crate::{Error, Value, invalid};
pub use hirsel_kernels::walk::Entry;
use hirsel_kernels::walk::Walk;
use hirsel_syntax::DefPath;
use hirsel_types::BuiltinType;

fn state(directory: Value, walk: Walk) -> Value {
    Value::Record {
        definition: (&DefPath::core("WalkState")).into(),
        fields: vec![
            Some(directory),
            Some(collection::sequence(
                BuiltinType::List,
                walk.pages.into_iter().map(|page| {
                    collection::sequence(
                        BuiltinType::List,
                        page.into_iter().map(|e| {
                            Value::Tuple(vec![Some(Value::String(e.path)), Some(u32_value(e.kind))])
                        }),
                    )
                }),
            )),
        ],
    }
}
pub fn iterator(directory: Value, entries: Vec<Entry>) -> Value {
    let mut walk = Walk::default();
    walk.push(entries);
    state(directory, walk)
}
/// Find a walk under any number of map adapters. None means an unrelated iterator.
/// Each read is one confined, recorded read_dir operation; a failure is yielded
/// once, after which the cursor pages are empty. Adapter refusal is not a FileError.
pub fn pull(
    it: &mut Value,
    mut read: impl FnMut(&Value, &str) -> Result<Result<Vec<Entry>, Value>, Error>,
) -> Result<Option<Option<Value>>, Error> {
    let base = it;
    let Value::Record { definition, fields } = base else {
        return Ok(None);
    };
    if !definition.is_core("WalkState") {
        return Ok(None);
    }
    let directory = fields[0]
        .as_ref()
        .ok_or_else(|| invalid("dead walk directory"))?
        .clone();
    let mut walk = Walk::default();
    for page in collection::elements(fields[1].as_ref().unwrap(), BuiltinType::List)? {
        let mut entries = std::collections::VecDeque::new();
        for entry in collection::elements(&page, BuiltinType::List)? {
            let Value::Tuple(pair) = entry else {
                return Err(invalid("walk entry"));
            };
            let Value::String(path) = pair[0].as_ref().unwrap() else {
                return Err(invalid("walk path"));
            };
            entries.push_back(Entry {
                path: path.clone(),
                kind: match pair[1].as_ref().unwrap() {
                    Value::Integer(n) => n.bits() as u32,
                    _ => return Err(invalid("walk kind")),
                },
            });
        }
        walk.pages.push(entries);
    }
    let mut item = None;
    while let Some(entry) = walk.next_entry() {
        match entry.kind {
            1 => match read(&directory, &entry.path) {
                Err(error) => {
                    // A host continuation has not delivered this page yet.
                    // Keep all completed pages and retry only this directory.
                    walk.pages.push([entry].into());
                    *base = state(directory, walk);
                    return Err(error);
                }
                Ok(result) => match result {
                    Ok(entries) => walk.push(entries),
                    Err(error) => {
                        walk.stop();
                        item = Some(error);
                        break;
                    }
                },
            },
            0 | 2 => {
                item = Some(ok(path(&entry.path)));
                break;
            }
            _ => {
                walk.stop();
                item = Some(file_fault(1));
                break;
            }
        }
    }
    *base = state(directory, walk);
    Ok(Some(item))
}

pub fn directory(mut it: &Value) -> Option<&Value> {
    loop {
        match it {
            Value::Record { definition, fields } if definition.is_core("IterMap") => {
                it = fields.first()?.as_ref()?
            }
            Value::Record { definition, fields } if definition.is_core("WalkState") => {
                return fields.first()?.as_ref();
            }
            _ => return None,
        }
    }
}
