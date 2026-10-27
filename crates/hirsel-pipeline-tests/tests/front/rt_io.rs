//! hirsel-rt's io tests whose recordings the interpreter also replays (`rt_replay`), beside
//! the rest of them in `crates/hirsel-rt/tests/it/io.rs`.
use crate::rt_replay::replay_interpreter;
use crate::rt_support;
use crate::rt_support::filesystem::*;
use crate::rt_support::library::{Io, Library};
use hirsel_rt::{io::*, io_real::RealIo, io_sim::SimIo};
use std::collections::BTreeMap;
#[test]
fn disposable_filesystem_tape_replays_exact_outputs_partial_writes_tree_and_handles() {
    let (_temp, real) = setup();
    let mut real = Library(real);
    actions(&mut real);
    let tape = real.0.tape().to_vec();
    let mut sim = SimIo::new(initial(), tape.clone());
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(crate::rt_support::library::delivered(&event.completion))
        );
    }
    sim.finish().unwrap();
    assert_eq!(sim.stdout(), real.0.stdout());
    assert_eq!(sim.stdout(), "α\n".as_bytes());
    assert_eq!(sim.tree(), &real.0.snapshot().unwrap());
    assert_eq!(sim.handles(), real.0.handles());
    assert_eq!(sim.tree().files["old"], b"ab");
    assert_eq!(sim.tree().files["saved"], b"firstse");
    replay_interpreter(
        &tape,
        &initial(),
        sim.tree(),
        &sim.handles().observed(),
        sim.stdout(),
    );
}

#[test]
fn real_output_failure_records_and_replays_the_actual_committed_prefix() {
    struct Limited {
        remaining: usize,
    }
    impl std::io::Write for Limited {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.remaining == 0 {
                return Err(std::io::Error::other("injected sink failure"));
            }
            let n = self.remaining.min(bytes.len());
            self.remaining -= n;
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let temp = Disposable::new();
    let mut real =
        Library(RealIo::grant_with_output(temp.path(), Limited { remaining: 1 }).unwrap());
    let out = real.stdout(Io(1)).unwrap().unwrap();
    assert_eq!(real.print(out, "αb"), Ok(Err(IoError::Fault(1))));
    assert_eq!(real.0.stdout(), &[0xce]);
    let mut sim = SimIo::new(Tree::new(BTreeMap::new()).unwrap(), real.0.tape().to_vec());
    for e in real.0.tape() {
        assert_eq!(
            sim.call(e.request.clone()),
            Ok(crate::rt_support::library::delivered(&e.completion))
        );
    }
    assert_eq!(sim.stdout(), real.0.stdout());
    sim.finish().unwrap();
    replay_interpreter(
        real.0.tape(),
        &Tree::new(BTreeMap::new()).unwrap(),
        sim.tree(),
        &sim.handles().observed(),
        sim.stdout(),
    );
}

#[test]
fn hirsel_abi_leaf_exports_consume_taken_bytes_and_handles_on_every_result() {
    use std::{cell::RefCell, rc::Rc};
    struct Shared(Rc<RefCell<RealIo>>);
    impl Adapter for Shared {
        fn preflight(
            &self,
            request: &hirsel_rt::io::Request,
        ) -> Result<Option<hirsel_rt::io::Completion>, AdapterError> {
            self.0.borrow().preflight(request)
        }
        fn call(&mut self, r: Request) -> Result<Completion, AdapterError> {
            self.0.borrow_mut().call(r)
        }
    }
    let (temp, real) = setup();
    let real = Rc::new(RefCell::new(real));
    let ctx = rt_support::Ctx::diagnostic();
    let mut binding =
        Box::new(Binding::new(Shared(real.clone())).with_arguments(vec!["old".into()]));
    unsafe { binding.bind(&mut *ctx.0) };
    let root = binding.intern_path(".");
    let old = binding.intern_path("old");
    let subpath = binding.intern_path("sub");
    let mut result = HandleResult::default();
    let mut unit = UnitResult::of(Ok(()));
    unsafe {
        assert_eq!(
            hirsel_rt_v2_io_dir(ctx.0, std::ptr::null_mut(), &mut result, &1, &root),
            0
        );
        assert_eq!(result.tag, 0);
        let dir = result.value;
        hirsel_rt_v2_dir_narrow(ctx.0, std::ptr::null_mut(), &mut result, &dir, &subpath);
        let sub = result.value;
        hirsel_rt_v2_file_open(ctx.0, std::ptr::null_mut(), &mut result, &dir, &old);
        let mut file = result.value;
        let original = file;
        assert_eq!(
            hirsel_rt_v2_file_narrow(ctx.0, std::ptr::null_mut(), &mut result, &sub, &mut file),
            0
        );
        assert_eq!((result.tag, file), (0, 0));
        assert!(!real.borrow().handles().observed().contains_key(&original));
        hirsel_rt_v2_file_size(ctx.0, std::ptr::null_mut(), &mut result, &dir, &old);
        assert_eq!(result, HandleResult::of(Ok(3)));
        for fault in [
            None,
            Some(Fault {
                error: IoError::Fault(71),
                partial: 2,
            }),
            Some(Fault {
                error: IoError::Canceled(1),
                partial: 9,
            }),
        ] {
            if let Some(fault) = fault {
                real.borrow_mut().inject(fault);
            }
            let (owner, start) = byte_block(&ctx, b"prefixabcdefTAIL");
            assert_eq!(
                hirsel_rt_v2_file_write_bytes(
                    ctx.0,
                    std::ptr::null_mut(),
                    &mut unit,
                    &dir,
                    &old,
                    owner,
                    start + 6,
                    6
                ),
                0
            );
            assert_eq!(ctx.stats().live_blocks, 0);
        }
        assert_eq!(std::fs::read(temp.path().join("old")).unwrap(), b"ab");
        let dst = binding.intern_path("saved");
        hirsel_rt_v2_file_create_atomic(ctx.0, std::ptr::null_mut(), &mut result, &dir, &dst);
        let mut atomic = result.value;
        let (owner, start) = byte_block(&ctx, b"first");
        assert_eq!(
            hirsel_rt_v2_file_write_all(
                ctx.0,
                std::ptr::null_mut(),
                &mut unit,
                &dir,
                &mut atomic,
                owner,
                start,
                5
            ),
            0
        );
        assert_eq!((*owner).count, 1);
        hirsel_rt::release::hirsel_rt_v2_release(
            ctx.0,
            owner,
            (*owner).index(),
            std::ptr::null(),
            0,
            0,
        );
        real.borrow_mut().inject(Fault {
            error: IoError::Canceled(2),
            partial: 99,
        });
        assert_eq!(
            hirsel_rt_v2_file_commit(ctx.0, std::ptr::null_mut(), &mut unit, &dir, &mut atomic),
            0
        );
        assert_eq!(atomic, 0);
        assert_eq!(unit, UnitResult::of(Err(IoError::Canceled(2))));
        assert!(!temp.path().join("saved").exists());
        hirsel_rt_v2_file_remove(ctx.0, std::ptr::null_mut(), &mut unit, &dir, &old);
        assert_eq!(unit, UnitResult::of(Ok(())));
        hirsel_rt_v2_arg_path(
            ctx.0,
            std::ptr::null_mut(),
            &mut result,
            &hirsel_rt::launch::ARGS_HANDLE,
            0,
        );
        assert_eq!(result, HandleResult::of(Ok(old)));
        (*ctx.0).bind_io_data(std::ptr::null_mut());
    }
    let tape = real.borrow().tape().to_vec();
    let mut sim = SimIo::new(initial(), tape.clone());
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(crate::rt_support::library::delivered(&event.completion))
        );
    }
    sim.finish().unwrap();
    replay_interpreter(
        &tape,
        &initial(),
        sim.tree(),
        &sim.handles().observed(),
        sim.stdout(),
    );
}

fn byte_block(ctx: &rt_support::Ctx, bytes: &[u8]) -> (*mut hirsel_rt::Header, u64) {
    // A whole slice of fresh byte storage: its owner and start.
    let slice = unsafe { hirsel_rt::text::allocate_bytes(ctx.0, bytes) };
    (slice.owner, slice.start)
}

#[test]
fn walk_yields_symlinks_without_descending_and_replays_directory_pages() {
    use std::os::unix::fs::symlink;
    let (temp, real) = setup();
    symlink("sub", temp.path().join("alias")).unwrap();
    symlink("../old", temp.path().join("sub/link")).unwrap();
    symlink("alias/../old", temp.path().join("chain")).unwrap();
    let mut real = Library(real);
    let root = real.directory(Io(1), "").unwrap().unwrap();
    let expected = vec!["alias", "chain", "old", "sub/a", "sub/link", "sub/é"];
    assert_eq!(
        real.walk(root),
        Ok(Ok(expected.iter().map(|p| (*p).into()).collect()))
    );
    for p in ["alias/link", "chain", "sub/link"] {
        assert_eq!(real.read(root, p), Ok(Ok(b"old".to_vec())));
    }
    // test.dir has only (Path, Bytes) entries. Snapshot the resolved view into
    // that existing format; both models must return the same alias paths/bytes.
    let tree = real.0.snapshot().unwrap();
    let mut sim = SimIo::new(tree.clone(), real.0.tape().to_vec());
    for e in real.0.tape() {
        assert_eq!(
            sim.call(e.request.clone()),
            Ok(crate::rt_support::library::delivered(&e.completion))
        );
    }
    sim.finish().unwrap();
    replay_interpreter(
        real.0.tape(),
        &tree,
        sim.tree(),
        &sim.handles().observed(),
        sim.stdout(),
    );
}

#[test]
fn write_through_directory_alias_then_read_target_replays_the_same_bytes() {
    use std::os::unix::fs::symlink;
    let (temp, real) = setup();
    symlink("sub", temp.path().join("alias")).unwrap();
    let mut real = Library(real);
    let tree = real.0.snapshot().unwrap();
    let root = real.directory(Io(1), "").unwrap().unwrap();
    real.write(root, "alias/a", b"B".to_vec()).unwrap().unwrap();
    assert_eq!(real.read(root, "sub/a"), Ok(Ok(b"B".to_vec())));
    let linked = real.narrow_dir(root, "alias").unwrap().unwrap();
    assert_eq!(real.read(linked, "a"), Ok(Ok(b"B".to_vec())));
    let mut atomic = real.create_atomic(linked, "saved").unwrap().unwrap();
    real.0.inject(Fault {
        error: IoError::Fault(71),
        partial: 2,
    });
    assert_eq!(
        real.write_all(linked, &mut atomic, b"data"),
        Ok(Err(IoError::Fault(71)))
    );
    real.commit(linked, atomic).unwrap().unwrap();
    assert_eq!(real.read(root, "sub/saved"), Ok(Ok(b"da".to_vec())));
    let abandoned = real.create_atomic(linked, "abandoned").unwrap().unwrap();
    real.abort(abandoned).unwrap().unwrap();
    let mut sim = SimIo::new(tree.clone(), real.0.tape().to_vec());
    for event in real.0.tape() {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(crate::rt_support::library::delivered(&event.completion))
        );
    }
    sim.finish().unwrap();
    assert_eq!(sim.tree(), &real.0.snapshot().unwrap());
    replay_interpreter(
        real.0.tape(),
        &tree,
        sim.tree(),
        &sim.handles().observed(),
        sim.stdout(),
    );
}

#[test]
fn exclusive_temporaries_skip_poisoned_links_and_commit_the_prepared_regular_inode() {
    use std::os::unix::fs::{MetadataExt, symlink};
    for poison in [false, true] {
        let (temp, io) = setup();
        if poison {
            symlink("victim", temp.path().join("saved.hirsel-tmp-3")).unwrap();
        }
        let mut real = Library(io);
        let initial = real.0.snapshot().unwrap();
        let root = real.directory(Io(1), "").unwrap().unwrap();
        let mut atomic = real.create_atomic(root, "saved").unwrap().unwrap();
        let name = format!("saved.hirsel-tmp-{}", if poison { 4 } else { 3 });
        let before = std::fs::symlink_metadata(temp.path().join(&name)).unwrap();
        assert!(before.is_file());
        real.write_all(root, &mut atomic, b"prepared")
            .unwrap()
            .unwrap();
        real.commit(root, atomic).unwrap().unwrap();
        let after = std::fs::symlink_metadata(temp.path().join("saved")).unwrap();
        assert!(after.is_file());
        assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
        assert_eq!(
            std::fs::read(temp.path().join("saved")).unwrap(),
            b"prepared"
        );
        assert!(!temp.path().join("victim").exists());
        if poison {
            assert!(
                std::fs::symlink_metadata(temp.path().join("saved.hirsel-tmp-3"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
        let mut sim = SimIo::new(initial.clone(), real.0.tape().to_vec());
        for event in real.0.tape() {
            assert_eq!(
                sim.call(event.request.clone()),
                Ok(crate::rt_support::library::delivered(&event.completion))
            );
        }
        sim.finish().unwrap();
        assert_eq!(sim.tree(), &real.0.snapshot().unwrap());
        replay_interpreter(
            real.0.tape(),
            &initial,
            sim.tree(),
            &sim.handles().observed(),
            sim.stdout(),
        );
    }
}

#[test]
fn snapshots_preserve_links_and_invalid_names_without_refusing_admission() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let (temp, io) = setup();
    std::fs::write(
        temp.path().join(std::ffi::OsString::from_vec(vec![0xff])),
        b"invalid",
    )
    .unwrap();
    symlink("../outside", temp.path().join("escape")).unwrap();
    let mut real = Library(io);
    let tree = real.0.snapshot().unwrap();
    assert_eq!(tree.links["escape"], "../outside");
    let root = real.directory(Io(1), "").unwrap().unwrap();
    assert_eq!(real.walk(root), Ok(Err(IoError::Fault(1))));
    assert_eq!(real.read(root, "escape"), Ok(Err(IoError::Fault(1))));
    assert_eq!(real.read(root, "old"), Ok(Ok(b"old".to_vec())));
    let mut sim = SimIo::new(tree.clone(), real.0.tape().to_vec());
    for event in real.0.tape() {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(crate::rt_support::library::delivered(&event.completion))
        );
    }
    sim.finish().unwrap();
    replay_interpreter(
        real.0.tape(),
        &tree,
        sim.tree(),
        &sim.handles().observed(),
        sim.stdout(),
    );
}

#[test]
fn opened_identity_matches_disk_and_replays_live_unlink_identical_replacement_and_aba() {
    for mutation in [
        "live", "unlink", "replace", "recreate", "partial", "zero", "canceled", "empty",
    ] {
        let (_temp, io) = setup();
        let mut real = Library(io);
        let dir = real.directory(Io(1), ".").unwrap().unwrap();
        let alias = real.directory(Io(1), ".").unwrap().unwrap();
        let first = real.open(dir, "old").unwrap().unwrap();
        let second = real.open(alias, "old").unwrap().unwrap();
        let transferred = real.narrow_file(alias, first).unwrap().unwrap();
        if matches!(mutation, "unlink" | "recreate") {
            real.remove(alias, "old").unwrap().unwrap();
        }
        if matches!(
            mutation,
            "replace" | "recreate" | "partial" | "zero" | "canceled" | "empty"
        ) {
            let fault = match mutation {
                "partial" => Some(Fault {
                    error: IoError::Fault(71),
                    partial: 3,
                }),
                "zero" => Some(Fault {
                    error: IoError::Fault(71),
                    partial: 0,
                }),
                "canceled" => Some(Fault {
                    error: IoError::Canceled(0),
                    partial: 3,
                }),
                _ => None,
            };
            if let Some(fault) = fault {
                real.0.inject(fault);
            }
            let result = real
                .write(
                    alias,
                    "old",
                    if mutation == "empty" {
                        vec![]
                    } else {
                        b"old".to_vec()
                    },
                )
                .unwrap();
            assert_eq!(
                result.is_err(),
                matches!(mutation, "partial" | "zero" | "canceled")
            );
        }
        let survives = matches!(mutation, "live" | "zero" | "canceled");
        for file in [transferred, second] {
            let result = real.narrow_file(dir, file).unwrap();
            assert_eq!(result.is_some(), survives, "{mutation}");
            if let Some(file) = result {
                real.close_file(file).unwrap().unwrap();
            }
        }
        let tape = identity_tape(real.0.tape());
        let mut sim = SimIo::new(initial(), tape.clone());
        for event in &tape {
            assert_eq!(
                sim.call(event.request.clone()),
                Ok(crate::rt_support::library::delivered(&event.completion)),
                "{mutation}"
            );
        }
        sim.finish().unwrap();
        assert_eq!(sim.handles(), real.0.handles());
        assert_eq!(sim.tree(), &real.0.snapshot().unwrap());
        replay_interpreter(
            &tape,
            &initial(),
            sim.tree(),
            &sim.handles().observed(),
            &[],
        );
    }
}

#[test]
fn atomic_append_preserves_opened_identity_until_publication_and_replays_the_transfer() {
    let (_temp, io) = setup();
    let mut real = Library(io);
    let dir = real.directory(Io(1), ".").unwrap().unwrap();
    let old = real.open(dir, "old").unwrap().unwrap();
    let mut atomic = real.create_atomic(dir, "old").unwrap().unwrap();
    let hirsel_rt::handles::Grant::AtomicFile { temporary, .. } =
        real.0.handles().observed()[&atomic.0].clone()
    else {
        panic!()
    };
    let prepared = real.open(dir, &temporary).unwrap().unwrap();
    real.0.inject(Fault {
        error: IoError::Fault(71),
        partial: 2,
    });
    assert_eq!(
        real.write_all(dir, &mut atomic, b"old"),
        Ok(Err(IoError::Fault(71)))
    );
    let prepared = real.narrow_file(dir, prepared).unwrap().unwrap();
    real.close_file(prepared).unwrap().unwrap();
    real.write_all(dir, &mut atomic, b"d").unwrap().unwrap();
    let old = real.narrow_file(dir, old).unwrap().unwrap();
    real.commit(dir, atomic).unwrap().unwrap();
    assert_eq!(real.narrow_file(dir, old), Ok(None));
    let fresh = real.open(dir, "old").unwrap().unwrap();
    let fresh = real.narrow_file(dir, fresh).unwrap().unwrap();
    real.close_file(fresh).unwrap().unwrap();
    let tape = identity_tape(real.0.tape());
    let mut sim = SimIo::new(initial(), tape.clone());
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(crate::rt_support::library::delivered(&event.completion))
        );
    }
    sim.finish().unwrap();
    assert_eq!(sim.tree(), &initial());
    assert_eq!(sim.handles(), real.0.handles());
    replay_interpreter(
        &tape,
        &initial(),
        sim.tree(),
        &sim.handles().observed(),
        &[],
    );
}

#[test]
fn opened_aliases_compare_resolved_identity_after_identical_target_and_leaf_replacement() {
    use std::os::unix::fs::symlink;
    let (temp, io) = setup();
    symlink("sub", temp.path().join("alias")).unwrap();
    symlink("old", temp.path().join("leaf")).unwrap();
    let mut real = Library(io);
    let initial = real.0.snapshot().unwrap();
    let root = real.directory(Io(1), ".").unwrap().unwrap();
    let alias = real.narrow_dir(root, "alias").unwrap().unwrap();
    let via_alias = real.open(root, "alias/a").unwrap().unwrap();
    let via_alias = real.narrow_file(root, via_alias).unwrap().unwrap();
    real.write(alias, "a", b"A".to_vec()).unwrap().unwrap();
    assert_eq!(real.narrow_file(root, via_alias), Ok(None));
    let leaf = real.open(root, "leaf").unwrap().unwrap();
    real.write(root, "leaf", b"old".to_vec()).unwrap().unwrap();
    assert_eq!(real.narrow_file(root, leaf), Ok(None));
    let tape = identity_tape(real.0.tape());
    let mut sim = SimIo::new(initial.clone(), tape.clone());
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(crate::rt_support::library::delivered(&event.completion))
        );
    }
    sim.finish().unwrap();
    assert_eq!(sim.handles(), real.0.handles());
    assert_eq!(sim.tree(), &real.0.snapshot().unwrap());
    replay_interpreter(&tape, &initial, sim.tree(), &sim.handles().observed(), &[]);
}

#[test]
fn atomic_commit_rejects_identical_replacement_of_its_prepared_temporary_in_each_tree() {
    for fake in [false, true] {
        let (_temp, mut io) = setup();
        let dir = if fake {
            io.test_context(7);
            let Completion::Handle(Ok(id)) = io
                .call(Request::TestDir {
                    test: 1,
                    entries: vec![],
                })
                .unwrap()
            else {
                panic!()
            };
            rt_support::library::Dir(id)
        } else {
            let Completion::Handle(Ok(id)) = io
                .call(Request::Directory {
                    io: 1,
                    path: ".".into(),
                })
                .unwrap()
            else {
                panic!()
            };
            rt_support::library::Dir(id)
        };
        let mut real = Library(io);
        let mut atomic = real.create_atomic(dir, "saved").unwrap().unwrap();
        real.write_all(dir, &mut atomic, b"data").unwrap().unwrap();
        let hirsel_rt::handles::Grant::AtomicFile { temporary, .. } =
            real.0.handles().observed()[&atomic.0].clone()
        else {
            panic!()
        };
        real.write(dir, &temporary, b"data".to_vec())
            .unwrap()
            .unwrap();
        assert_eq!(real.commit(dir, atomic), Ok(Err(IoError::Fault(1))));
        assert_eq!(real.read(dir, "saved"), Ok(Err(IoError::NotFound)));
        let mut tape = identity_tape(real.0.tape());
        // This refusal also follows from the prepared identity, without a host fault input.
        for event in &mut tape {
            if matches!(event.request, Request::Commit { .. }) {
                event.fault = None;
            }
        }
        let mut sim = SimIo::new(initial(), tape.clone());
        if fake {
            sim.test_context(7);
        }
        for event in &tape {
            assert_eq!(
                sim.call(event.request.clone()),
                Ok(crate::rt_support::library::delivered(&event.completion))
            );
        }
        sim.finish().unwrap();
        assert_eq!(sim.handles(), real.0.handles());
        assert_eq!(sim.fake_trees(), real.0.fake_trees());
        if !fake {
            replay_interpreter(
                &tape,
                &initial(),
                sim.tree(),
                &sim.handles().observed(),
                &[],
            );
        }
    }
}

#[test]
fn file_stream_records_pinned_reads_seek_append_prefixes_and_replays_without_host() {
    let (temp, mut real) = setup();
    let initial = initial();
    let directory = match real
        .call(Request::Directory {
            io: 1,
            path: "".into(),
        })
        .unwrap()
    {
        Completion::Handle(Ok(id)) => id,
        other => panic!("{other:?}"),
    };
    let file = match real
        .call(Request::Open {
            directory,
            path: "old".into(),
        })
        .unwrap()
    {
        Completion::Handle(Ok(id)) => id,
        other => panic!("{other:?}"),
    };
    let read = |max| Request::FileRead {
        directory,
        file,
        max,
    };
    assert_eq!(
        real.call(read(2)),
        Ok(Completion::Bytes(Ok(b"ol".to_vec())))
    );
    let seek = Request::FileSeek {
        directory,
        file,
        offset: 1,
    };
    assert_eq!(real.call(seek), Ok(Completion::Unit(Ok(()))));
    real.inject(Fault {
        error: IoError::Canceled(0),
        partial: 99,
    });
    assert_eq!(
        real.call(read(2)),
        Ok(Completion::Bytes(Err(IoError::Canceled(0))))
    );
    real.call(Request::Write {
        directory,
        path: "old".into(),
        bytes: b"replacement".to_vec(),
    })
    .unwrap();
    assert_eq!(
        real.call(read(8)),
        Ok(Completion::Bytes(Ok(b"ld".to_vec())))
    );
    assert_eq!(real.call(read(8)), Ok(Completion::Bytes(Ok(vec![]))));
    let writer = match real
        .call(Request::Create {
            directory,
            path: "saved".into(),
        })
        .unwrap()
    {
        Completion::Handle(Ok(id)) => id,
        other => panic!("{other:?}"),
    };
    real.inject(Fault {
        error: IoError::Fault(71),
        partial: 2,
    });
    assert_eq!(
        real.call(Request::FileAppend {
            directory,
            file: writer,
            bytes: b"first".to_vec()
        }),
        Ok(Completion::Unit(Err(IoError::Fault(71))))
    );
    assert_eq!(
        real.call(Request::FileWrite {
            directory,
            file: writer,
            bytes: b"nal".to_vec()
        }),
        Ok(Completion::Size(Ok(3)))
    );
    let reader = match real
        .call(Request::Open {
            directory,
            path: "saved".into(),
        })
        .unwrap()
    {
        Completion::Handle(Ok(id)) => id,
        other => panic!("{other:?}"),
    };
    // Truncation changes the same inode, including another already-open reader.
    let trunc = match real
        .call(Request::Create {
            directory,
            path: "saved".into(),
        })
        .unwrap()
    {
        Completion::Handle(Ok(id)) => id,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        real.call(Request::FileRead {
            directory,
            file: reader,
            max: 9
        }),
        Ok(Completion::Bytes(Ok(vec![])))
    );
    real.call(Request::FileAppend {
        directory,
        file: trunc,
        bytes: b"new".to_vec(),
    })
    .unwrap();
    for handle in [file, writer, reader, trunc] {
        real.call(Request::Close { handle }).unwrap();
    }
    let tree = real.snapshot().unwrap();
    assert_eq!(tree.files["saved"], b"new");
    let tape = real.tape().to_vec();
    assert_eq!(rt_support::tape::events(&tape), tape);
    let mut sim = SimIo::new(initial.clone(), tape.clone());
    std::fs::remove_file(temp.path().join("saved")).unwrap();
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(crate::rt_support::library::delivered(&event.completion))
        );
    }
    sim.finish().unwrap();
    assert_eq!(sim.tree(), &tree);
    replay_interpreter(
        &tape,
        &initial,
        &tree,
        &real.handles().observed(),
        real.stdout(),
    );
    let mut forged = tape.clone();
    let event = forged
        .iter_mut()
        .find(|e| {
            matches!(e.request, Request::FileRead { .. })
                && matches!(
                    crate::rt_support::library::delivered(&e.completion),
                    Completion::Bytes(Ok(_))
                )
        })
        .unwrap();
    event.completion = Completion::Bytes(Ok(b"XX".to_vec()));
    let mut sim = SimIo::new(initial, forged.clone());
    for event in &forged {
        let position = sim.position();
        if matches!(event.request, Request::FileRead { .. }) {
            assert_eq!(
                sim.call(event.request.clone()),
                Err(AdapterError::ReplayMismatch(position))
            );
            assert_eq!(sim.position(), position);
            break;
        }
        sim.call(event.request.clone()).unwrap();
    }
}

#[test]
fn portable_file_errors_round_trip_provider_faults_tapes_and_result_abi() {
    let (_temp, mut real) = setup();
    let directory = match real
        .call(Request::Directory {
            io: 1,
            path: "".into(),
        })
        .unwrap()
    {
        Completion::Handle(Ok(id)) => id,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        real.call(Request::Open {
            directory,
            path: "sub".into()
        }),
        Ok(Completion::Handle(Err(IoError::IsDirectory)))
    );
    assert_eq!(
        real.call(Request::NarrowDir {
            directory,
            path: "old".into()
        }),
        Ok(Completion::Handle(Err(IoError::NotDirectory)))
    );
    assert_eq!(
        real.call(Request::Open {
            directory,
            path: "old/child".into()
        }),
        Ok(Completion::Handle(Err(IoError::NotDirectory)))
    );
    assert_eq!(
        real.call(Request::Read {
            directory,
            path: "missing".into()
        }),
        Ok(Completion::Bytes(Err(IoError::NotFound)))
    );
    for (ordinal, error) in [
        (3, IoError::NotFound),
        (4, IoError::PermissionDenied),
        (5, IoError::AlreadyExists),
        (6, IoError::IsDirectory),
        (7, IoError::NotDirectory),
        (8, IoError::DirectoryNotEmpty),
        (9, IoError::CrossDevice),
    ] {
        assert_eq!(
            UnitResult::of(Err(error.clone())),
            UnitResult {
                tag: 1,
                error: ordinal,
                code: 0
            }
        );
        assert_eq!(
            HandleResult::of(Err(error.clone())),
            HandleResult {
                tag: 1,
                padding: 0,
                value: u64::from(ordinal)
            }
        );
        real.inject(Fault {
            error: error.clone(),
            partial: 0,
        });
        assert_eq!(
            real.call(Request::Read {
                directory,
                path: "old".into()
            }),
            Ok(Completion::Bytes(Err(error)))
        );
    }
    let writer = match real
        .call(Request::Create {
            directory,
            path: "saved".into(),
        })
        .unwrap()
    {
        Completion::Handle(Ok(id)) => id,
        other => panic!("{other:?}"),
    };
    real.inject(Fault {
        error: IoError::PermissionDenied,
        partial: 2,
    });
    assert_eq!(
        real.call(Request::FileAppend {
            directory,
            file: writer,
            bytes: b"abcd".to_vec()
        }),
        Ok(Completion::Unit(Err(IoError::PermissionDenied)))
    );
    real.call(Request::Close { handle: writer }).unwrap();
    let final_tree = real.snapshot().unwrap();
    assert_eq!(final_tree.files["saved"], b"ab");
    let tape = real.tape().to_vec();
    assert_eq!(rt_support::tape::events(&tape), tape);
    let mut sim = SimIo::new(initial(), tape.clone());
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(crate::rt_support::library::delivered(&event.completion))
        );
    }
    sim.finish().unwrap();
    assert_eq!(sim.tree(), &final_tree);
    replay_interpreter(
        &tape,
        &initial(),
        &final_tree,
        &real.handles().observed(),
        real.stdout(),
    );
}

// OS-created links and inode permission state cannot be supplied by test.dir.
// Rust std remove_dir_all's outside-link and nested vectors, plus prefix replay.
#[test]
fn filesystem_breadth_confines_symlinks_and_replays_partial_removal() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    let root = Disposable::new();
    let outside = Disposable::new();
    std::fs::write(outside.path().join("keep"), b"outside").unwrap();
    std::fs::create_dir_all(root.path().join("tree/deep")).unwrap();
    std::fs::write(root.path().join("tree/a"), b"a").unwrap();
    std::fs::write(root.path().join("tree/deep/b"), b"b").unwrap();
    symlink(outside.path(), root.path().join("escape")).unwrap();
    symlink(outside.path(), root.path().join("tree/escape")).unwrap();
    symlink("..", root.path().join("tree/parent")).unwrap();
    let mut host = RealIo::grant(root.path()).unwrap();
    let mut initial = host.snapshot().unwrap();
    // The independent model receives this filesystem input explicitly. Ordinary
    // content snapshots intentionally omit timestamps for readable regular files.
    let metadata = std::fs::metadata(root.path().join("tree/a")).unwrap();
    initial.metadata.insert(
        "tree/a".into(),
        Metadata {
            kind: FileKind::Regular,
            size: metadata.len(),
            modified_unix_ns: Some(
                i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec()),
            ),
            permissions: metadata.mode() & 0o777,
        },
    );
    let Completion::Handle(Ok(dir)) = host
        .call(Request::Directory {
            io: 1,
            path: "".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    let Completion::Handle(Ok(nested)) = host
        .call(Request::NarrowDir {
            directory: dir,
            path: "tree/deep".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        host.call(Request::SetPermissions {
            directory: dir,
            path: "tree/a".into(),
            permissions: 0o640
        })
        .unwrap(),
        Completion::Unit(Ok(()))
    );
    let Completion::Metadata(Ok(meta)) = host
        .call(Request::Metadata {
            directory: dir,
            path: "tree/a".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(meta.permissions, 0o640);
    assert_eq!(meta.size, 1);
    assert!(meta.modified_unix_ns.is_some());
    let outside_mode = std::fs::metadata(outside.path())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        host.call(Request::SetPermissions {
            directory: dir,
            path: "escape".into(),
            permissions: 0
        })
        .unwrap(),
        Completion::Unit(Err(IoError::Fault(1)))
    );
    assert_eq!(
        std::fs::metadata(outside.path())
            .unwrap()
            .permissions()
            .mode(),
        outside_mode
    );
    assert_eq!(
        host.call(Request::RemoveAll {
            directory: dir,
            path: "escape/keep".into()
        })
        .unwrap(),
        Completion::Unit(Err(IoError::Fault(1)))
    );
    assert_eq!(
        host.call(Request::RemoveAll {
            directory: dir,
            path: "escape".into()
        })
        .unwrap(),
        Completion::Unit(Ok(()))
    );
    assert!(!root.path().join("escape").exists());
    assert_eq!(
        std::fs::read(outside.path().join("keep")).unwrap(),
        b"outside"
    );
    host.inject(Fault {
        error: IoError::Canceled(0),
        partial: 1,
    });
    assert_eq!(
        host.call(Request::RemoveAll {
            directory: dir,
            path: "tree".into()
        })
        .unwrap(),
        Completion::Unit(Err(IoError::Canceled(0)))
    );
    assert!(root.path().join("tree/a").exists());
    host.inject(Fault {
        error: IoError::PermissionDenied,
        partial: 1,
    });
    assert_eq!(
        host.call(Request::RemoveAll {
            directory: dir,
            path: "tree".into()
        })
        .unwrap(),
        Completion::Unit(Err(IoError::PermissionDenied))
    );
    assert!(!root.path().join("tree/a").exists());
    assert!(root.path().join("tree/deep/b").exists());
    assert_eq!(
        host.call(Request::RemoveAll {
            directory: dir,
            path: "tree".into()
        })
        .unwrap(),
        Completion::Unit(Ok(()))
    );
    assert_eq!(
        host.call(Request::ReadDirPage {
            directory: nested,
            path: "".into(),
            after: String::new(),
            recursive: false
        })
        .unwrap(),
        Completion::Entries(Ok(vec![]))
    );
    assert_eq!(
        host.call(Request::RemoveAll {
            directory: dir,
            path: "tree/..".into()
        })
        .unwrap(),
        Completion::Unit(Err(IoError::Fault(1)))
    );
    assert_eq!(
        std::fs::read(outside.path().join("keep")).unwrap(),
        b"outside"
    );
    let final_tree = host.snapshot().unwrap();
    let handles = host.handles().observed();
    let tape = host.tape().to_vec();
    assert_eq!(rt_support::tape::events(&tape), tape);
    drop(host);
    drop(root);
    drop(outside);
    let mut replay = SimIo::new(initial.clone(), tape.clone());
    for event in &tape {
        assert_eq!(
            replay.call(event.request.clone()).unwrap(),
            crate::rt_support::library::delivered(&event.completion)
        );
    }
    replay.finish().unwrap();
    replay_interpreter(&tape, &initial, &final_tree, &handles, b"");
}

#[test]
fn put_new_and_sync_on_a_host_tree_record_round_trip_as_text_and_replay() {
    let (temp, io) = setup();
    let mut real = Library(io);
    let dir = real.directory(Io(1), "").unwrap().unwrap();
    let put = |path: &str, bytes: &[u8], durable| Request::PutNew {
        directory: dir.0,
        path: path.into(),
        bytes: bytes.to_vec(),
        durable,
    };
    let unit = |result| Ok(Completion::Unit(result));
    assert_eq!(real.0.call(put("sub/new", b"new", true)), unit(Ok(())));
    assert_eq!(real.0.call(put("sub/new", b"new", false)), unit(Ok(())));
    assert_eq!(real.0.call(put("sub/volatile", b"v", false)), unit(Ok(())));
    assert_eq!(
        real.0.call(put("sub/new", b"other", true)),
        unit(Err(IoError::AlreadyExists))
    );
    assert_eq!(
        real.0.call(put("sub", b"", true)),
        unit(Err(IoError::AlreadyExists))
    );
    assert_eq!(
        real.0.call(put("missing/x", b"", true)),
        unit(Err(IoError::NotFound))
    );
    // The hidden temporaries are gone: only the objects remain beside the originals.
    let mut names: Vec<_> = std::fs::read_dir(temp.path().join("sub"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["a", "new", "volatile", "é"]);
    assert_eq!(std::fs::read(temp.path().join("sub/new")).unwrap(), b"new");
    let file = real.open(dir, "old").unwrap().unwrap();
    assert_eq!(
        real.0.call(Request::Sync {
            directory: dir.0,
            file: file.0,
        }),
        unit(Ok(()))
    );
    real.close_file(file).unwrap().unwrap();
    let tape = real.0.tape().to_vec();
    assert_eq!(rt_support::tape::events(&tape), tape);
    let mut sim = SimIo::new(initial(), tape.clone());
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(crate::rt_support::library::delivered(&event.completion))
        );
    }
    sim.finish().unwrap();
    assert_eq!(sim.tree(), &real.0.snapshot().unwrap());
    replay_interpreter(
        &tape,
        &initial(),
        sim.tree(),
        &sim.handles().observed(),
        &[],
    );
}
