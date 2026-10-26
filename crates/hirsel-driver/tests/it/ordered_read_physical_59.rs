//! Real stdin is supplied only after the entered read has returned its stack.
use hirsel_driver::native::{self, Backend, OptLevel, Partition, TargetSpec, Toolchain};
use hirsel_engine::*;

#[test]
fn retained_stdin_pending_releases_its_pooled_stack_and_replays_on_both_backends() {
    with_compiler_stack(|| {
        let mut engine = crate::source_std::engine();
        let source = super::ordered_read_resume_59::STDIN;
        let file = engine.sources.add("main.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let sites = native::Sites::new(&input);
        let validated = engine.import_native_input(input.clone()).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let program = native::build(&validated, &native::Options::new(target.clone())).unwrap();
        let root = tempfile::Builder::new()
            .prefix("read59-physical-")
            .tempdir()
            .unwrap();
        let tools =
            Toolchain::in_tree_diagnostic(&root.path().join("runtime"), &[&target]).unwrap();
        let entry = entry_path(&default_context());
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let objects = native::emit_backend_objects(
                &program,
                backend,
                OptLevel::Default,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            for width in [1, 4] {
                let dir = root.path().join(format!("{backend:?}-{width}"));
                std::fs::create_dir_all(dir.join("root")).unwrap();
                let exe = native::link_standalone_objects(&program, &entry, &objects, &tools, &dir)
                    .unwrap();
                let grants = dir.join("grants.json");
                std::fs::write(&grants,format!(r#"{{"io":{{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","stdin":"stdin.bytes@1","dirs":[]}},"args":[],"placement":{{"cores":{width}}}}}"#)).unwrap();
                let tape = dir.join("read.tape");
                let mut expected = None;
                for replay in [false, true] {
                    let observation = dir.join(format!("observation-{replay}"));
                    let trace = dir.join(format!("compact-{replay}"));
                    let mut command = std::process::Command::new(&exe);
                    command
                        .env_clear()
                        .env("HIRSEL_RT_ROOT", dir.join("root"))
                        .env("HIRSEL_RT_DIAGNOSTIC", "1")
                        .env("HIRSEL_RT_OBSERVATION", &observation)
                        .env("HIRSEL_RT_COMPACT_TRACE", &trace)
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped());
                    let cell = format!("{backend:?} width={width} replay={replay}");
                    let output = if replay {
                        command.arg("--hirsel-replay").arg(&tape);
                        finish(command.spawn().unwrap(), &cell)
                    } else {
                        use std::io::Write;
                        command
                            .arg("--hirsel-record")
                            .arg(&tape)
                            .arg("--hirsel-grants")
                            .arg(&grants)
                            .stdin(std::process::Stdio::piped());
                        let mut process = command.spawn().unwrap();
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(10);
                        loop {
                            let text = std::fs::read_to_string(&trace).unwrap_or_default();
                            if pending_left(&text) {
                                process.stdin.take().unwrap().write_all(b"G\0\xff").unwrap();
                                break;
                            }
                            if process.try_wait().unwrap().is_some() {
                                let output = process.wait_with_output().unwrap();
                                panic!(
                                    "{cell}: returned before entered read leave: {text}, {output:?}"
                                );
                            }
                            if std::time::Instant::now() >= deadline {
                                process.kill().unwrap();
                                let output = process.wait_with_output().unwrap();
                                panic!("{cell}: no entered read leave: {text}, {output:?}");
                            }
                            std::thread::yield_now();
                        }
                        finish(process, &cell)
                    };
                    let launched = native::Launched {
                        audit_required: true,
                        exit: output.status.code(),
                        signal: None,
                        stdout: output.stdout,
                        stderr: output.stderr,
                        record: std::fs::read_to_string(&observation).unwrap(),
                    };
                    let actual = native::observe(&launched, &program, &sites).unwrap();
                    assert_eq!(
                        actual.output,
                        b"7",
                        "{cell}: {actual:?}, {}",
                        String::from_utf8_lossy(&launched.stderr)
                    );
                    if let Some(expected) = &expected {
                        assert_eq!(&actual, expected, "{cell}")
                    } else {
                        expected = Some(actual)
                    }
                    assert!(
                        pending_left(&std::fs::read_to_string(&trace).unwrap()),
                        "{cell}: no retained read stack release"
                    );
                }
            }
        }
    });
}
fn pending_left(text: &str) -> bool {
    let lines: Vec<Vec<_>> = text
        .lines()
        .map(|l| l.split_whitespace().collect())
        .collect();
    lines.iter().enumerate().any(|(i, w)| {
        w.first() == Some(&"runtime")
            && w.get(4) == Some(&"34")
            && lines[i + 1..].iter().any(|l| {
                l.first() == Some(&"leave") && l.get(1) == w.get(1) && l.get(4) == Some(&"1")
            })
    })
}
fn finish(mut child: std::process::Child, cell: &str) -> std::process::Output {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!("{cell}: read did not complete: {output:?}");
        }
        std::thread::yield_now();
    }
}
