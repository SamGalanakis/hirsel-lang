//! Private iterator state, represented as ordinary evaluator data. No tree snapshot.
use super::{collection, dispatch::*, ok, path};
use crate::{Error, Value, invalid};
pub use hirsel_kernels::walk::Entry;
use hirsel_kernels::walk::Walk;
use hirsel_syntax::DefPath;
use hirsel_types::BuiltinType;

fn state(directory: Value, walk: Walk, recursive: bool) -> Value {
    Value::Record {
        definition: (&DefPath::core(if recursive {
            "WalkState"
        } else {
            "ReadDirState"
        }))
            .into(),
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
pub fn iterator(directory: Value, path: &str, entries: Vec<Entry>, recursive: bool) -> Value {
    let mut walk = Walk::default();
    walk.push_page(path, entries, recursive);
    state(directory, walk, recursive)
}
/// Find a walk under any number of map adapters. None means an unrelated iterator.
/// Each read is one confined, recorded read_dir operation; a failure is yielded
/// once, after which the cursor pages are empty. Adapter refusal is not a FileError.
pub fn pull(
    it: &mut Value,
    mut read: impl FnMut(&Value, &str, &str, bool) -> Result<Result<Vec<Entry>, Value>, Error>,
) -> Result<Option<Option<Value>>, Error> {
    let base = it;
    let Value::Record { definition, fields } = base else {
        return Ok(None);
    };
    let recursive = definition.is_core("WalkState");
    if !recursive && !definition.is_core("ReadDirState") {
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
            1 | 4 if recursive || entry.kind == 4 => {
                let (p, after) = if entry.kind == 4 {
                    entry
                        .path
                        .split_once('\0')
                        .ok_or_else(|| invalid("directory cursor"))?
                } else {
                    (entry.path.as_str(), "")
                };
                match read(&directory, p, after, recursive) {
                    Err(error) => {
                        // A host continuation has not delivered this page yet.
                        // Keep all completed pages and retry only this directory.
                        walk.pages.push([entry].into());
                        *base = state(directory, walk, recursive);
                        return Err(error);
                    }
                    Ok(result) => match result {
                        Ok(entries) => walk.push_page(p, entries, recursive),
                        Err(error) => {
                            walk.stop();
                            item = Some(error);
                            break;
                        }
                    },
                }
            }
            0..=3 if !recursive || entry.kind != 3 => {
                item = Some(ok(if recursive {
                    path(&entry.path)
                } else {
                    Value::Record {
                        definition: (&DefPath::core("DirEntry")).into(),
                        fields: vec![Some(path(&entry.path)), Some(variant(entry.kind, vec![]))],
                    }
                }));
                break;
            }
            _ => {
                walk.stop();
                item = Some(file_fault(1));
                break;
            }
        }
    }
    *base = state(directory, walk, recursive);
    Ok(Some(item))
}

pub fn directory(mut it: &Value) -> Option<&Value> {
    loop {
        match it {
            Value::Record { definition, fields } if definition.is_core("IterMap") => {
                it = fields.first()?.as_ref()?
            }
            Value::Record { definition, fields }
                if definition.is_core("WalkState") || definition.is_core("ReadDirState") =>
            {
                return fields.first()?.as_ref();
            }
            _ => return None,
        }
    }
}
