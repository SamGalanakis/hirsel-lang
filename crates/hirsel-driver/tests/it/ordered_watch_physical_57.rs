//! Inject a real filesystem event only after entered WatchNext returns its stack.
use hirsel_driver::native::{self, Backend, OptLevel, Partition, TargetSpec, Toolchain};
use hirsel_engine::*;

#[test]
fn watch_pending_releases_its_pooled_stack_before_external_event_and_replays() {
    with_compiler_stack(|| {
        let mut engine = crate::source_std::engine();
        let source = r#"
pub type E = Grant(GrantError) | Files(FileError)
pub fn main(rt:Io)->Result[(),E] {
  let tasks=io.tasks(rt)?Grant
  let output=io.stdout(rt)?Grant
  let d=io.dir(rt,path.of("data"))?Files
  let value=task.scope(tasks,fn(s:Scope)->u64 {
    let child=task.async(s,fn()->u64 {
      let mut watch=match file.watch(d,path.of(""),false) {
        Ok(w)=>take w,
        Error(_)=>return 0u64,
      }
      match watcher.next(d,mut watch) {
        Ok(Created(p))=>if path.text(p)=="wake" { 7u64 } else { 0u64 },
        _=>0u64,
      }
    })
    task.await(s,take child)
  })
  failure.check(value==7u64,Arithmetic())
  let _=out.print(output,f"{value}")
  Ok(())
}
"#;
        let file = engine.sources.add("main.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let sites = native::Sites::new(&input);
        let validated = engine.import_native_input(input.clone()).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let program = native::build(&validated, &native::Options::new(target.clone())).unwrap();
        let root = tempfile::Builder::new()
            .prefix("watch57-physical-")
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
                let data = dir.join("root/data");
                std::fs::create_dir_all(&data).unwrap();
                std::fs::write(
                    &grants,
                    serde_json::json!({
                        "io": {"stdout":"stdout.bytes@1", "tasks":"tasks.cooperative@1",
                          "dirs":[{"path":"data","host":data,"access":"read_write"}]},
                        "args":[], "placement":{"cores":width}
                    })
                    .to_string(),
                )
                .unwrap();
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
                        // Replay consumes the selected event; it needs no live watcher.
                        std::fs::remove_dir_all(&data).unwrap();
                        command.arg("--hirsel-replay").arg(&tape);
                        finish(command.spawn().unwrap(), &cell)
                    } else {
                        command
                            .arg("--hirsel-record")
                            .arg(&tape)
                            .arg("--hirsel-grants")
                            .arg(&grants);
                        let mut process = command.spawn().unwrap();
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(10);
                        loop {
                            let text = std::fs::read_to_string(&trace).unwrap_or_default();
                            if pending_left(&text) {
                                std::fs::File::create(data.join("wake")).unwrap();
                                break;
                            }
                            if process.try_wait().unwrap().is_some() {
                                let output = process.wait_with_output().unwrap();
                                panic!(
                                    "{cell}: returned before entered watch leave: {text}, {output:?}"
                                );
                            }
                            if std::time::Instant::now() >= deadline {
                                process.kill().unwrap();
                                let output = process.wait_with_output().unwrap();
                                panic!("{cell}: no entered watch leave: {text}, {output:?}");
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
                        "{cell}: no retained watch stack release"
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
            && w.get(4) == Some(&"35")
            && w.get(5) == Some(&"host-park")
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
            panic!("{cell}: watch did not complete: {output:?}");
        }
        std::thread::yield_now();
    }
}
