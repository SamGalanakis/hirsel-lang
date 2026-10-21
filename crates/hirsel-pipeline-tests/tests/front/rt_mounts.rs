//! hirsel-rt's mount tests whose recordings the interpreter also replays (`rt_replay`), beside
//! the rest of them in `crates/hirsel-rt/tests/it/mounts.rs`.
use crate::rt_support::library::{Io, Library};
use crate::rt_support::mounts::Scratch;
use hirsel_rt::{
    io::{Adapter, Completion as C, Fault, IoError, Request as R, Tree},
    io_real::RealIo,
    io_sim::SimIo,
};
use hirsel_run_schema::Access;
use std::{collections::BTreeMap, fs};
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
    crate::rt_replay::replay_interpreter_with_mounts(
        &tape,
        &initial,
        final_tree,
        &real.handles().observed(),
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
        library::{Library as Reference, SimulationConfig, path},
    };
    use hirsel_span::{SourceMap, Span};
    use hirsel_tir::RunConfiguration;
    use hirsel_types::TypeInterner;
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
        entry: hirsel_syntax::DefPath::core("main"),
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
    let mut types = TypeInterner::default();
    let result_type = types.push(hirsel_types::Type::unit());
    let sources = SourceMap::default();
    let mut stdout = vec![];
    let mut trace = vec![];
    let Some(OperationResult::Complete { value, .. }) = reference
        .call(
            OperationCall {
                runtime: &Default::default(),
                path: &hirsel_syntax::DefPath::core("io.dir"),
                generics: &[],
                arguments: &mut [io, path("work")],
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
