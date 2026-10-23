use crate::support;
use hirsel_rt::io::*;
use hirsel_rt::io_sim::SimIo;
use std::sync::{Arc, Mutex};

struct Pages {
    seen: Arc<Mutex<Vec<Request>>>,
}
impl Adapter for Pages {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        self.seen.lock().unwrap().push(request.clone());
        let Request::ReadDirPage {
            after,
            recursive: false,
            ..
        } = request
        else {
            panic!("{request:?}")
        };
        Ok(Completion::Entries(if after.is_empty() {
            Ok((0..128)
                .map(|n| DirEntry {
                    path: format!("{n:04}"),
                    kind: FileKind::Regular,
                })
                .collect())
        } else {
            Err(IoError::PermissionDenied)
        }))
    }
}
#[test]
fn read_dir_stream_error_follows_the_page_and_fuses_without_reading_ahead() {
    let ctx = support::Ctx::diagnostic();
    let seen = Arc::new(Mutex::new(vec![]));
    let mut binding = Box::new(Binding::new(Pages { seen: seen.clone() }));
    let path = binding.intern_path("");
    unsafe {
        binding.bind(&mut *ctx.0);
    }
    let mut out = std::mem::MaybeUninit::<WalkResult>::uninit();
    assert_eq!(
        unsafe {
            hirsel_rt_v2_file_read_dir(ctx.0, std::ptr::null_mut(), out.as_mut_ptr(), &7, &path)
        },
        0
    );
    let out = unsafe { out.assume_init() };
    assert_eq!(out.tag, 0);
    let iterator = out.value;
    for n in 0..128 {
        let mut entry = DirEntryOption::default();
        assert_eq!(
            unsafe {
                hirsel_rt_v2_file_read_dir_step(
                    ctx.0,
                    iterator.environment,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut entry,
                )
            },
            0
        );
        assert_eq!((entry.tag, entry.result, entry.kind), (1, 0, 0));
        assert_eq!(entry.path, binding.intern_path(&format!("{n:04}")));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
    let mut entry = DirEntryOption::default();
    unsafe {
        hirsel_rt_v2_file_read_dir_step(
            ctx.0,
            iterator.environment,
            0,
            0,
            std::ptr::null_mut(),
            &mut entry,
        );
    }
    assert_eq!((entry.tag, entry.result), (1, 1));
    unsafe {
        hirsel_rt_v2_file_read_dir_step(
            ctx.0,
            iterator.environment,
            0,
            0,
            std::ptr::null_mut(),
            &mut entry,
        );
    }
    assert_eq!(entry.tag, 0);
    assert_eq!(seen.lock().unwrap().len(), 2);
    unsafe {
        hirsel_rt::release::hirsel_rt_v2_release(
            ctx.0,
            iterator.environment,
            (*iterator.environment).index(),
            std::ptr::null(),
            0,
            0,
        );
    }
    unsafe {
        (*ctx.0).bind_io_data(std::ptr::null_mut());
    }
    assert_eq!(ctx.stats().live_blocks, 0);
}
#[test]
fn read_dir_stream_pages_cover_byte_neighbors_and_replay_checks_the_cursor() {
    let mut tree = Tree::new(Default::default()).unwrap();
    tree.files
        .extend((0..517).map(|n| (format!("{n:04}"), vec![])));
    tree.directories.insert("a".into());
    tree.files.extend([
        ("a-".into(), vec![]),
        ("a.c".into(), vec![]),
        ("a0".into(), vec![]),
        ("é".into(), vec![]),
    ]);
    let temp = tempfile::tempdir().unwrap();
    for p in &tree.directories {
        if !p.is_empty() {
            std::fs::create_dir_all(temp.path().join(p)).unwrap();
        }
    }
    for p in tree.files.keys() {
        std::fs::write(temp.path().join(p), b"").unwrap();
    }
    let mut io = hirsel_rt::io_real::RealIo::grant(temp.path()).unwrap();
    let Completion::Handle(Ok(d)) = io
        .call(Request::Directory {
            io: 1,
            path: "".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    let mut names = vec![];
    let mut requests = vec![];
    let mut after = String::new();
    loop {
        let request = Request::ReadDirPage {
            directory: d,
            path: "".into(),
            after: after.clone(),
            recursive: false,
        };
        let Completion::Entries(Ok(entries)) = io.call(request.clone()).unwrap() else {
            panic!()
        };
        assert!(entries.len() <= 128);
        requests.push(request);
        let Some(last) = entries.last() else {
            break;
        };
        after = last.path.clone();
        names.extend(entries.into_iter().map(|e| e.path));
    }
    let mut expected: Vec<_> = tree
        .files
        .keys()
        .chain(tree.directories.iter())
        .filter(|s| !s.is_empty())
        .cloned()
        .collect();
    expected.sort();
    assert_eq!(names, expected);
    let events = io.tape().to_vec();
    let mut replay = SimIo::new(tree, events);
    replay
        .call(Request::Directory {
            io: 1,
            path: "".into(),
        })
        .unwrap();
    for request in requests {
        replay.call(request).unwrap();
    }
    assert_eq!(replay.position(), io.tape().len());
    assert!(matches!(
        io.call(Request::ReadDirPage {
            directory: 999,
            path: "".into(),
            after: "".into(),
            recursive: false
        }),
        Err(AdapterError::InvalidHandle(999))
    ));
}

#[test]
fn read_dir_stream_refill_skips_insertions_behind_the_cursor_and_keeps_future_names() {
    let temp = tempfile::tempdir().unwrap();
    for n in 0..130 {
        std::fs::write(temp.path().join(format!("{n:04}")), b"").unwrap();
    }
    let mut io = hirsel_rt::io_real::RealIo::grant(temp.path()).unwrap();
    let Completion::Handle(Ok(directory)) = io
        .call(Request::Directory {
            io: 1,
            path: "".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    let request = |after: &str| Request::ReadDirPage {
        directory,
        path: "".into(),
        after: after.into(),
        recursive: false,
    };
    let Completion::Entries(Ok(first)) = io.call(request("")).unwrap() else {
        panic!()
    };
    assert_eq!(first.len(), 128);
    std::fs::write(temp.path().join("0000-new"), b"").unwrap();
    std::fs::remove_file(temp.path().join("0128")).unwrap();
    std::fs::write(temp.path().join("0130"), b"").unwrap();
    let Completion::Entries(Ok(next)) = io.call(request(&first.last().unwrap().path)).unwrap()
    else {
        panic!()
    };
    assert_eq!(
        next.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
        ["0129", "0130"]
    );
    assert_eq!(first[0].path, "0000");
}
