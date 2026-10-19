//! A run, not the process, is the unit of failure: one process runs programs one after another,
//! and a run that blocks (CONC-8), passes the codec's operational budget (environmental, in
//! both engines) or fails ends alone, with its own outcome, while runs of other programs before and after it complete. Natively the
//! programs are libraries loaded into one host's runtime, so each failure names its own program's
//! site from that program's failure table.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{
    Backend, Configuration, Engine, OptLevel, Options, Partition, Runtime, TargetSpec, Toolchain,
    build, emit_backend_objects, link_host, link_library, run_host,
};
use hirsel_engine::{
    QueryEngine, RunOutcome, SimulationConfig, check_progress, default_context,
    default_environment, entry_path, with_compiler_stack,
};
use std::path::PathBuf;

const BLOCKS: &str = "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let stdout = io.stdout(rt)?
  let (sender, receiver): (Sender[u8], Receiver[u8]) = channel.open(tasks)
  let mut rx = take receiver
  let _ = out.print(stdout, \"blocks waits\\n\")
  let _ = channel.receive(tasks, mut rx)
  Ok(())
}
";
const COMPLETES: &str = "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let stdout = io.stdout(rt)?
  let _ = out.print(stdout, \"completes ok\\n\")
  Ok(())
}
";
/// Doubles a tree 19 times (a node and its list are two values each), past the codec's
/// 1,000,000-value budget, then encodes it.
const ENCODES: &str = "@serializable
type Tree {
  kids: List[Tree],
}

pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let stdout = io.stdout(rt)?
  let mut tree = Tree(kids: [])
  let mut depth: u64 = 0
  while depth < 19 {
    let mut kids: List[Tree] = []
    list.push(mut kids, copy tree)
    list.push(mut kids, take tree)
    tree = Tree(kids: kids)
    depth += 1
  }
  let _ = out.print(stdout, \"encodes\\n\")
  let bytes = codec.encode(tree)
  let _ = out.print(stdout, \"encoded\\n\")
  Ok(())
}
";
const FAILS: &str = "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let xs: List[u8] = [7u8]
  let i: u64 = 1
  let x = xs[i]
  Ok(())
}
";
const FAILS_LATER: &str = "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let xs: List[u8] = [7u8]
  let i: u64 = 2

  let x = xs[i]
  Ok(())
}
";

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn interpreted_runs_that_block_stop_or_fail_end_alone_in_one_process() {
    with_compiler_stack(|| {
        let dir = scratch("isolation-interpreted");
        let run = |name: &str, source: &str| {
            let path = dir.join(format!("{name}.hir"));
            std::fs::write(&path, source).unwrap();
            let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
            let outcome = hirsel_driver::run_path_with_simulation(
                &path,
                SimulationConfig::default(),
                &mut stdout,
                &mut stderr,
            );
            (outcome, String::from_utf8(stdout).unwrap())
        };
        let completes = |label: &str| {
            let (outcome, stdout) = run("completes", COMPLETES);
            assert_eq!(
                outcome.unwrap(),
                RunOutcome::Returned(hirsel_run_config::Returned::Ok),
                "{label}"
            );
            assert_eq!(stdout, "completes ok\n", "{label}");
        };
        let (outcome, stdout) = run("blocks", BLOCKS);
        assert!(matches!(outcome, Ok(RunOutcome::Blocked(_))), "{outcome:?}");
        assert_eq!(stdout, "blocks waits\n");
        completes("after blocked");
        let (outcome, stdout) = run("encodes", ENCODES);
        assert!(
            matches!(
                outcome,
                Ok(RunOutcome::Environmental(
                    hirsel_run_config::Environmental::CodecLimit
                ))
            ),
            "{outcome:?}"
        );
        assert_eq!(stdout, "encodes\n");
        completes("after the codec limit");
        for (name, source, line) in [("fails", FAILS, 4), ("fails-later", FAILS_LATER, 5)] {
            let (outcome, stdout) = run(name, source);
            let Ok(RunOutcome::Failed {
                primary: hirsel_run_config::Failure { rule, site, .. },
                ..
            }) = outcome
            else {
                panic!("{name}: {outcome:?}")
            };
            assert_eq!(
                (rule.as_str(), site.line, site.column),
                ("MEM-14", line, 11)
            );
            assert!(site.file.ends_with(&format!("{name}.hir")), "{site:?}");
            assert!(stdout.is_empty());
        }
        completes("after the failures");
        std::fs::remove_dir_all(dir).unwrap();
    });
}

fn x86() -> TargetSpec {
    TargetSpec::x86_64_unknown_linux_gnu()
}

#[test]
fn native_runs_that_block_stop_or_fail_end_alone_in_one_host_process_on_both_backends() {
    with_compiler_stack(|| {
        let configuration = Configuration {
            options: Options::new(x86()),
            engine: Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime: Runtime::Default,
            },
        };
        let toolchain = Toolchain::for_matrix(
            &PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native-runtime"),
            &[configuration],
        )
        .unwrap();
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let dir = scratch(&format!("isolation-{backend:?}"));
            let library = |name: &str, source: &str| {
                // Checked only: the reference observes no Blocked or stopped run.
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let context = default_context();
                let file = engine
                    .sources
                    .add(format!("{name}.hir"), source.into())
                    .unwrap();
                let output = check_progress(&mut engine, file, &context).unwrap();
                assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
                let input = engine.export_native_input().unwrap();
                let validated = engine.import_native_input(input).unwrap();
                let entry = entry_path(&context);
                let ready = build(&validated, &Options::new(x86())).unwrap();
                let objects = emit_backend_objects(
                    &ready,
                    backend,
                    OptLevel::Speed,
                    Partition::Whole,
                    false,
                    None,
                )
                .unwrap();
                let mut record = hirsel_run_config::RunConfig::for_program(
                    ready.revision(&entry).unwrap(),
                    hirsel_run_config::LaunchKind::Main,
                );
                record.io = Some(hirsel_run_config::IoGrants {
                    stdout: Some("stdout.bytes@1".into()),
                    tasks: Some("tasks.cooperative@1".into()),
                    ..Default::default()
                });
                record.args = Some(vec![]);
                let run = record.validated().unwrap();
                let dir = dir.join(name);
                std::fs::create_dir_all(&dir).unwrap();
                link_library(&ready, &entry, &objects, &toolchain, &dir, Some(&run)).unwrap()
            };
            let blocks = library("blocks", BLOCKS);
            let completes = library("completes", COMPLETES);
            let encodes = library("encodes", ENCODES);
            let fails = library("fails", FAILS);
            let fails_later = library("fails-later", FAILS_LATER);
            let runs = [
                &blocks,
                &completes,
                &encodes,
                &completes,
                &fails,
                &fails_later,
                &completes,
            ]
            .map(Clone::clone);
            let host = link_host(&runs, &x86(), &toolchain, &dir).unwrap();
            let output = run_host(&host, &runs, &toolchain).unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                output.status.code(),
                Some(0),
                "{backend:?}: {stdout}\n{stderr}"
            );
            assert_eq!(
                stdout,
                "blocks waits\nhost: run 1 exited 4\n\
                 completes ok\nhost: run 2 exited 0\n\
                 encodes\nhost: run 3 exited 6\n\
                 completes ok\nhost: run 4 exited 0\n\
                 host: run 5 exited 2\n\
                 host: run 6 exited 2\n\
                 completes ok\nhost: run 7 exited 0\n",
                "{backend:?}: {stderr}"
            );
            let reports: Vec<&str> = stderr
                .lines()
                .filter(|line| line.starts_with("runtime failure"))
                .collect();
            let [fails, fails_later] = reports.as_slice() else {
                panic!("{backend:?}: {stderr}")
            };
            assert!(fails.contains("(fails.hir:4:11)"), "{stderr}");
            assert!(fails_later.contains("(fails-later.hir:5:11)"), "{stderr}");
            assert!(
                stderr.contains("codec operational budget exhausted"),
                "{stderr}"
            );
            std::fs::remove_dir_all(dir).unwrap();
        }
    });
}
