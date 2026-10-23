//! Journal lane: the append-open capability seam and complete application workflows.
use serde_json::json;
use std::{fs, path::PathBuf, process::Command};

struct Scratch(Option<tempfile::TempDir>);
impl Scratch {
    fn new() -> Self {
        let base = std::env::var_os("JOURNAL_KV_29_EVIDENCE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        fs::create_dir_all(&base).unwrap();
        Self(Some(
            tempfile::Builder::new()
                .prefix("journal-kv-29-")
                .tempdir_in(base)
                .unwrap(),
        ))
    }
    fn path(&self) -> &std::path::Path {
        self.0.as_ref().unwrap().path()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if std::env::var_os("JOURNAL_KV_29_EVIDENCE_DIR").is_some() {
            eprintln!(
                "JOURNAL_KV_29_EVIDENCE {}",
                self.0.take().unwrap().keep().display()
            );
        }
    }
}

const APPEND: &str = include_str!("../support/journal_kv_29/open_append.hir");

fn append_fixture(root: &std::path::Path) -> PathBuf {
    let source = root.join("main.hir");
    fs::write(&source, APPEND).unwrap();
    let stdlib = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    fs::write(
        source.with_extension("hir.json"),
        json!({"schema":1,
            "package":{"name":"appendtest","version":"1"},
            "sources":[{"path":"main.hir","module":[]}],
            "entry":{"module":[],"name":"main"},
            "dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}]
        })
        .to_string(),
    )
    .unwrap();
    source
}

#[test]
fn journal_kv_29_open_append_reference_checked() {
    hirsel_engine::with_compiler_stack(|| {
        let root = Scratch::new();
        let mut loaded = hirsel_driver::load_program(&append_fixture(root.path())).unwrap();
        loaded.check_for_execution().unwrap();
        let config = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for existing in [false, true] {
            let mut world = hirsel_engine::SimulationConfig::default();
            world.directories.insert("data".into());
            world
                .mounts
                .insert("data".into(), hirsel_run_schema::Access::ReadWrite);
            if existing {
                world.files.insert("data/log".into(), b"prefix".to_vec());
            }
            let expected = if existing {
                b"prefixfirstsecond".as_slice()
            } else {
                b"firstsecond".as_slice()
            };
            let mut previous = None;
            for checked in [false, true] {
                let execution = if checked {
                    hirsel_engine::run_observed_checked(&mut loaded.engine, &config, world.clone())
                } else {
                    hirsel_engine::run_observed_snapshot(&mut loaded.engine, &config, world.clone())
                }
                .unwrap();
                let observation = execution.observation.as_ref().unwrap();
                assert_eq!(observation.exit_code, Some(0), "{execution:?}");
                assert_eq!(observation.output, expected);
                assert!(observation.failures.is_empty());
                if let Some(previous) = previous {
                    assert_eq!(*observation, previous);
                }
                previous = Some(observation.clone());
                eprintln!("JOURNAL_KV_29_PASS append existing={existing} checked={checked}");
            }
        }
    });
}

#[test]
fn journal_kv_29_open_append_native_replay() {
    let root = Scratch::new();
    let source = append_fixture(root.path());
    let cli = env!("CARGO_BIN_EXE_hirsel");
    fs::write(
        root.path().join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    let mut apps = vec![];
    for backend in ["llvm", "cranelift"] {
        let app = root.path().join(backend);
        let built = Command::new(cli)
            .args(["build", "--backend", backend])
            .arg(&source)
            .arg("-o")
            .arg(&app)
            .output()
            .unwrap();
        assert!(built.status.success(), "{built:?}");
        apps.push(app);
    }
    for (backend, app) in ["llvm", "cranelift"].into_iter().zip(&apps) {
        let data = root.path().join(format!("data-{backend}"));
        fs::create_dir(&data).unwrap();
        fs::write(data.join("log"), b"prefix").unwrap();
        let grants = root.path().join(format!("grants-{backend}.json"));
        fs::write(&grants,json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1", "dirs":[{"path":"data","host":data,"access":"read_write"}]}}).to_string()).unwrap();
        let tape = root.path().join(format!("{backend}.tape"));
        let run = Command::new("python3")
            .arg(root.path().join("capped"))
            .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
            .arg(app)
            .env("HIRSEL_MEMORY_LIMIT", "2G")
            .env("HIRSEL_JOBS", "4")
            .arg("--hirsel-record")
            .arg(&tape)
            .arg("--hirsel-grants")
            .arg(&grants)
            .output()
            .unwrap();
        assert_eq!(run.status.code(), Some(0), "{run:?}");
        assert_eq!(run.stdout, b"prefixfirstsecond");
        assert_eq!(fs::read(data.join("log")).unwrap(), run.stdout);
        assert!(!data.join("denied").exists());
        fs::remove_dir_all(&data).unwrap();
        fs::remove_file(grants).unwrap();
        let immutable = fs::read(&tape).unwrap();
        let interpreted = Command::new("python3")
            .arg(root.path().join("capped"))
            .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
            .arg(cli)
            .env("HIRSEL_MEMORY_LIMIT", "2G")
            .env("HIRSEL_JOBS", "4")
            .arg("run")
            .arg(&source)
            .arg("--replay")
            .arg(&tape)
            .output()
            .unwrap();
        assert_eq!(interpreted.status.code(), Some(0), "{interpreted:?}");
        assert_eq!(interpreted.stdout, run.stdout);
        for replay_app in &apps {
            let replay = Command::new("python3")
                .arg(root.path().join("capped"))
                .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
                .arg(replay_app)
                .env("HIRSEL_MEMORY_LIMIT", "2G")
                .env("HIRSEL_JOBS", "4")
                .arg("--hirsel-replay")
                .arg(&tape)
                .output()
                .unwrap();
            assert_eq!(replay.status.code(), Some(0), "{replay:?}");
            assert_eq!(replay.stdout, run.stdout);
            assert_eq!(fs::read(&tape).unwrap(), immutable);
            assert!(!data.exists());
        }
        eprintln!("JOURNAL_KV_29_PASS append {backend} + three replay adapters");
    }
    for backend in [None, Some("llvm"), Some("cranelift")] {
        let mut cmd = Command::new(cli);
        cmd.arg("test").arg(&source);
        if let Some(backend) = backend {
            cmd.args(["--native", "--backend", backend]);
        }
        let tests = cmd.output().unwrap();
        assert_eq!(tests.status.code(), Some(0), "{tests:?}");
        assert!(
            String::from_utf8_lossy(&tests.stdout).contains("2 passed"),
            "{tests:?}"
        );
    }
}

#[test]
fn journal_kv_29_open_append_crash_modes_checked() {
    hirsel_engine::with_compiler_stack(|| {
        let root = Scratch::new();
        let source = append_fixture(root.path());
        // The public execution adapter requires a public root entry. Keep the
        // native fixture unchanged and expose only this scratch test selection.
        fs::write(&source, APPEND.replace("@test\nfn ", "@test\npub fn ")).unwrap();
        let mut loaded = hirsel_driver::load_program(&source).unwrap();
        loaded.check_for_execution().unwrap();
        for name in ["crash_reopen", "create_still_truncates_and_path_refusals"] {
            let mut entry = loaded.entry.clone();
            entry.0.name = name.into();
            let config = hirsel_engine::RunConfiguration {
                entry,
                hash_seed: 0,
                inputs: vec![],
            };
            let mut world = hirsel_engine::SimulationConfig::default();
            world.grants.insert("Test".into());
            let execution =
                hirsel_engine::run_observed_checked(&mut loaded.engine, &config, world).unwrap();
            let observation = execution.observation.as_ref().unwrap();
            assert_eq!(observation.exit_code, Some(0), "{name}: {execution:?}");
            assert!(observation.failures.is_empty(), "{name}: {execution:?}");
            eprintln!("JOURNAL_KV_29_PASS {name} checked-h");
        }
    });
}
