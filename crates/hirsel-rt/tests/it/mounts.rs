//! Mount authority is selected before an effect and retained through narrowed grants.
use crate::support;
use crate::support::library::{Io, Library};
use crate::support::mounts::Scratch;
use hirsel_rt::{
    host_paths::{PinnedDirectory, RecorderDirectory},
    io::{Adapter, AdapterError, Completion as C, Fault, IoError, Request as R},
    io_real::RealIo,
    io_sim::SimIo,
};
use hirsel_run_config::{Access, RunRefusal};
use std::fs;

#[test]
fn a_foreign_atomic_file_is_refused_before_rights_consumption_or_effect() {
    let scratch = Scratch::new();
    let mut real = Library(scratch.io(&[
        ("read", Access::Read, "one"),
        ("write", Access::ReadWrite, "two"),
    ]));
    let read = real.directory(Io(1), "read").unwrap().unwrap();
    let write = real.directory(Io(1), "write").unwrap().unwrap();
    let mut atomic = real.create_atomic(write, "saved").unwrap().unwrap();
    let handles = real.0.handles().clone();
    let tape_len = real.0.tape().len();
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
        assert_eq!(
            real.0.call(request),
            Err(AdapterError::InvalidHandle(atomic.0))
        );
        assert_eq!(real.0.handles(), &handles);
        assert_eq!(real.0.tape().len(), tape_len);
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
            real.0
                .prepare_recording_with_files(&scratch.0.join(path), &[])
                .unwrap_err(),
            RunRefusal::RecorderOverlap {
                mount: "archive".into()
            }
        );
    }
    assert_eq!(fs::read(scratch.0.join("retained/b")).unwrap(), b"B");
    assert!(!scratch.0.join("retained/tape").exists());
    // A file recorder owns only the leaf, so common-parent placement is accepted.
    real.0
        .prepare_recording_with_files(&scratch.0.join("beside-mounts"), &[])
        .unwrap();
    real.0
        .prepare_recording_with_files(&scratch.0.join("two/tape"), &[])
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
        fn preflight(
            &self,
            request: &hirsel_rt::io::Request,
        ) -> Result<Option<hirsel_rt::io::Completion>, AdapterError> {
            self.0.borrow().preflight(request)
        }
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
            hirsel_rt_v2_io_close(ctx.0, result.value, 0, 0);
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
