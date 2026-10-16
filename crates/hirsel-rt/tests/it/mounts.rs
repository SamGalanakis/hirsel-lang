//! Mount authority is selected before an effect and retained through narrowed grants.
use crate::support;
use hirsel_rt::{
    handles::Io,
    host_paths::{PinnedDirectory, RecorderDirectory},
    io::{Adapter, AdapterError, Completion as C, Fault, IoError, Request as R, Tree},
    io_real::RealIo,
    io_sim::SimIo,
    operations::Library,
};
use hirsel_run_config::{Access, RunRefusal};
use std::{collections::BTreeMap, fs, path::PathBuf};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "mounts-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        for dir in ["one", "one/sub", "two", "tapes"] {
            fs::create_dir(root.join(dir)).unwrap();
        }
        fs::write(root.join("one/sub/a"), b"A").unwrap();
        fs::write(root.join("two/b"), b"B").unwrap();
        Self(root)
    }
    fn io(&self, mounts: &[(&str, Access, &str)]) -> RealIo {
        RealIo::from_pinned_mounts(
            mounts.iter().map(|(name, access, host)| {
                (
                    name.to_string(),
                    *access,
                    PinnedDirectory::open(&self.0.join(host)).unwrap(),
                )
            }),
            std::io::sink(),
        )
        .unwrap()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn replay(real: &RealIo, initial: Tree, mounts: &BTreeMap<String, Access>, final_tree: &Tree) {
    let mut tape = real.tape().to_vec();
    for event in &mut tape {
        if matches!(
            (&event.request, &event.completion),
            (R::NarrowFile { .. }, C::OptionalFile(None))
        ) {
            // Virtual containment and ordered file generations are derived in
            // both simulations without an injected host refusal.
            event.fault = None;
        }
    }
    let mut sim = SimIo::new(initial.clone(), tape.clone());
    sim.set_mounts(mounts.clone()).unwrap();
    for event in &tape {
        assert_eq!(sim.call(event.request.clone()).unwrap(), event.completion);
    }
    sim.finish().unwrap();
    assert_eq!(sim.tree(), final_tree);
    assert_eq!(sim.handles(), real.handles());
    support::io::replay_interpreter_with_mounts(
        &tape,
        &initial,
        final_tree,
        real.handles().observed(),
        real.stdout(),
        mounts,
    );
}

#[test]
fn selectors_and_read_rights_survive_narrow_file_and_walk_and_replay() {
    let scratch = Scratch::new();
    let mounts = BTreeMap::from([
        ("work".into(), Access::Read),
        ("output".into(), Access::ReadWrite),
    ]);
    let mut real = Library(scratch.io(&[
        ("work", Access::Read, "one"),
        ("output", Access::ReadWrite, "two"),
    ]));
    let initial = real.0.snapshot().unwrap();
    assert_eq!(real.directory(Io(1), ""), Ok(Err(IoError::NotGranted)));
    assert_eq!(
        real.directory(Io(1), "omitted"),
        Ok(Err(IoError::NotGranted))
    );
    assert_eq!(
        real.directory(Io(1), "working"),
        Ok(Err(IoError::NotGranted))
    );
    let work = real.directory(Io(1), "work").unwrap().unwrap();
    let sub = real.narrow_dir(work, "sub").unwrap().unwrap();
    assert_eq!(real.walk(work), Ok(Ok(vec!["sub/a".into()])));
    assert_eq!(real.read(sub, "a"), Ok(Ok(b"A".to_vec())));
    assert_eq!(real.size(sub, "a"), Ok(Ok(1)));
    let file = real.open(work, "sub/a").unwrap().unwrap();
    let file = real.narrow_file(sub, file).unwrap().unwrap();
    real.close_file(file).unwrap().unwrap();
    assert_eq!(
        real.write(sub, "a", b"X".to_vec()),
        Ok(Err(IoError::NotGranted))
    );
    assert!(real.0.tape().last().unwrap().fault.is_none());
    assert_eq!(real.remove(sub, "a"), Ok(Err(IoError::NotGranted)));
    assert!(matches!(
        real.create_atomic(sub, "a"),
        Ok(Err(IoError::NotGranted))
    ));
    let output = real.directory(Io(1), "output").unwrap().unwrap();
    real.write(output, "b", b"new".to_vec()).unwrap().unwrap();
    let mut atomic = real.create_atomic(output, "saved").unwrap().unwrap();
    real.write_all(output, &mut atomic, b"atom")
        .unwrap()
        .unwrap();
    real.commit(output, atomic).unwrap().unwrap();
    real.remove(output, "b").unwrap().unwrap();
    // Policy wins over an injected partial host error, without consuming provider input.
    real.0.inject(Fault {
        error: IoError::Fault(71),
        partial: 1,
    });
    assert_eq!(
        real.write(sub, "a", b"X".to_vec()),
        Ok(Err(IoError::NotGranted))
    );
    assert!(real.0.tape().last().unwrap().fault.is_none());
    assert_eq!(fs::read(scratch.0.join("one/sub/a")).unwrap(), b"A");
    let final_tree = real.0.snapshot().unwrap();
    replay(&real.0, initial, &mounts, &final_tree);
}

#[test]
fn another_mount_does_not_grant_work_and_empty_io_needs_no_directory() {
    let scratch = Scratch::new();
    for configured in [false, true] {
        let mut real = Library(if configured {
            scratch.io(&[("other", Access::ReadWrite, "two")])
        } else {
            RealIo::from_pinned_mounts([], std::io::sink()).unwrap()
        });
        let initial = real.0.snapshot().unwrap();
        assert_eq!(real.directory(Io(1), "work"), Ok(Err(IoError::NotGranted)));
        let mounts = if configured {
            BTreeMap::from([("other".into(), Access::ReadWrite)])
        } else {
            BTreeMap::new()
        };
        assert_eq!(real.0.mount_pins().len(), usize::from(configured));
        replay(&real.0, initial.clone(), &mounts, &initial);
    }
}

#[test]
fn a_read_directory_denies_atomic_mutation_before_consumption_or_effect() {
    let scratch = Scratch::new();
    let mut real = Library(scratch.io(&[
        ("read", Access::Read, "one"),
        ("write", Access::ReadWrite, "two"),
    ]));
    let read = real.directory(Io(1), "read").unwrap().unwrap();
    let write = real.directory(Io(1), "write").unwrap().unwrap();
    let mut atomic = real.create_atomic(write, "saved").unwrap().unwrap();
    for request in [
        R::WriteAll {
            directory: read.0,
            file: atomic.0,
            bytes: b"bad".to_vec(),
        },
        R::Commit {
            directory: read.0,
            file: atomic.0,
        },
    ] {
        assert_eq!(real.0.call(request), Ok(C::Unit(Err(IoError::NotGranted))));
        assert!(real.0.handles().observed().contains_key(&atomic.0));
        assert!(real.0.tape().last().unwrap().fault.is_none());
    }
    assert!(!scratch.0.join("two/saved").exists());
    real.write_all(write, &mut atomic, b"good")
        .unwrap()
        .unwrap();
    real.commit(write, atomic).unwrap().unwrap();
    assert_eq!(fs::read(scratch.0.join("two/saved")).unwrap(), b"good");
}

#[test]
fn replay_derives_rights_and_rejects_a_forged_denial_fault_or_success() {
    let scratch = Scratch::new();
    let mut real = Library(scratch.io(&[("work", Access::Read, "one")]));
    let initial = real.0.snapshot().unwrap();
    let work = real.directory(Io(1), "work").unwrap().unwrap();
    real.write(work, "sub/a", b"X".to_vec())
        .unwrap()
        .unwrap_err();
    for mutant in 0..2 {
        let mut tape = real.0.tape().to_vec();
        if mutant == 0 {
            tape[1].completion = C::Unit(Ok(()));
        } else {
            tape[1].fault = Some(Fault {
                error: IoError::Fault(71),
                partial: 1,
            });
        }
        let mut sim = SimIo::new(initial.clone(), tape);
        sim.set_mounts([("work".into(), Access::Read)]).unwrap();
        sim.call(real.0.tape()[0].request.clone()).unwrap();
        let before = sim.handles().clone();
        let result = sim.call(real.0.tape()[1].request.clone());
        assert_eq!(
            result,
            Err(if mutant == 0 {
                AdapterError::ReplayMismatch(1)
            } else {
                AdapterError::InvalidCompletion
            })
        );
        assert_eq!(sim.tree(), &initial);
        assert_eq!(sim.handles(), &before);
        assert_eq!(sim.position(), 1);
    }
}

#[test]
fn aliased_mounts_share_snapshot_storage_without_sharing_write_rights() {
    let scratch = Scratch::new();
    let mounts = BTreeMap::from([
        ("a/read".into(), Access::Read),
        ("b/write".into(), Access::ReadWrite),
    ]);
    let mut real = Library(scratch.io(&[
        ("a/read", Access::Read, "one"),
        ("b/write", Access::ReadWrite, "one"),
    ]));
    let initial = real.0.snapshot().unwrap();
    let read = real.directory(Io(1), "a/read").unwrap().unwrap();
    let write = real.directory(Io(1), "b/write").unwrap().unwrap();
    let stale = real.open(read, "sub/a").unwrap().unwrap();
    real.write(write, "sub/a", b"shared".to_vec())
        .unwrap()
        .unwrap();
    assert_eq!(real.read(read, "sub/a"), Ok(Ok(b"shared".to_vec())));
    assert!(real.narrow_file(read, stale).unwrap().is_none());
    let current = real.open(read, "sub/a").unwrap().unwrap();
    let current = real.narrow_file(read, current).unwrap().unwrap();
    real.close_file(current).unwrap().unwrap();
    assert_eq!(
        real.write(read, "sub/a", b"denied".to_vec()),
        Ok(Err(IoError::NotGranted))
    );
    let final_tree = real.0.snapshot().unwrap();
    replay(&real.0, initial, &mounts, &final_tree);
}

#[test]
fn mount_pins_survive_replacement_and_every_recorder_checks_every_retained_pin() {
    let scratch = Scratch::new();
    let mut real = Library(scratch.io(&[
        ("work", Access::ReadWrite, "one"),
        ("archive", Access::Read, "two"),
    ]));
    fs::rename(scratch.0.join("two"), scratch.0.join("retained")).unwrap();
    fs::create_dir(scratch.0.join("two")).unwrap();
    fs::write(scratch.0.join("two/b"), b"replacement").unwrap();
    let archive = real.directory(Io(1), "archive").unwrap().unwrap();
    assert_eq!(real.read(archive, "b"), Ok(Ok(b"B".to_vec())));
    for path in ["retained/tape", "retained/b"] {
        assert_eq!(
            real.0.prepare_recording(&scratch.0.join(path)).unwrap_err(),
            RunRefusal::RecorderOverlap {
                mount: "archive".into()
            }
        );
    }
    assert_eq!(fs::read(scratch.0.join("retained/b")).unwrap(), b"B");
    assert!(!scratch.0.join("retained/tape").exists());
    // A file recorder owns only the leaf, so common-parent placement is accepted.
    real.0
        .prepare_recording(&scratch.0.join("beside-mounts"))
        .unwrap();
    real.0
        .prepare_recording(&scratch.0.join("two/tape"))
        .unwrap();
    assert_eq!(
        RecorderDirectory::pin(&scratch.0)
            .unwrap()
            .check(&real.0.mount_pins())
            .unwrap_err(),
        RunRefusal::RecorderOverlap {
            mount: "archive".into()
        }
    );
    RecorderDirectory::pin(&scratch.0.join("tapes"))
        .unwrap()
        .check(&real.0.mount_pins())
        .unwrap();
}

#[test]
fn malformed_overlapping_and_duplicate_selectors_refuse_with_disjoint_twins() {
    let scratch = Scratch::new();
    for names in [
        ["a", "a/b"],
        ["a", "a"],
        ["", "a"],
        ["a/../b", "c"],
        ["a//b", "c"],
    ] {
        let io = RealIo::from_pinned_mounts(
            names.map(|name| {
                (
                    name.into(),
                    Access::Read,
                    PinnedDirectory::open(&scratch.0.join("one")).unwrap(),
                )
            }),
            std::io::sink(),
        );
        assert!(matches!(io, Err(RunRefusal::Invalid { .. })));
        scratch.io(&[("a", Access::Read, "one"), ("ab", Access::ReadWrite, "two")]);
    }
}

#[test]
fn native_abi_denies_mount_acquisition_and_writes_and_consumes_owned_bytes() {
    use hirsel_rt::io::*;
    use std::{cell::RefCell, rc::Rc};
    struct Shared(Rc<RefCell<RealIo>>);
    impl Adapter for Shared {
        fn call(&mut self, request: R) -> Result<C, AdapterError> {
            self.0.borrow_mut().call(request)
        }
    }
    fn byte_block(ctx: &support::Ctx, bytes: &[u8]) -> (*mut hirsel_rt::Header, u64) {
        // A whole slice of fresh byte storage: its owner and start.
        let slice = unsafe { hirsel_rt::text::allocate_bytes(ctx.0, bytes) };
        (slice.owner, slice.start)
    }
    let scratch = Scratch::new();
    for access in [Access::Read, Access::ReadWrite] {
        fs::write(scratch.0.join("one/sub/a"), b"A").unwrap();
        let real = Rc::new(RefCell::new(
            scratch.io(&[("work", access, "one"), ("child", access, "one/sub")]),
        ));
        let ctx = support::Ctx::diagnostic();
        let mut binding = Box::new(Binding::new(Shared(real.clone())));
        let work = binding.intern_path("work");
        let absent = binding.intern_path("absent");
        let subpath = binding.intern_path("sub");
        let a = binding.intern_path("a");
        let child_path = binding.intern_path("child");
        let full = binding.intern_path("sub/a");
        let mut result = HandleResult::default();
        let mut unit = UnitResult::of(Ok(()));
        unsafe {
            binding.bind(&mut *ctx.0);
            assert_eq!(
                hirsel_rt_v2_io_dir(ctx.0, std::ptr::null_mut(), &mut result, &1, &absent),
                0
            );
            assert_eq!(result, HandleResult::of(Err(IoError::NotGranted)));
            hirsel_rt_v2_io_dir(ctx.0, std::ptr::null_mut(), &mut result, &1, &work);
            assert_eq!(result.tag, 0);
            let directory = result.value;
            hirsel_rt_v2_dir_narrow(
                ctx.0,
                std::ptr::null_mut(),
                &mut result,
                &directory,
                &subpath,
            );
            assert_eq!(result.tag, 0);
            let sub = result.value;
            hirsel_rt_v2_io_dir(ctx.0, std::ptr::null_mut(), &mut result, &1, &child_path);
            assert_eq!(result.tag, 0);
            let child = result.value;
            hirsel_rt_v2_file_open(ctx.0, std::ptr::null_mut(), &mut result, &directory, &full);
            assert_eq!(result.tag, 0);
            let mut file = result.value;
            hirsel_rt_v2_file_narrow(ctx.0, std::ptr::null_mut(), &mut result, &child, &mut file);
            assert_eq!((result.tag, file), (0, 0));
            hirsel_rt_v2_file_open(ctx.0, std::ptr::null_mut(), &mut result, &directory, &full);
            assert_eq!(result.tag, 0);
            let mut file = result.value;
            hirsel_rt_v2_file_narrow(ctx.0, std::ptr::null_mut(), &mut result, &sub, &mut file);
            assert_eq!((result.tag, file), (1, 0));
            hirsel_rt_v2_io_close(ctx.0, result.value, 0);
            let (bytes, start) = byte_block(&ctx, b"native");
            assert_eq!(
                hirsel_rt_v2_file_write_bytes(
                    ctx.0,
                    std::ptr::null_mut(),
                    &mut unit,
                    &sub,
                    &a,
                    bytes,
                    start,
                    6
                ),
                0
            );
            assert_eq!(
                unit,
                UnitResult::of(if access == Access::Read {
                    Err(IoError::NotGranted)
                } else {
                    Ok(())
                })
            );
            assert_eq!(ctx.stats().live_blocks, 0);
            (*ctx.0).bind_io_data(std::ptr::null_mut());
        }
        assert_eq!(
            fs::read(scratch.0.join("one/sub/a")).unwrap(),
            if access == Access::Read {
                b"A".as_slice()
            } else {
                b"native".as_slice()
            }
        );
    }
}

#[test]
fn virtual_mount_and_narrow_roots_confine_parent_paths_and_links() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new();
    symlink("../two/b", scratch.0.join("one/escape")).unwrap();
    symlink("a", scratch.0.join("one/sub/alias")).unwrap();
    symlink("../sub/a", scratch.0.join("one/sub/parent")).unwrap();
    let mut real = Library(scratch.io(&[
        ("work", Access::Read, "one"),
        ("other", Access::ReadWrite, "two"),
    ]));
    let initial = real.0.snapshot().unwrap();
    let work = real.directory(Io(1), "work").unwrap().unwrap();
    assert_eq!(
        real.directory(Io(1), "work/../other"),
        Ok(Err(IoError::Fault(1)))
    );
    assert_eq!(real.read(work, "escape"), Ok(Err(IoError::Fault(1))));
    let sub = real.narrow_dir(work, "sub").unwrap().unwrap();
    assert_eq!(real.read(sub, "alias"), Ok(Ok(b"A".to_vec())));
    assert_eq!(real.read(sub, "parent"), Ok(Err(IoError::Fault(1))));
    assert_eq!(real.read(sub, "../sub/a"), Ok(Err(IoError::Fault(1))));
    let mounts = BTreeMap::from([
        ("work".into(), Access::Read),
        ("other".into(), Access::ReadWrite),
    ]);
    replay(&real.0, initial.clone(), &mounts, &initial);
}

#[test]
fn nested_physical_mounts_share_publications_and_keep_their_own_confinement() {
    let scratch = Scratch::new();
    let mounts = BTreeMap::from([
        ("child".into(), Access::Read),
        ("parent".into(), Access::ReadWrite),
    ]);
    let mut real = Library(scratch.io(&[
        ("child", Access::Read, "one/sub"),
        ("parent", Access::ReadWrite, "one"),
    ]));
    let initial = real.0.snapshot().unwrap();
    let child = real.directory(Io(1), "child").unwrap().unwrap();
    let parent = real.directory(Io(1), "parent").unwrap().unwrap();
    real.write(parent, "sub/a", b"shared".to_vec())
        .unwrap()
        .unwrap();
    real.write(parent, "sub/new", b"new".to_vec())
        .unwrap()
        .unwrap();
    assert_eq!(real.read(child, "a"), Ok(Ok(b"shared".to_vec())));
    // Host containment cannot rebrand across virtual selectors.
    let file = real.open(parent, "sub/a").unwrap().unwrap();
    assert!(real.narrow_file(child, file).unwrap().is_none());
    let file = real.open(parent, "sub/a").unwrap().unwrap();
    let sub = real.narrow_dir(parent, "sub").unwrap().unwrap();
    let file = real.narrow_file(sub, file).unwrap().unwrap();
    real.close_file(file).unwrap().unwrap();
    assert_eq!(real.walk(child), Ok(Ok(vec!["a".into(), "new".into()])));
    assert_eq!(real.read(child, "../sub/a"), Ok(Err(IoError::Fault(1))));
    let final_tree = real.0.snapshot().unwrap();
    replay(&real.0, initial, &mounts, &final_tree);
}

#[test]
fn an_authorized_empty_virtual_mount_issues_a_directory_in_each_simulation() {
    use hirsel_interp::{
        OperationCall, OperationContext, OperationResult, Operations, Value,
        library::{Library as Reference, SimulationConfig, path, simulation::core_path},
    };
    use hirsel_span::{SourceMap, Span};
    use hirsel_tir::RunConfiguration;
    use hirsel_types::TypeStore;
    let tree = Tree::new(BTreeMap::new()).unwrap();
    let tape = vec![hirsel_rt::io::Event {
        request: R::Directory {
            io: 1,
            path: "work".into(),
        },
        completion: C::Handle(Ok(2)),
        fault: None,
    }];
    let mut sim = SimIo::new(tree, tape);
    sim.set_mounts([("work".into(), Access::Read)]).unwrap();
    assert_eq!(
        sim.call(R::Directory {
            io: 1,
            path: "work".into()
        }),
        Ok(C::Handle(Ok(2)))
    );
    sim.finish().unwrap();
    let conflicting = Tree::new(BTreeMap::from([("work".into(), b"file".to_vec())])).unwrap();
    let mut collision = SimIo::new(conflicting.clone(), vec![]);
    let handles = collision.handles().clone();
    assert!(
        collision
            .set_mounts([("work".into(), Access::Read)])
            .is_err()
    );
    assert_eq!(collision.tree(), &conflicting);
    assert_eq!(collision.handles(), &handles);

    let run = RunConfiguration {
        entry: core_path("main"),
        hash_seed: 0,
        inputs: vec![],
    };
    let mut reference = Reference::new(
        &run,
        SimulationConfig {
            mounts: BTreeMap::from([("work".into(), Access::Read)]),
            ..Default::default()
        },
    )
    .unwrap();
    let io = reference.simulation.grant_root("Io").unwrap();
    let mut types = TypeStore::default();
    let result_type = types.push(hirsel_types::Type::unit());
    let sources = SourceMap::default();
    let mut stdout = vec![];
    let mut trace = vec![];
    let Some(OperationResult::Complete { value, .. }) = reference
        .call(
            OperationCall {
                path: &core_path("io.dir"),
                generics: &[],
                arguments: &[io, path("work")],
                result_type,
                types: &types,
                site: Span::default(),
            },
            &mut OperationContext {
                configuration: &run,
                sources: &sources,
                stdout: &mut stdout,
                trace: &mut trace,
                trace_mode: Default::default(),
                anchor: None,
            },
        )
        .unwrap()
    else {
        panic!("directory acquisition");
    };
    assert!(matches!(value, Value::Variant { variant: 0, .. }));
}

#[test]
fn snapshots_preserve_operation_faults_for_unportable_host_entry_names() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new();
    fs::write(scratch.0.join("one/bad\\file"), b"unportable").unwrap();
    fs::create_dir(scratch.0.join("one/bad\\dir")).unwrap();
    fs::write(scratch.0.join("one/bad\\dir/child"), b"unportable").unwrap();
    symlink("sub/a", scratch.0.join("one/bad\\link")).unwrap();
    let mut real = Library(scratch.io(&[("work", Access::Read, "one")]));
    let initial = real.0.snapshot().unwrap();
    let root = real.directory(Io(1), "work").unwrap().unwrap();
    assert_eq!(real.read(root, "sub/a"), Ok(Ok(b"A".to_vec())));
    assert_eq!(real.read(root, "bad\\file"), Ok(Err(IoError::Fault(1))));
    assert_eq!(real.walk(root), Ok(Err(IoError::Fault(1))));
    replay(
        &real.0,
        initial.clone(),
        &BTreeMap::from([("work".into(), Access::Read)]),
        &initial,
    );
}
