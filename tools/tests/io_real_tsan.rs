//! Eight-worker real-adapter control, also the small independent-file scaling probe.
use hirsel_rt::io::{Completion, Request};
use hirsel_rt::io_concurrent::ConcurrentAdapter;
use hirsel_rt::io_real::RealIo;
use std::sync::{Arc, Barrier};

fn handle(result: Completion) -> u64 {
    match result {
        Completion::Handle(Ok(handle)) => handle,
        other => panic!("{other:?}"),
    }
}
fn main() {
    if std::env::args().any(|v| v == "--require-tsan") {
        assert_eq!(
            option_env!("HIRSEL_SANITIZER"),
            Some("thread"),
            "uninstrumented sanitizer control"
        );
    }
    let width = std::env::args()
        .nth(1)
        .map_or(8, |v| v.parse::<usize>().unwrap());
    assert!([1, 2, 4, 8].contains(&width));
    let rounds = std::env::args()
        .nth(2)
        .map_or(16, |v| v.parse::<usize>().unwrap());
    let temp = tempfile::tempdir().unwrap();
    let io = RealIo::grant(temp.path())
        .unwrap()
        .into_concurrent()
        .unwrap_or_else(|_| panic!("Send streams"));
    let directory = handle(
        io.call(Request::Directory {
            io: 1,
            path: String::new(),
        })
        .unwrap(),
    );
    let mut files = Vec::new();
    for i in 0..8 {
        files.push(handle(
            io.call(Request::Create {
                directory,
                path: format!("file{i}"),
            })
            .unwrap(),
        ));
    }
    let io = Arc::new(io);
    let barrier = Arc::new(Barrier::new(width));
    let start = std::time::Instant::now();
    std::thread::scope(|scope| {
        for worker in 0..width {
            let io = &io;
            let barrier = &barrier;
            let files = &files;
            scope.spawn(move || {
                barrier.wait();
                for index in (worker..8).step_by(width) {
                    for _ in 0..rounds {
                        let bytes = vec![index as u8; 64 * 1024];
                        assert_eq!(
                            io.call_at(
                                worker as u64 + 1,
                                Request::FileAppend {
                                    directory,
                                    file: files[index],
                                    bytes
                                }
                            ),
                            Ok(Completion::Unit(Ok(())))
                        );
                    }
                }
            });
        }
    });
    for i in 0..8 {
        assert_eq!(
            std::fs::read(temp.path().join(format!("file{i}"))).unwrap(),
            vec![i as u8; rounds * 64 * 1024]
        );
    }
    println!(
        "workers={width} files=8 rounds={rounds} elapsed_ns={}",
        start.elapsed().as_nanos()
    );
}
