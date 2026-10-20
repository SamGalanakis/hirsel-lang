use crate::support::library::{Io, Library};
use hirsel_rt::{io::*, io_real::RealIo, io_sim::SimIo};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use support::io::replay_interpreter;
struct Disposable(PathBuf);
impl Disposable {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "hirsel-l-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Disposable {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn initial() -> Tree {
    Tree::new(BTreeMap::from([
        ("old".into(), b"old".to_vec()),
        ("sub/é".into(), vec![0, 0xff]),
        ("sub/a".into(), b"A".to_vec()),
    ]))
    .unwrap()
}
fn setup() -> (Disposable, RealIo) {
    let temp = Disposable::new();
    let tree = initial();
    for d in &tree.directories {
        if !d.is_empty() {
            std::fs::create_dir_all(temp.path().join(d)).unwrap();
        }
    }
    for (p, b) in tree.files {
        std::fs::write(temp.path().join(p), b).unwrap();
    }
    let io = RealIo::grant(temp.path()).unwrap();
    (temp, io)
}
fn actions(real: &mut Library<RealIo>) {
    let out = real.stdout(Io(1)).unwrap().unwrap();
    let root = real.directory(Io(1), "").unwrap().unwrap();
    assert_eq!(real.print(out, "α\n"), Ok(Ok(())));
    let sub = real.narrow_dir(root, "sub").unwrap().unwrap();
    assert_eq!(real.walk(sub), Ok(Ok(vec!["a".into(), "é".into()])));
    assert_eq!(real.read(sub, "é"), Ok(Ok(vec![0, 0xff])));
    assert_eq!(real.size(root, "old"), Ok(Ok(3)));
    let file = real.open(root, "sub/a").unwrap().unwrap();
    let file = real.narrow_file(sub, file).unwrap().unwrap();
    real.close_file(file).unwrap().unwrap();
    real.0.inject(Fault {
        error: IoError::Fault(71),
        partial: 2,
    });
    assert_eq!(
        real.write(root, "old", b"abcdef".to_vec()),
        Ok(Err(IoError::Fault(71)))
    );
    real.0.inject(Fault {
        error: IoError::Canceled(1),
        partial: 999,
    });
    assert_eq!(
        real.print(out, "not printed"),
        Ok(Err(IoError::Canceled(1)))
    );
    real.0.inject(Fault {
        error: IoError::Canceled(0),
        partial: 999,
    });
    assert_eq!(
        real.write(root, "old", b"no".to_vec()),
        Ok(Err(IoError::Canceled(0)))
    );
    let mut atomic = real.create_atomic(root, "saved").unwrap().unwrap();
    real.write_all(root, &mut atomic, b"first")
        .unwrap()
        .unwrap();
    real.0.inject(Fault {
        error: IoError::Fault(72),
        partial: 2,
    });
    assert_eq!(
        real.write_all(root, &mut atomic, b"second"),
        Ok(Err(IoError::Fault(72)))
    );
    real.commit(root, atomic).unwrap().unwrap();
    let atomic = real.create_atomic(root, "abort").unwrap().unwrap();
    real.abort(atomic).unwrap().unwrap();
    let atomic = real.create_atomic(root, "cancel").unwrap().unwrap();
    real.0.inject(Fault {
        error: IoError::Canceled(2),
        partial: 99,
    });
    assert_eq!(real.commit(root, atomic), Ok(Err(IoError::Canceled(2))));
    assert_eq!(real.read(root, "missing"), Ok(Err(IoError::NotFound)));
    real.remove(sub, "a").unwrap().unwrap();
}
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
            Ok(event.completion.clone())
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
fn replay_rejects_missing_extra_reordered_and_forged_completions_without_effects() {
    let (_temp, real) = setup();
    let mut real = Library(real);
    actions(&mut real);
    let tape = real.0.tape().to_vec();
    let mut empty = SimIo::new(initial(), vec![]);
    assert_eq!(
        empty.call(tape[0].request.clone()),
        Err(AdapterError::MissingInput(0))
    );
    let mut sim = SimIo::new(initial(), tape.clone());
    assert_eq!(sim.finish(), Err(AdapterError::UnconsumedInput(0)));
    assert_eq!(
        sim.call(tape[1].request.clone()),
        Err(AdapterError::ReplayMismatch(0))
    );
    assert_eq!(sim.position(), 0);
    assert_eq!(sim.tree(), &initial());
    for event in &tape {
        sim.call(event.request.clone()).unwrap();
    }
    assert_eq!(
        sim.call(tape[0].request.clone()),
        Err(AdapterError::MissingInput(tape.len()))
    );
    let mut forged = tape.clone();
    forged[0].completion = Completion::Handle(Ok(99));
    let mut sim = SimIo::new(initial(), forged);
    assert_eq!(
        sim.call(tape[0].request.clone()),
        Err(AdapterError::ReplayMismatch(0))
    );
    assert_eq!(sim.handles().observed().len(), 1);
    let idx = tape
        .iter()
        .position(|e| matches!(&e.request, Request::Write { .. }))
        .unwrap();
    let mut forged = tape.clone();
    forged[idx].completion = Completion::Unit(Ok(()));
    let mut sim = SimIo::new(initial(), forged);
    for e in &tape[..idx] {
        sim.call(e.request.clone()).unwrap();
    }
    let before = sim.tree().clone();
    assert_eq!(
        sim.call(tape[idx].request.clone()),
        Err(AdapterError::ReplayMismatch(idx))
    );
    assert_eq!(sim.tree(), &before);
}
#[test]
fn confinement_refuses_escaping_links_and_follows_in_root_links() {
    use std::os::unix::fs::symlink;
    let (temp, real) = setup();
    let outside = Disposable::new();
    std::fs::write(outside.path().join("secret"), b"private").unwrap();
    symlink(outside.path(), temp.path().join("escape")).unwrap();
    symlink("../old", temp.path().join("sub/sibling")).unwrap();
    std::fs::create_dir(temp.path().join("sub/middle")).unwrap();
    symlink(outside.path(), temp.path().join("sub/middle/link")).unwrap();
    symlink(outside.path().join("secret"), temp.path().join("sub/last")).unwrap();
    symlink("sub", temp.path().join("dir-link")).unwrap();
    let mut real = Library(real);
    let root = real.directory(Io(1), "").unwrap().unwrap();
    let sub = real.narrow_dir(root, "sub").unwrap().unwrap();
    for path in ["/etc/passwd", "../old", "sibling", "../../secret"] {
        assert!(real.read(sub, path).unwrap().is_err(), "{path}");
    }
    for path in ["escape/secret", "sub/middle/link/secret", "sub/last"] {
        assert!(real.read(root, path).unwrap().is_err(), "{path}");
    }
    assert_eq!(real.read(root, "dir-link/a"), Ok(Ok(b"A".to_vec())));
    let linked = real.narrow_dir(root, "dir-link").unwrap().unwrap();
    assert_eq!(real.read(linked, "a"), Ok(Ok(b"A".to_vec())));
    assert!(real.read(linked, "sibling").unwrap().is_err());
    assert_eq!(real.read(sub, "é"), Ok(Ok(vec![0, 0xff])));
    assert_eq!(
        std::fs::read(outside.path().join("secret")).unwrap(),
        b"private"
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
        assert_eq!(sim.call(e.request.clone()), Ok(e.completion.clone()));
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
fn repeated_directory_grants_keep_distinct_inodes_and_narrow_refuses_a_replaced_file() {
    let (temp, real) = setup();
    let mut real = Library(real);
    let root = real.directory(Io(1), "").unwrap().unwrap();
    let old_sub = real.narrow_dir(root, "sub").unwrap().unwrap();
    let file = real.open(old_sub, "a").unwrap().unwrap();
    std::fs::rename(temp.path().join("sub"), temp.path().join("moved")).unwrap();
    std::fs::create_dir(temp.path().join("sub")).unwrap();
    std::fs::write(temp.path().join("sub/a"), b"replacement").unwrap();
    let new_sub = real.narrow_dir(root, "sub").unwrap().unwrap();
    assert_eq!(real.read(old_sub, "a"), Ok(Ok(b"A".to_vec())));
    assert_eq!(real.read(new_sub, "a"), Ok(Ok(b"replacement".to_vec())));
    assert_eq!(real.narrow_file(new_sub, file), Ok(None));
}

use crate::support;
#[test]
fn io_result_records_match_derived_layouts_on_both_targets() {
    use hirsel_layout::layout::{LayoutBuilder, OwnerRepresentation, Owners};

    use hirsel_lir::layout_types::{SizeSpec, TargetSpec};
    use hirsel_types::Primitive;
    use std::mem::{align_of, offset_of, size_of};
    for target in [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        let mut ty = support::schema::Ty::default();
        let u32ty = ty.p(Primitive::U32);
        let u64ty = ty.p(Primitive::U64);
        let cancel = ty.variant(&[&[], &[], &[]]);
        let error = ty.variant(&[&[u32ty], &[cancel]]);
        let file = ty.named(&support::schema::path("File"), Vec::new());
        let mut owners = Owners::default();
        owners.nominals.insert(
            support::schema::path("File"),
            OwnerRepresentation::Handle {
                resource: true,
                revision: Default::default(),
            },
        );
        let result = ty.variant(&[&[file], &[error]]);
        let size_result = ty.variant(&[&[u64ty], &[error]]);
        let option = ty.variant(&[&[], &[file]]);
        let unit = ty.variant(&[&[], &[error]]);
        let bytes = ty.p(Primitive::Bytes);
        let bytes_result = ty.variant(&[&[bytes], &[error]]);
        let path = ty.named(
            &support::schema::handle(&mut owners, "Path", false),
            Vec::new(),
        );
        let item = ty.variant(&[&[path], &[error]]);
        let iter = ty.add(hirsel_types::TypeKind::Builtin {
            kind: hirsel_types::BuiltinType::Iter,
            arguments: vec![hirsel_types::GenericArgument::Type(item)].into(),
        });
        let walk = ty.variant(&[&[iter], &[error]]);
        let step = ty.variant(&[&[], &[item]]);
        let kind = ty.variant(&[&[], &[], &[], &[]]);
        let i128ty = ty.p(Primitive::I128);
        let timestamp = ty.variant(&[&[], &[i128ty]]);
        let permissions = ty.p(Primitive::U32);
        let metadata = ty.record(&[kind, u64ty, timestamp, permissions]);
        let metadata_result = ty.variant(&[&[metadata], &[error]]);
        let mut builder = LayoutBuilder::new(&target, &owners);
        let metadata_id = builder.layout(&ty.store, metadata_result).unwrap();
        let ids: Vec<_> = [result, size_result, option, unit, bytes_result, walk, step]
            .iter()
            .map(|&t| builder.layout(&ty.store, t).unwrap())
            .collect();
        let table = builder.finish();
        hirsel_lir::layout_validate::validate_table(&table, &target).unwrap();
        let meta_layout = table.layout(metadata_id).unwrap();
        assert_eq!(
            meta_layout.size,
            SizeSpec::Fixed(size_of::<MetadataResult>() as u64)
        );
        assert_eq!(meta_layout.align, align_of::<MetadataResult>() as u32);
        assert_eq!(
            meta_layout.tag.as_ref().unwrap().variants[0].fields[0].offset,
            16
        );
        for &id in &ids[..3] {
            let record = table.layout(id).unwrap();
            assert_eq!(
                record.size,
                SizeSpec::Fixed(size_of::<HandleResult>() as u64)
            );
            assert_eq!(record.align, align_of::<HandleResult>() as u32);
            assert_eq!(
                record.tag.as_ref().unwrap().variants[1].fields[0].offset,
                offset_of!(HandleResult, value) as u64
            );
        }
        let record = table.layout(ids[3]).unwrap();
        assert_eq!(record.size, SizeSpec::Fixed(size_of::<UnitResult>() as u64));
        assert_eq!(record.align, align_of::<UnitResult>() as u32);
        assert_eq!(
            record.tag.as_ref().unwrap().variants[1].fields[0].offset,
            offset_of!(UnitResult, error) as u64
        );
        for (id, size, align, payload) in [
            (
                ids[4],
                size_of::<BytesResult>(),
                align_of::<BytesResult>(),
                offset_of!(BytesResult, value),
            ),
            (
                ids[5],
                size_of::<WalkResult>(),
                align_of::<WalkResult>(),
                offset_of!(WalkResult, value),
            ),
            (
                ids[6],
                size_of::<WalkOption>(),
                align_of::<WalkOption>(),
                offset_of!(WalkOption, value),
            ),
        ] {
            let record = table.layout(id).unwrap();
            assert_eq!(record.size, SizeSpec::Fixed(size as u64));
            assert_eq!(record.align, align as u32);
            assert_eq!(
                record.tag.as_ref().unwrap().variants[if id == ids[6] { 1 } else { 0 }].fields[0]
                    .offset,
                payload as u64
            );
        }
    }
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
    let ctx = support::Ctx::diagnostic();
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
            Ok(event.completion.clone())
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
fn byte_block(ctx: &support::Ctx, bytes: &[u8]) -> (*mut hirsel_rt::Header, u64) {
    // A whole slice of fresh byte storage: its owner and start.
    let slice = unsafe { hirsel_rt::text::allocate_bytes(ctx.0, bytes) };
    (slice.owner, slice.start)
}

#[test]
fn walk_step_stops_after_the_first_error_and_releases_its_environment() {
    let ctx = support::Ctx::diagnostic();
    let items = [
        HandleResult::of(Ok(7)),
        HandleResult::of(Err(IoError::Canceled(2))),
        HandleResult::of(Ok(8)),
    ];
    unsafe {
        let environment = walk_environment(ctx.0, &items);
        let mut out = WalkOption::default();
        assert_eq!(
            hirsel_rt_v2_file_walk_step(ctx.0, environment, 0, 0, std::ptr::null_mut(), &mut out),
            0
        );
        assert_eq!(
            out,
            WalkOption {
                tag: 1,
                padding: 0,
                value: items[0]
            }
        );
        assert_eq!(
            hirsel_rt_v2_file_walk_step(ctx.0, environment, 0, 0, std::ptr::null_mut(), &mut out),
            0
        );
        assert_eq!(
            out,
            WalkOption {
                tag: 1,
                padding: 0,
                value: items[1]
            }
        );
        for _ in 0..2 {
            assert_eq!(
                hirsel_rt_v2_file_walk_step(
                    ctx.0,
                    environment,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut out
                ),
                0
            );
            assert_eq!(out.tag, 0);
        }
        assert!(
            hirsel_rt::io::hirsel_rt_v2_file_walk_descriptor
                .0
                .finalizer
                .is_none()
        );
        hirsel_rt::release::hirsel_rt_v2_release(
            ctx.0,
            environment,
            (*environment).index(),
            std::ptr::null(),
            0,
            0,
        );
    }
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn launch_recording_round_trips_inputs_empty_fields_and_faults_and_refuses_malformed_data() {
    use hirsel_rt::launch::{
        Recording, parse_recording, parse_tape, render_recording, render_tape,
    };
    let (_temp, real) = setup();
    let mut real = Library(real);
    actions(&mut real);
    let mut events = real.0.tape().to_vec();
    events.push(Event {
        request: Request::MonotonicClock { io: 1 },
        completion: Completion::ClockGrant(Ok(20)),
        fault: None,
    });
    events.push(Event {
        request: Request::Random { io: 1 },
        completion: Completion::Handle(Ok(21)),
        fault: None,
    });
    events.push(Event {
        request: Request::TestDir {
            test: 22,
            entries: vec![("".into(), vec![]), ("é".into(), vec![0, 255])],
        },
        completion: Completion::Handle(Ok(23)),
        fault: None,
    });
    events.push(Event {
        request: Request::TestCheck {
            test: 22,
            condition: false,
        },
        completion: Completion::Checked(false),
        fault: None,
    });
    let text = render_tape(&events).unwrap();
    assert_eq!(parse_tape(&text).unwrap(), events);
    let record = Recording {
        arguments: vec!["".into(), "é/x".into()],
        run_identity: [0; 32],
        hash_seed: 0,
        random_seed: 0,
        test_seed: u64::MAX,
        binding: None,
        tree: initial(),
        events,
    };
    let text = render_recording(&record).unwrap();
    assert_eq!(parse_recording(&text).unwrap(), record);
    for malformed in [
        text.replace("test_seed 18446744073709551615", "test_seed x"),
        text.replace("events\n", ""),
        text.replace("directory \n", "directory 2e2e\n"),
        text.replace("argument c3a92f78", "argument ff"),
        text.replace("test_seed 18446744073709551615", "test_seed 1\ntest_seed 2"),
    ] {
        assert!(parse_recording(&malformed).is_err());
    }
}

#[test]
fn path_order_uses_exact_text_without_allocating_or_comparing_capability_ids() {
    let ctx = support::Ctx::diagnostic();
    let mut binding = Box::new(Binding::new(SimIo::new(
        Tree::new(BTreeMap::new()).unwrap(),
        vec![],
    )));
    let z = binding.intern_path("z");
    let a = binding.intern_path("a");
    let unicode = binding.intern_path("é");
    let decomposed = binding.intern_path("e\u{301}");
    assert!(a > z);
    assert_eq!(binding.intern_path("a"), a);
    assert_ne!(unicode, decomposed);
    unsafe { binding.bind(&mut *ctx.0) };
    let allocations = support::host_calls();
    unsafe {
        assert_eq!(hirsel_rt_v2_path_compare(ctx.0, a, z), -1);
        assert_eq!(hirsel_rt_v2_path_compare(ctx.0, z, a), 1);
        assert_eq!(hirsel_rt_v2_path_compare(ctx.0, a, a), 0);
        assert_eq!(hirsel_rt_v2_path_compare(ctx.0, decomposed, unicode), -1);
        (*ctx.0).bind_io_data(std::ptr::null_mut());
    }
    assert_eq!(support::host_calls(), allocations);
}

#[test]
fn walk_follows_relative_links_component_by_component_and_replays_their_paths() {
    use std::os::unix::fs::symlink;
    let (temp, real) = setup();
    symlink("sub", temp.path().join("alias")).unwrap();
    symlink("../old", temp.path().join("sub/link")).unwrap();
    symlink("alias/../old", temp.path().join("chain")).unwrap();
    let mut real = Library(real);
    let root = real.directory(Io(1), "").unwrap().unwrap();
    let expected = vec![
        "alias/a",
        "alias/link",
        "alias/é",
        "chain",
        "old",
        "sub/a",
        "sub/link",
        "sub/é",
    ];
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
        assert_eq!(sim.call(e.request.clone()), Ok(e.completion.clone()));
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
fn link_and_directory_cycles_terminate_with_file_fault() {
    use std::os::unix::fs::symlink;
    let (temp, real) = setup();
    let mut real = Library(real);
    let root = real.directory(Io(1), "").unwrap().unwrap();
    symlink("second", temp.path().join("first")).unwrap();
    symlink("first", temp.path().join("second")).unwrap();
    assert_eq!(real.walk(root), Ok(Err(IoError::Fault(1))));
    assert_eq!(real.read(root, "first"), Ok(Err(IoError::Fault(1))));
    std::fs::remove_file(temp.path().join("first")).unwrap();
    std::fs::remove_file(temp.path().join("second")).unwrap();
    symlink("..", temp.path().join("sub/back")).unwrap();
    assert_eq!(real.walk(root), Ok(Err(IoError::Fault(1))));
    std::fs::remove_file(temp.path().join("sub/back")).unwrap();
    // A cycle through sibling directories must preserve traversal ancestry
    // independently of the descriptor ancestry used to resolve each link.
    std::fs::create_dir(temp.path().join("other")).unwrap();
    symlink("../other", temp.path().join("sub/across")).unwrap();
    symlink("../sub", temp.path().join("other/across")).unwrap();
    assert_eq!(real.walk(root), Ok(Err(IoError::Fault(1))));
    std::fs::remove_file(temp.path().join("sub/across")).unwrap();
    std::fs::remove_file(temp.path().join("other/across")).unwrap();
    assert!(real.walk(root).unwrap().is_ok());
}

#[test]
fn fifo_and_socket_entries_are_refused_before_data_open() {
    use std::ffi::CString;
    use std::os::unix::net::UnixListener;
    let (temp, real) = setup();
    let fifo = CString::new(temp.path().join("fifo").as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: terminated disposable path; mkfifo creates no open descriptor.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    // Watching IN_OPEN proves that refusal precedes a FIFO data open. Merely
    // using O_NONBLOCK would terminate too, but would emit this event.
    use std::os::fd::FromRawFd;
    let monitor = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    assert!(monitor >= 0);
    let watch = unsafe { libc::inotify_add_watch(monitor, fifo.as_ptr(), libc::IN_OPEN) };
    assert!(watch >= 0);
    let mut monitor = unsafe { std::fs::File::from_raw_fd(monitor) };
    let mut real = Library(real);
    let root = real.directory(Io(1), "").unwrap().unwrap();
    assert_eq!(real.walk(root), Ok(Err(IoError::Fault(1))));
    assert!(real.read(root, "fifo").unwrap().is_err());
    assert_eq!(
        std::io::Read::read(&mut monitor, &mut [0; 1024])
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    std::fs::remove_file(temp.path().join("fifo")).unwrap();
    let _socket = UnixListener::bind(temp.path().join("socket")).unwrap();
    assert_eq!(real.walk(root), Ok(Err(IoError::Fault(1))));
    std::fs::remove_file(temp.path().join("socket")).unwrap();
    assert!(real.walk(root).unwrap().is_ok());
}

#[test]
fn absolute_link_targets_are_refused_even_when_they_point_inside_the_grant() {
    use std::os::unix::fs::symlink;
    let (temp, real) = setup();
    let mut real = Library(real);
    let root = real.directory(Io(1), "").unwrap().unwrap();
    symlink(temp.path().join("old"), temp.path().join("link")).unwrap();
    assert_eq!(real.read(root, "link"), Ok(Err(IoError::Fault(1))));
    assert_eq!(real.walk(root), Ok(Err(IoError::Fault(1))));
    std::fs::remove_file(temp.path().join("link")).unwrap();
    symlink("old", temp.path().join("link")).unwrap();
    assert_eq!(real.read(root, "link"), Ok(Ok(b"old".to_vec())));
    assert!(real.walk(root).unwrap().is_ok());
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
            Ok(event.completion.clone())
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
                Ok(event.completion.clone())
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
fn commit_refuses_a_replaced_temporary_with_a_regular_prepared_twin() {
    use std::os::unix::fs::symlink;
    let (temp, io) = setup();
    let mut real = Library(io);
    let root = real.directory(Io(1), "").unwrap().unwrap();
    let mut atomic = real.create_atomic(root, "saved").unwrap().unwrap();
    real.write_all(root, &mut atomic, b"prepared")
        .unwrap()
        .unwrap();
    std::fs::remove_file(temp.path().join("saved.hirsel-tmp-3")).unwrap();
    symlink("old", temp.path().join("saved.hirsel-tmp-3")).unwrap();
    assert_eq!(real.commit(root, atomic), Ok(Err(IoError::Fault(1))));
    assert!(!temp.path().join("saved").exists());
    assert_eq!(std::fs::read(temp.path().join("old")).unwrap(), b"old");
    let mut atomic = real.create_atomic(root, "saved").unwrap().unwrap();
    real.write_all(root, &mut atomic, b"prepared")
        .unwrap()
        .unwrap();
    real.commit(root, atomic).unwrap().unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("saved")).unwrap(),
        b"prepared"
    );
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
            Ok(event.completion.clone())
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

/// Discard the host rejection input to require the model to derive reachability
/// from the recorded publications. Round-trip the real on-disk tape format too.
fn identity_tape(events: &[Event]) -> Vec<Event> {
    let mut events = events.to_vec();
    for event in &mut events {
        if matches!(event.request, Request::NarrowFile { .. }) {
            event.fault = None;
        }
    }
    hirsel_rt::launch::parse_tape(&hirsel_rt::launch::render_tape(&events).unwrap()).unwrap()
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
                Ok(event.completion.clone()),
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
fn fake_test_directories_derive_identity_and_refuse_foreign_trees() {
    use crate::support::library::{Dir, File};
    let (temp, mut io) = setup();
    io.test_context(7);
    let mut real = Library(io);
    let make = |io: &mut RealIo| match io
        .call(Request::TestDir {
            test: 1,
            entries: vec![("sub/value".into(), b"data".to_vec())],
        })
        .unwrap()
    {
        Completion::Handle(Ok(id)) => Dir(id),
        other => panic!("{other:?}"),
    };
    let dir = make(&mut real.0);
    let foreign = make(&mut real.0);
    let sub = real.narrow_dir(dir, "sub").unwrap().unwrap();
    let file = real.open(dir, "sub/value").unwrap().unwrap();
    assert_eq!(real.narrow_file(foreign, file), Ok(None));
    let old = real.open(dir, "sub/value").unwrap().unwrap();
    let old = real.narrow_file(sub, old).unwrap().unwrap();
    real.write(dir, "sub/value", b"data".to_vec())
        .unwrap()
        .unwrap();
    assert_eq!(real.narrow_file(sub, old), Ok(None));
    let old = real.open(dir, "sub/value").unwrap().unwrap();
    real.remove(sub, "value").unwrap().unwrap();
    real.write(sub, "value", b"data".to_vec()).unwrap().unwrap();
    assert_eq!(real.narrow_file(dir, old), Ok(None));
    let old = real.open(dir, "sub/value").unwrap().unwrap();
    let mut atomic = real.create_atomic(sub, "value").unwrap().unwrap();
    real.write_all(sub, &mut atomic, b"data").unwrap().unwrap();
    real.commit(sub, atomic).unwrap().unwrap();
    assert_eq!(real.narrow_file(sub, old), Ok(None));
    let fresh = real.open(sub, "value").unwrap().unwrap();
    let File(id) = real.narrow_file(sub, fresh).unwrap().unwrap();
    real.close_file(File(id)).unwrap().unwrap();
    assert!(!temp.path().join("value").exists());
    let tape = identity_tape(real.0.tape());
    let mut sim = SimIo::new(initial(), tape.clone());
    sim.test_context(7);
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(event.completion.clone())
        );
    }
    sim.finish().unwrap();
    assert_eq!(sim.handles(), real.0.handles());
    assert_eq!(sim.fake_trees(), real.0.fake_trees());
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
            Ok(event.completion.clone())
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
fn replay_rejects_forged_success_for_a_stale_identity_without_consuming_its_handle() {
    let (_temp, io) = setup();
    let mut real = Library(io);
    let dir = real.directory(Io(1), ".").unwrap().unwrap();
    let file = real.open(dir, "old").unwrap().unwrap();
    real.write(dir, "old", b"old".to_vec()).unwrap().unwrap();
    assert_eq!(real.narrow_file(dir, file), Ok(None));
    let mut tape = identity_tape(real.0.tape());
    let last = tape.last_mut().unwrap();
    last.completion = Completion::OptionalFile(Some(99));
    let mut sim = SimIo::new(initial(), tape.clone());
    for event in &tape[..tape.len() - 1] {
        sim.call(event.request.clone()).unwrap();
    }
    let before = sim.handles().clone();
    let event = tape.last().unwrap();
    for _ in 0..2 {
        assert_eq!(
            sim.call(event.request.clone()),
            Err(AdapterError::ReplayMismatch(tape.len() - 1))
        );
        assert_eq!(sim.position(), tape.len() - 1);
        assert_eq!(sim.handles(), &before);
        assert_eq!(sim.tree(), &initial());
    }
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
            Ok(event.completion.clone())
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
            support::library::Dir(id)
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
            support::library::Dir(id)
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
                Ok(event.completion.clone())
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
fn projection_denial_is_a_recorded_result_and_replay_cannot_mint_a_grant() {
    use std::collections::BTreeSet;
    for request in [
        Request::Stdout { io: 1 },
        Request::Tasks { io: 1 },
        Request::Net { io: 1 },
        Request::Random { io: 1 },
        Request::Directory {
            io: 1,
            path: "".into(),
        },
    ] {
        let (temp, mut real) = setup();
        real.set_projections(BTreeSet::new());
        let before = real.handles().clone();
        assert_eq!(
            real.call(request.clone()),
            Ok(Completion::Handle(Err(IoError::NotGranted)))
        );
        assert_eq!(real.handles(), &before);
        assert!(real.stdout().is_empty());
        let tape = real.tape().to_vec();
        let mut replay = SimIo::new(initial(), tape.clone());
        replay.set_projections(BTreeSet::new());
        assert_eq!(
            replay.call(request.clone()),
            Ok(Completion::Handle(Err(IoError::NotGranted)))
        );
        replay.finish().unwrap();
        assert_eq!(replay.handles(), &before);
        let mut forged = tape;
        forged[0].completion = Completion::Handle(Ok(2));
        let mut replay = SimIo::new(initial(), forged);
        replay.set_projections(BTreeSet::new());
        assert_eq!(
            replay.call(request.clone()),
            Err(AdapterError::ReplayMismatch(0))
        );
        assert_eq!(replay.position(), 0);
        assert_eq!(replay.handles(), &before);
        let mut granted = RealIo::grant(temp.path()).unwrap();
        assert!(matches!(
            granted.call(request),
            Ok(Completion::Handle(Ok(_)))
        ));
    }
}

#[test]
fn grant_and_file_refusals_have_the_declared_inline_layouts() {
    use std::mem::{align_of, offset_of, size_of};
    assert_eq!(
        (size_of::<GrantResult>(), align_of::<GrantResult>()),
        (16, 8)
    );
    assert_eq!(offset_of!(GrantResult, value), 8);
    assert_eq!(
        GrantResult::of(Err(IoError::NotGranted)),
        GrantResult {
            tag: 1,
            padding: 0,
            value: 0
        }
    );
    assert_eq!(
        GrantResult::of(Ok(17)),
        GrantResult {
            tag: 0,
            padding: 0,
            value: 17
        }
    );
    assert_eq!(
        HandleResult::of(Err(IoError::NotGranted)),
        HandleResult {
            tag: 1,
            padding: 0,
            value: 2
        }
    );
    assert_eq!(
        UnitResult::of(Err(IoError::NotGranted)),
        UnitResult {
            tag: 1,
            error: 2,
            code: 0
        }
    );
}
#[test]
fn host_input_tape_rechecks_exact_grants_and_never_reads_the_provider_on_replay() {
    use std::cell::Cell;
    use std::rc::Rc;
    #[derive(Clone)]
    struct Input {
        calls: Rc<Cell<usize>>,
        bytes: std::io::Cursor<Vec<u8>>,
    }
    impl std::io::Read for Input {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            self.calls.set(self.calls.get() + 1);
            std::io::Read::read(&mut self.bytes, out)
        }
    }
    let calls = Rc::new(Cell::new(0));
    let mut real = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
    real.set_stdin(Input {
        calls: calls.clone(),
        bytes: std::io::Cursor::new(vec![0, 255, 10]),
    });
    real.set_env(BTreeMap::from([
        ("SET".into(), Some("λ".into())),
        ("UNSET".into(), None),
    ]));
    real.set_projections(Default::default());
    assert_eq!(
        real.call(Request::Stdin { io: 1 }),
        Ok(Completion::Handle(Err(IoError::NotGranted)))
    );
    assert_eq!(calls.get(), 0);
    real.set_projections(["stdin".into(), "stderr".into()].into_iter().collect());
    let Completion::Handle(Ok(stdin)) = real.call(Request::Stdin { io: 1 }).unwrap() else {
        panic!()
    };
    real.inject(Fault {
        error: IoError::Canceled(1),
        partial: 0,
    });
    assert_eq!(
        real.call(Request::ReadStdin { stdin, max: 0 }),
        Ok(Completion::Input(Err(InputError::InvalidInput)))
    );
    assert_eq!(calls.get(), 0);
    assert_eq!(
        real.call(Request::ReadStdin { stdin: 999, max: 2 }),
        Err(AdapterError::InvalidHandle(999))
    );
    assert_eq!(calls.get(), 0);
    assert_eq!(
        real.call(Request::ReadStdin { stdin, max: 1 }),
        Ok(Completion::Input(Err(InputError::Canceled(1))))
    );
    assert_eq!(calls.get(), 0);
    for (max, expected) in [(2, vec![0, 255]), (10, vec![10]), (1, vec![])] {
        assert_eq!(
            real.call(Request::ReadStdin { stdin, max }),
            Ok(Completion::Input(Ok(expected)))
        );
    }
    for (name, expected) in [
        ("SET", Ok(Some("λ".into()))),
        ("UNSET", Ok(None)),
        ("SECRET", Err(EnvError::NotGranted)),
    ] {
        assert_eq!(
            real.call(Request::EnvGet {
                env: ENV_HANDLE,
                name: name.into()
            }),
            Ok(Completion::Env(expected))
        );
    }
    let Completion::Handle(Ok(stderr)) = real.call(Request::Stderr { io: 1 }).unwrap() else {
        panic!()
    };
    assert_eq!(
        real.call(Request::PrintStderr {
            stderr,
            bytes: b"separate\n".to_vec()
        }),
        Ok(Completion::Unit(Ok(())))
    );
    assert_eq!(real.stdout(), b"");
    assert_eq!(real.stderr(), b"separate\n");
    let tape = real.tape().to_vec();
    let encoded = hirsel_rt::launch::render_tape(&tape).unwrap();
    assert_eq!(hirsel_rt::launch::parse_tape(&encoded).unwrap(), tape);
    let mut replay = SimIo::new(Tree::new(BTreeMap::new()).unwrap(), tape.clone());
    replay.set_env(BTreeMap::from([
        ("SET".into(), Some("λ".into())),
        ("UNSET".into(), None),
    ]));
    replay.set_projections(Default::default());
    for (index, event) in tape.iter().enumerate() {
        if index == 1 {
            replay.set_projections(["stdin".into(), "stderr".into()].into_iter().collect());
        }
        match &event.completion {
            Completion::Refused(id) => assert_eq!(
                replay.call(event.request.clone()),
                Err(AdapterError::InvalidHandle(*id))
            ),
            _ => assert_eq!(
                replay.call(event.request.clone()),
                Ok(event.completion.clone())
            ),
        }
    }
    replay.finish().unwrap();
    assert_eq!(replay.stderr(), real.stderr());
    // Tape data alone cannot grant a name absent from admission.
    let event = tape
        .iter()
        .find(|e| matches!(&e.request,Request::EnvGet { name,.. } if name=="SET"))
        .unwrap()
        .clone();
    let mut denied = SimIo::new(Tree::new(BTreeMap::new()).unwrap(), vec![event.clone()]);
    assert_eq!(
        denied.call(event.request),
        Err(AdapterError::ReplayMismatch(0))
    );
    assert_eq!(denied.position(), 0);
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
    let encoded = hirsel_rt::launch::render_tape(&tape).unwrap();
    assert_eq!(hirsel_rt::launch::parse_tape(&encoded).unwrap(), tape);
    let mut sim = SimIo::new(initial.clone(), tape.clone());
    std::fs::remove_file(temp.path().join("saved")).unwrap();
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(event.completion.clone())
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
                && matches!(e.completion, Completion::Bytes(Ok(_)))
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
    assert_eq!(
        hirsel_rt::launch::parse_tape(&hirsel_rt::launch::render_tape(&tape).unwrap()).unwrap(),
        tape
    );
    let mut sim = SimIo::new(initial(), tape.clone());
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(event.completion.clone())
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

// Rust fs::tests mkdir_path_already_exists_error, test_rename_directory_to_non_empty_directory,
// test_rename_file_over_open_file and test_rename_symlink, through confined provider requests.
#[test]
fn filesystem_lifecycle_replays_without_host_paths() {
    let (temp, mut host) = setup();
    std::os::unix::fs::symlink("missing", temp.path().join("link")).unwrap();
    std::os::unix::fs::symlink("../outside", temp.path().join("escape")).unwrap();
    let tree = host.snapshot().unwrap();
    let Completion::Handle(Ok(dir)) = host
        .call(Request::Directory {
            io: 1,
            path: "".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    let mut expected = vec![host.tape().last().unwrap().clone()];
    {
        let mut call = |request: Request| {
            let result = host.call(request).unwrap();
            expected.push(host.tape().last().unwrap().clone());
            result
        };
        assert_eq!(
            call(Request::Mkdir {
                directory: dir,
                path: "new".into()
            }),
            Completion::Unit(Ok(()))
        );
        assert_eq!(
            call(Request::Mkdir {
                directory: dir,
                path: "new".into()
            }),
            Completion::Unit(Err(IoError::AlreadyExists))
        );
        assert_eq!(
            call(Request::Mkdir {
                directory: dir,
                path: "new/empty".into()
            }),
            Completion::Unit(Ok(()))
        );
        let Completion::Entries(Ok(entries)) = call(Request::ReadDir {
            directory: dir,
            path: "new".into(),
        }) else {
            panic!()
        };
        assert_eq!(
            entries,
            vec![DirEntry {
                path: "new/empty".into(),
                kind: FileKind::Directory
            }]
        );
        let Completion::Metadata(Ok(meta)) = call(Request::Metadata {
            directory: dir,
            path: "escape".into(),
        }) else {
            panic!()
        };
        assert_eq!(meta.kind, FileKind::Symlink);
        assert_eq!(meta.size, 10);
        assert!(meta.modified_unix_ns.is_some());
        assert_eq!(
            call(Request::Read {
                directory: dir,
                path: "escape".into()
            }),
            Completion::Bytes(Err(IoError::Fault(1)))
        );
        assert_eq!(
            call(Request::Rename {
                directory: dir,
                from: "link".into(),
                to: "renamed-link".into()
            }),
            Completion::Unit(Ok(()))
        );
        assert_eq!(
            call(Request::RemoveDir {
                directory: dir,
                path: "renamed-link".into()
            }),
            Completion::Unit(Err(IoError::NotDirectory))
        );
        assert_eq!(
            call(Request::Rename {
                directory: dir,
                from: "new".into(),
                to: "sub".into()
            }),
            Completion::Unit(Err(IoError::DirectoryNotEmpty))
        );
        assert_eq!(
            call(Request::RemoveDir {
                directory: dir,
                path: "new".into()
            }),
            Completion::Unit(Err(IoError::DirectoryNotEmpty))
        );
        assert_eq!(
            call(Request::Rename {
                directory: dir,
                from: "new".into(),
                to: "moved".into()
            }),
            Completion::Unit(Ok(()))
        );
        assert_eq!(
            call(Request::ReadDir {
                directory: dir,
                path: "moved".into()
            }),
            Completion::Entries(Ok(vec![DirEntry {
                path: "moved/empty".into(),
                kind: FileKind::Directory
            }]))
        );
        assert_eq!(
            call(Request::RemoveDir {
                directory: dir,
                path: "moved/empty".into()
            }),
            Completion::Unit(Ok(()))
        );
        assert_eq!(
            call(Request::RemoveDir {
                directory: dir,
                path: "moved".into()
            }),
            Completion::Unit(Ok(()))
        );
        assert_eq!(
            call(Request::RemoveDir {
                directory: dir,
                path: ".".into()
            }),
            Completion::Unit(Err(IoError::Fault(1)))
        );
        assert_eq!(
            call(Request::Rename {
                directory: dir,
                from: "old".into(),
                to: "".into()
            }),
            Completion::Unit(Err(IoError::Fault(1)))
        );
    }
    drop(host);
    drop(temp);
    let text = hirsel_rt::launch::render_tape(&expected).unwrap();
    let tape = hirsel_rt::launch::parse_tape(&text).unwrap();
    assert_eq!(tape, expected);
    let mut replay = SimIo::new(tree, tape.clone());
    for event in tape {
        assert_eq!(replay.call(event.request).unwrap(), event.completion);
    }
    replay.finish().unwrap();
}

#[test]
fn filesystem_mutations_deny_read_grants_before_resolving_paths_and_replay_cross_device() {
    use hirsel_run_config::Access;
    let temp = Disposable::new();
    let pin = hirsel_rt::host_paths::PinnedDirectory::open(temp.path()).unwrap();
    let mut host =
        RealIo::from_pinned_mounts([("data".into(), Access::Read, pin)], std::io::sink()).unwrap();
    let Completion::Handle(Ok(dir)) = host
        .call(Request::Directory {
            io: 1,
            path: "data".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    for request in [
        Request::Mkdir {
            directory: dir,
            path: "../outside".into(),
        },
        Request::RemoveDir {
            directory: dir,
            path: "missing/child".into(),
        },
        Request::Rename {
            directory: dir,
            from: "missing".into(),
            to: "/outside".into(),
        },
    ] {
        assert_eq!(
            host.call(request),
            Ok(Completion::Unit(Err(IoError::NotGranted)))
        );
        assert_eq!(host.tape().last().unwrap().fault, None);
    }
    assert!(std::fs::read_dir(temp.path()).unwrap().next().is_none());
    let tape = host.tape().to_vec();
    let mut replay = SimIo::new(Tree::new(Default::default()).unwrap(), tape.clone());
    replay.set_mounts([("data".into(), Access::Read)]).unwrap();
    for event in tape {
        assert_eq!(replay.call(event.request), Ok(event.completion));
    }
    replay.finish().unwrap();

    let (_temp, mut host) = setup();
    let Completion::Handle(Ok(dir)) = host
        .call(Request::Directory {
            io: 1,
            path: "".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    host.inject(Fault {
        error: IoError::CrossDevice,
        partial: 0,
    });
    assert_eq!(
        host.call(Request::Rename {
            directory: dir,
            from: "old".into(),
            to: "new".into()
        }),
        Ok(Completion::Unit(Err(IoError::CrossDevice)))
    );
    let tape = hirsel_rt::launch::parse_tape(&hirsel_rt::launch::render_tape(host.tape()).unwrap())
        .unwrap();
    let mut replay = SimIo::new(initial(), tape.clone());
    for event in tape {
        assert_eq!(replay.call(event.request), Ok(event.completion));
    }
    replay.finish().unwrap();
    assert_eq!(replay.tree(), &initial());
}

#[test]
fn filesystem_metadata_abi_preserves_negative_nanoseconds_and_initializes_inactive_bytes() {
    struct MetadataOnly;
    impl Adapter for MetadataOnly {
        fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
            assert!(matches!(request, Request::Metadata { directory: 7, .. }));
            Ok(None)
        }
        fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
            assert!(matches!(request, Request::Metadata { directory: 7, .. }));
            Ok(Completion::Metadata(Ok(Metadata {
                kind: FileKind::Symlink,
                size: 9,
                modified_unix_ns: Some(-1),
                permissions: 0o666,
            })))
        }
    }
    let ctx = support::Ctx::diagnostic();
    let mut binding = Box::new(Binding::new(MetadataOnly));
    let path = binding.intern_path("link");
    let mut out = MetadataResult {
        words: [u64::MAX; 10],
    };
    unsafe {
        binding.bind(&mut *ctx.0);
        assert_eq!(
            hirsel_rt_v2_file_metadata(ctx.0, std::ptr::null_mut(), &mut out, &7, &path),
            0
        );
    }
    assert_eq!(out.words, [0, 0, 2, 9, 1, 0, u64::MAX, u64::MAX, 0o666, 0]);
}

#[test]
fn filesystem_hard_link_rename_keeps_both_names_and_replays_inode_sharing() {
    let (temp, mut host) = setup();
    std::fs::hard_link(temp.path().join("old"), temp.path().join("other")).unwrap();
    let tree = host.snapshot().unwrap();
    assert_eq!(tree.file_aliases["other"], "old");
    let Completion::Handle(Ok(dir)) = host
        .call(Request::Directory {
            io: 1,
            path: "".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        host.call(Request::Rename {
            directory: dir,
            from: "old".into(),
            to: "other".into()
        }),
        Ok(Completion::Unit(Ok(())))
    );
    assert!(temp.path().join("old").exists());
    let Completion::Handle(Ok(file)) = host
        .call(Request::Create {
            directory: dir,
            path: "other".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    host.call(Request::FileAppend {
        directory: dir,
        file,
        bytes: b"shared".to_vec(),
    })
    .unwrap();
    host.call(Request::Close { handle: file }).unwrap();
    assert_eq!(
        host.call(Request::Read {
            directory: dir,
            path: "old".into()
        }),
        Ok(Completion::Bytes(Ok(b"shared".to_vec())))
    );
    host.call(Request::Rename {
        directory: dir,
        from: "old".into(),
        to: "new".into(),
    })
    .unwrap();
    host.call(Request::Remove {
        directory: dir,
        path: "other".into(),
    })
    .unwrap();
    let final_tree = host.snapshot().unwrap();
    let record = hirsel_rt::launch::Recording {
        arguments: vec![],
        run_identity: [0; 32],
        hash_seed: 0,
        random_seed: 0,
        test_seed: 0,
        binding: None,
        tree,
        events: host.tape().to_vec(),
    };
    let decoded =
        hirsel_rt::launch::parse_recording(&hirsel_rt::launch::render_recording(&record).unwrap())
            .unwrap();
    assert_eq!(decoded, record);
    drop(host);
    drop(temp);
    let mut replay = SimIo::new(decoded.tree, decoded.events.clone());
    for event in decoded.events {
        assert_eq!(replay.call(event.request), Ok(event.completion));
    }
    replay.finish().unwrap();
    assert_eq!(replay.tree(), &final_tree);
}

#[test]
fn filesystem_mount_pins_survive_rename_and_removal_before_directory_projection() {
    use hirsel_run_config::Access;
    let temp = Disposable::new();
    std::fs::create_dir(temp.path().join("sub")).unwrap();
    let pin = |path: &Path| hirsel_rt::host_paths::PinnedDirectory::open(path).unwrap();
    let mut host = RealIo::from_pinned_mounts(
        [
            ("wide".into(), Access::ReadWrite, pin(temp.path())),
            (
                "narrow".into(),
                Access::ReadWrite,
                pin(&temp.path().join("sub")),
            ),
        ],
        std::io::sink(),
    )
    .unwrap();
    let tree = host.snapshot().unwrap();
    let Completion::Handle(Ok(wide)) = host
        .call(Request::Directory {
            io: 1,
            path: "wide".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    host.call(Request::Rename {
        directory: wide,
        from: "sub".into(),
        to: "moved".into(),
    })
    .unwrap();
    let Completion::Handle(Ok(narrow)) = host
        .call(Request::Directory {
            io: 1,
            path: "narrow".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        host.call(Request::Mkdir {
            directory: narrow,
            path: "child".into()
        }),
        Ok(Completion::Unit(Ok(())))
    );
    host.call(Request::RemoveDir {
        directory: narrow,
        path: "child".into(),
    })
    .unwrap();
    host.call(Request::RemoveDir {
        directory: wide,
        path: "moved".into(),
    })
    .unwrap();
    let Completion::Handle(Ok(removed)) = host
        .call(Request::Directory {
            io: 1,
            path: "narrow".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        host.call(Request::ReadDir {
            directory: removed,
            path: "".into()
        }),
        Ok(Completion::Entries(Ok(vec![])))
    );
    assert_eq!(
        host.call(Request::Mkdir {
            directory: removed,
            path: "child".into()
        }),
        Ok(Completion::Unit(Err(IoError::NotFound)))
    );
    let final_tree = host.snapshot().unwrap();
    let tape = host.tape().to_vec();
    drop(host);
    drop(temp);
    let mut replay = SimIo::new(tree, tape.clone());
    replay
        .set_mounts([
            ("wide".into(), Access::ReadWrite),
            ("narrow".into(), Access::ReadWrite),
        ])
        .unwrap();
    for event in tape {
        assert_eq!(replay.call(event.request), Ok(event.completion));
    }
    replay.finish().unwrap();
    assert_eq!(replay.tree(), &final_tree);
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
        host.call(Request::ReadDir {
            directory: nested,
            path: "".into()
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
    let text = hirsel_rt::launch::render_tape(&tape).unwrap();
    assert_eq!(hirsel_rt::launch::parse_tape(&text).unwrap(), tape);
    drop(host);
    drop(root);
    drop(outside);
    let mut replay = SimIo::new(initial.clone(), tape.clone());
    for event in &tape {
        assert_eq!(
            replay.call(event.request.clone()).unwrap(),
            event.completion
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
    let text = hirsel_rt::launch::render_tape(&tape).unwrap();
    assert_eq!(hirsel_rt::launch::parse_tape(&text).unwrap(), tape);
    let mut sim = SimIo::new(initial(), tape.clone());
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(event.completion.clone())
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

#[test]
fn a_durable_journal_of_a_host_run_recovers_at_every_record_boundary_and_replays() {
    use hirsel_rt::journal::{DurableJournal, recover};
    use hirsel_rt::launch::Recording;
    let (temp, io) = setup();
    let path = temp.path().join("../").join(format!(
        "{}.journal",
        temp.path().file_name().unwrap().to_str().unwrap()
    ));
    let header = Recording {
        arguments: vec![],
        run_identity: [1; 32],
        hash_seed: 0,
        random_seed: 0,
        test_seed: 0,
        binding: None,
        tree: initial(),
        events: vec![],
    };
    let file = std::fs::File::create(&path).unwrap();
    let mut real = Library(io);
    real.0
        .journal_to(Box::new(DurableJournal::create(file, &header).unwrap()));
    actions(&mut real);
    let outcome = hirsel_run_config::RunOutcome::Returned(hirsel_run_config::Returned::Ok);
    real.0.finish_recording(&outcome).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let tape = real.0.tape().to_vec();
    let recovered = recover(&bytes).unwrap();
    assert!(recovered.complete());
    assert_eq!(recovered.outcome, Some(outcome));
    assert_eq!(recovered.recording.events, tape);
    assert_eq!(recovered.recording.tree, initial());
    // Every request was prepared, in order, before its completion.
    assert_eq!(
        recovered
            .prepared
            .iter()
            .map(|p| &p.request)
            .collect::<Vec<_>>(),
        tape.iter().map(|e| &e.request).collect::<Vec<_>>()
    );
    // A crash at any byte leaves a valid prefix: its events begin the tape, and its requests
    // in doubt are exactly the prepared ones it does not complete.
    let mut boundaries = 0;
    for cut in 0..bytes.len() {
        let Ok(prefix) = recover(&bytes[..cut]) else {
            continue;
        };
        boundaries += usize::from(prefix.valid == cut);
        assert!(tape.starts_with(&prefix.recording.events));
        let completed = prefix.recording.events.len();
        let doubt: Vec<_> = prefix.prepared[completed..].to_vec();
        assert_eq!(prefix.in_doubt, doubt, "{cut}");
        assert_eq!(prefix.outcome, None);
    }
    assert!(boundaries > 2 * tape.len());
    // The complete journal replays exactly as its tape.
    let mut sim = SimIo::new(recovered.recording.tree.clone(), recovered.recording.events);
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(event.completion.clone())
        );
    }
    sim.finish().unwrap();
    assert_eq!(sim.stdout(), real.0.stdout());
    assert_eq!(sim.tree(), &real.0.snapshot().unwrap());
}
