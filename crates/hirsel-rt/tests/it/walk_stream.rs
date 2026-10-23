//! Streaming contract at the native entry, independently of filesystem setup cost.
use crate::support;
use hirsel_rt::io::*;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

struct Pages {
    pages: BTreeMap<String, Result<Vec<DirEntry>, IoError>>,
    reads: Arc<Mutex<Vec<String>>>,
}
impl Adapter for Pages {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        let Request::ReadDirPage { path, .. } = request else {
            panic!("unexpected {request:?}")
        };
        self.reads.lock().unwrap().push(path.clone());
        Ok(Completion::Entries(
            self.pages.get(&path).expect("unexpected directory").clone(),
        ))
    }
}
fn entry(path: &str, kind: FileKind) -> DirEntry {
    DirEntry {
        path: path.into(),
        kind,
    }
}
unsafe fn open(ctx: &support::Ctx) -> WalkResult {
    let mut out = std::mem::MaybeUninit::uninit();
    assert_eq!(
        unsafe { hirsel_rt_v2_file_walk(ctx.0, std::ptr::null_mut(), out.as_mut_ptr(), &7) },
        0
    );
    unsafe { out.assume_init() }
}
unsafe fn step(ctx: &support::Ctx, it: WalkIterator) -> WalkOption {
    let mut out = WalkOption::default();
    assert_eq!(
        unsafe {
            hirsel_rt_v2_file_walk_step(ctx.0, it.environment, 0, 0, std::ptr::null_mut(), &mut out)
        },
        0
    );
    out
}
unsafe fn release(ctx: &support::Ctx, it: WalkIterator) {
    unsafe {
        hirsel_rt::release::hirsel_rt_v2_release(
            ctx.0,
            it.environment,
            (*it.environment).index(),
            std::ptr::null(),
            0,
            0,
        )
    };
}
#[test]
fn walk_stream_descendant_error_follows_earlier_paths_and_fuses() {
    let ctx = support::Ctx::diagnostic();
    let reads = Arc::new(Mutex::new(vec![]));
    let pages = Pages {
        reads: reads.clone(),
        pages: [
            (
                "".into(),
                Ok(vec![
                    entry("a", FileKind::Regular),
                    entry("z", FileKind::Directory),
                ]),
            ),
            ("z".into(), Err(IoError::PermissionDenied)),
        ]
        .into(),
    };
    let mut binding = Box::new(Binding::new(pages));
    let a = binding.intern_path("a");
    unsafe {
        binding.bind(&mut *ctx.0);
        let out = open(&ctx);
        assert_eq!(out.tag, 0);
        assert_eq!(*reads.lock().unwrap(), [""]);
        assert_eq!(step(&ctx, out.value).value, HandleResult::of(Ok(a)));
        assert_eq!(*reads.lock().unwrap(), [""]);
        let error = step(&ctx, out.value);
        assert_eq!(error.tag, 1);
        assert_eq!(
            error.value,
            HandleResult::of(Err(IoError::PermissionDenied))
        );
        for _ in 0..3 {
            assert_eq!(step(&ctx, out.value).tag, 0);
        }
        assert_eq!(*reads.lock().unwrap(), ["", "z"]);
        release(&ctx, out.value);
        (*ctx.0).bind_io_data(std::ptr::null_mut());
    }
    assert_eq!(ctx.stats().live_blocks, 0);
}
#[test]
fn walk_stream_root_error_is_outer_without_an_environment() {
    let ctx = support::Ctx::diagnostic();
    let mut binding = Box::new(Binding::new(Pages {
        reads: Default::default(),
        pages: [("".into(), Err(IoError::PermissionDenied))].into(),
    }));
    unsafe {
        binding.bind(&mut *ctx.0);
        assert_eq!(open(&ctx).tag, 1);
        (*ctx.0).bind_io_data(std::ptr::null_mut());
    }
    assert_eq!(ctx.stats().live_blocks, 0);
}
#[test]
fn walk_stream_take_ten_and_drop_does_not_read_the_later_subtree() {
    let ctx = support::Ctx::diagnostic();
    let reads = Arc::new(Mutex::new(vec![]));
    let mut entries: Vec<_> = (0..10)
        .map(|i| entry(&format!("a{i}"), FileKind::Regular))
        .collect();
    entries.push(entry("z", FileKind::Directory));
    let mut binding = Box::new(Binding::new(Pages {
        reads: reads.clone(),
        pages: [("".into(), Ok(entries))].into(),
    }));
    unsafe {
        binding.bind(&mut *ctx.0);
        let out = open(&ctx);
        for _ in 0..10 {
            assert_eq!(step(&ctx, out.value).tag, 1);
        }
        release(&ctx, out.value);
        (*ctx.0).bind_io_data(std::ptr::null_mut());
    }
    assert_eq!(*reads.lock().unwrap(), [""]);
    assert_eq!(ctx.stats().live_blocks, 0);
}
#[test]
fn walk_stream_sorts_directory_work_at_slash_and_yields_symlinks() {
    let ctx = support::Ctx::diagnostic();
    let reads = Arc::new(Mutex::new(vec![]));
    let mut binding = Box::new(Binding::new(Pages {
        reads: reads.clone(),
        pages: [
            (
                "".into(),
                Ok(vec![
                    entry("a", FileKind::Directory),
                    entry("a.c", FileKind::Regular),
                    entry("a-", FileKind::Regular),
                    entry("a0", FileKind::Regular),
                    entry("alias", FileKind::Symlink),
                    entry("empty", FileKind::Directory),
                ]),
            ),
            ("a".into(), Ok(vec![entry("a/b", FileKind::Regular)])),
            ("empty".into(), Ok(vec![])),
        ]
        .into(),
    }));
    let expected = ["a-", "a.c", "a/b", "a0", "alias"].map(|p| binding.intern_path(p));
    unsafe {
        binding.bind(&mut *ctx.0);
        let out = open(&ctx);
        for path in expected {
            assert_eq!(step(&ctx, out.value).value, HandleResult::of(Ok(path)));
        }
        assert_eq!(step(&ctx, out.value).tag, 0);
        release(&ctx, out.value);
        (*ctx.0).bind_io_data(std::ptr::null_mut());
    }
    assert_eq!(*reads.lock().unwrap(), ["", "a", "empty"]);
    assert_eq!(ctx.stats().live_blocks, 0);
}
