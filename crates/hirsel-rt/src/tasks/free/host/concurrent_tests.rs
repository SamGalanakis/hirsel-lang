//! Real ConcurrentIo through native Free phases, with replay metadata only.
use super::*;
use crate::io_concurrent::ConcurrentAdapter;
use crate::io_keys::{Kind, Registry};
use hirsel_free::projection::Projection;

unsafe fn request(
    ctx: *mut TaskCtx,
    request: Request,
    adapter: Option<&dyn ConcurrentAdapter>,
    registry: &mut Registry,
) -> Completion {
    let mut result = unsafe {
        if let Some(adapter) = adapter {
            crate::stop::check(
                ctx,
                prepare_with_keys(ctx, request.clone(), adapter.keys(&request)),
            );
            assert!(crate::stop::check(ctx, claim(ctx)));
            let token = (*ctx).pending_host.as_ref().unwrap().request.id;
            crate::stop::check(ctx, invoke_concurrent(ctx, adapter));
            crate::stop::check(ctx, ended(ctx));
            let result = crate::stop::check(ctx, complete_result(ctx));
            let _segment = role::Segment::new(ctx);
            adapter.finish(token);
            result
        } else {
            crate::stop::check(
                ctx,
                replay_request(ctx, request.clone(), registry.keys(&request)),
            )
        }
    };
    for word in handles::fields(&mut result).unwrap().into_iter().flatten() {
        let (key, kind) = if let Some(adapter) = adapter {
            adapter.identity(*word).expect("registered live result")
        } else {
            crate::stop::check(ctx, replay_created_identity(ctx, *word))
        };
        registry.bind(*word, key, kind);
    }
    result.unwrap()
}

unsafe fn file_program(ctx: *mut TaskCtx, adapter: Option<&dyn ConcurrentAdapter>) {
    let mut registry = Registry::default();
    let directory = unsafe {
        request(
            ctx,
            Request::Directory {
                io: 1,
                path: String::new(),
            },
            adapter,
            &mut registry,
        )
    };
    let Completion::Handle(Ok(directory)) = directory else {
        panic!("directory")
    };
    let file = unsafe {
        request(
            ctx,
            Request::Create {
                directory,
                path: "data".into(),
            },
            adapter,
            &mut registry,
        )
    };
    let Completion::Handle(Ok(file)) = file else {
        panic!("file")
    };
    let (resource, kind) = registry.identity(file).unwrap();
    assert_eq!(kind, Kind::File);
    assert_ne!(resource, crate::io_keys::COUPLED);
    let bytes = b"real concurrent host result\0\xff".to_vec();
    assert_eq!(
        unsafe {
            request(
                ctx,
                Request::FileWrite {
                    directory,
                    file,
                    bytes: bytes.clone(),
                },
                adapter,
                &mut registry,
            )
        },
        Completion::Size(Ok(bytes.len() as u64))
    );
    assert_eq!(
        unsafe {
            request(
                ctx,
                Request::FileSeek {
                    directory,
                    file,
                    offset: 0,
                },
                adapter,
                &mut registry,
            )
        },
        Completion::Unit(Ok(()))
    );
    assert_eq!(
        unsafe {
            request(
                ctx,
                Request::FileRead {
                    directory,
                    file,
                    max: 4096,
                },
                adapter,
                &mut registry,
            )
        },
        Completion::Bytes(Err(crate::io::IoError::Fault(1)))
    );
    let alias = unsafe {
        request(
            ctx,
            Request::Open {
                directory,
                path: "data".into(),
            },
            adapter,
            &mut registry,
        )
    };
    let Completion::Handle(Ok(alias)) = alias else {
        panic!("file alias")
    };
    assert_ne!(file, alias);
    assert_eq!(registry.identity(alias), Some((resource, Kind::File)));
    assert_eq!(
        unsafe { request(ctx, Request::Close { handle: file }, adapter, &mut registry) },
        Completion::Unit(Ok(()))
    );
    assert_eq!(
        unsafe {
            request(
                ctx,
                Request::FileRead {
                    directory,
                    file: alias,
                    max: 4096,
                },
                adapter,
                &mut registry,
            )
        },
        Completion::Bytes(Ok(bytes.clone()))
    );
    let mapped = unsafe {
        request(
            ctx,
            Request::Map {
                directory,
                path: "data".into(),
            },
            adapter,
            &mut registry,
        )
    };
    let Completion::Handle(Ok(mapped)) = mapped else {
        panic!("mapping")
    };
    assert_eq!(registry.identity(mapped), Some((resource, Kind::File)));
    let view = unsafe {
        request(
            ctx,
            Request::MapView { directory, mapped },
            adapter,
            &mut registry,
        )
    };
    assert_eq!(
        unsafe {
            request(
                ctx,
                Request::Close { handle: mapped },
                adapter,
                &mut registry,
            )
        },
        Completion::Unit(Ok(()))
    );
    let Completion::View(view) = view else {
        panic!("view")
    };
    assert_eq!(view.block, 0);
    assert_eq!(view.contents(), bytes);
    assert_eq!(
        unsafe {
            request(
                ctx,
                Request::Close { handle: alias },
                adapter,
                &mut registry,
            )
        },
        Completion::Unit(Ok(()))
    );
}

#[test]
fn real_concurrent_files_discover_aliases_and_replay_without_mount_or_adapter() {
    for durable in [false, true] {
        let path = std::env::temp_dir().join(format!(
            "hirsel-f1-real-concurrent-{}-{durable}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        let mut real = crate::io_real::RealIo::grant(&path).unwrap();
        real.discard_history();
        let io = Arc::new(
            real.into_concurrent()
                .unwrap_or_else(|_| panic!("built-in Send adapter")),
        );
        let recording =
            super::super::record::Recording::new(b"real concurrent free files", durable);
        let install = recording.clone();
        let observed = io.clone();
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                let _role = role::Guard::new(ctx);
                file_program(ctx, Some(&*observed));
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(
            std::fs::read(path.join("data")).unwrap(),
            b"real concurrent host result\0\xff"
        );
        drop(io);
        std::fs::remove_dir_all(path).unwrap();
        let cut = recording
            .recorder
            .stop("returned", None)
            .unwrap()
            .unwrap()
            .0;
        let projection = Projection::new("returned", &cut.progress, &recording.trace());
        let bytes = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
        for width in [1, 8] {
            let replay = super::super::replay::Replay::new(&bytes).unwrap();
            let install = replay.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    let _role = role::Guard::new(ctx);
                    file_program(ctx, None);
                    super::super::replay::start(ctx);
                    unreachable!("real file replay past cut");
                },
            );
            assert!(
                matches!(outcome.ended, Err(Stop::Blocked(_))),
                "{outcome:?}"
            );
            assert_eq!(replay.finish().unwrap(), projection);
        }
    }
}
