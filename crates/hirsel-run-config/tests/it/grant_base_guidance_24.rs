//! Retain the refusal classification while adding path-resolution context,
//! following clap's error-context/edit_error separation.
use hirsel_run_config::{RunConfig, ValidatedRun};
use hirsel_run_schema::{LaunchKind, RunRefusal};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "grant-base-guidance-24-{}-{} λ space\"",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("fixtures")).unwrap();
        Self(root)
    }

    fn read(&self, name: &str, template: &Value) -> Result<ValidatedRun, RunRefusal> {
        let path = self.0.join(name);
        fs::write(&path, serde_json::to_vec(template).unwrap()).unwrap();
        RunConfig::read_grants(&path, [0; 32], "probe@1/main".into(), LaunchKind::Main)
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn template(host: &str) -> Value {
    json!({"io":{"dirs":[{"path":"data","host":host,"access":"read"}]}})
}

fn context(error: RunRefusal, tree: &Tree, host: &str) {
    let encoded = error.to_string();
    let json: Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(json["kind"], "invalid");
    assert_eq!(json["field"], "io.dirs.host");
    assert_eq!(RunRefusal::parse(&encoded), Some(error.clone()));
    let RunRefusal::Invalid { detail, .. } = error else {
        panic!("{encoded}")
    };
    let candidate = tree.0.join(host);
    let cause = candidate.canonicalize().unwrap_err().to_string();
    // The original cause survives independently of the added explanation.
    assert!(detail.contains(&cause), "{detail}");
    for fragment in [
        format!("host {host:?}"),
        format!("grants file directory {}", tree.0.display()),
        format!("resolved candidate {}", candidate.display()),
        "relative host paths resolve relative to the grants file".into(),
    ] {
        assert!(detail.contains(&fragment), "missing {fragment:?}: {detail}");
    }
    assert!(!candidate.exists());
}

#[test]
fn relative_success_and_relocated_templates_bind_their_own_directory() {
    let first = Tree::new();
    let second = Tree::new();
    let template = template("fixtures");
    let a = first.read("good.json", &template).unwrap();
    let b = second.read("relocated.json", &template).unwrap();
    for (tree, run) in [(&first, &a), (&second, &b)] {
        let mount = &run.record().io.as_ref().unwrap().dirs[0];
        assert_eq!(
            mount.host,
            tree.0
                .join("fixtures")
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
        );
        assert_eq!(mount.path, "data");
        assert_eq!(serde_json::to_value(mount).unwrap()["access"], "read");
        assert_eq!(RunConfig::validate(run.canonical()).unwrap(), *run);
    }
    assert_ne!(a.identity(), b.identity());
}

#[test]
fn missing_relative_and_doubled_prefix_keep_the_cause_and_add_context() {
    let tree = Tree::new();
    for (name, host) in [
        ("missing.json", "missing"),
        ("doubled.json", "examples/configcheck/fixtures"),
    ] {
        context(tree.read(name, &template(host)).unwrap_err(), &tree, host);
    }
}

#[test]
fn escaped_host_and_template_directory_round_trip_in_refusal_json() {
    let tree = Tree::new();
    let host = "missing λ space\"\\\nfixture";
    context(
        tree.read("escaped.json", &template(host)).unwrap_err(),
        &tree,
        host,
    );
}

#[test]
fn nested_runner_mounts_use_the_same_base_and_guidance() {
    let tree = Tree::new();
    let runner = |host| {
        json!({"io":{"dirs":[],"runners":[{
            "name":"pool","provider":"local.threads@1",
            "grants":{"dirs":[{"path":"data","host":host,"access":"read"}]}
        }]}})
    };
    let run = tree.read("runner-good.json", &runner("fixtures")).unwrap();
    let mount = &run.record().io.as_ref().unwrap().runners[0]
        .grants
        .as_ref()
        .unwrap()
        .dirs[0];
    assert_eq!(
        mount.host,
        tree.0
            .join("fixtures")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert_eq!(mount.path, "data");
    context(
        tree.read("runner-bad.json", &runner("missing"))
            .unwrap_err(),
        &tree,
        "missing",
    );
}

#[test]
fn absolute_hosts_keep_existing_validation_and_defer_existence_to_admission() {
    let tree = Tree::new();
    for (name, path) in [
        ("absolute.json", tree.0.join("fixtures")),
        ("absent.json", tree.0.join("absent")),
    ] {
        let host = path.to_str().unwrap();
        let run = tree.read(name, &template(host)).unwrap();
        assert_eq!(run.record().io.as_ref().unwrap().dirs[0].host, host);
    }
    let error = tree
        .read("noncanonical.json", &template("/tmp/../missing"))
        .unwrap_err();
    assert_eq!(
        error,
        RunRefusal::Invalid {
            field: "io.dirs.host".into(),
            detail: "expected a canonical absolute host root".into()
        }
    );
}

#[test]
fn inline_relative_hosts_and_invalid_virtual_paths_remain_refused() {
    let tree = Tree::new();
    let bytes = serde_json::to_vec(&template("fixtures")).unwrap();
    let error = RunConfig::bind_grants(&bytes, [0; 32], "probe@1/main".into(), LaunchKind::Main)
        .unwrap_err();
    assert_eq!(
        error,
        RunRefusal::Invalid {
            field: "io.dirs.host".into(),
            detail: "expected a canonical absolute host root".into()
        }
    );
    let mut bad = template("fixtures");
    bad["io"]["dirs"][0]["path"] = json!("../outside");
    assert!(
        matches!(tree.read("virtual.json", &bad), Err(RunRefusal::Invalid { field, .. }) if field == "io.dirs.path")
    );
}
