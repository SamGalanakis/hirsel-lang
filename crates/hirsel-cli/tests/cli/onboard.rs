use super::*;

const PROGRAM: &str = "pub fn main(rt: Io) -> Result[(), GrantError] {\n  let n = 42u64\n  let output = io.stdout(rt)?\n  let _ = out.print(output, f\"{n}\\n\")\n  Ok(())\n}\n";

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(args)
        .output()
        .unwrap()
}
fn success(args: &[&str]) -> std::process::Output {
    let output = cli(args);
    assert!(output.status.success(), "{args:?}: {output:?}");
    output
}

#[test]
fn manifestless_f_strings_use_std_and_explicit_sidecars_still_win() {
    let source = file(PROGRAM);
    let path = source.to_str().unwrap();
    success(&["check", path]);
    for flags in [vec![], vec!["--native"]] {
        let mut args = vec![
            "run",
            path,
            "--grants",
            r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]}}"#,
        ];
        args.extend(flags);
        assert_eq!(success(&args).stdout, b"42\n");
    }
    success(&["hash", path]);
    success(&["doc", path]);
    let sidecar = source.with_extension("hir.json");
    std::fs::write(&sidecar, serde_json::json!({
        "schema":1,"package":{"name":"standalone","version":"0"},
        "entry":{"module":[],"name":"main"},
        "sources":[{"path":source.file_name().unwrap().to_str().unwrap(),"module":[]}],"dependencies":[]
    }).to_string()).unwrap();
    let denied = cli(&["check", path]);
    assert_eq!(denied.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&denied.stderr);
    assert!(
        stderr.contains("SEM-3") && stderr.contains(sidecar.to_str().unwrap()),
        "{stderr}"
    );
    std::fs::remove_file(sidecar).unwrap();
    std::fs::remove_file(source).unwrap();
}

#[test]
fn new_creates_tested_runnable_packages_and_refuses_existing_contents() {
    for library in [false, true] {
        let directory = std::env::temp_dir().join(format!(
            "onboard_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let path = directory.to_str().unwrap();
        let mut args = vec!["new", path];
        if library {
            args.push("--lib");
        }
        success(&args);
        let manifest = directory.join("hirsel.json");
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        assert_eq!(value.get("entry").is_none(), library);
        assert_eq!(value["package"]["version"], "0");
        let tested = success(&["test", manifest.to_str().unwrap(), "--json"]);
        let tests: serde_json::Value = serde_json::from_slice(&tested.stdout).unwrap();
        assert_eq!(tests["summary"]["passed"], 1, "{tests}");
        if !library {
            let source = directory.join("main.hir");
            let grants = directory.join("grants.json");
            for selected in [&source, &manifest] {
                assert_eq!(
                    success(&[
                        "run",
                        selected.to_str().unwrap(),
                        "--grants",
                        grants.to_str().unwrap()
                    ])
                    .stdout,
                    b"Hello, Hirsel!\n"
                );
            }
        }
        let original = std::fs::read(&manifest).unwrap();
        assert!(!cli(&args).status.success());
        assert_eq!(std::fs::read(&manifest).unwrap(), original);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn refused_launch_hint_selects_all_requested_roots_and_can_be_used_verbatim() {
    let source = file(&PROGRAM.replace("rt: Io", "rt: Io, args: Args, env: Env"));
    let path = source.to_str().unwrap();
    for flags in [vec![], vec!["--native"]] {
        let mut args = vec!["run", path];
        args.extend(flags);
        let refused = cli(&args);
        assert_eq!(refused.status.code(), Some(3), "{refused:?}");
        assert!(refused.stdout.is_empty());
        let stderr = String::from_utf8(refused.stderr).unwrap();
        let hint = stderr
            .lines()
            .find_map(|l| {
                l.strip_prefix("hirsel: grant the entry's requested roots with --grants '")
            })
            .unwrap()
            .strip_suffix('\'')
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(hint).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":[],"env":{}})
        );
        args.extend(["--grants", hint]);
        assert_eq!(success(&args).stdout, b"42\n");
    }
    std::fs::remove_file(source).unwrap();
}

#[test]
fn new_rejects_invalid_package_names_before_creating_the_directory() {
    let parent = std::env::temp_dir().join(format!(
        "onboard_invalid_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    for name in ["bad-name", "fn", "7demo"] {
        let directory = parent.join(name);
        assert!(!cli(&["new", directory.to_str().unwrap()]).status.success());
        assert!(!directory.exists());
    }
}
