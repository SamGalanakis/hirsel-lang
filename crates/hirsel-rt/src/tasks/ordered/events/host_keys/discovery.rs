use super::*;
use crate::io::{AdapterError, Completion, Request};
use crate::io_concurrent::ConcurrentAdapter;
use crate::ordered::footprint::ObjectId;

fn operation(ordinal: u64) -> ObjectId {
    ObjectId {
        creator: vec![1, 2],
        ordinal,
    }
}

fn handle(result: &Result<Completion, AdapterError>) -> u64 {
    let Ok(Completion::Handle(Ok(word))) = result else {
        panic!("handle result")
    };
    *word
}

#[test]
fn host_creation_discovery_is_atomic_and_preserves_distinct_split_handle_identities() {
    for shared in [false, true] {
        let mut log = super::super::Log::default();
        let stream = Ok(Completion::NetHandle(Ok(90)));
        log.host_created(
            operation(0),
            &stream,
            &BTreeMap::from([(90, (Key(40), Kind::Other))]),
        )
        .unwrap();
        let halves = Ok(Completion::Halves(Ok((91, 92))));
        let discoveries = if shared {
            BTreeMap::from([(91, (Key(40), Kind::Other)), (92, (Key(40), Kind::Other))])
        } else {
            BTreeMap::from([
                (91, (Key(41), Kind::Half { stream: Key(40) })),
                (92, (Key(42), Kind::Half { stream: Key(40) })),
            ])
        };
        let parent = log.host_handles[&90].clone();
        assert!(
            log.host_created(
                operation(1),
                &halves,
                &BTreeMap::from([(91, discoveries[&91])])
            )
            .is_err()
        );
        assert_eq!(
            log.host_handles.len(),
            1,
            "missing second discovery installs no first handle"
        );
        assert!(log.host_resources.resource(Key(41)).is_err());
        let rows = log
            .host_created(operation(1), &halves, &discoveries)
            .unwrap();
        assert_eq!(
            rows.iter().map(|row| row.handle.field).collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(rows[0].handle.operation, operation(1));
        assert_ne!(rows[0].handle, rows[1].handle);
        if shared {
            assert_eq!(rows[0].resource, Resource::Created(parent));
            assert_eq!(rows[0].resource, rows[1].resource);
        } else {
            assert_ne!(rows[0].resource, rows[1].resource);
            assert_eq!(
                rows[0].kind,
                ResourceKind::Half {
                    stream: Resource::Created(parent)
                }
            );
        }
        assert_eq!(
            log.host_created(operation(1), &halves, &discoveries)
                .unwrap(),
            rows
        );
        let mut changed = discoveries.clone();
        changed.insert(92, (Key(42), Kind::File));
        assert!(log.host_created(operation(1), &halves, &changed).is_err());
        assert_eq!(log.host_bindings[&rows[1].handle], rows[1]);
        // Reusing a creating operation for a new native handle cannot replace
        // the earlier field identity, even if the resource would alias.
        assert!(
            log.host_created(
                operation(1),
                &Ok(Completion::Handle(Ok(93))),
                &BTreeMap::from([(93, (Key(40), Kind::Other))])
            )
            .is_err()
        );
        assert!(!log.host_handles.contains_key(&93));
        assert!(
            log.host_created(
                operation(2),
                &Ok(Completion::Halves(Ok((94, 94)))),
                &BTreeMap::from([(94, (Key(44), Kind::Other))])
            )
            .is_err()
        );
        assert!(!log.host_handles.contains_key(&94));
    }
    let mut log = super::super::Log::default();
    let halves = Ok(Completion::Halves(Ok((91, 92))));
    assert!(
        log.host_created(
            operation(0),
            &halves,
            &BTreeMap::from([
                (91, (Key(41), Kind::Other)),
                (92, (Key(42), Kind::Half { stream: Key(99) })),
            ])
        )
        .is_err()
    );
    assert!(log.host_handles.is_empty());
    assert!(log.host_bindings.is_empty());
    assert!(log.host_resources.resource(Key(41)).is_err());
}

#[test]
fn real_host_creation_discovery_keeps_alias_resources_after_close() {
    let root = tempfile::tempdir().unwrap();
    let real = crate::io_real::RealIo::grant(root.path()).unwrap();
    let io = real
        .into_concurrent()
        .unwrap_or_else(|_| panic!("built-in Send adapter"));
    let mut log = super::super::Log::default();
    let mut ordinal = 0;
    let mut create = |request| {
        let mut result = io.call(request);
        let mut discovered = BTreeMap::new();
        for word in crate::io_observation::fields(&mut result)
            .unwrap()
            .into_iter()
            .flatten()
        {
            discovered.insert(
                *word,
                io.identity(*word)
                    .expect("adapter completed live discovery"),
            );
        }
        let rows = log
            .host_created(operation(ordinal), &result, &discovered)
            .unwrap();
        ordinal += 1;
        (result, rows)
    };
    let (directory, _) = create(Request::Directory {
        io: 1,
        path: String::new(),
    });
    let directory = handle(&directory);
    let (file, file_rows) = create(Request::Create {
        directory,
        path: "data".into(),
    });
    let file = handle(&file);
    let (alias, alias_rows) = create(Request::Open {
        directory,
        path: "data".into(),
    });
    let alias = handle(&alias);
    assert_ne!(file_rows[0].handle, alias_rows[0].handle);
    assert_eq!(file_rows[0].resource, alias_rows[0].resource);
    let request = Request::FileWrite {
        directory,
        file,
        bytes: vec![0, 255, 7],
    };
    let keys = log.host_resources.project(&io.keys(&request)).unwrap();
    assert_eq!(
        keys,
        [
            (Resource::Namespace, Mode::Shared),
            (file_rows[0].resource.clone(), Mode::Exclusive)
        ]
    );
    assert_eq!(io.call(request), Ok(Completion::Size(Ok(3))));
    assert_eq!(
        io.call(Request::Close { handle: file }),
        Ok(Completion::Unit(Ok(())))
    );
    assert_eq!(
        io.call(Request::Close { handle: alias }),
        Ok(Completion::Unit(Ok(())))
    );
    assert_eq!(
        std::fs::read(root.path().join("data")).unwrap(),
        [0, 255, 7]
    );
    // No adapter lookup is needed to project a late observation after close.
    let observed = super::super::host_values::project(&Ok(Completion::Handle(Ok(file))), |word| {
        log.host_handles.get(&word).cloned()
    })
    .unwrap();
    assert_eq!(observed.handles, [file_rows[0].handle.clone()]);
    let closed = log
        .host_resources
        .project(&io.keys(&Request::Close { handle: alias }))
        .unwrap();
    assert!(closed.contains(&(file_rows[0].resource.clone(), Mode::Exclusive)));
}
