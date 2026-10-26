//! Independently routed phases of the original 300-replay deadline/select matrix.
//! Phase separation follows Tokio ce685ee6 time/timeout.rs:204-224; Hirsel's
//! deadline and cancellation precedence and the original vector stay unchanged.
use super::e1_replay_liveness_28::{package, write_grants};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

fn from_project(variable: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(std::env::var_os(variable).expect(variable))
}

// Files drain both streams while the child runs, including a failing compiler.
// Killing our own process group also bounds compiler/linker descendants.
fn phase(command: &mut Command, root: &Path, label: &str) -> Result<Output, String> {
    let stem = label.replace(' ', "-");
    let stdout = root.join(format!("{stem}.stdout"));
    let stderr = root.join(format!("{stem}.stderr"));
    eprintln!("REPLAY_LIVENESS STARTED {label}");
    let mut phases = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("phases.log"))
        .unwrap();
    writeln!(phases, "STARTED {label}").unwrap();
    let start = Instant::now();
    let mut child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(File::create(&stdout).unwrap())
        .stderr(File::create(&stderr).unwrap())
        .spawn()
        .map_err(|error| format!("{label}: spawn: {error}"))?;
    let mut expired = false;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() >= Duration::from_secs(20) {
            expired = true;
            // SAFETY: this child created a new group whose ID is its own PID.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            break child.wait().unwrap();
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let output = Output {
        status,
        stdout: fs::read(stdout).unwrap(),
        stderr: fs::read(stderr).unwrap(),
    };
    let disposition = if expired { "TIMED_OUT" } else { "COMPLETED" };
    let evidence = format!(
        "{disposition} {label} elapsed={:?} status={} stdout={:?} stderr={:?}\n",
        start.elapsed(),
        output.status,
        output.stdout,
        output.stderr
    );
    eprintln!("REPLAY_LIVENESS {evidence}");
    phases.write_all(evidence.as_bytes()).unwrap();
    if expired || !output.status.success() {
        return Err(evidence);
    }
    Ok(output)
}

fn prepare(root: &Path, engine: &str) -> Result<(), String> {
    let manifest = package(root);
    let grants = root.join("grants.json");
    write_grants(&grants);
    let exe = root.join("native");
    phase(
        Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["build", "--backend", engine])
            .arg(manifest)
            .arg("-o")
            .arg(&exe),
        root,
        &format!("{engine} build"),
    )?;
    let tape = root.join("recorded.tape");
    let recorded = phase(
        Command::new(&exe)
            .arg("--hirsel-record")
            .arg(&tape)
            .arg("--hirsel-grants")
            .arg(grants),
        root,
        &format!("{engine} record"),
    )?;
    if recorded.stdout != b"done\n" || !recorded.stderr.is_empty() {
        return Err(format!("{engine} record: {recorded:?}"));
    }
    fs::set_permissions(&tape, fs::Permissions::from_mode(0o444)).unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o555)).unwrap();
    Ok(())
}

// Buck runs this once per backend as a capped fixture action. A failure is
// durable input to every dependent cell, so each cell returns a real failure
// with the first unfinished phase rather than disappearing behind setup.
#[test]
#[ignore = "invoked by the deadline replay fixture action"]
fn fixture() {
    let root = from_project("HIRSEL_REPLAY_FIXTURE_OUT");
    fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let engine = std::env::var("HIRSEL_REPLAY_BACKEND").unwrap();
    // Recording requires source ancestry; the launcher's private TMPDIR has
    // that property on the pool, whereas the build output bind mount does not.
    let scratch = tempfile::tempdir().unwrap();
    let result = prepare(scratch.path(), &engine);
    for entry in fs::read_dir(scratch.path()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            fs::copy(entry.path(), root.join(entry.file_name())).unwrap();
        }
    }
    fs::write(
        root.join("result"),
        match result {
            Ok(()) => "ok\n".to_owned(),
            Err(error) => error,
        },
    )
    .unwrap();
}

#[test]
#[ignore = "invoked separately for reference and each native width by Buck"]
fn cell() {
    let fixture = from_project("HIRSEL_REPLAY_FIXTURE")
        .canonicalize()
        .unwrap();
    let result = fs::read_to_string(fixture.join("result")).unwrap();
    assert_eq!(result, "ok\n", "first unfinished setup phase: {result}");
    let engine = std::env::var("HIRSEL_REPLAY_BACKEND").unwrap();
    let cell = std::env::var("HIRSEL_REPLAY_CELL").unwrap();
    let tape = fixture.join("recorded.tape");
    let original = fs::read(&tape).unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    if cell == "reference" {
        // Fresh compiler startup remains inside the original 20 second bound.
        let manifest = package(root);
        let output = phase(
            Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .arg("run")
                .arg(manifest)
                .arg("--replay")
                .arg(&tape),
            root,
            &format!("{engine} interpreter replay"),
        )
        .unwrap();
        assert_eq!(output.stdout, b"done\n");
        assert!(output.stderr.is_empty(), "{output:?}");
    } else {
        let width: u32 = cell.parse().unwrap();
        assert!([1, 2, 4].contains(&width));
        for repeat in 0..50 {
            let output = phase(
                Command::new(fixture.join("native"))
                    .arg("--hirsel-replay")
                    .arg(&tape)
                    .env("HIRSEL_RT_WORKERS", width.to_string())
                    .env("HIRSEL_JOBS", "4"),
                root,
                &format!("{engine} workers={width} repeat={repeat}"),
            )
            .unwrap();
            assert_eq!(output.stdout, b"done\n");
            assert!(output.stderr.is_empty(), "{output:?}");
            assert_eq!(
                fs::read(&tape).unwrap(),
                original,
                "tape mutated at repeat={repeat}"
            );
        }
    }
    assert_eq!(fs::read(tape).unwrap(), original);
}
