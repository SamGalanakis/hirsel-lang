//! Launch admission and confinement controls for grants-file-relative hosts.
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn launch(manifest: &Path, grants: &Path, cwd: &Path, directory: &str) -> Output {
    let executable = Path::new(env!("CARGO_BIN_EXE_hirsel"))
        .canonicalize()
        .unwrap();
    let mut output = Command::new("python3")
        .arg(manifest.parent().unwrap().parent().unwrap().join("capped"))
        .args(["--timeout", "60"])
        .arg(executable)
        .env("HIRSEL_DAEMON", "off")
        .env("HIRSEL_RESULTS", "none@1")
        .env("HIRSEL_JOBS", "1")
        .env("HIRSEL_MEMORY_LIMIT", "2G")
        .current_dir(cwd)
        .arg("run")
        .arg(manifest)
        .arg("--grants")
        .arg(grants)
        .args(["--", directory])
        .output()
        .unwrap();
    // capped reports its resource accounting on stderr; preserve the launcher's bytes.
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    output.stderr = stderr
        .split_inclusive('\n')
        .filter(|line| !line.starts_with("capped: "))
        .collect::<String>()
        .into_bytes();
    output
}

fn grants(path: &Path, host: &str) {
    fs::write(
        path,
        json!({"io":{"stdout":"stdout.bytes@1","dirs":[
        {"path":"data","host":host,"access":"read"}
    ]},"args":[]})
        .to_string(),
    )
    .unwrap();
}

#[test]
fn grant_base_guidance_24_refuses_before_entry_and_preserves_mount_authority() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(
        root.join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    let package = root.join("package λ space\"");
    let cwd = root.join("unrelated-cwd");
    let relocated = root.join("relocated");
    for path in [&package, &cwd, &relocated] {
        fs::create_dir(path).unwrap();
    }
    for path in [&package, &cwd, &relocated] {
        fs::create_dir(path.join("fixtures")).unwrap();
    }
    fs::write(package.join("fixtures/input.txt"), "hello\n").unwrap();
    fs::write(cwd.join("fixtures/input.txt"), "CWD-SENTINEL\n").unwrap();
    fs::write(relocated.join("fixtures/input.txt"), "relocated\n").unwrap();
    fs::write(root.join("input.txt"), "OUTSIDE-SENTINEL\n").unwrap();
    // Printing on entry makes refusal-before-body observable, even if a read would fail.
    let source = include_str!("../../../../examples/native-verification-9/main.hir").replace(
        "let data = io.dir(rt, path.of(\"data\"))?",
        "out.print(io.stdout(rt)?, \"entered\\n\")?\n  let data = io.dir(rt, path.of(label))?",
    );
    fs::write(package.join("main.hir"), source).unwrap();
    let mut manifest: Value = serde_json::from_str(include_str!(
        "../../../../examples/native-verification-9/hirsel.json"
    ))
    .unwrap();
    manifest["dependencies"][0]["manifest"] = json!(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap()
    );
    let manifest_path = package.join("hirsel.json");
    fs::write(&manifest_path, manifest.to_string()).unwrap();

    for (name, host) in [
        ("missing.json", "missing"),
        ("doubled.json", "package λ space\"/fixtures"),
        ("escaped.json", "missing λ \"\\\nfixture"),
    ] {
        let path = package.join(name);
        grants(&path, host);
        let out = launch(&manifest_path, &path, &cwd, "data");
        assert_eq!(out.status.code(), Some(3), "{out:?}");
        assert!(out.stdout.is_empty(), "entry ran: {out:?}");
        let stderr = std::str::from_utf8(&out.stderr).unwrap();
        let refusal: Value =
            serde_json::from_str(stderr.trim().strip_prefix("hirsel: ").unwrap()).unwrap();
        assert_eq!(refusal["kind"], "invalid");
        assert_eq!(refusal["field"], "io.dirs.host");
        let detail = refusal["detail"].as_str().unwrap();
        for fragment in [
            format!("host {host:?}"),
            format!("grants file directory {}", package.display()),
            format!("resolved candidate {}", package.join(host).display()),
            "relative host paths resolve relative to the grants file".into(),
            package.join(host).canonicalize().unwrap_err().to_string(),
        ] {
            assert!(detail.contains(&fragment), "{fragment:?}: {detail}");
        }
        assert!(!package.join(host).exists());
    }

    let correct = package.join("good.json");
    grants(&correct, "fixtures");
    for launch_cwd in [&cwd, root] {
        let out = launch(&manifest_path, &correct, launch_cwd, "data");
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        assert_eq!(out.stdout, b"entered\ndata:hello\n");
        assert!(out.stderr.is_empty(), "{out:?}");
    }
    let moved = relocated.join("good.json");
    fs::copy(&correct, &moved).unwrap();
    let out = launch(&manifest_path, &moved, &cwd, "data");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(out.stdout, b"entered\ndata:relocated\n");
    assert!(out.stderr.is_empty(), "{out:?}");

    let absolute = package.join("absolute.json");
    grants(&absolute, package.join("fixtures").to_str().unwrap());
    let out = launch(&manifest_path, &absolute, &cwd, "data");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(out.stdout, b"entered\ndata:hello\n");
    assert!(out.stderr.is_empty(), "{out:?}");
    let absent = package.join("absolute-absent.json");
    grants(&absent, package.join("absent").to_str().unwrap());
    let out = launch(&manifest_path, &absent, &cwd, "data");
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(!stderr.contains("relative host paths"), "{stderr}");
    assert!(stderr.contains("os error 2"), "{stderr}");

    for outside in ["outside", "fixtures", "../", root.to_str().unwrap()] {
        let out = launch(&manifest_path, &correct, &cwd, outside);
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        assert_eq!(out.stdout, b"entered\n");
        assert!(!String::from_utf8_lossy(&out.stderr).contains("OUTSIDE-SENTINEL"));
    }
    assert_eq!(
        fs::read(root.join("input.txt")).unwrap(),
        b"OUTSIDE-SENTINEL\n"
    );
    assert_eq!(
        fs::read(package.join("fixtures/input.txt")).unwrap(),
        b"hello\n"
    );
}
