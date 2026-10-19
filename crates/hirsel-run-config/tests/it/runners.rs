use hirsel_run_config::*;
use serde_json::{Value, json};

fn record(runners: Value) -> Value {
    let mut record = serde_json::to_value(RunConfig::empty(
        [0; 32],
        "probe@1/main".into(),
        LaunchKind::Main,
    ))
    .unwrap();
    record["io"] = json!({"dirs": [], "runners": runners});
    record
}
fn validate(record: &Value) -> Result<ValidatedRun, RunRefusal> {
    RunConfig::validate(&serde_json::to_vec(record).unwrap())
}
fn canonical(run: &ValidatedRun) -> String {
    String::from_utf8(run.canonical().to_vec()).unwrap()
}
fn pool() -> Value {
    json!({"name": "pool", "provider": "local.threads@1"})
}
fn node() -> Value {
    json!({"name": "node", "provider": "sim@1", "node": "n", "models": "remote.tls@1"})
}
fn refused(runner: Value, field: &str) {
    match validate(&record(json!([runner]))) {
        Err(RunRefusal::Invalid { field: f, .. } | RunRefusal::Unsupported { field: f })
            if f == field => {}
        other => panic!("expected a refusal at {field}, got {other:?}"),
    }
}
fn with(mut base: Value, member: &str, value: Value) -> Value {
    base[member] = value;
    base
}

#[test]
fn records_without_runners_or_a_fault_seed_keep_their_bytes() {
    let mut plain = record(json!([]));
    plain["io"].as_object_mut().unwrap().remove("runners");
    let without = validate(&plain).unwrap();
    let empty = validate(&record(json!([]))).unwrap();
    assert_eq!(without.canonical(), empty.canonical());
    assert!(!canonical(&empty).contains("runners"));
    assert!(!canonical(&empty).contains("fault_seed"));
    let mut seeded = plain.clone();
    seeded["inputs"]["fault_seed"] = json!(7);
    let seeded = validate(&seeded).unwrap();
    assert!(canonical(&seeded).contains(r#""fault_seed":7"#));
    assert_ne!(seeded.identity(), without.identity());
}

#[test]
fn runner_defaults_normalize_to_absence_and_authority_is_spelled_null() {
    let defaults = json!({
        "name": "pool", "provider": "local.threads@1", "threads": 1, "queue": 64,
        "retry": {"attempts": 0}, "cache": null, "code": {"provider": "code.none@1"}
    });
    let normalized = validate(&record(json!([defaults]))).unwrap();
    assert!(canonical(&normalized).contains(
        r#""runners":[{"name":"pool","provider":"local.threads@1","grants":null,"definitions":null}]"#
    ));
    assert_eq!(
        normalized.identity(),
        validate(&record(json!([pool()]))).unwrap().identity()
    );
    let threads = validate(&record(json!([with(pool(), "threads", json!(4))]))).unwrap();
    assert!(canonical(&threads).contains(r#""threads":4"#));
    assert_ne!(threads.identity(), normalized.identity());
    let granted = with(
        pool(),
        "grants",
        json!({"stdout": "stdout.bytes@1", "dirs": []}),
    );
    let granted = validate(&record(json!([granted]))).unwrap();
    assert_ne!(granted.identity(), normalized.identity());
    let entry = &granted.record().io.as_ref().unwrap().runners[0];
    assert_eq!(entry.threads(), DEFAULT_THREADS);
    assert_eq!(entry.queue(), DEFAULT_QUEUE);
    assert_eq!(entry.contract(), "local.threads@1");
    assert_eq!(
        validate(&record(json!([node()])))
            .unwrap()
            .record()
            .io
            .as_ref()
            .unwrap()
            .runners[0]
            .contract(),
        "remote.tls@1"
    );
}

#[test]
fn runner_lists_are_sorted_unique_and_named() {
    let named = |name: &str| with(pool(), "name", json!(name));
    validate(&record(json!([named("a"), named("b")]))).unwrap();
    for (runners, field) in [
        (json!([named("b"), named("a")]), "io.runners"),
        (json!([named("a"), named("a")]), "io.runners"),
        (json!([named("A")]), "io.runners.name"),
        (json!([named("")]), "io.runners.name"),
    ] {
        match validate(&record(runners)) {
            Err(RunRefusal::Invalid { field: f, .. }) if f == field => {}
            other => panic!("expected {field}, got {other:?}"),
        }
    }
}

#[test]
fn provider_settings_refuse_with_valid_twins() {
    validate(&record(json!([pool()]))).unwrap();
    validate(&record(json!([node()]))).unwrap();
    refused(
        with(pool(), "provider", json!("remote.tls@1")),
        "io.runners.provider",
    );
    refused(with(pool(), "node", json!("n")), "io.runners");
    refused(with(pool(), "threads", json!(0)), "io.runners.threads");
    refused(with(node(), "threads", json!(2)), "io.runners.threads");
    let mut nodeless = node();
    nodeless.as_object_mut().unwrap().remove("node");
    refused(nodeless, "io.runners.node");
    let mut modelless = node();
    modelless.as_object_mut().unwrap().remove("models");
    refused(modelless, "io.runners.models");
    refused(with(node(), "models", json!("db@1")), "io.runners.models");
    let rate = |enable: u32, fire: u32| json!({"enable": enable, "fire": fire});
    validate(&record(json!([with(
        node(),
        "faults",
        json!({"crash": rate(1, 1)})
    )])))
    .unwrap();
    refused(
        with(node(), "faults", json!({"crash": rate(1_000_001, 1)})),
        "io.runners.faults",
    );
    refused(
        with(node(), "faults", json!({"melt": rate(1, 1)})),
        "io.runners.faults",
    );
    // A node modeling local.threads@1 can only refuse for capacity.
    let threads_model = with(node(), "models", json!("local.threads@1"));
    validate(&record(json!([with(
        threads_model.clone(),
        "faults",
        json!({"overload": rate(1, 1)})
    )])))
    .unwrap();
    refused(
        with(threads_model, "faults", json!({"crash": rate(1, 1)})),
        "io.runners.faults",
    );
}

#[test]
fn seam_members_refuse_with_valid_twins() {
    validate(&record(json!([with(pool(), "recording", json!("record"))]))).unwrap();
    refused(
        with(pool(), "recording", json!("durable")),
        "io.runners.recording",
    );
    refused(
        with(pool(), "recording", json!("always")),
        "io.runners.recording",
    );
    validate(&record(json!([with(
        pool(),
        "retry",
        json!({"attempts": 3})
    )])))
    .unwrap();
    refused(
        with(pool(), "cache", json!({"provider": "cache.memory@1"})),
        "io.runners.cache",
    );
    validate(&record(json!([with(
        pool(),
        "code",
        json!({"provider": "code.dir@1", "path": "/var/code"})
    )])))
    .unwrap();
    refused(
        with(pool(), "code", json!({"provider": "code.git@1"})),
        "io.runners.code.provider",
    );
    refused(
        with(pool(), "code", json!("code.none@1")),
        "io.runners.code",
    );
}

#[test]
fn callee_grants_are_an_io_record_without_stdin_or_runners() {
    let grants = |extra: Value| {
        let mut grants = json!({"stdout": "stdout.bytes@1", "dirs": []});
        for (k, v) in extra.as_object().unwrap() {
            grants[k] = v.clone();
        }
        with(pool(), "grants", grants)
    };
    validate(&record(json!([grants(json!({}))]))).unwrap();
    refused(
        grants(json!({"stdin": "stdin.bytes@1"})),
        "io.runners.grants.stdin",
    );
    refused(
        grants(json!({"runners": [pool()]})),
        "io.runners.grants.runners",
    );
    refused(
        grants(json!({"tasks": "tasks.threads@9"})),
        "io.runners.grants.tasks",
    );
    let definitions = |paths: Value| with(pool(), "definitions", paths);
    validate(&record(json!([definitions(json!(["a@1/f", "b@1/g"]))]))).unwrap();
    validate(&record(json!([definitions(json!([]))]))).unwrap();
    refused(
        definitions(json!(["b@1/g", "a@1/f"])),
        "io.runners.definitions",
    );
    refused(definitions(json!(["f"])), "io.runners.definitions");
}

#[test]
fn a_fixture_list_validates_alone_and_host_launches_refuse_runners() {
    let list = validate_runners(&serde_json::to_vec(&json!([node(), pool()])).unwrap()).unwrap();
    assert_eq!(list.len(), 2);
    assert!(validate_runners(b"[{\"name\": \"pool\"}]").is_err());
    let run = validate(&record(json!([pool()]))).unwrap();
    let facts = ProgramFacts {
        program: run.program(),
        entry: run.record().entry.clone(),
        launch: run.record().launch,
        roots: vec![],
        code: CodeReach::Safe,
        fueled: true,
    };
    assert_eq!(
        run.clone().bind(&facts).map(|_| ()),
        Err(RunRefusal::Unsupported {
            field: "io.runners".into()
        })
    );
    validate(&record(json!([]))).unwrap().bind(&facts).unwrap();
}
