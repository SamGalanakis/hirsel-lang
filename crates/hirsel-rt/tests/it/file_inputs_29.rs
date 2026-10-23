//! Tape-only host observations preserve exact policy and legacy snapshot checks.
use hirsel_rt::io::{Adapter, AdapterError, Completion as C, Event, FileInput, Request as R, Tree};
use hirsel_rt::io_sim::SimIo;
use std::collections::BTreeMap;

fn setup(request: R, completion: C) -> (SimIo, R) {
    let tree = Tree::new(BTreeMap::from([("data/in".into(), b"old".to_vec())])).unwrap();
    let mut io = SimIo::new(
        tree,
        vec![
            Event {
                request: R::Directory {
                    io: 1,
                    path: "data".into(),
                },
                completion: C::Handle(Ok(2)),
                fault: None,
            },
            Event {
                request: request.clone(),
                completion,
                fault: None,
            },
        ],
    );
    assert_eq!(
        io.call(R::Directory {
            io: 1,
            path: "data".into()
        }),
        Ok(C::Handle(Ok(2)))
    );
    (io, request)
}
#[test]
fn tagged_file_inputs_do_not_relax_requests_policy_handles_or_read_bounds() {
    for (request, completion) in [
        (
            R::Size {
                directory: 999,
                path: "mid".into(),
            },
            C::FileInput(FileInput::Size(5)),
        ),
        (
            R::Size {
                directory: 2,
                path: "../escaped".into(),
            },
            C::FileInput(FileInput::Size(5)),
        ),
        (
            R::FileRead {
                directory: 2,
                file: 999,
                max: 10,
            },
            C::FileInput(FileInput::Bytes(vec![1])),
        ),
        (
            R::FileRead {
                directory: 2,
                file: 999,
                max: 1,
            },
            C::FileInput(FileInput::Bytes(vec![1, 2])),
        ),
        (
            R::Write {
                directory: 2,
                path: "in".into(),
                bytes: b"new".to_vec(),
            },
            C::FileInput(FileInput::Bytes(vec![])),
        ),
    ] {
        let (mut io, request) = setup(request, completion);
        assert!(io.call(request).is_err());
        assert_eq!(io.finish(), Err(AdapterError::UnconsumedInput(1)));
    }
    let (mut io, _) = setup(
        R::Size {
            directory: 2,
            path: "mid".into(),
        },
        C::FileInput(FileInput::Size(5)),
    );
    assert_eq!(
        io.call(R::Size {
            directory: 2,
            path: "changed".into()
        }),
        Err(AdapterError::ReplayMismatch(1))
    );
}
#[test]
fn old_unmarked_file_completions_keep_their_exact_snapshot_checks() {
    for (path, size, succeeds) in [("in", 3, true), ("in", 4, false), ("mid", 5, false)] {
        let (mut io, request) = setup(
            R::Size {
                directory: 2,
                path: path.into(),
            },
            C::Size(Ok(size)),
        );
        if succeeds {
            assert_eq!(io.call(request), Ok(C::Size(Ok(size))));
            io.finish().unwrap();
        } else {
            assert_eq!(io.call(request), Err(AdapterError::ReplayMismatch(1)));
        }
    }
}

#[test]
fn simulated_files_reject_forged_host_inputs() {
    let request = R::TestDir {
        test: 2,
        entries: vec![("in".into(), b"old".to_vec())],
    };
    let size = R::Size {
        directory: 3,
        path: "in".into(),
    };
    let mut io = SimIo::new(
        Tree::default(),
        vec![
            Event {
                request: request.clone(),
                completion: C::Handle(Ok(3)),
                fault: None,
            },
            Event {
                request: size.clone(),
                completion: C::FileInput(FileInput::Size(4)),
                fault: None,
            },
        ],
    );
    assert_eq!(io.grant_test(0), 2);
    assert_eq!(io.call(request), Ok(C::Handle(Ok(3))));
    assert_eq!(io.call(size), Err(AdapterError::InvalidCompletion));
}
