use crate::support::library::{Io, Library};
use hirsel_rt::{io::*, io_real::RealIo};
fn temp() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let p = std::env::temp_dir().join(format!(
        "hirsel-handles-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir(&p).unwrap();
    p
}
#[test]
fn narrow_and_commit_consume_handles_on_success_fault_cancellation_and_nonfit() {
    for outcome in [None, Some(IoError::Fault(99)), Some(IoError::Canceled(1))] {
        let p = temp();
        std::fs::create_dir(p.join("sub")).unwrap();
        std::fs::write(p.join("file"), b"x").unwrap();
        let mut library = Library(RealIo::grant(&p).unwrap());
        let root = library.directory(Io(1), "").unwrap().unwrap();
        let sub = library.narrow_dir(root, "sub").unwrap().unwrap();
        let file = library.open(root, "file").unwrap().unwrap();
        let old = file.0;
        if let Some(error) = outcome.clone() {
            library.0.inject(Fault { error, partial: 9 });
        }
        let next = library.narrow_file(root, file).unwrap();
        assert!(!library.0.handles().observed().contains_key(&old));
        if outcome.is_none() {
            library.close_file(next.unwrap()).unwrap().unwrap();
        } else {
            assert!(next.is_none());
        }
        let file = library.open(root, "file").unwrap().unwrap();
        let old = file.0;
        assert_eq!(library.narrow_file(sub, file), Ok(None));
        assert!(!library.0.handles().observed().contains_key(&old));
        let atomic = library.create_atomic(root, "dst").unwrap().unwrap();
        let old = atomic.0;
        if let Some(error) = outcome.clone() {
            // Commit has no byte prefix; only writes admit partial file faults.
            library.0.inject(Fault { error, partial: 0 });
        }
        let result = library.commit(root, atomic).unwrap();
        assert_eq!(result, outcome.clone().map_or(Ok(()), Err));
        assert!(!library.0.handles().observed().contains_key(&old));
        assert!(!std::fs::read_dir(&p).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("hirsel-tmp")
        }));
        drop(library);
        std::fs::remove_dir_all(p).unwrap();
    }
}
#[test]
fn bound_runtime_close_thunk_aborts_atomic_and_closes_once_without_ambient_state() {
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
    let p = temp();
    let real = Rc::new(RefCell::new(RealIo::grant(&p).unwrap()));
    let mut library = Library(Shared(real.clone()));
    let dir = library.directory(Io(1), "").unwrap().unwrap();
    let atomic = library.create_atomic(dir, "abort").unwrap().unwrap();
    let mut binding = Box::new(Binding::new(Shared(real.clone())));
    let config = hirsel_rt::Config {
        close_handle: Some(hirsel_rt_v2_io_close),
        ..hirsel_rt::Config::new()
    };
    let mut ctx = std::ptr::null_mut();
    unsafe {
        assert_eq!(
            hirsel_rt::context::hirsel_rt_v2_ctx_create(&config, &mut ctx),
            0
        );
        binding.bind(&mut *ctx);
        assert_eq!(hirsel_rt_v2_io_close(ctx, atomic.0, 8, 8), 0);
        (*ctx).bind_io_data(std::ptr::null_mut());
        hirsel_rt::context::hirsel_rt_v2_ctx_destroy(ctx);
    }
    assert!(!real.borrow().handles().observed().contains_key(&atomic.0));
    assert!(!p.join("abort.hirsel-tmp-3").exists());
    assert!(
        matches!(real.borrow().tape().last().unwrap().request,Request::Close {handle} if handle==atomic.0)
    );
    assert_eq!(
        real.borrow_mut().call(Request::Close { handle: atomic.0 }),
        Err(AdapterError::InvalidHandle(atomic.0))
    );
    drop(binding);
    drop(library);
    drop(real);
    std::fs::remove_dir_all(p).unwrap();
}
